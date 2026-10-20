import type { ReactNode } from 'react';
import { Link } from 'react-router';
import type { ApiError } from '../api/errors';
import { STALE_AFTER_MS, type QueryResult } from '../hooks/useQuery';
import { useNow } from '../hooks/misc';
import { formatAge, formatDate } from '../lib/format';

export function Loading({ label }: { label: string }) {
  return (
    <p className="status-line" role="status" aria-live="polite">
      <span className="spinner" aria-hidden="true" /> Loading {label}…
    </p>
  );
}

export function RequestId({ id }: { id: string | null }) {
  if (!id) return null;
  return (
    <span className="request-id">
      Request ID <code>{id}</code>
    </span>
  );
}

function defaultTitle(error: ApiError): string {
  if (error.code === 'csrf_failed') return 'Security check failed';
  if (error.isForbidden) return 'Access denied';
  if (error.status === 409) return 'Conflict';
  if (error.code === 'rate_limited') return 'Too many requests';
  if (error.code === 'network_error') return 'Connection problem';
  if (error.isNotFound) return 'Not found';
  return 'Something went wrong';
}

/** A failed call: the API's message and request id, plus an optional retry. */
export function ErrorNotice({ error, onRetry, title }: { error: ApiError; onRetry?: () => void; title?: string }) {
  const heading = title ?? defaultTitle(error);
  return (
    <div className="notice notice--error" role="alert">
      <p className="notice__title">{heading}</p>
      <p>{error.message}</p>
      {error.code === 'csrf_failed' ? <p>Reload the page to start a fresh session check, then try again.</p> : null}
      <p className="notice__meta">
        {error.status > 0 ? <span>HTTP {error.status}</span> : null}
        {error.code ? (
          <span>
            Code <code>{error.code}</code>
          </span>
        ) : null}
        <RequestId id={error.requestId} />
      </p>
      {onRetry ? (
        <button type="button" className="button" onClick={onRetry}>
          Try again
        </button>
      ) : null}
    </div>
  );
}

/** Shown for 403 responses (and for admin-only views opened by members). */
export function Forbidden({ error, what }: { error?: ApiError | undefined; what?: string }) {
  return (
    <section className="notice notice--forbidden" aria-labelledby="forbidden-title">
      <h2 id="forbidden-title" className="notice__title">
        Access denied
      </h2>
      <p>
        {error?.message ? `${error.message}.` : null} You do not have permission to view {what ?? 'this page'}. Ask an
        administrator if you need access.
      </p>
      {error ? (
        <p className="notice__meta">
          <span>HTTP {error.status}</span>
          <RequestId id={error.requestId} />
        </p>
      ) : null}
      <p>
        <Link to="/">Back to the overview</Link>
      </p>
    </section>
  );
}

export function EmptyState({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <div className="empty">
      <p className="empty__title">{title}</p>
      {children ? <div className="empty__body">{children}</div> : null}
    </div>
  );
}

/** "Updated 12s ago" with a stale marker and a refresh button. */
export function FreshnessBar({
  updatedAt,
  isFetching,
  onRefresh,
  label,
}: {
  updatedAt: number | undefined;
  isFetching: boolean;
  onRefresh: () => void;
  label: string;
}) {
  const now = useNow(5_000);
  const stale = updatedAt !== undefined && now - updatedAt > STALE_AFTER_MS;
  return (
    <div className={`freshness${stale ? ' freshness--stale' : ''}`}>
      <span>
        Last updated{' '}
        {updatedAt !== undefined ? (
          <time dateTime={new Date(updatedAt).toISOString()} title={formatDate(new Date(updatedAt).toISOString())}>
            {formatAge(updatedAt, now)}
          </time>
        ) : (
          '—'
        )}
      </span>
      {stale ? (
        <span className="badge badge--warn" title="Loaded more than a minute ago">
          Stale
        </span>
      ) : null}
      <button
        type="button"
        className="button button--quiet"
        onClick={onRefresh}
        disabled={isFetching}
        aria-label={`Refresh ${label}`}
      >
        {isFetching ? 'Refreshing…' : 'Refresh'}
      </button>
      <span className="visually-hidden" role="status" aria-live="polite">
        {isFetching ? `Refreshing ${label}` : stale ? `${label} may be out of date` : ''}
      </span>
    </div>
  );
}

interface BoundaryProps<T> {
  query: QueryResult<T>;
  /** Plural noun used in status messages, e.g. "repositories". */
  label: string;
  isEmpty?: (data: T) => boolean;
  empty?: ReactNode;
  /** What a 403 means here, e.g. "the job queue". */
  forbiddenWhat?: string;
  freshness?: boolean;
  children: (data: T) => ReactNode;
}

/**
 * Explicit loading, error, forbidden, empty and stale states for one query.
 * Data that loaded once stays visible when a refresh fails.
 */
export function QueryBoundary<T>({
  query,
  label,
  isEmpty,
  empty,
  forbiddenWhat,
  freshness = true,
  children,
}: BoundaryProps<T>) {
  const { data, error, status, isFetching, updatedAt, refetch } = query;
  if (status === 'loading') return <Loading label={label} />;
  if (status === 'error' && error) {
    if (error.isForbidden) return <Forbidden error={error} what={forbiddenWhat ?? label} />;
    if (error.isUnauthenticated) {
      return (
        <div className="notice notice--error" role="alert">
          <p className="notice__title">Your session has ended</p>
          <p>
            <Link to="/login">Sign in again</Link> to continue.
          </p>
        </div>
      );
    }
    return <ErrorNotice error={error} onRetry={refetch} />;
  }
  if (data === undefined) return <Loading label={label} />;
  return (
    <>
      {freshness ? <FreshnessBar updatedAt={updatedAt} isFetching={isFetching} onRefresh={refetch} label={label} /> : null}
      {error ? <ErrorNotice error={error} onRetry={refetch} title="Refresh failed; showing earlier data" /> : null}
      {isEmpty?.(data) ? (empty ?? <EmptyState title={`No ${label} yet.`} />) : children(data)}
    </>
  );
}

/** Live region for the outcome of a mutation. */
export function ActionStatus({
  pending,
  error,
  success,
  pendingText = 'Working…',
}: {
  pending: boolean;
  error: ApiError | undefined;
  success?: string | undefined;
  pendingText?: string;
}) {
  return (
    <>
      <p className="action-status" role="status" aria-live="polite">
        {pending ? pendingText : !error && success ? success : ''}
      </p>
      {error && !pending ? <ErrorNotice error={error} /> : null}
    </>
  );
}
