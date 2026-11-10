'use client';

import { SWRConfig } from 'swr';

import { aliaApiUrl } from '@/lib/api-url';
import { createClient } from '@/lib/api/generated/client';
import { TayoriProvider } from '@/lib/data';

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
