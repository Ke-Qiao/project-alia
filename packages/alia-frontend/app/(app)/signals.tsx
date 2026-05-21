import { ListPage } from '@/components/app/page';
import { EmptyState } from '@/components/data/empty-state';
import { FriendlyRecord, InfoRows, formatFriendlyValue } from '@/components/data/friendly-data';
import { StatusBadge } from '@/components/data/status-badge';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Text } from '@/components/ui/text';
import { useSensors } from '@/lib/api/hooks';
import { useTranslation } from '@/lib/session/use-translation';
import { Radar } from 'lucide-react-native';
import { View } from 'react-native';

export default function SignalsScreen() {
  const tr = useTranslation();
  const sensors = useSensors();
  const rows = sensors.data ?? [];

  return (
    <ListPage
      title={tr('signals.title')}
      description="Signals are server-side observable inputs. They may come from any channel or integration."
      data={rows}
      keyExtractor={(sensor) => sensor.sensor_id}
      empty={
        <EmptyState
          title="No signals registered"
          description="Sensor definitions and latest readings will appear here when providers register them."
          icon={Radar}
        />
      }
      footer={
        sensors.error ? (
          <Text className="text-destructive text-sm">{sensors.error.message}</Text>
        ) : null
      }
      renderItem={({ item: sensor }) => (
        <Card>
          <CardHeader>
            <View className="gap-2 sm:flex-row sm:items-start sm:justify-between">
              <View className="min-w-0 flex-1 gap-1">
                <CardTitle numberOfLines={1}>{sensor.display_name}</CardTitle>
                <CardDescription>{sensor.sensor_id}</CardDescription>
              </View>
              <StatusBadge status={sensor.last_seen_at ? 'active' : 'inactive'} />
            </View>
          </CardHeader>
          <CardContent className="gap-3">
            <View className="flex-row flex-wrap gap-2">
              <StatusBadge status={sensor.source} />
              <StatusBadge status={sensor.value_type} />
              {sensor.unit ? <StatusBadge status={sensor.unit} /> : null}
            </View>
            <InfoRows
              rows={[
                {
                  label: 'Current value',
                  value: sensor.latest ? formatFriendlyValue(sensor.latest.value) : undefined,
                },
                { label: 'Last seen', value: sensor.latest?.timestamp ?? sensor.last_seen_at },
                {
                  label: 'Quality',
                  value: sensor.latest ? `${Math.round(sensor.latest.quality * 100)}%` : undefined,
                },
              ]}
              empty="No reading has been received yet."
            />
            {sensor.metadata ? (
              <FriendlyRecord value={sensor.metadata} empty="No metadata available." />
            ) : null}
          </CardContent>
        </Card>
      )}
    />
  );
}
