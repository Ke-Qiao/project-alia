import { Badge } from '@/components/ui/badge';
import { Text } from '@/components/ui/text';

type StatusBadgeProps = {
  status?: string | boolean | null;
};

export function StatusBadge({ status }: StatusBadgeProps) {
  const normalized =
    typeof status === 'boolean' ? (status ? 'active' : 'inactive') : (status ?? 'unknown').toLowerCase();
  const variant =
    normalized.includes('healthy') || normalized === 'ok' || normalized === 'active' || normalized === 'enabled'
      ? 'default'
      : normalized.includes('degraded') || normalized.includes('disabled') || normalized.includes('setup')
        ? 'secondary'
        : normalized.includes('unhealthy') || normalized.includes('error') || normalized.includes('failed')
          ? 'destructive'
          : 'outline';

  return (
    <Badge variant={variant}>
      <Text>{String(status ?? 'unknown')}</Text>
    </Badge>
  );
}
