import { ListPage } from '@/components/app/page';
import { PendingEmbeddedCard } from '@/components/app/pending-embedded-card';
import { EmptyState } from '@/components/data/empty-state';
import { StatBadges } from '@/components/data/friendly-data';
import { StatusBadge } from '@/components/data/status-badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Icon } from '@/components/ui/icon';
import { Text } from '@/components/ui/text';
import { useApiAction, useChannels } from '@/lib/api/hooks';
import type { ChannelInfo, DisabledChannelInfo } from '@/lib/api/types';
import { useTranslation } from '@/lib/session/use-translation';
import { Cable, MessageCircle, RotateCcw } from 'lucide-react-native';
import { View } from 'react-native';

type ChannelRow =
  | { channel: ChannelInfo; kind: 'active' }
  | { channel: DisabledChannelInfo; kind: 'disabled' };

export default function ConnectorsScreen() {
  const tr = useTranslation();
  const channels = useChannels();
  const action = useApiAction();
  const active = channels.data?.active ?? [];
  const disabled = channels.data?.disabled ?? [];
  const rows: ChannelRow[] = [
    ...active.map((channel) => ({ channel, kind: 'active' as const })),
    ...disabled.map((channel) => ({ channel, kind: 'disabled' as const })),
  ];

  async function restart(id: string) {
    await action.trigger({ path: `/api/v1/channels/${encodeURIComponent(id)}/restart` });
    await channels.mutate();
  }

  return (
    <ListPage
      title={tr('channels.title')}
      description={tr('channels.description')}
      data={rows}
      keyExtractor={(row) => `${row.kind}:${row.channel.id}`}
      header={
        <>
          <PendingEmbeddedCard />
          <WorkspaceOverview activeCount={active.length} disabledCount={disabled.length} />
        </>
      }
      empty={
        <EmptyState
          title="No connection entries yet"
          description="When you add platform adapters on the server, they will appear here as live connection status."
          icon={Cable}
        />
      }
      footer={
        action.error ? (
          <Text className="text-destructive text-sm">{action.error.message}</Text>
        ) : null
      }
      renderItem={({ item: row }) => {
        if (row.kind === 'active') {
          const channel = row.channel;
          return (
            <Card>
              <CardHeader>
                <View className="flex-row items-start justify-between gap-3">
                  <View className="min-w-0 flex-1 gap-1">
                    <CardTitle numberOfLines={1}>{formatChannelName(channel.id)}</CardTitle>
                    <CardDescription>{formatChannelType(channel.channel_type)}</CardDescription>
                  </View>
                  <StatusBadge status={channel.health} />
                </View>
              </CardHeader>
              <CardContent className="gap-3">
                <Text className="text-muted-foreground text-sm leading-5">
                  {channelHealthText(channel.health)}
                </Text>
                <Button
                  variant="outline"
                  size="sm"
                  onPress={() => restart(channel.id)}
                  disabled={action.isMutating}>
                  <Icon as={RotateCcw} className="size-4" />
                  <Text>{tr('channels.restart')}</Text>
                </Button>
              </CardContent>
            </Card>
          );
        }

        const channel = row.channel;
        return (
          <Card className="border-amber-200 dark:border-amber-900">
            <CardHeader>
              <View className="flex-row items-start justify-between gap-3">
                <View className="min-w-0 flex-1 gap-1">
                  <CardTitle numberOfLines={1}>{formatChannelName(channel.id)}</CardTitle>
                  <CardDescription>{formatChannelType(channel.channel_type)}</CardDescription>
                </View>
                <StatusBadge status="disabled" />
              </View>
            </CardHeader>
            <CardContent>
              <Text className="text-muted-foreground text-sm leading-5">{channel.reason}</Text>
            </CardContent>
          </Card>
        );
      }}
    />
  );
}

function WorkspaceOverview({
  activeCount,
  disabledCount,
}: {
  activeCount: number;
  disabledCount: number;
}) {
  return (
    <Card>
      <CardHeader>
        <View className="flex-row items-start gap-3">
          <View className="bg-secondary rounded-lg p-2">
            <Icon as={MessageCircle} className="size-5" />
          </View>
          <View className="min-w-0 flex-1 gap-1">
            <CardTitle>Connection workspace</CardTitle>
            <CardDescription>
              A live view of the chat platforms, clients, and nearby device bridges connected to
              Alia.
            </CardDescription>
          </View>
        </View>
      </CardHeader>
      <CardContent className="gap-3">
        <StatBadges
          items={[
            { label: 'Active', value: activeCount },
            { label: 'Need setup', value: disabledCount },
          ]}
        />
        <Text className="text-muted-foreground text-sm leading-5">
          Adapter settings are organized in Settings. This page stays focused on what is currently
          running and whether it needs attention.
        </Text>
      </CardContent>
    </Card>
  );
}

function formatChannelName(id: string) {
  return id
    .split(/[-_.]+/)
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(' ');
}

function formatChannelType(type: string) {
  const normalized = type.replace(/[-_]+/g, ' ');
  return normalized
    ? `${normalized.charAt(0).toUpperCase()}${normalized.slice(1)} adapter`
    : 'Adapter';
}

function channelHealthText(status: string) {
  const normalized = status.toLowerCase();
  if (normalized.includes('healthy') || normalized === 'ok' || normalized === 'active') {
    return 'Ready to receive and send assistant messages.';
  }
  if (normalized.includes('disabled')) {
    return 'This connection is configured but not currently active.';
  }
  if (
    normalized.includes('error') ||
    normalized.includes('failed') ||
    normalized.includes('unhealthy')
  ) {
    return 'This connection needs attention before Alia can use it reliably.';
  }
  if (normalized.includes('starting') || normalized.includes('checking')) {
    return 'Alia is checking this connection.';
  }
  return 'Connection status reported by the server.';
}
