import { defineConfig, devices } from '@playwright/test';

// End-to-end tests against the REAL backend:
// - globalSetup (e2e/global-setup.ts) recreates the `antibody_e2e` database,
//   bootstraps an organization, starts `target/debug/antibody-api serve` on
//   127.0.0.1:18080 and seeds data through the HTTP API (plus one worker run);
//   its teardown stops the API.
// - webServer builds the app and serves it with `vite preview` on
//   http://localhost:4173, proxying /v1 to the API. The API's
//   ANTIBODY_PUBLIC_URL is that same origin, so cookie + CSRF + Origin checks
//   behave exactly as in production.
const WEB_ORIGIN = 'http://localhost:4173';
const API_PROXY = `http://127.0.0.1:${process.env.E2E_API_PORT ?? 18080}`;

export default defineConfig({
  testDir: './e2e',
  testMatch: /.*\.spec\.ts$/,
  fullyParallel: false,
  workers: 1,
  retries: 0,
  forbidOnly: Boolean(process.env.CI),
  timeout: 30_000,
  expect: { timeout: 10_000 },
  reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : 'list',
  globalSetup: './e2e/global-setup.ts',
  use: {
    baseURL: WEB_ORIGIN,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    command: 'pnpm exec vite build && pnpm exec vite preview --port 4173 --strictPort --host localhost',
    url: WEB_ORIGIN,
    reuseExistingServer: false,
    timeout: 120_000,
    env: { ANTIBODY_API_PROXY: API_PROXY },
    stdout: 'ignore',
    stderr: 'pipe',
  },
});
