import createClient, { type Client, type Middleware } from 'openapi-fetch';
import type { paths } from './schema';

/*
 * Typed client for the same-origin Rust API. Authentication is the HttpOnly
 * session cookie set by POST /v1/auth/login; the CSRF token returned in
 * SessionInfo lives only in this module's memory (never in Web Storage) and
 * is re-fetched from GET /v1/auth/session after a reload.
 */

let csrfToken: string | null = null;
let sessionRevision = 0;

export function setCsrfToken(token: string | null | undefined): void {
  csrfToken = token ?? null;
  ++sessionRevision;
}

export function getCsrfToken(): string | null {
  return csrfToken;
}

const SAFE_METHODS = new Set(['GET', 'HEAD', 'OPTIONS', 'TRACE']);

/** Adds `X-CSRF-Token` to every mutating request (POST/PUT/PATCH/DELETE). */
export const csrfMiddleware: Middleware = {
  onRequest({ request }) {
    if (!SAFE_METHODS.has(request.method.toUpperCase()) && csrfToken) {
      request.headers.set('X-CSRF-Token', csrfToken);
    }
    return request;
  },
};

type UnauthorizedHandler = () => void;
let unauthorizedHandler: UnauthorizedHandler | null = null;

/** Called when any API call (other than login/session probing) returns 401. */
export function setUnauthorizedHandler(handler: UnauthorizedHandler | null): void {
  unauthorizedHandler = handler;
}

const AUTH_PROBES = new Set(['/v1/auth/login', '/v1/auth/session', '/v1/auth/logout']);
const requestSessions = new WeakMap<Request, number>();

export const sessionExpiryMiddleware: Middleware = {
  onRequest({ request }) {
    requestSessions.set(request, sessionRevision);
  },
  onResponse({ request, response, schemaPath }) {
    if (response.status === 401 && !AUTH_PROBES.has(schemaPath) && requestSessions.get(request) === sessionRevision) {
      unauthorizedHandler?.();
    }
    return undefined;
  },
};

function currentOrigin(): string {
  // Absolute base URL: `Request` rejects relative URLs outside browsers (tests).
  return typeof window !== 'undefined' && window.location ? window.location.origin : 'http://localhost';
}

export type ApiClient = Client<paths>;

export function createApiClient(): ApiClient {
  const client = createClient<paths>({
    baseUrl: currentOrigin(),
    credentials: 'same-origin',
    headers: { Accept: 'application/json' },
    // Resolve fetch at call time so tests can stub it.
    fetch: (request: Request) => globalThis.fetch(request),
  });
  client.use(csrfMiddleware, sessionExpiryMiddleware);
  return client;
}

export const api: ApiClient = createApiClient();
