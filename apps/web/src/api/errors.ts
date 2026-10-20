import type { ErrorBody, ErrorCode } from './types';

/** Codes produced by the client itself (never by the API). */
export type ClientErrorCode = 'network_error' | 'unexpected_response';

/** A failed API call, normalized from the `ErrorBody` contract. */
export class ApiError extends Error {
  readonly status: number;
  readonly code: ErrorCode | ClientErrorCode;
  readonly requestId: string | null;

  constructor(status: number, code: ErrorCode | ClientErrorCode, message: string, requestId: string | null) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
    this.requestId = requestId;
  }

  /** Authorization denied (not a CSRF failure, which also uses 403). */
  get isForbidden(): boolean {
    return this.status === 403 && this.code !== 'csrf_failed';
  }

  get isUnauthenticated(): boolean {
    return this.status === 401;
  }

  get isNotFound(): boolean {
    return this.status === 404;
  }
}

export function isErrorBody(value: unknown): value is ErrorBody {
  if (typeof value !== 'object' || value === null || !('error' in value)) return false;
  const detail = (value as { error: unknown }).error;
  return (
    typeof detail === 'object' &&
    detail !== null &&
    typeof (detail as { code?: unknown }).code === 'string' &&
    typeof (detail as { message?: unknown }).message === 'string'
  );
}

/** Build an ApiError from an openapi-fetch error payload and its response. */
export function toApiError(error: unknown, response: Response): ApiError {
  const headerId = response.headers.get('x-request-id');
  if (isErrorBody(error)) {
    return new ApiError(response.status, error.error.code, error.error.message, error.error.request_id ?? headerId);
  }
  const fallback =
    response.status === 502 || response.status === 503 || response.status === 504
      ? 'The Antibody API is unavailable. Try again shortly.'
      : `Unexpected response from the API (HTTP ${response.status}).`;
  return new ApiError(response.status, 'unexpected_response', fallback, headerId);
}

export function networkError(): ApiError {
  return new ApiError(0, 'network_error', 'Could not reach the Antibody API. Check your connection and try again.', null);
}

type CallResult<D> = { data?: D; error?: unknown; response: Response };

/**
 * Await an openapi-fetch call and return its data, or throw an ApiError.
 * The data type is inferred from the generated schema.
 */
export async function unwrap<D>(call: Promise<CallResult<D>>): Promise<D> {
  let result: CallResult<D>;
  try {
    result = await call;
  } catch {
    throw networkError();
  }
  const { data, error, response } = result;
  if (!response.ok) throw toApiError(error, response);
  return data as D;
}

/** Same as `unwrap` for endpoints that answer 204 No Content. */
export async function unwrapVoid(call: Promise<CallResult<unknown>>): Promise<void> {
  await unwrap(call);
}

export function asApiError(e: unknown): ApiError {
  if (e instanceof ApiError) return e;
  return new ApiError(0, 'unexpected_response', e instanceof Error ? e.message : 'Unexpected error', null);
}
