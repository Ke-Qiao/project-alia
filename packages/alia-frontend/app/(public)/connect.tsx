import { ThemeToggle } from '@/components/app/theme-toggle';
import { EmptyState } from '@/components/data/empty-state';
import { Field } from '@/components/forms/field';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Icon } from '@/components/ui/icon';
import { Input } from '@/components/ui/input';
import { Progress } from '@/components/ui/progress';
import { Separator } from '@/components/ui/separator';
import { Text } from '@/components/ui/text';
import { useProbeServer } from '@/lib/api/hooks';
import type { DiscoveredEmbedded, DiscoveredServer } from '@/lib/discovery';
import {
  useEmbeddedDiscovery,
  useEmbeddedNetworkStatus,
  useProvisionEmbeddedNetwork,
  useServerDiscovery,
} from '@/lib/discovery/hooks';
import { useSession } from '@/lib/session/provider';
import { useTranslation } from '@/lib/session/use-translation';
import { Redirect, router, type Href } from 'expo-router';
import { Bluetooth, RefreshCw, Router, Server, Wifi } from 'lucide-react-native';
import * as React from 'react';
import { Platform, ScrollView, StyleSheet, View } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';

export default function ConnectScreen() {
  const session = useSession();
  const tr = useTranslation();
  const [serverUrl, setServerUrl] = React.useState(session.effectiveBaseUrl ?? '');
  const [error, setError] = React.useState<string | null>(null);
  const probe = useProbeServer();
  const checking = probe.isMutating;
  const mobileMode = Platform.OS !== 'web';

  if (!session.hydrated) return null;
  if (session.effectiveBaseUrl !== null && session.accessToken)
    return <Redirect href={'/runtime' as Href} />;

  async function connectToBaseUrl(rawBaseUrl: string | null) {
    setError(null);
    try {
      const baseUrl = rawBaseUrl?.trim().replace(/\/+$/, '') ?? '';
      if (mobileMode && !baseUrl) {
        setError('Server URL is required on mobile.');
        return;
      }
      const health = await probe.trigger({ baseUrl: baseUrl || null });
      await session.setServerBaseUrl(baseUrl || null);
      if (health.mode === 'oobe' || health.status === 'setup') {
        router.replace('/setup' as Href);
      } else {
        router.replace('/login' as Href);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Unable to reach server');
    }
  }

  async function connect() {
    await connectToBaseUrl(serverUrl);
  }

  async function sameOrigin() {
    setServerUrl('');
    await connectToBaseUrl(null);
  }

  return (
    <SafeAreaView edges={['top', 'bottom', 'left', 'right']} className="bg-background" style={styles.safeArea}>
      <View className="absolute right-4 top-4 z-10">
        <ThemeToggle />
      </View>
      <ScrollView
        className="flex-1"
        style={styles.scroller}
        contentContainerStyle={styles.scrollerContent}>
        <View className="gap-4" style={styles.contentFrame}>
          <Card>
            <CardHeader>
              <CardTitle>{tr('connect.title')}</CardTitle>
              <CardDescription>{tr('connect.description')}</CardDescription>
            </CardHeader>
            <CardContent className="gap-4">
              <Field label={tr('connect.url')} description="http://192.168.1.10:3000">
                <Input
                  autoCapitalize="none"
                  autoCorrect={false}
                  inputMode="url"
                  onChangeText={setServerUrl}
                  placeholder="http://localhost:3000"
                  value={serverUrl}
                />
              </Field>
              {error ? <Text className="text-destructive text-sm">{error}</Text> : null}
              <Button disabled={checking} onPress={connect}>
                <Text>{checking ? 'Checking...' : tr('connect.action')}</Text>
              </Button>
              {Platform.OS === 'web' ? (
                <Button variant="ghost" onPress={sameOrigin}>
                  <Text>Use same-origin backend</Text>
                </Button>
              ) : null}
            </CardContent>
          </Card>
          {mobileMode ? (
            <>
              <LanServersCard onConnect={connectToBaseUrl} connecting={checking} />
              <NearbyEmbeddedCard />
            </>
          ) : null}
          <EmptyState
            title="Decoupled by URL"
            description="The mobile app only needs an HTTP Alia Server endpoint. Platform adapters stay behind the server boundary."
            icon={Server}
          />
        </View>
      </ScrollView>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  safeArea: {
    flex: 1,
  },
  scroller: {
    flex: 1,
  },
  scrollerContent: {
    alignItems: 'center',
    flexGrow: 1,
    gap: 16,
    justifyContent: 'center',
    paddingHorizontal: 16,
    paddingVertical: 64,
  },
  contentFrame: {
    maxWidth: 672,
    width: '100%',
  },
});

function LanServersCard({
  connecting,
  onConnect,
}: {
  connecting: boolean;
  onConnect: (baseUrl: string) => Promise<void>;
}) {
  const discovery = useServerDiscovery();
  const [error, setError] = React.useState<string | null>(null);
  const servers = discovery.data ?? [];

  async function scan() {
    setError(null);
    try {
      await discovery.trigger({ timeoutMs: 3500 });
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Unable to scan for LAN servers.');
    }
  }

  return (
    <Card>
      <CardHeader>
        <View className="flex-row items-center gap-2">
          <Icon as={Router} className="size-4" />
          <CardTitle>LAN Servers</CardTitle>
        </View>
        <CardDescription>
          Discover Alia Server on the local network via _alia-server._tcp.
        </CardDescription>
      </CardHeader>
      <CardContent className="gap-4">
        <Button variant="outline" disabled={discovery.isMutating} onPress={scan}>
          <Icon as={RefreshCw} className="size-4" />
          <Text>{discovery.isMutating ? 'Scanning...' : 'Scan LAN'}</Text>
        </Button>
        {discovery.isMutating ? <Progress value={55} /> : null}
        {error ? <Text className="text-destructive text-sm">{error}</Text> : null}
        {!discovery.isMutating && discovery.data && servers.length === 0 ? (
          <Text className="text-muted-foreground text-sm">No Alia Server discovered.</Text>
        ) : null}
        <View className="gap-3">
          {servers.map((server) => (
            <ServerRow
              key={server.id ?? server.endpoint}
              server={server}
              disabled={connecting}
              onConnect={onConnect}
            />
          ))}
        </View>
      </CardContent>
    </Card>
  );
}

function ServerRow({
  disabled,
  onConnect,
  server,
}: {
  disabled: boolean;
  onConnect: (baseUrl: string) => Promise<void>;
  server: DiscoveredServer;
}) {
  const mode = server.mode ?? server.txt.mode ?? 'runtime';
  return (
    <View className="border-border gap-3 rounded-md border p-3">
      <View className="flex-row items-start justify-between gap-3">
        <View className="min-w-0 flex-1 gap-1">
          <Text className="font-medium">{server.name}</Text>
          <Text className="text-muted-foreground text-xs">{server.endpoint}</Text>
        </View>
        <Badge variant={mode === 'oobe' ? 'secondary' : 'outline'}>
          <Text>{mode}</Text>
        </Badge>
      </View>
      <Button size="sm" disabled={disabled} onPress={() => onConnect(server.endpoint)}>
        <Text>{disabled ? 'Checking...' : 'Connect'}</Text>
      </Button>
    </View>
  );
}

function NearbyEmbeddedCard() {
  const discovery = useEmbeddedDiscovery();
  const provision = useProvisionEmbeddedNetwork();
  const [selected, setSelected] = React.useState<DiscoveredEmbedded | null>(null);
  const [ssid, setSsid] = React.useState('');
  const [password, setPassword] = React.useState('');
  const [interfaceName, setInterfaceName] = React.useState('wlan0');
  const [error, setError] = React.useState<string | null>(null);
  const status = useEmbeddedNetworkStatus(
    selected?.nativeId ?? null,
    provision.isMutating ? 1200 : 2500
  );
  const devices = discovery.data ?? [];

  async function scan() {
    setError(null);
    try {
      const found = await discovery.trigger({ timeoutMs: 5000 });
      if (selected && !found.some((device) => device.nativeId === selected.nativeId)) {
        setSelected(null);
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Unable to scan for embedded devices.');
    }
  }

  async function submitProvision() {
    if (!selected) return;
    setError(null);
    try {
      await provision.trigger({
        nativeId: selected.nativeId,
        ssid: ssid.trim(),
        password,
        interfaceName: interfaceName.trim() || 'wlan0',
      });
      await status.mutate();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Unable to provision embedded network.');
    }
  }

  return (
    <Card>
      <CardHeader>
        <View className="flex-row items-center gap-2">
          <Icon as={Bluetooth} className="size-4" />
          <CardTitle>Nearby Embedded</CardTitle>
        </View>
        <CardDescription>
          Discover BLE beacons and write Wi-Fi credentials to Alia Embedded.
        </CardDescription>
      </CardHeader>
      <CardContent className="gap-4">
        <Button variant="outline" disabled={discovery.isMutating} onPress={scan}>
          <Icon as={RefreshCw} className="size-4" />
          <Text>{discovery.isMutating ? 'Scanning...' : 'Scan BLE'}</Text>
        </Button>
        {discovery.isMutating ? <Progress value={45} /> : null}
        {error ? <Text className="text-destructive text-sm">{error}</Text> : null}
        {!discovery.isMutating && discovery.data && devices.length === 0 ? (
          <Text className="text-muted-foreground text-sm">
            No nearby Alia Embedded device discovered.
          </Text>
        ) : null}
        <View className="gap-3">
          {devices.map((device) => (
            <EmbeddedRow
              key={device.nativeId}
              device={device}
              selected={selected?.nativeId === device.nativeId}
              onSelect={setSelected}
            />
          ))}
        </View>
        {selected ? (
          <>
            <Separator />
            <View className="gap-3">
              <View className="flex-row items-center gap-2">
                <Icon as={Wifi} className="size-4" />
                <Text className="font-medium">Wi-Fi Provisioning</Text>
              </View>
              <Field label="SSID">
                <Input
                  autoCapitalize="none"
                  autoCorrect={false}
                  onChangeText={setSsid}
                  value={ssid}
                />
              </Field>
              <Field label="Password">
                <Input
                  autoCapitalize="none"
                  autoCorrect={false}
                  onChangeText={setPassword}
                  secureTextEntry
                  value={password}
                />
              </Field>
              <Field label="Interface">
                <Input
                  autoCapitalize="none"
                  autoCorrect={false}
                  onChangeText={setInterfaceName}
                  value={interfaceName}
                />
              </Field>
              <Button disabled={provision.isMutating || !ssid.trim()} onPress={submitProvision}>
                <Text>{provision.isMutating ? 'Writing...' : 'Write Wi-Fi Config'}</Text>
              </Button>
              <ProvisionStatus
                status={status.data}
                loading={status.isLoading || status.isValidating}
              />
            </View>
          </>
        ) : null}
      </CardContent>
    </Card>
  );
}

function EmbeddedRow({
  device,
  onSelect,
  selected,
}: {
  device: DiscoveredEmbedded;
  onSelect: (device: DiscoveredEmbedded) => void;
  selected: boolean;
}) {
  const status = device.status ?? 'unknown';
  return (
    <View className="border-border gap-3 rounded-md border p-3">
      <View className="flex-row items-start justify-between gap-3">
        <View className="min-w-0 flex-1 gap-1">
          <Text className="font-medium">{device.name ?? 'Alia Embedded'}</Text>
          <Text className="text-muted-foreground text-xs">{device.nativeId}</Text>
          <Text className="text-muted-foreground text-xs">
            {typeof device.rssi === 'number' ? `${device.rssi} dBm` : 'RSSI unknown'}
            {device.port ? ` - port ${device.port}` : ''}
          </Text>
        </View>
        <Badge variant={status === 'unbound' ? 'secondary' : 'outline'}>
          <Text>{status}</Text>
        </Badge>
      </View>
      <Button
        size="sm"
        variant={selected ? 'secondary' : 'outline'}
        onPress={() => onSelect(device)}>
        <Text>{selected ? 'Selected' : 'Configure'}</Text>
      </Button>
    </View>
  );
}

function ProvisionStatus({
  loading,
  status,
}: {
  loading: boolean;
  status:
    | {
        state: string;
        message: string;
        updated_at?: string;
      }
    | undefined;
}) {
  if (!status && !loading) {
    return null;
  }

  const state = status?.state ?? 'reading';
  const progressValue = state === 'connected' ? 100 : state === 'failed' ? 100 : loading ? 60 : 35;

  return (
    <View className="gap-2">
      <View className="flex-row items-center justify-between gap-3">
        <Text className="text-sm font-medium">Provision status</Text>
        <Badge
          variant={
            state === 'failed' ? 'destructive' : state === 'connected' ? 'secondary' : 'outline'
          }>
          <Text>{state}</Text>
        </Badge>
      </View>
      <Progress value={progressValue} />
      <Text className="text-muted-foreground text-xs">
        {status?.message ?? 'Reading embedded network status...'}
      </Text>
    </View>
  );
}
