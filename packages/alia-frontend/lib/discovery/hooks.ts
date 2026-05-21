import useSWR from 'swr';
import useSWRMutation from 'swr/mutation';

import {
  provisionEmbeddedNetwork,
  readEmbeddedNetworkStatus,
  scanEmbedded,
  scanServers,
} from '@/lib/discovery';
import type { DiscoveryScanOptions, ProvisionEmbeddedNetworkRequest } from '@/lib/discovery/types';

type MutationArg<TBody> = {
  arg: TBody;
};

export function useServerDiscovery(defaultTimeoutMs = 3500) {
  return useSWRMutation(
    ['discovery', 'servers'],
    (_key, { arg }: MutationArg<DiscoveryScanOptions | undefined>) =>
      scanServers({ timeoutMs: arg?.timeoutMs ?? defaultTimeoutMs })
  );
}

export function useEmbeddedDiscovery(defaultTimeoutMs = 5000) {
  return useSWRMutation(
    ['discovery', 'embedded'],
    (_key, { arg }: MutationArg<DiscoveryScanOptions | undefined>) =>
      scanEmbedded({ timeoutMs: arg?.timeoutMs ?? defaultTimeoutMs })
  );
}

export function useProvisionEmbeddedNetwork() {
  return useSWRMutation(
    ['discovery', 'embedded', 'provision'],
    (_key, { arg }: MutationArg<ProvisionEmbeddedNetworkRequest>) => provisionEmbeddedNetwork(arg)
  );
}

export function useEmbeddedNetworkStatus(nativeId: string | null, refreshInterval = 2000) {
  return useSWR(
    nativeId ? ['discovery', 'embedded', nativeId, 'network-status'] : null,
    ([, , id]) => readEmbeddedNetworkStatus({ nativeId: String(id) }),
    { refreshInterval }
  );
}
