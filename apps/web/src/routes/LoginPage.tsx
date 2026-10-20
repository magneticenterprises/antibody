import { useState, type FormEvent } from 'react';
import { Navigate, useLocation, useNavigate } from 'react-router';
import { asApiError, type ApiError } from '../api/errors';
import { useAuth } from '../auth/AuthContext';
import { ErrorNotice, Loading } from '../components/Status';
import { useDocumentTitle } from '../hooks/misc';

interface LoginLocationState {
  from?: string;
  expired?: boolean;
}

function safeReturnPath(from: string | undefined): string {
  // Only same-app paths; never protocol-relative or absolute URLs.
  if (!from || !from.startsWith('/') || from.startsWith('//') || from.startsWith('/login')) return '/';
  return from;
}

export function LoginPage() {
  useDocumentTitle('Sign in');
  const { state, login } = useAuth();
  const location = useLocation();
  const navigate = useNavigate();
  const locState = (location.state ?? {}) as LoginLocationState;
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<ApiError | undefined>();

  if (state.status === 'authenticated' && !pending) {
    return <Navigate to={safeReturnPath(locState.from)} replace />;
  }

  const onSubmit = async (e: FormEvent) => {
    e.preventDefault();
    setError(undefined);
    setPending(true);
    try {
      await login(username.trim(), password);
      setPassword('');
      navigate(safeReturnPath(locState.from), { replace: true });
    } catch (err) {
      setError(asApiError(err));
      setPending(false);
    }
  };

  const expired = locState.expired || (state.status === 'anonymous' && state.expired);

  return (
    <main id="main" className="login">
      <div className="login__card">
        <p className="brand__name brand__name--large">Antibody</p>
        <h1>Sign in</h1>
        <p className="lede">Collective memory for your coding agents.</p>
        {state.status === 'loading' ? <Loading label="your session" /> : null}
        {expired && !error ? (
          <p className="notice notice--info" role="status">
            Your session has ended. Sign in again to continue.
          </p>
        ) : null}
        {error ? (
          <ErrorNotice
            error={error}
            title={error.code === 'rate_limited' ? 'Too many attempts' : error.isUnauthenticated ? 'Sign-in failed' : undefined}
          />
        ) : null}
        <form className="form" onSubmit={onSubmit} aria-describedby="login-help">
          <div className="field">
            <label htmlFor="login-username">Username</label>
            <input
              id="login-username"
              name="username"
              autoComplete="username"
              autoCapitalize="none"
              spellCheck={false}
              required
              maxLength={128}
              value={username}
              onChange={(e) => setUsername(e.target.value)}
            />
          </div>
          <div className="field">
            <label htmlFor="login-password">Password</label>
            <input
              id="login-password"
              name="password"
              type="password"
              autoComplete="current-password"
              required
              maxLength={256}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
          </div>
          <button type="submit" className="button button--primary button--block" disabled={pending}>
            {pending ? 'Signing in…' : 'Sign in'}
          </button>
          <p id="login-help" className="hint">
            Accounts are created by an administrator. Agent API tokens cannot be used here.
          </p>
          <p className="visually-hidden" role="status" aria-live="polite">
            {pending ? 'Signing in' : ''}
          </p>
        </form>
      </div>
    </main>
  );
}
