'use client';

import { SWRConfig } from 'swr';
import { tayori } from 'tayori';

import { aliaApiUrl } from '@/lib/api-url';
import { createClient } from '@/lib/api/generated/client';

const { TayoriProvider } = tayori();

export function DataFetchingProvider({ children }: React.PropsWithChildren) {
  return (
    <TayoriProvider
      initClient={() => createClient({
        baseUrl: aliaApiUrl,
        throwOnError: true
      })}
    >
      <SWRConfig value={{}}>
        {children}
      </SWRConfig>
    </TayoriProvider>
  );
}
