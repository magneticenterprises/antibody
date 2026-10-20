import { afterEach, describe, expect, it, vi } from 'vitest';
import { deferred, errorBody, json, mockApi } from '../test/utils';
import { api, setCsrfToken, setUnauthorizedHandler } from './client';
import { ApiError, unwrap, unwrapVoid } from './errors';

afterEach(() => {
  setCsrfToken(null);
  setUnauthorizedHandler(null);
});

describe('CSRF middleware', () => {
  it('sends X-CSRF-Token on mutating requests only', async () => {
    const m = mockApi({
      'POST /v1/admin/retention/sweep': () =>
        json(200, { payloads_purged: 0, web_sessions_expired: 0, deliveries_pruned: 0, jobs_pruned: 0 }),
      'PATCH /v1/repositories/0190cccc-0000-7000-8000-000000000001': () => json(204),
      'DELETE /v1/admin/tokens/0190cccc-0000-7000-8000-000000000009': () => json(204),
      'PUT /v1/admin/grants': () => json(204),
      'GET /v1/overview': () => json(200, { stats: {}, queue: null }),
    });
    setCsrfToken('tok-123');

    await unwrap(api.POST('/v1/admin/retention/sweep'));
    await unwrapVoid(
      api.PATCH('/v1/repositories/{id}', {
        params: { path: { id: '0190cccc-0000-7000-8000-000000000001' } },
        body: { status: 'disconnected' },
      }),
    );
    await unwrapVoid(api.DELETE('/v1/admin/tokens/{id}', { params: { path: { id: '0190cccc-0000-7000-8000-000000000009' } } }));
    await unwrapVoid(
      api.PUT('/v1/admin/grants', {
        body: { principal_id: '0190cccc-0000-7000-8000-000000000002', repository_id: '0190cccc-0000-7000-8000-000000000003', permission: 'read' },
      }),
    );
    await unwrap(api.GET('/v1/overview'));

    const byMethod = Object.fromEntries(m.calls.map((c) => [c.method, c.headers.get('X-CSRF-Token')]));
    expect(byMethod).toEqual({ POST: 'tok-123', PATCH: 'tok-123', DELETE: 'tok-123', PUT: 'tok-123', GET: null });
  });

  it('omits the header when no token is known, and the token never touches Web Storage', async () => {
    const m = mockApi({ 'POST /v1/auth/logout': () => json(204) });
    const setItem = vi.spyOn(Storage.prototype, 'setItem');
    await unwrapVoid(api.POST('/v1/auth/logout'));
    expect(m.calls[0]?.headers.get('X-CSRF-Token')).toBeNull();
    setCsrfToken('memory-only');
    expect(setItem).not.toHaveBeenCalled();
    expect(Object.keys(localStorage)).toHaveLength(0);
    expect(Object.keys(sessionStorage)).toHaveLength(0);
  });

  it('sends JSON bodies to same-origin /v1 URLs with credentials', async () => {
    const m = mockApi({ 'POST /v1/auth/login': () => errorBody(401, 'unauthenticated', 'invalid username or password') });
    await expect(unwrap(api.POST('/v1/auth/login', { body: { username: 'a', password: 'b' } }))).rejects.toBeInstanceOf(ApiError);
    const call = m.calls[0];
    expect(call?.path).toBe('/v1/auth/login');
    expect(call?.body).toEqual({ username: 'a', password: 'b' });
    expect(call?.headers.get('Content-Type')).toBe('application/json');
  });
});

describe('error normalization', () => {
  it('maps ErrorBody to ApiError with message, code and request id', async () => {
    mockApi({ 'GET /v1/admin/settings': () => errorBody(403, 'forbidden', 'administrator access required', 'req-42') });
    const err = await unwrap(api.GET('/v1/admin/settings')).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    const e = err as ApiError;
    expect(e.status).toBe(403);
    expect(e.code).toBe('forbidden');
    expect(e.message).toBe('administrator access required');
    expect(e.requestId).toBe('req-42');
    expect(e.isForbidden).toBe(true);
  });

  it('does not treat CSRF failures as authorization failures', async () => {
    mockApi({ 'POST /v1/admin/retention/sweep': () => errorBody(403, 'csrf_failed', 'missing or invalid CSRF token') });
    const e = (await unwrap(api.POST('/v1/admin/retention/sweep')).catch((x: unknown) => x)) as ApiError;
    expect(e.code).toBe('csrf_failed');
    expect(e.isForbidden).toBe(false);
  });

  it('handles non-JSON gateway errors and network failures', async () => {
    mockApi({ 'GET /v1/overview': () => new Response('Bad Gateway', { status: 502, headers: { 'x-request-id': 'gw-1' } }) });
    const e = (await unwrap(api.GET('/v1/overview')).catch((x: unknown) => x)) as ApiError;
    expect(e.status).toBe(502);
    expect(e.message).toMatch(/unavailable/i);
    expect(e.requestId).toBe('gw-1');

    vi.stubGlobal('fetch', vi.fn(() => Promise.reject(new TypeError('Failed to fetch'))));
    const n = (await unwrap(api.GET('/v1/overview')).catch((x: unknown) => x)) as ApiError;
    expect(n.code).toBe('network_error');
    expect(n.status).toBe(0);
  });

  it('notifies the auth layer when a call returns 401', async () => {
    mockApi({ 'GET /v1/overview': () => errorBody(401, 'unauthenticated', 'missing or invalid credentials') });
    const handler = vi.fn();
    setUnauthorizedHandler(handler);
    await unwrap(api.GET('/v1/overview')).catch(() => undefined);
    expect(handler).toHaveBeenCalledTimes(1);
  });

  it('ignores a delayed 401 from a previous session', async () => {
    const pending = deferred<Response>();
    const m = mockApi({ 'GET /v1/overview': () => pending.promise });
    const handler = vi.fn();
    setUnauthorizedHandler(handler);
    setCsrfToken('old-session');
    const request = api.GET('/v1/overview');
    await vi.waitFor(() => expect(m.callsTo('GET', '/v1/overview')).toHaveLength(1));
    setCsrfToken('new-session');
    pending.resolve(errorBody(401, 'unauthenticated', 'old session ended'));
    await request;
    expect(handler).not.toHaveBeenCalled();
  });
});
