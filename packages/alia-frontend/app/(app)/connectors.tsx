import { ListPage } from '@/components/app/page';
import { PendingEmbeddedCard } from '@/components/app/pending-embedded-card';
import { EmptyState } from '@/components/data/empty-state';
import { StatusBadge } from '@/components/data/status-badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Icon } from '@/components/ui/icon';
import { Text } from '@/components/ui/text';
import { useApiAction, useChannels } from '@/lib/api/hooks';
import type { ChannelInfo, DisabledChannelInfo } from '@/lib/api/types';
import { useTranslation } from '@/lib/session/use-translation';
import { Cable, RotateCcw } from 'lucide-react-native';
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
      header={<PendingEmbeddedCard />}
      empty={
        <EmptyState
          title="No connections yet"
          description="Add server-side channel adapters when you want Alia to appear in a chat app or client."
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
                    <CardTitle numberOfLines={1}>{channel.id}</CardTitle>
                    <CardDescription>{channel.channel_type}</CardDescription>
                  </View>
                  <StatusBadge status={channel.health} />
                </View>
              </CardHeader>
              <CardContent className="gap-3">
                <Text className="text-muted-foreground text-sm">{channel.health}</Text>
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
                  <CardTitle numberOfLines={1}>{channel.id}</CardTitle>
                  <CardDescription>{channel.channel_type}</CardDescription>
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
