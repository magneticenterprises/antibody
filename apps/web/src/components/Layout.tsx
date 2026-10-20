import { useState, type ReactNode } from 'react';
import { NavLink, Navigate, Outlet, useLocation, useNavigate } from 'react-router';
import { asApiError, type ApiError } from '../api/errors';
import { useAuth, useSession } from '../auth/AuthContext';
import { ErrorNotice, Forbidden, Loading } from './Status';

const NAV = [
  { to: '/', label: 'Overview', end: true },
  { to: '/repositories', label: 'Repositories', end: false },
  { to: '/knowledge', label: 'Knowledge', end: false },
  { to: '/activity', label: 'Activity', end: false },
];

export function Layout() {
  const session = useSession();
  const { logout } = useAuth();
  const navigate = useNavigate();
  const [signOutError, setSignOutError] = useState<ApiError | undefined>();
  const isAdmin = session.role === 'admin';
  const items = isAdmin ? [...NAV, { to: '/settings', label: 'Settings', end: false }] : NAV;

  return (
    <div className="app">
      <a className="skip-link" href="#main">
        Skip to content
      </a>
      <header className="site-header">
        <div className="site-header__inner">
          <div className="brand">
            <span className="brand__name">Antibody</span>
            <span className="brand__org">{session.organization_name}</span>
          </div>
          <nav aria-label="Primary" className="primary-nav">
            <ul>
              {items.map((item) => (
                <li key={item.to}>
                  <NavLink to={item.to} end={item.end} className={({ isActive }) => (isActive ? 'nav-link nav-link--active' : 'nav-link')}>
                    {item.label}
                  </NavLink>
                </li>
              ))}
            </ul>
          </nav>
          <div className="account">
            <span className="account__who">
              <span className="account__name">{session.display_name}</span>{' '}
              <span className="account__role">{session.role === 'admin' ? 'Administrator' : 'Member'}</span>
            </span>
            <button
              type="button"
              className="button button--quiet"
              onClick={() => {
                setSignOutError(undefined);
                logout()
                  .then(() => navigate('/login', { replace: true }))
                  .catch((e: unknown) => setSignOutError(asApiError(e)));
              }}
            >
              Sign out
            </button>
          </div>
        </div>
      </header>
      <main id="main" className="main" tabIndex={-1}>
        {signOutError ? <ErrorNotice error={signOutError} title="Sign-out did not complete on the server" /> : null}
        <Outlet />
      </main>
      <footer className="site-footer">
        <p>
          Signed in to <strong>{session.organization_slug}</strong>. Suggestions and evidence marked untrusted come from
          agents and are never treated as verification.
        </p>
      </footer>
    </div>
  );
}

/** Gate for signed-in routes: probes the session, else redirects to /login. */
export function RequireAuth({ children }: { children: ReactNode }) {
  const { state, reload } = useAuth();
  const location = useLocation();
  if (state.status === 'loading') {
    return (
      <main id="main" className="main main--center">
        <Loading label="your session" />
      </main>
    );
  }
  if (state.status === 'error') {
    return (
      <main id="main" className="main main--center">
        <h1 className="visually-hidden">Antibody</h1>
        <ErrorNotice error={state.error} onRetry={reload} title="Could not check your session" />
      </main>
    );
  }
  if (state.status === 'anonymous') {
    const from = location.pathname + location.search;
    return <Navigate to="/login" replace state={{ from, expired: state.expired }} />;
  }
  return <>{children}</>;
}

/** Hides admin-only views from members; the API still enforces access (403). */
export function RequireAdmin({ children, what }: { children: ReactNode; what: string }) {
  const session = useSession();
  if (session.role !== 'admin') {
    return (
      <>
        <header className="page-header">
          <h1 tabIndex={-1}>Administrator access required</h1>
        </header>
        <Forbidden what={what} />
      </>
    );
  }
  return <>{children}</>;
}

export function NotFound() {
  return (
    <>
      <header className="page-header">
        <h1 tabIndex={-1}>Page not found</h1>
      </header>
      <p>
        There is nothing at this address. <NavLink to="/">Go to the overview</NavLink>.
      </p>
    </>
  );
}
