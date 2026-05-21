import { StatusBadge } from '@/components/data/status-badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Icon } from '@/components/ui/icon';
import { Text } from '@/components/ui/text';
import { ApiError } from '@/lib/api/types';
import { useBindDevice, useDeviceCandidate } from '@/lib/api/hooks';
import { useSession } from '@/lib/session/provider';
import { Cable, Link2 } from 'lucide-react-native';
import { Platform, View } from 'react-native';

export function PendingEmbeddedCard() {
  const session = useSession();
  const device = useDeviceCandidate();
  const bind = useBindDevice();

  if (device.error instanceof ApiError && device.error.status === 404) {
    return null;
  }

  const candidate = device.data;
  if (!candidate?.bind_required) {
    return null;
  }

  const serverEndpoint =
    session.effectiveBaseUrl ||
    (Platform.OS === 'web' && typeof globalThis.location !== 'undefined'
      ? globalThis.location.origin
      : undefined);

  async function bindCandidate() {
    await bind.trigger({
      server_endpoint: serverEndpoint || undefined,
      require_confirmation: true,
    });
    await device.mutate();
  }

  return (
    <Card className="border-amber-200 bg-amber-50/40 dark:border-amber-900 dark:bg-amber-950/20">
      <CardHeader>
        <View className="gap-3 sm:flex-row sm:items-start sm:justify-between">
          <View className="min-w-0 flex-1 gap-1">
            <View className="flex-row items-center gap-2">
              <Icon as={Cable} className="size-5 text-amber-600 dark:text-amber-400" />
              <CardTitle>Nearby Alia device found</CardTitle>
            </View>
            <CardDescription>
              A local embedded endpoint is asking to pair with this server. Bind it only if you
              recognize the device.
            </CardDescription>
          </View>
          <StatusBadge status={candidate.status} />
        </View>
      </CardHeader>
      <CardContent className="gap-4">
        <View className="gap-1">
          <Text className="font-medium">
            {candidate.device_name || candidate.device_id || 'Alia device'}
          </Text>
          <Text className="text-muted-foreground text-sm">
            {candidate.endpoint || 'Waiting for a network endpoint after provisioning'}
          </Text>
          {candidate.provisioning_mode ? (
            <Text className="text-muted-foreground text-sm">
              Provisioning mode is active. Keep your phone near the device while setup finishes.
            </Text>
          ) : null}
        </View>
        <View className="gap-2 sm:flex-row sm:items-center">
          <Button onPress={bindCandidate} disabled={bind.isMutating || !serverEndpoint}>
            <Icon as={Link2} className="size-4" />
            <Text>{bind.isMutating ? 'Pairing...' : 'Pair with this server'}</Text>
          </Button>
          {!serverEndpoint ? (
            <Text className="text-destructive text-sm">
              Open the console through a LAN-reachable server URL before binding.
            </Text>
          ) : null}
        </View>
        {bind.error ? <Text className="text-destructive text-sm">{bind.error.message}</Text> : null}
      </CardContent>
    </Card>
  );
}
