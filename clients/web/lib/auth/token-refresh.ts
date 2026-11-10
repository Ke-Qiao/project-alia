import type { TokenResponse } from '@/lib/api/generated/types.gen';

import type { Session, SessionStore } from './session-store';

type RunExclusive = <T>(task: () => Promise<T>) => Promise<T>;

function statusOf(error: unknown): number | undefined {
  if (typeof error !== 'object' || error === null || !('status' in error)) return undefined;
  return typeof error.status === 'number' ? error.status : undefined;
}

/**
 * Returns `refreshAccessToken(staleAccessToken)`, which resolves to a usable
 * access token, or `null` when the request should not be retried.
 *
 * The server revokes a refresh token on use, so:
 * - concurrent 401s in one tab share a single refresh request;
 * - `runExclusive` serializes refreshes across tabs (Web Locks in the browser),
 *   and the stored session is re-read first in case another tab already rotated it;
 * - only a 401 from the refresh endpoint ends the session. Network failures and
 *   5xx are rethrown and leave the session untouched.
 */
export function createTokenRefresher(
  store: Pick<SessionStore, 'get' | 'updateTokens' | 'clear'>,
  requestRefresh: (refreshToken: string) => Promise<TokenResponse>,
  runExclusive: RunExclusive = task => task()
) {
  let inflight: Promise<string | null> | null = null;

  function alreadyRotated(stale: Session): string | null | undefined {
    const current = store.get();
    if (!current) return null;
    if (current.refreshToken !== stale.refreshToken) return current.accessToken;
    return undefined;
  }

  async function refresh(stale: Session): Promise<string | null> {
    const rotated = alreadyRotated(stale);
    if (rotated !== undefined) return rotated;

    try {
      const tokens = await requestRefresh(stale.refreshToken);
      return store.updateTokens(stale.refreshToken, tokens) ? tokens.access_token : null;
    } catch (error) {
      if (statusOf(error) !== 401) throw error;
      const rotatedMeanwhile = alreadyRotated(stale);
      if (rotatedMeanwhile !== undefined) return rotatedMeanwhile;
      store.clear('expired');
      return null;
    }
  }

  return function refreshAccessToken(staleAccessToken: string): Promise<string | null> {
    const current = store.get();
    if (!current) return Promise.resolve(null);
    // The 401 belongs to a token that has been replaced since the request was sent.
    if (current.accessToken !== staleAccessToken) return Promise.resolve(current.accessToken);

    inflight ??= runExclusive(() => refresh(current)).finally(() => {
      inflight = null;
    });
    return inflight;
  };
}

/**
 * Wraps `fetch` so that a 401 on a request that carried a bearer token is
 * retried once with a refreshed token. Requests without a bearer token
 * (sign-in, register, refresh, health) pass through untouched, so a 401 from
 * sign-in keeps meaning "wrong credentials".
 */
export function createAuthFetch(
  refreshAccessToken: (staleAccessToken: string) => Promise<string | null>,
  baseFetch: typeof fetch = (input, init) => fetch(input, init)
): typeof fetch {
  return async (input, init) => {
    const request = new Request(input, init);
    const authorization = request.headers.get('Authorization');
    if (!authorization?.startsWith('Bearer ')) return baseFetch(request);

    const retry = request.clone();
    const response = await baseFetch(request);
    if (response.status !== 401) return response;

    const accessToken = await refreshAccessToken(authorization.slice('Bearer '.length));
    if (!accessToken) return response;

    retry.headers.set('Authorization', `Bearer ${accessToken}`);
    return baseFetch(retry);
  };
}
