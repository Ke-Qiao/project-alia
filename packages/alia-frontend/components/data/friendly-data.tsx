import { Badge } from '@/components/ui/badge';
import { Text } from '@/components/ui/text';
import { cn } from '@/lib/utils';
import { View } from 'react-native';

type InfoRowsProps = {
  rows: Array<{
    label: string;
    value?: unknown;
    hidden?: boolean;
  }>;
  empty?: string;
  className?: string;
};

export function InfoRows({ className, empty = 'No details available.', rows }: InfoRowsProps) {
  const visibleRows = rows.filter(
    (row) => !row.hidden && row.value !== undefined && row.value !== null && row.value !== ''
  );

  if (!visibleRows.length) {
    return <Text className="text-muted-foreground text-sm">{empty}</Text>;
  }

  return (
    <View className={cn('gap-2', className)}>
      {visibleRows.map((row) => (
        <View
          key={row.label}
          className="border-border flex-row items-start justify-between gap-3 rounded-md border p-3">
          <Text className="text-muted-foreground text-sm">{row.label}</Text>
          <Text className="max-w-[70%] text-right text-sm font-medium">
            {formatFriendlyValue(row.value)}
          </Text>
        </View>
      ))}
    </View>
  );
}

type FriendlyRecordProps = {
  value: unknown;
  empty?: string;
  maxItems?: number;
};

export function FriendlyRecord({
  empty = 'No details available.',
  maxItems = 8,
  value,
}: FriendlyRecordProps) {
  const entries = objectEntries(value).slice(0, maxItems);
  const total = objectEntries(value).length;

  if (!entries.length) {
    return <Text className="text-muted-foreground text-sm">{empty}</Text>;
  }

  return (
    <View className="gap-2">
      {entries.map(([key, entryValue]) => (
        <View key={key} className="flex-row items-start justify-between gap-3">
          <Text className="text-muted-foreground text-sm" numberOfLines={1}>
            {humanizeKey(key)}
          </Text>
          <Text className="max-w-[65%] text-right text-sm font-medium" numberOfLines={2}>
            {formatFriendlyValue(entryValue)}
          </Text>
        </View>
      ))}
      {total > maxItems ? (
        <Text className="text-muted-foreground text-xs">+{total - maxItems} more fields</Text>
      ) : null}
    </View>
  );
}

type StatBadgesProps = {
  items: Array<{ label: string; value?: unknown; hidden?: boolean }>;
};

export function StatBadges({ items }: StatBadgesProps) {
  const visibleItems = items.filter(
    (item) => !item.hidden && item.value !== undefined && item.value !== null
  );

  if (!visibleItems.length) return null;

  return (
    <View className="flex-row flex-wrap gap-2">
      {visibleItems.map((item) => (
        <Badge key={item.label} variant="outline">
          <Text>
            {item.label}: {formatFriendlyValue(item.value)}
          </Text>
        </Badge>
      ))}
    </View>
  );
}

export function formatFriendlyValue(value: unknown): string {
  if (value === null || value === undefined) return 'None';
  if (typeof value === 'boolean') return value ? 'Yes' : 'No';
  if (typeof value === 'number') return Number.isInteger(value) ? String(value) : value.toFixed(2);
  if (typeof value === 'string') return value || 'Empty';
  if (Array.isArray(value)) {
    if (!value.length) return 'None';
    if (value.every((item) => ['string', 'number', 'boolean'].includes(typeof item))) {
      return (
        value.slice(0, 4).map(formatFriendlyValue).join(', ') +
        (value.length > 4 ? ` +${value.length - 4}` : '')
      );
    }
    return `${value.length} items`;
  }
  if (typeof value === 'object') {
    return `${Object.keys(value as Record<string, unknown>).length} fields`;
  }
  return String(value);
}

export function humanizeKey(key: string) {
  return key
    .replace(/[_-]+/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
    .replace(/\b\w/g, (char) => char.toUpperCase());
}

function objectEntries(value: unknown): Array<[string, unknown]> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return [];
  return Object.entries(value as Record<string, unknown>);
}
