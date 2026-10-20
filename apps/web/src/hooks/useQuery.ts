import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ApiError, asApiError } from '../api/errors';

/** Data older than this is shown as stale (with a refresh button). */
export const STALE_AFTER_MS = 60_000;

export type QueryStatus = 'loading' | 'error' | 'success';

export interface QueryResult<T> {
  data: T | undefined;
  error: ApiError | undefined;
  status: QueryStatus;
  /** A request is in flight (initial load or refresh). */
  isFetching: boolean;
  /** Epoch milliseconds of the last successful load. */
  updatedAt: number | undefined;
  refetch: () => void;
  /** Replace this key's data and discard earlier in-flight reads. */
  setData: (next: T) => void;
}

interface InternalState<T> {
  key: string | null;
  data: T | undefined;
  error: ApiError | undefined;
  isFetching: boolean;
  updatedAt: number | undefined;
}

/**
 * Minimal data-loading hook: one request per `key` (pass `null` to disable),
 * manual refetch, out-of-order responses ignored. Data for a previous key is
 * never shown for a new key.
 */
export function useQuery<T>(key: string | null, fetcher: () => Promise<T>): QueryResult<T> {
  const fetcherRef = useRef(fetcher);
  fetcherRef.current = fetcher;
  const scope = useMemo(() => ({ key }), [key]);
  const currentScope = useRef(scope);
  currentScope.current = scope;
  const seq = useRef(0);
  const [nonce, setNonce] = useState(0);
  const [state, setState] = useState<InternalState<T>>({
    key,
    data: undefined,
    error: undefined,
    isFetching: key !== null,
    updatedAt: undefined,
  });

  useEffect(() => {
    if (key === null) return;
    const id = ++seq.current;
    setState((prev) =>
      prev.key === key
        ? { ...prev, isFetching: true }
        : { key, data: undefined, error: undefined, isFetching: true, updatedAt: undefined },
    );
    fetcherRef
      .current()
      .then((data) => {
        if (id !== seq.current || currentScope.current !== scope) return;
        setState({ key, data, error: undefined, isFetching: false, updatedAt: Date.now() });
      })
      .catch((e: unknown) => {
        if (id !== seq.current || currentScope.current !== scope) return;
        setState((prev) => ({ ...prev, key, error: asApiError(e), isFetching: false }));
      });
    return () => {
      if (id === seq.current) ++seq.current;
    };
  }, [key, scope, nonce]);

  const refetch = useCallback(() => setNonce((n) => n + 1), []);
  const setData = useCallback(
    (next: T) => {
      if (key === null || currentScope.current !== scope) return;
      ++seq.current;
      setState({ key, data: next, error: undefined, isFetching: false, updatedAt: Date.now() });
    },
    [key, scope],
  );

  const sameKey = state.key === key;
  const data = sameKey ? state.data : undefined;
  const error = sameKey ? state.error : undefined;
  const isFetching = key !== null && (!sameKey || state.isFetching);
  const status: QueryStatus = data !== undefined ? 'success' : error ? 'error' : 'loading';
  return {
    data,
    error,
    status,
    isFetching,
    updatedAt: sameKey ? state.updatedAt : undefined,
    refetch,
    setData,
  };
}
