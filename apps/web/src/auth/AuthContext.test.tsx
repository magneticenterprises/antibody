import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { api, getCsrfToken, setCsrfToken } from '../api/client';
import { adminSession, deferred, errorBody, json, memberSession, mockApi } from '../test/utils';
import { AuthProvider, useAuth } from './AuthContext';

function AuthControls() {
  const { state, login, logout, reload } = useAuth();
  return <>
    <output aria-label="Auth state">{state.status}</output>
    <button onClick={() => void login('admin', 'synthetic-password').catch(() => undefined)}>Login</button>
    <button onClick={() => void logout()}>Logout</button>
    <button onClick={reload}>Reload</button>
  </>;
}

function renderAuth() {
  setCsrfToken(null);
  return render(<AuthProvider><AuthControls /></AuthProvider>);
}

describe('session probe invalidation', () => {
  it('does not restore an old session after logout', async () => {
    const probe = deferred<Response>();
    mockApi({
      'GET /v1/auth/session': () => probe.promise,
      'POST /v1/auth/logout': () => json(204),
    });
    const user = userEvent.setup();
    renderAuth();
    await user.click(screen.getByRole('button', { name: 'Logout' }));
    await waitFor(() => expect(screen.getByLabelText('Auth state')).toHaveTextContent('anonymous'));
    await act(async () => probe.resolve(json(200, adminSession)));
    expect(screen.getByLabelText('Auth state')).toHaveTextContent('anonymous');
    expect(getCsrfToken()).toBeNull();
  });

  it('ignores an earlier probe after a reload', async () => {
    const oldProbe = deferred<Response>();
    let reads = 0;
    mockApi({ 'GET /v1/auth/session': () => ++reads === 1 ? oldProbe.promise : json(200, memberSession) });
    const user = userEvent.setup();
    renderAuth();
    await user.click(screen.getByRole('button', { name: 'Reload' }));
    await waitFor(() => expect(getCsrfToken()).toBe(memberSession.csrf_token));
    await act(async () => oldProbe.resolve(errorBody(401, 'unauthenticated', 'old session ended')));
    expect(screen.getByLabelText('Auth state')).toHaveTextContent('authenticated');
    expect(getCsrfToken()).toBe(memberSession.csrf_token);
  });

  it('does not restore a session after a current API call expires it during a probe', async () => {
    const probe = deferred<Response>();
    mockApi({
      'GET /v1/auth/session': () => probe.promise,
      'GET /v1/overview': () => errorBody(401, 'unauthenticated', 'session ended'),
    });
    renderAuth();
    await act(async () => { await api.GET('/v1/overview'); });
    await act(async () => probe.resolve(json(200, adminSession)));
    expect(screen.getByLabelText('Auth state')).toHaveTextContent('anonymous');
    expect(getCsrfToken()).toBeNull();
  });

  it('ends the loading state when login fails after invalidating the initial probe', async () => {
    const probe = deferred<Response>();
    mockApi({
      'GET /v1/auth/session': () => probe.promise,
      'POST /v1/auth/login': () => errorBody(401, 'unauthenticated', 'invalid credentials'),
    });
    const user = userEvent.setup();
    renderAuth();
    await user.click(screen.getByRole('button', { name: 'Login' }));
    await waitFor(() => expect(screen.getByLabelText('Auth state')).toHaveTextContent('anonymous'));
    await act(async () => probe.resolve(json(200, adminSession)));
    expect(screen.getByLabelText('Auth state')).toHaveTextContent('anonymous');
    expect(getCsrfToken()).toBeNull();
  });
});
