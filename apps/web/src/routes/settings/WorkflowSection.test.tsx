import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { adminSession, errorBody, json, mockApi, renderApp, sessionRoute } from '../../test/utils';

const created = {
  id: '0190abab-0000-7000-8000-000000000001',
  target_scope: 'organization',
  title: 'Run cargo check before tests',
  proposed_change: 'Agents should run cargo check before cargo test to fail fast.',
  version: 1,
  state: 'candidate',
  created_at: '2026-10-19T10:00:00Z',
  updated_at: '2026-10-19T10:00:00Z',
};

function routes(post: () => Response) {
  return {
    'GET /v1/auth/session': sessionRoute(adminSession),
    'GET /v1/admin/workflow-improvements': () => json(200, []),
    'GET /v1/repositories': () => json(200, { items: [] }),
    'POST /v1/admin/workflow-improvements': post,
  };
}

async function propose(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByText('Propose an improvement'));
  await user.type(screen.getByLabelText('Title'), created.title);
  await user.type(screen.getByLabelText('Proposed change'), created.proposed_change);
  await user.click(screen.getByRole('button', { name: 'Propose improvement' }));
}

describe('workflow improvements', () => {
  it('proposes an improvement and lists it as a candidate', async () => {
    const m = mockApi(routes(() => json(201, created)));
    const user = userEvent.setup();
    renderApp('/settings/workflow');
    expect(await screen.findByText('No workflow improvements proposed yet.')).toBeInTheDocument();
    await propose(user);

    expect(await screen.findByText(`Proposed “${created.title}” as a candidate.`)).toBeInTheDocument();
    const card = screen.getByRole('heading', { level: 3, name: created.title }).closest('li') as HTMLElement;
    expect(within(card).getByText('Candidate', { selector: '.badge' })).toBeInTheDocument();
    expect(m.callsTo('POST', '/v1/admin/workflow-improvements')[0]?.body).toEqual({
      title: created.title,
      proposed_change: created.proposed_change,
      target_scope: 'organization',
      repository_id: null,
    });
  });

  it('shows the 409 for an equivalent proposal', async () => {
    mockApi(routes(() => errorBody(409, 'conflict', 'an equivalent proposal already exists', 'req-wf-409')));
    const user = userEvent.setup();
    renderApp('/settings/workflow');
    await propose(user);
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('Conflict');
    expect(alert).toHaveTextContent('an equivalent proposal already exists');
    expect(alert).toHaveTextContent('req-wf-409');
  });
});
