import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { adminSession, deferred, errorBody, json, mockApi, REPO_ID, renderApp, sessionRoute, type Handler } from '../../test/utils';

const PRINCIPAL_ID = '0190ffff-0000-7000-8000-000000000001';
const SECRET = 'abt_live_0123456789abcdefSECRET';

const repository = {
  id: REPO_ID,
  provider: 'github',
  full_name: 'acme/api',
  default_branch: 'main',
  status: 'active',
  permission: 'write',
  components: [],
  session_count: 0,
  solution_count: 0,
  created_at: '2026-10-19T09:00:00Z',
};

function accessRoutes(extra: Record<string, Handler>): Record<string, Handler> {
  return {
    'GET /v1/auth/session': sessionRoute(adminSession),
    'GET /v1/admin/principals': () =>
      json(200, [{ id: PRINCIPAL_ID, kind: 'service', role: 'member', display_name: 'ci-bot', created_at: '2026-10-19T09:00:00Z' }]),
    'GET /v1/repositories': () => json(200, { items: [repository] }),
    'GET /v1/admin/tokens': () => json(200, []),
    'GET /v1/admin/grants': () => json(200, []),
    ...extra,
  };
}

describe('repository grants', () => {
  it('lists grants and revokes one after confirmation', async () => {
    let grants = [
      {
        principal_id: PRINCIPAL_ID,
        principal_name: 'ci-bot',
        repository_id: REPO_ID,
        repository: 'acme/api',
        permission: 'write',
        created_at: '2026-10-19T09:00:00Z',
      },
    ];
    const m = mockApi(
      accessRoutes({
        'GET /v1/admin/grants': () => json(200, grants),
        'DELETE /v1/admin/grants': () => {
          grants = [];
          return json(204);
        },
      }),
    );
    const user = userEvent.setup();
    renderApp('/settings/access');
    const list = await screen.findByRole('region', { name: 'Grant list' });
    const row = within(list).getByRole('row', { name: /ci-bot acme\/api Write/ });
    await user.click(within(row).getByRole('button', { name: 'Revoke' }));
    const dialog = screen.getByRole('alertdialog', { name: "Revoke ci-bot's access to acme/api?" });
    await user.click(within(dialog).getByRole('button', { name: 'Revoke grant' }));

    expect(await screen.findByText(/No grants yet/)).toBeInTheDocument();
    const call = m.callsTo('DELETE', '/v1/admin/grants')[0];
    expect(call?.body).toEqual({ principal_id: PRINCIPAL_ID, repository_id: REPO_ID });
    expect(call?.headers.get('X-CSRF-Token')).toBe('csrf-admin-token');
  });

  it('creates a grant and refreshes the list', async () => {
    let grants: unknown[] = [];
    const m = mockApi(
      accessRoutes({
        'GET /v1/admin/grants': () => json(200, grants),
        'PUT /v1/admin/grants': () => {
          grants = [
            {
              principal_id: PRINCIPAL_ID,
              principal_name: 'ci-bot',
              repository_id: REPO_ID,
              repository: 'acme/api',
              permission: 'read',
              created_at: '2026-10-19T09:00:00Z',
            },
          ];
          return json(204);
        },
      }),
    );
    const user = userEvent.setup();
    renderApp('/settings/access');
    const section = await screen.findByRole('region', { name: 'Repository grants' });
    await user.selectOptions(await within(section).findByLabelText('Principal'), PRINCIPAL_ID);
    await user.selectOptions(within(section).getByLabelText('Repository'), REPO_ID);
    await user.click(within(section).getByRole('button', { name: 'Grant access' }));
    expect(await within(section).findByText('Granted read access on acme/api to ci-bot.')).toBeInTheDocument();
    expect(await within(section).findByRole('region', { name: 'Grant list' })).toHaveTextContent('ci-bot');
    expect(m.callsTo('PUT', '/v1/admin/grants')[0]?.body).toEqual({
      principal_id: PRINCIPAL_ID,
      repository_id: REPO_ID,
      permission: 'read',
    });
  });
});

describe('API tokens', () => {
  it('blocks token creation while repositories load or fail, and restores the allow-list after retry', async () => {
    const firstLoad = deferred<Response>();
    const retry = deferred<Response>();
    let reads = 0;
    const m = mockApi(accessRoutes({
      'GET /v1/repositories': () => ++reads === 1 ? firstLoad.promise : retry.promise,
      'POST /v1/admin/tokens': () => json(201, { id: 'token-id', token: 'SYNTHETIC-TEST-ONLY', prefix: 'synthetic' }),
    }));
    const user = userEvent.setup();
    renderApp('/settings/access');
    const tokens = await screen.findByRole('region', { name: 'API tokens' });
    await user.selectOptions(await within(tokens).findByLabelText('Principal'), PRINCIPAL_ID);
    await user.type(within(tokens).getByLabelText('Token name'), 'limited token');
    const create = within(tokens).getByRole('button', { name: 'Create token' });
    expect(screen.getByText('Loading repositories…')).toBeInTheDocument();
    expect(create).toBeDisabled();
    expect(within(tokens).getByRole('group', { name: /Repository allow-list/ })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Grant access' })).toBeDisabled();
    fireEvent.submit(create.closest('form') as HTMLFormElement);
    expect(m.callsTo('POST', '/v1/admin/tokens')).toHaveLength(0);

    await act(async () => firstLoad.resolve(errorBody(503, 'unavailable', 'Repository list unavailable', 'req-repositories')));
    expect(await screen.findByRole('alert')).toHaveTextContent('Repository list unavailable');
    expect(screen.getByRole('alert')).toHaveTextContent('req-repositories');
    expect(create).toBeDisabled();
    fireEvent.submit(create.closest('form') as HTMLFormElement);
    expect(m.callsTo('POST', '/v1/admin/tokens')).toHaveLength(0);

    await user.click(screen.getByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(reads).toBe(2));
    expect(create).toBeDisabled();
    await act(async () => retry.resolve(json(200, { items: [repository] })));
    await user.click(await within(tokens).findByRole('checkbox', { name: 'acme/api' }));
    expect(create).toBeEnabled();
    await user.click(create);
    expect(await screen.findByLabelText('New API token')).toHaveValue('SYNTHETIC-TEST-ONLY');
    expect(m.callsTo('POST', '/v1/admin/tokens')[0]?.body).toMatchObject({ repository_ids: [REPO_ID] });
    expect(screen.queryByText('Repository list unavailable')).not.toBeInTheDocument();
  });

  it('keeps the allow-list visible but blocks token and grant creation after a repository refresh fails', async () => {
    const refresh = deferred<Response>();
    let reads = 0;
    const m = mockApi(accessRoutes({
      'GET /v1/repositories': () => ++reads === 1 ? json(200, { items: [repository] }) : refresh.promise,
    }));
    const user = userEvent.setup();
    renderApp('/settings/access');
    const tokens = await screen.findByRole('region', { name: 'API tokens' });
    await user.selectOptions(await within(tokens).findByLabelText('Principal'), PRINCIPAL_ID);
    await user.type(within(tokens).getByLabelText('Token name'), 'limited token');
    await user.click(within(tokens).getByRole('checkbox', { name: 'acme/api' }));
    const grants = screen.getByRole('region', { name: 'Repository grants' });
    await user.selectOptions(within(grants).getByLabelText('Principal'), PRINCIPAL_ID);
    await user.selectOptions(within(grants).getByLabelText('Repository'), REPO_ID);
    expect(screen.getByRole('button', { name: 'Grant access' })).toBeEnabled();

    await user.click(screen.getByRole('button', { name: 'Refresh repositories' }));
    expect(screen.getByRole('button', { name: 'Create token' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Grant access' })).toBeDisabled();
    await act(async () => refresh.resolve(errorBody(503, 'unavailable', 'Repository refresh unavailable')));
    expect(await screen.findByRole('alert')).toHaveTextContent('Repository refresh unavailable');
    expect(within(tokens).getByRole('checkbox', { name: 'acme/api' })).toBeChecked();
    expect(screen.getByRole('button', { name: 'Create token' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Grant access' })).toBeDisabled();
    fireEvent.submit(within(tokens).getByRole('button', { name: 'Create token' }).closest('form') as HTMLFormElement);
    fireEvent.submit(within(grants).getByRole('button', { name: 'Grant access' }).closest('form') as HTMLFormElement);
    expect(m.callsTo('POST', '/v1/admin/tokens')).toHaveLength(0);
    expect(m.callsTo('PUT', '/v1/admin/grants')).toHaveLength(0);
  });

  it('blocks dependent forms when principals fail to load', async () => {
    mockApi(accessRoutes({
      'GET /v1/admin/principals': () => errorBody(503, 'unavailable', 'Principal list unavailable'),
    }));
    renderApp('/settings/access');
    expect(await screen.findByRole('alert')).toHaveTextContent('Principal list unavailable');
    expect(screen.getByRole('button', { name: 'Create token' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Grant access' })).toBeDisabled();
  });

  it('shows a newly created token exactly once, with a copy button', async () => {
    const tokens: unknown[] = [];
    const m = mockApi(
      accessRoutes({
      'GET /v1/admin/tokens': () => json(200, tokens),
      'POST /v1/admin/tokens': () => {
        tokens.push({
          id: '0190ffff-0000-7000-8000-0000000000aa',
          principal_id: PRINCIPAL_ID,
          principal_name: 'ci-bot',
          name: 'ci-runner',
          token_prefix: 'abt_live_01',
          scopes: ['context:read', 'ci:write'],
          created_at: '2026-10-19T10:00:00Z',
        });
        return json(201, { id: '0190ffff-0000-7000-8000-0000000000aa', token: SECRET, prefix: 'abt_live_01' });
      },
      }),
    );
    const user = userEvent.setup();
    renderApp('/settings/access');

    expect(await screen.findByText('No tokens yet.')).toBeInTheDocument();
    await user.selectOptions(await screen.findByLabelText('Principal', { selector: 'select[id^="tok"]' }), PRINCIPAL_ID);
    await user.type(screen.getByLabelText('Token name'), 'ci-runner');
    await user.click(screen.getByRole('checkbox', { name: /ci:write/ }));
    await user.type(screen.getByLabelText(/Expires in days/), '30');
    await user.click(screen.getByRole('button', { name: 'Create token' }));

    const field = await screen.findByLabelText('New API token');
    expect(field).toHaveValue(SECRET);
    expect(field).toHaveAttribute('readonly');
    expect(screen.getByRole('button', { name: 'Copy new API token' })).toBeInTheDocument();
    expect(screen.getByText(/only time the token is shown/)).toBeInTheDocument();

    const body = m.callsTo('POST', '/v1/admin/tokens')[0]?.body;
    expect(body).toEqual({
      principal_id: PRINCIPAL_ID,
      name: 'ci-runner',
      scopes: ['context:read', 'ci:write'],
      repository_ids: null,
      expires_in_days: 30,
    });

    // The list is refreshed and shows only the prefix.
    const table = await screen.findByRole('region', { name: 'Token list' });
    await waitFor(() => expect(within(table).getByText('abt_live_01')).toBeInTheDocument());
    expect(within(table).queryByText(SECRET)).not.toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: /I have stored it/ }));
    expect(screen.queryByDisplayValue(SECRET)).not.toBeInTheDocument();
    expect(document.body.textContent ?? '').not.toContain(SECRET);
  });
});
