import { Page } from '@/components/app/page';
import { PendingEmbeddedCard } from '@/components/app/pending-embedded-card';
import { JsonPreview } from '@/components/data/json-preview';
import { MetricCard } from '@/components/data/metric-card';
import { StatusBadge } from '@/components/data/status-badge';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Progress } from '@/components/ui/progress';
import { Text } from '@/components/ui/text';
import { useChannels, useHealth, useMemoryPrivacy, useRelationshipProfile } from '@/lib/api/hooks';
import { useTranslation } from '@/lib/session/use-translation';
import { Activity, Brain, Cable, Database } from 'lucide-react-native';
import { View } from 'react-native';

export default function RuntimeScreen() {
  const tr = useTranslation();
  const health = useHealth();
  const channels = useChannels();
  const memory = useMemoryPrivacy();
  const relationship = useRelationshipProfile();
  const modules = Object.entries(health.data?.modules ?? {});
  const activeConnectors = channels.data?.active.length ?? 0;
  const disabledConnectors = channels.data?.disabled.length ?? 0;

  return (
    <Page title={tr('runtime.title')} description="Live state for the Alia Server runtime and personal assistant subsystems.">
      <PendingEmbeddedCard />

      <View className="gap-3 md:flex-row">
        <MetricCard title={tr('runtime.health')} value={health.data?.status ?? 'offline'} detail={health.data?.version} icon={Activity} />
        <MetricCard title={tr('runtime.modules')} value={modules.length} detail="registered runtime modules" icon={Brain} />
        <MetricCard
          title={tr('runtime.connectors')}
          value={activeConnectors}
          detail={`${disabledConnectors} disabled`}
          icon={Cable}
        />
        <MetricCard
          title={tr('runtime.memory')}
          value={memory.data?.enabled ? 'on' : 'off'}
          detail={memory.data?.capture_mode ?? 'unknown'}
          icon={Database}
        />
      </View>

      <View className="gap-4 lg:flex-row">
        <Card className="flex-1">
          <CardHeader>
            <CardTitle>Modules</CardTitle>
          </CardHeader>
          <CardContent className="gap-3">
            {modules.slice(0, 16).map(([name, module]) => (
              <View key={name} className="flex-row items-center justify-between gap-3">
                <View className="min-w-0 flex-1">
                  <Text className="text-sm font-medium" numberOfLines={1}>
                    {name}
                  </Text>
                  {module.message ? (
                    <Text className="text-muted-foreground text-xs" numberOfLines={1}>
                      {module.message}
                    </Text>
                  ) : null}
                </View>
                <StatusBadge status={module.status} />
              </View>
            ))}
            {!modules.length ? <Text className="text-muted-foreground text-sm">No module health received.</Text> : null}
          </CardContent>
        </Card>

        <Card className="flex-1">
          <CardHeader>
            <CardTitle>{tr('runtime.expression')}</CardTitle>
          </CardHeader>
          <CardContent className="gap-4">
            <View className="gap-2">
              <View className="flex-row justify-between">
                <Text className="text-sm font-medium">Trust</Text>
                <Text className="text-muted-foreground text-sm">{Math.round((relationship.data?.trust ?? 0) * 100)}%</Text>
              </View>
              <Progress value={(relationship.data?.trust ?? 0) * 100} />
            </View>
            <View className="gap-2">
              <View className="flex-row justify-between">
                <Text className="text-sm font-medium">Familiarity</Text>
                <Text className="text-muted-foreground text-sm">
                  {Math.round((relationship.data?.familiarity ?? 0) * 100)}%
                </Text>
              </View>
              <Progress value={(relationship.data?.familiarity ?? 0) * 100} />
            </View>
            <Text className="text-muted-foreground text-sm leading-5">
              {relationship.data?.summary || 'No relationship summary has been produced yet.'}
            </Text>
          </CardContent>
        </Card>
      </View>

      <Card>
        <CardHeader>
          <CardTitle>Memory Policy</CardTitle>
        </CardHeader>
        <CardContent>
          <JsonPreview value={memory.data ?? { status: 'unavailable' }} />
        </CardContent>
      </Card>
    </Page>
  );
}
