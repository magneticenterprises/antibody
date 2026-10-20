import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { api, setCsrfToken, setUnauthorizedHandler } from '../api/client';
import { ApiError, asApiError, unwrap } from '../api/errors';
import type { SessionInfo } from '../api/types';

export type AuthState =
  | { status: 'loading' }
  | { status: 'anonymous'; expired: boolean }
  | { status: 'authenticated'; session: SessionInfo }
  | { status: 'error'; error: ApiError };

interface AuthContextValue {
  state: AuthState;
  login: (username: string, password: string) => Promise<SessionInfo>;
  logout: () => Promise<void>;
  /** Re-probe GET /v1/auth/session (e.g. after a network error). */
  reload: () => void;
}

const AuthContext = createContext<AuthContextValue | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<AuthState>({ status: 'loading' });
  const [probe, setProbe] = useState(0);
  const revision = useRef(0);

  useEffect(() => {
    let active = true;
    const id = ++revision.current;
    setState({ status: 'loading' });
    unwrap(api.GET('/v1/auth/session'))
      .then((session) => {
        if (!active || id !== revision.current) return;
        setCsrfToken(session.csrf_token);
        setState({ status: 'authenticated', session });
      })
      .catch((e: unknown) => {
        if (!active || id !== revision.current) return;
        const err = asApiError(e);
        setCsrfToken(null);
        setState(err.isUnauthenticated ? { status: 'anonymous', expired: false } : { status: 'error', error: err });
      });
    return () => {
      active = false;
    };
  }, [probe]);

  useEffect(() => {
    setUnauthorizedHandler(() => {
      ++revision.current;
      setCsrfToken(null);
      setState((prev) => (prev.status === 'anonymous' ? prev : { status: 'anonymous', expired: true }));
    });
    return () => {
      ++revision.current;
      setUnauthorizedHandler(null);
    };
  }, []);

  const login = useCallback(async (username: string, password: string) => {
    const id = ++revision.current;
    try {
      const session = await unwrap(api.POST('/v1/auth/login', { body: { username, password } }));
      if (id === revision.current) {
        setCsrfToken(session.csrf_token);
        setState({ status: 'authenticated', session });
      }
      return session;
    } catch (error) {
      if (id === revision.current) {
        setState((prev) => (prev.status === 'loading' ? { status: 'anonymous', expired: false } : prev));
      }
      throw error;
    }
  }, []);

  const logout = useCallback(async () => {
    const id = ++revision.current;
    try {
      await unwrap(api.POST('/v1/auth/logout'));
    } catch (e) {
      // Already signed out server-side: fine. Anything else: keep the local
      // session so the user can retry (the cookie may still be valid).
      const err = asApiError(e);
      if (!err.isUnauthenticated) throw err;
    }
    if (id === revision.current) {
      setCsrfToken(null);
      setState({ status: 'anonymous', expired: false });
    }
  }, []);

  const reload = useCallback(() => {
    ++revision.current;
    setProbe((n) => n + 1);
  }, []);

  const value = useMemo(() => ({ state, login, logout, reload }), [state, login, logout, reload]);
  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth(): AuthContextValue {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error('useAuth must be used inside <AuthProvider>');
  return ctx;
}

/** The signed-in session (only valid below <RequireAuth>). */
export function useSession(): SessionInfo {
  const { state } = useAuth();
  if (state.status !== 'authenticated') throw new Error('useSession requires an authenticated session');
  return state.session;
}

export function useIsAdmin(): boolean {
  return useSession().role === 'admin';
}
