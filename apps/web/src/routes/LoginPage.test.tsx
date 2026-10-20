import { act, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { getCsrfToken } from '../api/client';
import { adminSession, deferred, errorBody, json, memberSession, mockApi, renderApp, sessionRoute } from '../test/utils';

const overview = {
  stats: {
    repositories: 1,
    sessions_total: 1,
    sessions_7d: 1,
    extraction_pending: 0,
    extraction_failed: 0,
    solutions_total: 1,
    solutions_candidate: 1,
    solutions_corroborated: 0,
    solutions_verified: 0,
    solutions_needing_revalidation: 0,
    retrievals_7d: 0,
    retrievals_with_results_7d: 0,
    retrieval_context_tokens_7d: 0,
    outcomes_used: 0,
    outcomes_rejected: 0,
    outcomes_inapplicable: 0,
    confirmed_reuse: 0,
    ci_events_7d: 0,
    ci_events_with_matches_7d: 0,
  },
  queue: null,
};

describe('login', () => {
  it('redirects anonymous visitors to a labelled sign-in form', async () => {
    mockApi({ 'GET /v1/auth/session': sessionRoute(null) });
    renderApp('/knowledge');
    expect(await screen.findByRole('heading', { name: 'Sign in' })).toBeInTheDocument();
    expect(screen.getByLabelText('Username')).toHaveAttribute('autocomplete', 'username');
    expect(screen.getByLabelText('Password')).toHaveAttribute('type', 'password');
  });

  it('signs in, keeps the CSRF token in memory and returns to the requested page', async () => {
    const m = mockApi({
      'GET /v1/auth/session': sessionRoute(null),
      'POST /v1/auth/login': () => json(200, adminSession),
      'GET /v1/overview': () => json(200, overview),
    });
    const user = userEvent.setup();
    renderApp('/');
    await user.type(await screen.findByLabelText('Username'), 'admin');
    await user.type(screen.getByLabelText('Password'), 'correct horse battery');
    await user.click(screen.getByRole('button', { name: 'Sign in' }));

    expect(await screen.findByRole('heading', { name: 'Overview' })).toBeInTheDocument();
    expect(m.callsTo('POST', '/v1/auth/login')[0]?.body).toEqual({ username: 'admin', password: 'correct horse battery' });
    expect(getCsrfToken()).toBe('csrf-admin-token');
    expect(await screen.findByText('Used outcomes confirmed by signed CI')).toBeInTheDocument();
  });

  it('shows the API error message and request id on a bad login', async () => {
    mockApi({
      'GET /v1/auth/session': sessionRoute(null),
      'POST /v1/auth/login': () => errorBody(401, 'unauthenticated', 'invalid username or password', 'req-login-9'),
    });
    const user = userEvent.setup();
    renderApp('/login');
    await user.type(await screen.findByLabelText('Username'), 'admin');
    await user.type(screen.getByLabelText('Password'), 'wrong password!');
    await user.click(screen.getByRole('button', { name: 'Sign in' }));

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('Sign-in failed');
    expect(alert).toHaveTextContent('invalid username or password');
    expect(alert).toHaveTextContent('req-login-9');
    expect(screen.getByRole('heading', { name: 'Sign in' })).toBeInTheDocument();
  });

  it.each(['anonymous', 'other user'])('keeps a successful login when the initial session probe later returns %s', async (oldSession) => {
    const probe = deferred<Response>();
    mockApi({
      'GET /v1/auth/session': () => probe.promise,
      'POST /v1/auth/login': () => json(200, adminSession),
      'GET /v1/overview': () => json(200, overview),
    });
    const user = userEvent.setup();
    renderApp('/login');
    await user.type(await screen.findByLabelText('Username'), 'admin');
    await user.type(screen.getByLabelText('Password'), 'synthetic-password');
    await user.click(screen.getByRole('button', { name: 'Sign in' }));
    expect(await screen.findByRole('heading', { name: 'Overview' })).toBeInTheDocument();
    await act(async () => probe.resolve(oldSession === 'anonymous'
      ? errorBody(401, 'unauthenticated', 'missing credentials')
      : json(200, memberSession)));
    expect(screen.getByRole('heading', { name: 'Overview' })).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'Sign in' })).not.toBeInTheDocument();
    expect(getCsrfToken()).toBe(adminSession.csrf_token);
  });
});
