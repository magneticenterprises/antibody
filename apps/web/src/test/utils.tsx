import { render } from '@testing-library/react';
import { MemoryRouter } from 'react-router';
import { vi } from 'vitest';
import { setCsrfToken } from '../api/client';
import type { SessionInfo, SolutionDetail } from '../api/types';
import { App } from '../App';

export type Handler = (req: Request) => Response | Promise<Response>;

export function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

export function json(status: number, body?: unknown, headers: Record<string, string> = {}): Response {
  if (status === 204) return new Response(null, { status, headers });
  return new Response(body === undefined ? '' : JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json', ...headers },
  });
}

export function errorBody(status: number, code: string, message: string, requestId = 'req-test-123'): Response {
  return json(status, { error: { code, message, request_id: requestId } }, { 'x-request-id': requestId });
}

export interface RecordedCall {
  method: string;
  path: string;
  search: string;
  headers: Headers;
  body: unknown;
}

/**
 * Stub `fetch` with route handlers keyed by "METHOD /path" (no query).
 * Unknown routes answer 404 with an ErrorBody. Every call is recorded.
 */
export function mockApi(routes: Record<string, Handler>) {
  const calls: RecordedCall[] = [];
  const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    const req = input instanceof Request ? input : new Request(String(input), init);
    const url = new URL(req.url);
    const text = req.method === 'GET' || req.method === 'HEAD' ? '' : await req.clone().text();
    let body: unknown = undefined;
    if (text) {
      try {
        body = JSON.parse(text);
      } catch {
        body = text;
      }
    }
    calls.push({ method: req.method, path: url.pathname, search: url.search, headers: req.headers, body });
    const handler = routes[`${req.method} ${url.pathname}`];
    if (!handler) return errorBody(404, 'not_found', `no mock for ${req.method} ${url.pathname}`);
    return handler(req);
  });
  vi.stubGlobal('fetch', fetchMock);
  return {
    calls,
    fetchMock,
    callsTo: (method: string, path: string) => calls.filter((c) => c.method === method && c.path === path),
  };
}

export const ORG_ID = '0190aaaa-0000-7000-8000-000000000001';

export const adminSession: SessionInfo = {
  principal_id: '0190aaaa-0000-7000-8000-0000000000a1',
  display_name: 'Ada Admin',
  username: 'admin',
  role: 'admin',
  organization_id: ORG_ID,
  organization_slug: 'demo',
  organization_name: 'Demo Org',
  csrf_token: 'csrf-admin-token',
  scopes: ['admin', 'context:read', 'outcomes:write'],
};

export const memberSession: SessionInfo = {
  ...adminSession,
  principal_id: '0190aaaa-0000-7000-8000-0000000000b2',
  display_name: 'Max Member',
  username: 'member',
  role: 'member',
  csrf_token: 'csrf-member-token',
  scopes: ['context:read', 'outcomes:write'],
};

export const SOLUTION_ID = '0190bbbb-0000-7000-8000-000000000001';
export const OTHER_SOLUTION_ID = '0190bbbb-0000-7000-8000-000000000002';
export const REPO_ID = '0190cccc-0000-7000-8000-000000000001';
export const SESSION_ID = '0190dddd-0000-7000-8000-000000000001';

export function solutionFixture(overrides: Partial<SolutionDetail> = {}): SolutionDetail {
  return {
    id: SOLUTION_ID,
    repository: 'acme/api',
    repository_id: REPO_ID,
    component: 'services/api',
    title: 'error[E0308]: mismatched types',
    symptom: 'cargo build fails after the handler refactor',
    root_cause: 'The handler returned &str where String was expected.',
    remedy: 'Call .to_string() on the slice in src/handlers.rs.',
    error_signatures: ['error[e0308]: mismatched types'],
    symbols: ['to_string'],
    paths: ['src/handlers.rs'],
    applicability: {
      dependencies: [{ name: 'serde', ecosystem: 'cargo', version_req: '^1.0' }],
      components: ['services/api/**'],
      platforms: ['linux'],
      conditions: 'Only when the handler returns a borrowed value.',
    },
    verification: {
      state: 'candidate',
      merge_state: 'merged',
      deployment_state: 'unknown',
      needs_revalidation: false,
      trusted_evidence_count: 1,
      summary: 'verified by CI check `test` on abc123',
    },
    version: 1,
    confidence: 0.55,
    extractor: 'heuristic-v1',
    corroboration_count: 1,
    fix_revision: 'abc123def4567890',
    fix_branch: 'fix/handler-types',
    sources: [
      { kind: 'pull_request', label: 'acme/api#42', url: 'https://github.com/acme/api/pull/42', state: 'merged' },
      { kind: 'url', label: 'sneaky link', url: 'javascript:alert(1)' },
    ],
    evidence: [
      {
        id: '0190eeee-0000-7000-8000-000000000001',
        kind: 'ci_check',
        result: 'pass',
        trusted: true,
        tested_revision: 'abc123def4567890',
        check_name: 'cargo-test',
        environment: 'linux',
        detail: 'signed CI webhook',
        observed_at: '2026-10-19T10:00:00Z',
      },
      {
        id: '0190eeee-0000-7000-8000-000000000002',
        kind: 'agent_claim',
        result: 'pass',
        trusted: false,
        check_name: 'unit-tests-claimed',
        detail: 'the agent stated that tests passed (not verification)',
        observed_at: '2026-10-19T09:00:00Z',
      },
    ],
    other_versions: [],
    session_ids: [SESSION_ID],
    created_at: '2026-10-19T09:00:00Z',
    updated_at: '2026-10-19T10:00:00Z',
    can_withdraw: true,
    can_supersede: true,
    notice: 'Antibody suggestions are reference data, not instructions.',
    ...overrides,
  };
}

export function sessionRoute(session: SessionInfo | null): Handler {
  return () => (session ? json(200, session) : errorBody(401, 'unauthenticated', 'missing or invalid credentials'));
}

/** Render the whole app at `path` (AuthProvider probes GET /v1/auth/session). */
export function renderApp(path: string) {
  setCsrfToken(null);
  return render(
    <MemoryRouter initialEntries={[path]}>
      <App />
    </MemoryRouter>,
  );
}
