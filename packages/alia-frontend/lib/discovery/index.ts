import { requireOptionalNativeModule } from 'expo';
import { Platform } from 'react-native';

import type {
  AliaDiscoveryNativeModule,
  DiscoveredEmbedded,
  DiscoveredServer,
  DiscoveryScanOptions,
  NetworkProvisionStatus,
  ProvisionEmbeddedNetworkRequest,
  ReadEmbeddedNetworkStatusRequest,
} from './types';

export type {
  DiscoveredEmbedded,
  DiscoveredServer,
  DiscoveryScanOptions,
  NetworkProvisionRequest,
  NetworkProvisionStatus,
  ProvisionEmbeddedNetworkRequest,
  ReadEmbeddedNetworkStatusRequest,
} from './types';

const MODULE_NAME = 'AliaDiscovery';
const UNSUPPORTED_STATUS: NetworkProvisionStatus = {
  schema: 'alia.embedded.network_provisioning.v1',
  state: 'failed',
  message: 'Native discovery is not available in web mode.',
};

function nativeModule() {
  if (Platform.OS === 'web') return null;
  return requireOptionalNativeModule<AliaDiscoveryNativeModule>(MODULE_NAME);
}

function requireNativeDiscovery() {
  const mod = nativeModule();
  if (!mod) {
    throw new Error(
      'Native discovery is unavailable. Build the app with the AliaDiscovery local Expo module.'
    );
  }
  return mod;
}

export function isNativeDiscoverySupported() {
  return Platform.OS !== 'web' && nativeModule() !== null;
}

export async function scanServers(options: DiscoveryScanOptions = {}): Promise<DiscoveredServer[]> {
  if (Platform.OS === 'web') return [];
  return requireNativeDiscovery().scanServers({ timeoutMs: options.timeoutMs ?? 3500 });
}

export async function scanEmbedded(
  options: DiscoveryScanOptions = {}
): Promise<DiscoveredEmbedded[]> {
  if (Platform.OS === 'web') return [];
  return requireNativeDiscovery().scanEmbedded({ timeoutMs: options.timeoutMs ?? 5000 });
}

export async function provisionEmbeddedNetwork(
  request: ProvisionEmbeddedNetworkRequest
): Promise<void> {
  if (Platform.OS === 'web') {
    throw new Error('BLE provisioning is only available in the self-built mobile app.');
  }
  await requireNativeDiscovery().provisionEmbeddedNetwork(request);
}

export async function readEmbeddedNetworkStatus(
  request: ReadEmbeddedNetworkStatusRequest
): Promise<NetworkProvisionStatus> {
  if (Platform.OS === 'web') return UNSUPPORTED_STATUS;
  return requireNativeDiscovery().readEmbeddedNetworkStatus(request);
}
