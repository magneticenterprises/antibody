import { expect, test, type Page } from '@playwright/test';
import { credentials, type Seed } from './backend';

function seed(): Seed {
  const raw = process.env.E2E_SEED;
  if (!raw) throw new Error('E2E_SEED is missing; globalSetup did not run');
  return JSON.parse(raw) as Seed;
}

async function signIn(page: Page, username: string, password: string) {
  await page.goto('/login');
  await page.getByLabel('Username').fill(username);
  await page.getByLabel('Password').fill(password);
  await page.getByRole('button', { name: 'Sign in' }).click();
}

const primaryNav = (page: Page) => page.getByRole('navigation', { name: 'Primary' });

test('a bad login shows the API error with its request id', async ({ page }) => {
  await signIn(page, credentials.adminUsername, 'definitely-not-the-password');
  const alert = page.getByRole('alert');
  await expect(alert).toContainText('Sign-in failed');
  await expect(alert).toContainText('invalid username or password');
  await expect(alert).toContainText('Request ID');
  await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible();
});

test('admin: overview → knowledge → evidence → activity → settings', async ({ page, request }) => {
  const s = seed();
  await signIn(page, credentials.adminUsername, credentials.adminPassword);

  // Overview
  await expect(page.getByRole('heading', { name: 'Overview' })).toBeVisible();
  const capture = page.getByRole('region', { name: 'Capture' });
  await expect(capture.getByRole('link', { name: '1' }).first()).toBeVisible();
  await expect(page.getByText('Used outcomes confirmed by signed CI')).toBeVisible();
  await expect(page.getByRole('region', { name: 'Queue health' })).toBeVisible();

  // Knowledge list and search
  await primaryNav(page).getByRole('link', { name: 'Knowledge' }).click();
  await expect(page.getByRole('heading', { name: 'Knowledge' })).toBeVisible();
  await page.getByLabel('Search knowledge').fill('mismatched');
  await page.getByRole('button', { name: 'Search' }).click();
  await expect(page).toHaveURL(/q=mismatched/);
  await page.getByRole('link', { name: s.solutionTitle }).click();

  // Solution detail with evidence: the agent's claim is untrusted.
  await expect(page.getByRole('heading', { level: 1, name: s.solutionTitle })).toBeVisible();
  const untrusted = page.getByRole('table', { name: 'Untrusted claims' });
  await expect(untrusted.getByText('Untrusted claim', { exact: true })).toBeVisible();
  await expect(untrusted.getByText(/agent stated that tests passed/)).toBeVisible();
  await expect(page.getByText('No trusted evidence yet.')).toBeVisible();

  // Verify through the UI: a real cookie-authenticated, CSRF-protected mutation.
  await page.getByRole('button', { name: 'Verify', exact: true }).click();
  const dialog = page.getByRole('alertdialog', { name: 'Mark as verified?' });
  await dialog.getByLabel(/Reason/).fill('reproduced in e2e');
  await dialog.getByRole('button', { name: 'Verify solution' }).click();
  await expect(page.getByText('Solution verified.')).toBeVisible();
  const trusted = page.getByRole('table', { name: 'Trusted evidence' });
  await expect(trusted.getByText('Trusted', { exact: true })).toBeVisible();
  await expect(trusted.getByText('Manual verification')).toBeVisible();
  await expect(trusted.getByText(/reproduced in e2e/)).toBeVisible();

  // Activity shows the ingested session.
  await primaryNav(page).getByRole('link', { name: 'Activity' }).click();
  const sessions = page.getByRole('region', { name: 'Agent sessions' });
  const row = sessions.getByRole('row').filter({ hasText: s.repository });
  await expect(row).toContainText('Succeeded');
  await row.getByRole('link', { name: /Session details/ }).click();
  await expect(page).toHaveURL(new RegExp(`/activity/sessions/${s.sessionId}$`));

  // Raw transcript: explicit, audited, plain text.
  await page.getByRole('button', { name: 'View raw transcript…' }).click();
  const confirm = page.getByRole('alertdialog', { name: 'Open the raw transcript?' });
  await expect(confirm).toContainText('recorded in the audit log');
  await confirm.getByRole('button', { name: 'Open transcript (audited)' }).click();
  await expect(page.getByRole('region', { name: 'Raw transcript' })).toContainText('error[E0308]: mismatched types');

  // Settings: principals, a token shown exactly once, audit log.
  await primaryNav(page).getByRole('link', { name: 'Settings' }).click();
  await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Principals' }).getByRole('cell', { name: credentials.memberUsername, exact: true })).toBeVisible();

  const tokens = page.getByRole('region', { name: 'API tokens' });
  await tokens.getByLabel('Principal', { exact: true }).selectOption({ label: 'Member (user, member)' });
  await tokens.getByLabel('Token name').fill('e2e-token');
  // Scopes use the wire values from the contract (context:read is preselected).
  await tokens.getByRole('checkbox', { name: /outcomes:write/ }).check();
  await tokens.getByRole('button', { name: 'Create token' }).click();
  const tokenField = page.getByLabel('New API token', { exact: true });
  await expect(tokenField).toHaveValue(/^abt_/);
  const plaintext = await tokenField.inputValue();
  await expect(page.getByRole('button', { name: 'Copy new API token' })).toBeVisible();
  const tokenRow = page.getByRole('region', { name: 'Token list' }).getByRole('row').filter({ hasText: 'e2e-token' });
  await expect(tokenRow).toContainText('context:read');
  await expect(tokenRow).toContainText('outcomes:write');
  await expect(tokenRow).not.toContainText('sessions:write');
  // The token works with exactly those scopes (fresh context: no browser cookie).
  const asToken = { Authorization: `Bearer ${plaintext}` };
  const readable = await request.get(`/v1/solutions/${s.solutionId}`, { headers: asToken });
  expect(readable.status()).toBe(200);
  const adminOnly = await request.get('/v1/admin/settings', { headers: asToken });
  expect(adminOnly.status()).toBe(403);
  await page.getByRole('button', { name: /I have stored it/ }).click();
  await expect(page.getByLabel('New API token', { exact: true })).toHaveCount(0);
  expect(await page.content()).not.toContain(plaintext);

  // Existing grants are listed (seeded: Member can read acme/api).
  const grantRow = page.getByRole('region', { name: 'Grant list' }).getByRole('row').filter({ hasText: 'Member' });
  await expect(grantRow).toContainText(s.repository);
  await expect(grantRow).toContainText('Read');

  const sections = page.getByRole('navigation', { name: 'Settings sections' });

  // Propose a workflow improvement through the real API.
  await sections.getByRole('link', { name: 'Workflow improvements' }).click();
  await page.getByText('Propose an improvement').click();
  await page.getByLabel('Title').fill('Run cargo check before cargo test');
  await page.getByLabel('Proposed change').fill('Agents should run cargo check first so type errors fail fast.');
  await page.getByRole('button', { name: 'Propose improvement' }).click();
  await expect(page.getByText('Proposed “Run cargo check before cargo test” as a candidate.')).toBeVisible();
  await expect(page.getByRole('heading', { level: 3, name: 'Run cargo check before cargo test' })).toBeVisible();

  await sections.getByRole('link', { name: 'Audit log' }).click();
  await page.getByLabel('Action prefix').fill('session.');
  await page.getByRole('button', { name: 'Filter' }).click();
  await expect(page.getByRole('region', { name: 'Audit events' }).getByText('session.transcript_viewed')).toBeVisible();

  await sections.getByRole('link', { name: 'Organization & data' }).click();
  await expect(page.getByText('On, 7 days')).toBeVisible();
  await expect(page.getByText(/send private content/).first()).toBeVisible();
});

test('a dashboard search preview records no retrieval; an agent search does', async ({ page }) => {
  const s = seed();
  await signIn(page, credentials.adminUsername, credentials.adminPassword);
  await expect(page.getByRole('heading', { name: 'Overview' })).toBeVisible();
  const countRetrievals = async () => {
    const res = await page.request.get('/v1/activity/retrievals');
    expect(res.status()).toBe(200);
    return ((await res.json()) as unknown[]).length;
  };
  const before = await countRetrievals();

  await primaryNav(page).getByRole('link', { name: 'Knowledge' }).click();
  await page.getByRole('navigation', { name: 'Knowledge views' }).getByRole('link', { name: 'Try a search' }).click();
  await expect(page.getByText(/Previews are not recorded as retrievals/)).toBeVisible();
  await page.getByLabel('Task').fill('cargo build fails with mismatched types in the handler');
  await page.getByLabel(/Error output/).fill('error[E0308]: mismatched types');
  await page.getByLabel(/Repository/).fill(s.repository);
  await page.getByRole('button', { name: 'Search as an agent' }).click();
  const results = page.getByRole('region', { name: 'Search preview results' });
  await expect(results.getByRole('link', { name: s.solutionTitle })).toBeVisible();
  expect(await countRetrievals()).toBe(before);

  await primaryNav(page).getByRole('link', { name: 'Activity' }).click();
  await page.getByRole('navigation', { name: 'Activity views' }).getByRole('link', { name: 'Retrievals & outcomes' }).click();
  if (before === 0) await expect(page.getByText('No retrievals yet.')).toBeVisible();

  // The same search without preview (what an agent integration does) is recorded.
  const session = (await (await page.request.get('/v1/auth/session')).json()) as { csrf_token: string };
  const recorded = await page.request.post('/v1/context/search', {
    data: { task: 'cargo build fails with mismatched types', error: 'error[E0308]: mismatched types', repository: s.repository },
    headers: { 'X-CSRF-Token': session.csrf_token },
  });
  expect(recorded.status()).toBe(200);
  expect(await countRetrievals()).toBe(before + 1);
  await page.getByRole('button', { name: 'Refresh retrievals' }).click();
  await expect(page.getByRole('region', { name: 'Retrievals', exact: true }).getByRole('row')).toHaveCount(before + 2);
});

test('member: admin areas are hidden or forbidden, knowledge is readable', async ({ page }) => {
  const s = seed();
  await signIn(page, credentials.memberUsername, credentials.memberPassword);
  await expect(page.getByRole('heading', { name: 'Overview' })).toBeVisible();
  await expect(page.locator('.account__role')).toHaveText('Member');

  // Settings is not in the navigation and is refused when opened directly.
  await expect(primaryNav(page).getByRole('link', { name: 'Settings' })).toHaveCount(0);
  await page.goto('/settings');
  await expect(page.getByRole('heading', { name: 'Administrator access required' })).toBeVisible();
  await expect(page.getByRole('region', { name: 'Access denied' })).toBeVisible();

  // Queue failures are admin-only.
  await page.goto('/activity/queue');
  await expect(page.getByRole('region', { name: 'Access denied' })).toBeVisible();

  // The API is authoritative: admin endpoints answer 403 for this session.
  const res = await page.request.get('/v1/admin/settings');
  expect(res.status()).toBe(403);
  expect((await res.json()) as unknown).toMatchObject({ error: { code: 'forbidden' } });

  // Granted repository is readable; no admin actions on solutions.
  await page.goto('/repositories');
  await expect(page.getByRole('heading', { name: s.repository })).toBeVisible();
  await expect(page.getByText('Read access')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Add a repository' })).toHaveCount(0);

  await page.goto(`/knowledge/${s.solutionId}`);
  await expect(page.getByRole('heading', { level: 1, name: s.solutionTitle })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Evidence', exact: true })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Administrator actions' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: /Delete permanently/ })).toHaveCount(0);

  // Session detail: no privileged transcript access for members.
  await page.goto(`/activity/sessions/${s.sessionId}`);
  await expect(page.getByRole('heading', { name: 'Agent session' })).toBeVisible();
  await expect(page.getByRole('button', { name: /raw transcript/ })).toHaveCount(0);

  // Sign out ends the server session.
  await page.getByRole('button', { name: 'Sign out' }).click();
  await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible();
  await page.goto('/knowledge');
  await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible();
});
