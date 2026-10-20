import { useCallback, useEffect, useRef, useState } from 'react';
import { ApiError, asApiError } from '../api/errors';

/** Re-render periodically so relative times and staleness stay current. */
export function useNow(intervalMs = 10_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(t);
  }, [intervalMs]);
  return now;
}

export function useDocumentTitle(title: string): void {
  useEffect(() => {
    document.title = title ? `${title} · Antibody` : 'Antibody';
  }, [title]);
}

export interface ActionState<R> {
  pending: boolean;
  error: ApiError | undefined;
  result: R | undefined;
}

/**
 * Track one mutation: pending flag, last error and last result. `run`
 * resolves to the result, or `undefined` when the call failed (the error is
 * kept in state for display).
 */
export function useAction<A extends unknown[], R>(fn: (...args: A) => Promise<R>) {
  const fnRef = useRef(fn);
  fnRef.current = fn;
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const [state, setState] = useState<ActionState<R>>({ pending: false, error: undefined, result: undefined });

  const run = useCallback(async (...args: A): Promise<R | undefined> => {
    setState({ pending: true, error: undefined, result: undefined });
    try {
      const result = await fnRef.current(...args);
      if (mounted.current) setState({ pending: false, error: undefined, result });
      return result;
    } catch (e) {
      if (mounted.current) setState({ pending: false, error: asApiError(e), result: undefined });
      return undefined;
    }
  }, []);

  const reset = useCallback(() => setState({ pending: false, error: undefined, result: undefined }), []);
  return { ...state, run, reset };
}
