import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Icon } from '@/components/ui/icon';
import { Text } from '@/components/ui/text';
import type { LucideIcon } from 'lucide-react-native';
import { View } from 'react-native';

type MetricCardProps = {
  title: string;
  value: string | number;
  detail?: string;
  icon: LucideIcon;
};

export function MetricCard({ title, value, detail, icon }: MetricCardProps) {
  return (
    <Card className="flex-1">
      <CardHeader className="flex-row items-center justify-between gap-3 pb-2">
        <CardTitle className="text-sm">{title}</CardTitle>
        <View className="bg-secondary rounded-md p-2">
          <Icon as={icon} className="size-4" />
        </View>
      </CardHeader>
      <CardContent className="gap-1">
        <Text className="text-2xl font-semibold">{value}</Text>
        {detail ? <Text className="text-muted-foreground text-sm">{detail}</Text> : null}
      </CardContent>
    </Card>
  );
}
