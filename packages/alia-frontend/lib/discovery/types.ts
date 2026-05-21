export type DiscoveryScanOptions = {
  timeoutMs?: number;
};

export type DiscoveredServer = {
  id?: string;
  name: string;
  endpoint: string;
  host?: string;
  port?: number;
  mode?: 'runtime' | 'oobe' | string;
  txt: Record<string, string>;
};

export type DiscoveredEmbedded = {
  nativeId: string;
  name?: string;
  rssi?: number;
  status?: 'unbound' | 'bound' | 'handoff_pending' | 'unknown';
  port?: number;
  deviceFingerprint?: string;
  serverFingerprint?: string;
};

export type NetworkProvisionRequest = {
  ssid: string;
  password?: string;
  interfaceName?: string;
};

export type ProvisionEmbeddedNetworkRequest = NetworkProvisionRequest & {
  nativeId: string;
};

export type ReadEmbeddedNetworkStatusRequest = {
  nativeId: string;
};

export type NetworkProvisionStatus = {
  schema: 'alia.embedded.network_provisioning.v1';
  state: 'idle' | 'applying' | 'connected' | 'failed';
  message: string;
  updated_at?: string;
};

export type AliaDiscoveryNativeModule = {
  scanServers(options?: DiscoveryScanOptions): Promise<DiscoveredServer[]>;
  scanEmbedded(options?: DiscoveryScanOptions): Promise<DiscoveredEmbedded[]>;
  provisionEmbeddedNetwork(request: ProvisionEmbeddedNetworkRequest): Promise<void>;
  readEmbeddedNetworkStatus(
    request: ReadEmbeddedNetworkStatusRequest
  ): Promise<NetworkProvisionStatus>;
};
