import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { FreshnessBar } from '../components/Status';
import { adminSession, errorBody, json, memberSession, mockApi, renderApp, SESSION_ID, sessionRoute } from '../test/utils';

describe('route states', () => {
  it('shows a forbidden state with the request id for 403 responses', async () => {
    mockApi({
      'GET /v1/auth/session': sessionRoute(memberSession),
      'GET /v1/overview': () => errorBody(403, 'forbidden', 'this credential lacks the context_read scope', 'req-403'),
    });
    renderApp('/');
    const region = await screen.findByRole('region', { name: 'Access denied' });
    expect(region).toHaveTextContent('this credential lacks the context_read scope');
    expect(region).toHaveTextContent('req-403');
  });

  it('shows the error message, request id and a working retry for server errors', async () => {
    let fail = true;
    const m = mockApi({
      'GET /v1/auth/session': sessionRoute(adminSession),
      'GET /v1/repositories': () => (fail ? errorBody(500, 'internal', 'internal error', 'req-500') : json(200, { items: [] })),
    });
    const user = userEvent.setup();
    renderApp('/repositories');
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('Something went wrong');
    expect(alert).toHaveTextContent('internal error');
    expect(alert).toHaveTextContent('req-500');
    fail = false;
    await user.click(within(alert).getByRole('button', { name: 'Try again' }));
    expect(await screen.findByText('No repositories yet.')).toBeInTheDocument();
    expect(m.callsTo('GET', '/v1/repositories')).toHaveLength(2);
  });

  it('shows explicit empty states', async () => {
    mockApi({
      'GET /v1/auth/session': sessionRoute(memberSession),
      'GET /v1/sessions': () => json(200, { items: [] }),
    });
    renderApp('/activity/sessions?status=failed');
    expect(await screen.findByText('No sessions with status “failed”.')).toBeInTheDocument();
  });

  it('marks data older than a minute as stale and offers a refresh', async () => {
    const onRefresh = vi.fn();
    const user = userEvent.setup();
    render(<FreshnessBar updatedAt={Date.now() - 120_000} isFetching={false} onRefresh={onRefresh} label="overview" />);
    expect(screen.getByText('Stale')).toBeInTheDocument();
    expect(screen.getByText('2m ago')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Refresh overview' }));
    expect(onRefresh).toHaveBeenCalledTimes(1);
  });

  it('becomes stale over time on a live page', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      mockApi({
        'GET /v1/auth/session': sessionRoute(memberSession),
        'GET /v1/repositories': () => json(200, { items: [] }),
      });
      renderApp('/repositories');
      expect(await screen.findByText('No repositories yet.')).toBeInTheDocument();
      expect(screen.queryByText('Stale')).not.toBeInTheDocument();
      await act(async () => {
        vi.advanceTimersByTime(70_000);
      });
      expect(screen.getByText('Stale')).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it('returns to the sign-in page when the session expires mid-use', async () => {
    let expired = false;
    mockApi({
      'GET /v1/auth/session': sessionRoute(adminSession),
      'GET /v1/overview': () => errorBody(401, 'unauthenticated', 'missing or invalid credentials'),
      'GET /v1/repositories': () => (expired ? errorBody(401, 'unauthenticated', 'missing or invalid credentials') : json(200, { items: [] })),
    });
    renderApp('/repositories');
    expect(await screen.findByText('No repositories yet.')).toBeInTheDocument();
    expired = true;
    await userEvent.setup().click(screen.getByRole('button', { name: 'Refresh repositories' }));
    expect(await screen.findByRole('heading', { name: 'Sign in' })).toBeInTheDocument();
    expect(screen.getByText(/Your session has ended/)).toBeInTheDocument();
  });
});

describe('admin-only areas', () => {
  it('hides settings from members and shows a forbidden view if they navigate there', async () => {
    const m = mockApi({ 'GET /v1/auth/session': sessionRoute(memberSession) });
    renderApp('/settings');
    expect(await screen.findByRole('heading', { name: 'Administrator access required' })).toBeInTheDocument();
    const nav = screen.getByRole('navigation', { name: 'Primary' });
    expect(within(nav).queryByRole('link', { name: 'Settings' })).not.toBeInTheDocument();
    expect(within(nav).getByRole('link', { name: 'Knowledge' })).toBeInTheDocument();
    // No admin endpoint was called on the member's behalf.
    expect(m.calls.filter((c) => c.path.startsWith('/v1/admin'))).toHaveLength(0);
  });

  it('hides the queue tab from members', async () => {
    mockApi({
      'GET /v1/auth/session': sessionRoute(memberSession),
      'GET /v1/sessions': () => json(200, { items: [] }),
    });
    renderApp('/activity');
    const tabs = await screen.findByRole('navigation', { name: 'Activity views' });
    expect(within(tabs).queryByRole('link', { name: 'Queue failures' })).not.toBeInTheDocument();
  });

  it('requires an explicit, audited confirmation before loading a raw transcript', async () => {
    const session = {
      id: SESSION_ID,
      repository: 'acme/api',
      agent_type: 'claude_code',
      ingestion_status: 'accepted',
      extraction_status: 'succeeded',
      capture_mode: 'raw',
      raw_payload_available: true,
      message_count: 2,
      payload_bytes: 400,
      redaction_count: 1,
      solution_ids: [],
      created_at: '2026-10-19T09:00:00Z',
    };
    const m = mockApi({
      'GET /v1/auth/session': sessionRoute(adminSession),
      [`GET /v1/sessions/${SESSION_ID}`]: () => json(200, session),
      [`GET /v1/sessions/${SESSION_ID}/transcript`]: () =>
        json(200, {
          session_id: SESSION_ID,
          messages: [
            { role: 'tool', tool_name: 'bash', content: 'error[E0308]: <b>mismatched</b> types' },
            { role: 'assistant', content: 'Fixed it.' },
          ],
          redaction_count: 1,
          notice: 'Redaction is best-effort. Treat this transcript as untrusted text.',
        }),
    });
    const user = userEvent.setup();
    const { container } = renderApp(`/activity/sessions/${SESSION_ID}`);
    await user.click(await screen.findByRole('button', { name: 'View raw transcript…' }));
    const dialog = screen.getByRole('alertdialog', { name: 'Open the raw transcript?' });
    expect(dialog).toHaveTextContent('recorded in the audit log');
    expect(m.callsTo('GET', `/v1/sessions/${SESSION_ID}/transcript`)).toHaveLength(0);
    await user.click(within(dialog).getByRole('button', { name: 'Open transcript (audited)' }));
    expect(await screen.findByText('error[E0308]: <b>mismatched</b> types')).toBeInTheDocument();
    expect(container.querySelector('b')).toBeNull();
    expect(m.callsTo('GET', `/v1/sessions/${SESSION_ID}/transcript`)).toHaveLength(1);
  });
});
