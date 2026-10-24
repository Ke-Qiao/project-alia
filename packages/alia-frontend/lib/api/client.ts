import { ApiError, type JsonValue } from '@/lib/api/types';
import type { SessionContextShape } from '@/lib/api/session-shape';

type RequestOptions = Omit<RequestInit, 'body'> & {
  body?: unknown;
  token?: string | null;
};

export function normalizeApiPath(
  path: string,
  query?: Record<string, string | number | boolean | null | undefined>
) {
  const [pathname, existingQuery] = path.split('?');
  const params = new URLSearchParams(existingQuery ?? '');
  for (const [key, value] of Object.entries(query ?? {})) {
    if (value !== null && value !== undefined && value !== '') {
      params.set(key, String(value));
    }
  }
  const suffix = params.toString();
  return suffix ? `${pathname}?${suffix}` : pathname;
}

export function toApiUrl(baseUrl: string | null, path: string) {
  if (/^https?:\/\//i.test(path)) return path;
  const normalizedPath = path.startsWith('/') ? path : `/${path}`;
  if (!baseUrl) return normalizedPath;
  return `${baseUrl.replace(/\/+$/, '')}${normalizedPath}`;
}

function isJsonResponse(response: Response) {
  return response.headers.get('content-type')?.includes('application/json') ?? false;
}

export async function apiRequest<T>(
  baseUrl: string | null,
  path: string,
  { body, token, headers, ...init }: RequestOptions = {}
): Promise<T> {
  const response = await fetch(toApiUrl(baseUrl, path), {
    ...init,
    headers: {
      Accept: 'application/json',
      ...(body !== undefined ? { 'Content-Type': 'application/json' } : null),
      ...(token ? { Authorization: `Bearer ${token}` } : null),
      ...headers,
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });

  const payload = isJsonResponse(response) ? ((await response.json()) as JsonValue) : null;

  if (!response.ok) {
    const detail =
      payload && typeof payload === 'object' && !Array.isArray(payload)
        ? (payload.error ?? payload.message)
        : null;
    const message =
      typeof detail === 'string'
        ? detail
        : detail &&
            typeof detail === 'object' &&
            !Array.isArray(detail) &&
            typeof detail.message === 'string'
          ? detail.message
          : `Request failed with ${response.status}`;
    throw new ApiError(response.status, message, payload);
  }

  return payload as T;
}

export function authedFetcher<T>(session: SessionContextShape, path: string) {
  return apiRequest<T>(session.effectiveBaseUrl, path, { token: session.accessToken });
}
