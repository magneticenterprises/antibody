import { act, render, screen, within } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { App } from '../App';
import {
  adminSession,
  deferred,
  errorBody,
  json,
  memberSession,
  mockApi,
  OTHER_SOLUTION_ID,
  renderApp,
  sessionRoute,
  SOLUTION_ID,
  solutionFixture,
} from '../test/utils';

const path = `/knowledge/${SOLUTION_ID}`;
const apiPath = `/v1/solutions/${SOLUTION_ID}`;

function CurrentPath() {
  return <output aria-label="Current path">{useLocation().pathname}</output>;
}

function routes(session: typeof adminSession, solution = solutionFixture()) {
  return {
    'GET /v1/auth/session': sessionRoute(session),
    [`GET ${apiPath}`]: () => json(200, solution),
    [`GET ${apiPath}/propagation`]: () =>
      json(200, {
        solution_id: SOLUTION_ID,
        notice: 'Candidates only. Antibody never applies changes.',
        candidates: [
          {
            repository_id: '0190cccc-0000-7000-8000-000000000002',
            repository: 'acme/worker',
            component: 'crates/jobs',
            reason: 'same error signature seen in CI',
            related_solution_id: OTHER_SOLUTION_ID,
            matching_ci_failures: 2,
          },
        ],
      }),
    'GET /v1/solutions': () => json(200, { total: 0, items: [] }),
  };
}

describe('solution detail', () => {
  it('separates trusted evidence from untrusted agent claims', async () => {
    mockApi(routes(adminSession));
    renderApp(path);

    const trusted = (await screen.findByRole('heading', { name: /Trusted evidence/ })).closest('div') as HTMLElement;
    const untrusted = screen.getByRole('heading', { name: /Untrusted claims/ }).closest('div') as HTMLElement;

    const trustedTable = within(trusted).getByRole('table', { name: 'Trusted evidence' });
    expect(within(trustedTable).getByText('Trusted')).toBeInTheDocument();
    expect(within(trustedTable).getByText('cargo-test')).toBeInTheDocument();
    expect(within(trustedTable).queryByText('Untrusted claim')).not.toBeInTheDocument();

    const claimTable = within(untrusted).getByRole('table', { name: 'Untrusted claims' });
    expect(within(claimTable).getByText('Untrusted claim')).toBeInTheDocument();
    expect(within(claimTable).getByText(/agent stated that tests passed/)).toBeInTheDocument();
    expect(within(untrusted).getByText(/never counts as verification/)).toBeInTheDocument();

    // Test/check names come from the evidence.
    const signals = screen.getByRole('region', { name: 'Signals' });
    expect(within(signals).getByText('unit-tests-claimed')).toBeInTheDocument();
    // Merge state is shown separately from verification.
    expect(screen.getByText('Merge: merged')).toBeInTheDocument();
  });

  it('renders API text as plain text and only links http(s) sources', async () => {
    const solution = solutionFixture({
      remedy: '<img src=x onerror="alert(1)"> **not markdown**',
    });
    mockApi(routes(adminSession, solution));
    const { container } = renderApp(path);
    expect(await screen.findByText('<img src=x onerror="alert(1)"> **not markdown**')).toBeInTheDocument();
    expect(container.querySelector('img')).toBeNull();
    expect(container.querySelector('strong')?.textContent ?? '').not.toContain('not markdown');

    const pr = screen.getByRole('link', { name: /acme\/api#42/ });
    expect(pr).toHaveAttribute('href', 'https://github.com/acme/api/pull/42');
    expect(pr).toHaveAttribute('rel', 'noopener noreferrer');
    expect(pr).toHaveAttribute('target', '_blank');
    expect(screen.queryByRole('link', { name: /sneaky link/ })).not.toBeInTheDocument();
    expect(screen.getByText('sneaky link')).toBeInTheDocument();
  });

  it('shows admin actions to administrators', async () => {
    mockApi(routes(adminSession));
    renderApp(path);
    expect(await screen.findByRole('button', { name: 'Verify' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Withdraw' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Supersede…' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Delete permanently…' })).toBeInTheDocument();
  });

  it('hides admin actions from members', async () => {
    mockApi(routes(memberSession, solutionFixture({ can_withdraw: false, can_supersede: false })));
    renderApp(path);
    expect(await screen.findByRole('heading', { name: /Trusted evidence/ })).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Administrator actions' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Verify' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Withdraw' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Delete permanently/ })).not.toBeInTheDocument();
  });

  it('withdraws after confirmation, sending the CSRF token and showing the new state', async () => {
    const m = mockApi({
      ...routes(adminSession),
      [`POST ${apiPath}/state`]: () =>
        json(200, solutionFixture({ can_withdraw: false, verification: { ...solutionFixture().verification, state: 'withdrawn' } })),
    });
    const user = userEvent.setup();
    renderApp(path);
    await user.click(await screen.findByRole('button', { name: 'Withdraw' }));
    const dialog = screen.getByRole('alertdialog', { name: 'Withdraw this solution?' });
    await user.type(within(dialog).getByLabelText(/Reason/), 'obsolete');
    await user.click(within(dialog).getByRole('button', { name: 'Withdraw solution' }));

    expect(await screen.findByText('Solution withdrawn.')).toBeInTheDocument();
    const call = m.callsTo('POST', `${apiPath}/state`)[0];
    expect(call?.body).toEqual({ action: 'withdraw', reason: 'obsolete' });
    expect(call?.headers.get('X-CSRF-Token')).toBe('csrf-admin-token');
    expect(screen.getAllByText('Withdrawn').length).toBeGreaterThan(0);
  });

  it('shows a 409 from the API instead of re-implementing the state machine', async () => {
    mockApi({
      ...routes(adminSession),
      [`POST ${apiPath}/state`]: () => errorBody(409, 'invalid_state_transition', 'cannot verify a withdrawn solution', 'req-409'),
    });
    const user = userEvent.setup();
    renderApp(path);
    await user.click(await screen.findByRole('button', { name: 'Verify' }));
    await user.click(screen.getByRole('button', { name: 'Verify solution' }));
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('cannot verify a withdrawn solution');
    expect(alert).toHaveTextContent('req-409');
  });

  it('keeps solution B and targets B after a delayed verification of A completes', async () => {
    const pending = deferred<Response>();
    const a = solutionFixture({
      title: 'Solution A',
      other_versions: [{ id: OTHER_SOLUTION_ID, version: 2, state: 'candidate', applicability: {} }],
    });
    const b = solutionFixture({ id: OTHER_SOLUTION_ID, title: 'Solution B' });
    const bPath = `/v1/solutions/${OTHER_SOLUTION_ID}`;
    const m = mockApi({
      ...routes(adminSession, a),
      [`GET ${bPath}`]: () => json(200, b),
      [`GET ${bPath}/propagation`]: () => json(200, { solution_id: OTHER_SOLUTION_ID, notice: '', candidates: [] }),
      [`POST ${apiPath}/state`]: () => pending.promise,
      [`POST ${bPath}/state`]: () => json(200, { ...b, verification: { ...b.verification, state: 'verified' } }),
    });
    const user = userEvent.setup();
    render(<MemoryRouter initialEntries={[path]}><CurrentPath /><App /></MemoryRouter>);
    await user.click(await screen.findByRole('button', { name: 'Verify' }));
    await user.click(screen.getByRole('button', { name: 'Verify solution' }));
    await user.click(screen.getByRole('link', { name: 'Version 2' }));
    expect(await screen.findByRole('heading', { name: 'Solution B' })).toBeInTheDocument();
    await act(async () => pending.resolve(json(200, { ...a, verification: { ...a.verification, state: 'verified' } })));
    expect(screen.getByRole('heading', { name: 'Solution B' })).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Solution A' })).not.toBeInTheDocument();
    expect(screen.getByLabelText('Current path')).toHaveTextContent(`/knowledge/${OTHER_SOLUTION_ID}`);
    await user.click(screen.getByRole('button', { name: 'Verify' }));
    await user.click(screen.getByRole('button', { name: 'Verify solution' }));
    expect(await screen.findByText('Solution verified.')).toBeInTheDocument();
    expect(m.callsTo('POST', `${apiPath}/state`)).toHaveLength(1);
    expect(m.callsTo('POST', `${bPath}/state`)).toHaveLength(1);
  });

  it('deletes permanently only after an explicit, irreversible confirmation', async () => {
    const m = mockApi({
      ...routes(adminSession),
      [`DELETE ${apiPath}`]: () => json(204),
    });
    const user = userEvent.setup();
    renderApp(path);
    await user.click(await screen.findByRole('button', { name: 'Delete permanently…' }));
    const dialog = screen.getByRole('alertdialog', { name: 'Delete this solution permanently?' });
    expect(dialog).toHaveTextContent('cannot be undone');
    expect(dialog).toHaveTextContent(/leaked secrets/);
    expect(dialog).toHaveTextContent(/Withdrawing is the normal way/);
    expect(m.callsTo('DELETE', apiPath)).toHaveLength(0);
    await user.click(within(dialog).getByRole('button', { name: 'Delete permanently' }));
    expect(await screen.findByRole('heading', { name: 'Knowledge' })).toBeInTheDocument();
    expect(m.callsTo('DELETE', apiPath)[0]?.headers.get('X-CSRF-Token')).toBe('csrf-admin-token');
  });

  it('lists sibling locations with a link to the related solution', async () => {
    mockApi(routes(adminSession));
    renderApp(path);
    const section = await screen.findByRole('region', { name: 'May also apply to' });
    expect(await within(section).findByText('acme/worker')).toBeInTheDocument();
    expect(within(section).getByText('Candidates only. Antibody never applies changes.')).toBeInTheDocument();
    expect(within(section).getByRole('link', { name: 'Related solution' })).toHaveAttribute('href', `/knowledge/${OTHER_SOLUTION_ID}`);
  });

  it('shows the empty sibling state', async () => {
    mockApi({
      ...routes(adminSession),
      [`GET ${apiPath}/propagation`]: () => json(200, { solution_id: SOLUTION_ID, candidates: [], notice: 'Informational only.' }),
    });
    renderApp(path);
    expect(await screen.findByText('No sibling locations found')).toBeInTheDocument();
    expect(screen.getByText('Informational only.')).toBeInTheDocument();
  });
});
