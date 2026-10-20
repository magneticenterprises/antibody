// Real-backend harness for Playwright: recreates a dedicated database, runs
// `antibody-api bootstrap`, starts `antibody-api serve`, seeds data through
// the public HTTP API with the bootstrap admin token, and runs the worker
// once so extraction completes. Nothing here talks to the database directly
// except DROP/CREATE DATABASE (via psql) to start from a clean slate.
import { execFileSync, spawn, type ChildProcess } from 'node:child_process';
import { existsSync } from 'node:fs';
import { resolve } from 'node:path';

export const repoRoot = resolve(import.meta.dirname, '../../..');
export const API_PORT = Number(process.env.E2E_API_PORT ?? 18080);
export const API_ORIGIN = `http://127.0.0.1:${API_PORT}`;
// Must match the `vite preview` origin in playwright.config.ts.
export const WEB_ORIGIN = 'http://localhost:4173';

const PG = {
  host: process.env.E2E_PGHOST ?? '127.0.0.1',
  port: process.env.E2E_PGPORT ?? '55432',
  user: process.env.E2E_PGUSER ?? 'antibody',
  password: process.env.E2E_PGPASSWORD ?? 'antibody',
  database: process.env.E2E_PGDATABASE ?? 'antibody_e2e',
};

export const credentials = {
  adminUsername: 'admin',
  adminPassword: process.env.E2E_ADMIN_PASSWORD ?? 'e2e-admin-password-0123',
  memberUsername: 'member',
  memberPassword: process.env.E2E_MEMBER_PASSWORD ?? 'e2e-member-password-0123',
};

export interface Seed {
  repositoryId: string;
  repository: string;
  sessionId: string;
  solutionId: string;
  solutionTitle: string;
  memberId: string;
}

function bin(name: string): string {
  const p = resolve(repoRoot, 'target/debug', name);
  if (!existsSync(p)) {
    throw new Error(`${p} is missing. Build it first: cargo build -p antibody-api -p antibody-worker`);
  }
  return p;
}

export function backendEnv(): NodeJS.ProcessEnv {
  return {
    ...process.env,
    DATABASE_URL: `postgres://${PG.user}:${PG.password}@${PG.host}:${PG.port}/${PG.database}`,
    ANTIBODY_SECRET_KEY: 'e2e-secret-key-e2e-secret-key-0123456789',
    // Must equal the browser origin: the API rejects mutating cookie requests from other origins.
    ANTIBODY_PUBLIC_URL: WEB_ORIGIN,
    ANTIBODY_BIND: `127.0.0.1:${API_PORT}`,
    ANTIBODY_LOG_FORMAT: 'text',
    // Tests sign in many times; keep the limiter out of the way.
    ANTIBODY_LOGIN_RATE_LIMIT_PER_MINUTE: '1000',
    ANTIBODY_RATE_LIMIT_PER_MINUTE: '100000',
    E2E_ADMIN_PASSWORD: credentials.adminPassword,
    RUST_LOG: process.env.RUST_LOG ?? 'warn',
  };
}

export function resetDatabase(): void {
  const args = ['-h', PG.host, '-p', PG.port, '-U', PG.user, '-d', 'postgres', '-v', 'ON_ERROR_STOP=1', '-q'];
  const env = { ...process.env, PGPASSWORD: PG.password };
  execFileSync('psql', [...args, '-c', `DROP DATABASE IF EXISTS ${PG.database} WITH (FORCE)`], { env, stdio: 'pipe' });
  execFileSync('psql', [...args, '-c', `CREATE DATABASE ${PG.database}`], { env, stdio: 'pipe' });
}

export function bootstrap(): string {
  const out = execFileSync(
    bin('antibody-api'),
    ['bootstrap', '--org-slug', 'demo', '--org-name', 'Demo', '--admin-username', credentials.adminUsername, '--admin-password-env', 'E2E_ADMIN_PASSWORD'],
    { env: backendEnv(), encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] },
  );
  const parsed = JSON.parse(out) as { admin_token?: string };
  if (!parsed.admin_token) throw new Error('bootstrap did not return admin_token');
  return parsed.admin_token;
}

async function isUp(): Promise<boolean> {
  try {
    const r = await fetch(`${API_ORIGIN}/v1/health/live`);
    return r.ok;
  } catch {
    return false;
  }
}

export async function startApi(): Promise<ChildProcess> {
  if (await isUp()) {
    throw new Error(`Something is already listening on ${API_ORIGIN}; stop it before running the E2E suite.`);
  }
  const child = spawn(bin('antibody-api'), ['serve'], { env: backendEnv(), stdio: ['ignore', 'inherit', 'inherit'] });
  const deadline = Date.now() + 60_000;
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw new Error(`antibody-api exited early with code ${child.exitCode}`);
    try {
      const r = await fetch(`${API_ORIGIN}/v1/health/ready`);
      if (r.ok) return child;
    } catch {
      // not listening yet
    }
    await new Promise((r) => setTimeout(r, 250));
  }
  child.kill('SIGTERM');
  throw new Error('antibody-api did not become ready within 60s');
}

export function stopApi(child: ChildProcess | undefined): Promise<void> {
  if (!child || child.exitCode !== null) return Promise.resolve();
  return new Promise((done) => {
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      done();
    }, 10_000);
    child.once('exit', () => {
      clearTimeout(timer);
      done();
    });
    child.kill('SIGTERM');
  });
}

export function runWorkerOnce(): void {
  execFileSync(bin('antibody-worker'), ['--once'], { env: backendEnv(), stdio: ['ignore', 'pipe', 'inherit'] });
}

async function call<T>(token: string, method: string, path: string, body?: unknown, headers: Record<string, string> = {}): Promise<T> {
  const r = await fetch(`${API_ORIGIN}${path}`, {
    method,
    headers: { Authorization: `Bearer ${token}`, ...(body === undefined ? {} : { 'Content-Type': 'application/json' }), ...headers },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await r.text();
  if (!r.ok) throw new Error(`${method} ${path} -> ${r.status}: ${text}`);
  return (text ? JSON.parse(text) : undefined) as T;
}

export async function seed(token: string): Promise<Seed> {
  const repository = 'acme/api';
  const repo = await call<{ id: string }>(token, 'POST', '/v1/repositories', {
    provider: 'github',
    full_name: repository,
    url: 'https://github.com/acme/api',
  });
  // Retain raw transcripts so the audited transcript view can be exercised.
  await call(token, 'PATCH', '/v1/admin/settings', { raw_capture_enabled: true, raw_retention_days: 7 });

  const accepted = await call<{ id: string }>(
    token,
    'POST',
    '/v1/sessions',
    {
      agent_type: 'claude_code',
      external_id: 'e2e-session-1',
      repository,
      branch: 'fix/handler-types',
      messages: [
        { role: 'user', content: 'cargo build fails in the api service after the handler refactor' },
        {
          role: 'tool',
          tool_name: 'bash',
          content:
            'error[E0308]: mismatched types\n --> src/handlers.rs:42:17\n  |\n42 |     Ok(name)\n  |        ^^^^ expected `String`, found `&str`',
        },
        {
          role: 'assistant',
          content:
            'The root cause was that the handler returned a borrowed &str where the response type expects an owned String. I changed src/handlers.rs to call .to_string() on the slice and all tests pass now.',
        },
      ],
    },
    { 'Idempotency-Key': 'e2e-session-0001' },
  );

  runWorkerOnce();

  const status = await call<{ extraction_status: string; solution_ids: string[] }>(token, 'GET', `/v1/sessions/${accepted.id}`);
  const solutionId = status.solution_ids[0];
  if (status.extraction_status !== 'succeeded' || !solutionId) {
    throw new Error(`extraction did not produce a solution: ${JSON.stringify(status)}`);
  }
  const solution = await call<{ title: string }>(token, 'GET', `/v1/solutions/${solutionId}`);

  const member = await call<{ id: string }>(token, 'POST', '/v1/admin/principals', {
    kind: 'user',
    display_name: 'Member',
    role: 'member',
    username: credentials.memberUsername,
    password: credentials.memberPassword,
  });
  await call(token, 'PUT', '/v1/admin/grants', { principal_id: member.id, repository_id: repo.id, permission: 'read' });

  return {
    repositoryId: repo.id,
    repository,
    sessionId: accepted.id,
    solutionId,
    solutionTitle: solution.title,
    memberId: member.id,
  };
}
