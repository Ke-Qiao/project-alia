'use client';

import { SWRConfig } from 'swr';

import { isRetryableError, toApiError } from '@/lib/api-error';
import { aliaApiUrl } from '@/lib/api-url';
import { refreshToken } from '@/lib/api/generated';
import { createClient } from '@/lib/api/generated/client';
import { runRefreshExclusive, sessionStore } from '@/lib/auth/session';
import { createAuthFetch, createTokenRefresher } from '@/lib/auth/token-refresh';
import { TayoriProvider } from '@/lib/data';
import { createBrowserMockHost, mockEnabled } from '@/lib/mock';

function initClient() {
  const refreshAccessToken = createTokenRefresher(
    sessionStore,
    async (token) => {
      // eslint-disable-next-line @typescript-eslint/no-use-before-define -- only called after the client exists
      const { data } = await refreshToken({ client, body: { refresh_token: token } });
      return data;
    },
    runRefreshExclusive
  );

  const client = createClient({
    baseUrl: aliaApiUrl,
    throwOnError: true,
    // Used by operations that declare bearer security in OpenAPI.
    auth: () => sessionStore.get()?.accessToken,
    fetch: createAuthFetch(
      refreshAccessToken,
      mockEnabled && typeof window !== 'undefined' ? createBrowserMockHost().fetch : undefined
    )
  });

  client.interceptors.error.use((error, response) => toApiError(error, response));

  return client;
}

export function DataFetchingProvider({ children }: React.PropsWithChildren) {
  return (
    <TayoriProvider initClient={initClient}>
      <SWRConfig value={{ shouldRetryOnError: isRetryableError }}>
        {children}
      </SWRConfig>
    </TayoriProvider>
  );
}
