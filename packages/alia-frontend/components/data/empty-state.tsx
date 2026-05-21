import { Button } from '@/components/ui/button';
import { Icon } from '@/components/ui/icon';
import { Text } from '@/components/ui/text';
import type { LucideIcon } from 'lucide-react-native';
import { RefreshCcw } from 'lucide-react-native';
import { View } from 'react-native';

type EmptyStateProps = {
  title: string;
  description?: string;
  icon?: LucideIcon;
  actionLabel?: string;
  onAction?: () => void;
};

export function EmptyState({ title, description, icon: EmptyIcon = RefreshCcw, actionLabel, onAction }: EmptyStateProps) {
  return (
    <View className="border-border bg-muted/40 items-center gap-3 rounded-lg border border-dashed p-8">
      <Icon as={EmptyIcon} className="text-muted-foreground size-6" />
      <View className="items-center gap-1">
        <Text className="text-center font-medium">{title}</Text>
        {description ? <Text className="text-muted-foreground max-w-md text-center text-sm">{description}</Text> : null}
      </View>
      {actionLabel && onAction ? (
        <Button variant="outline" size="sm" onPress={onAction}>
          <Text>{actionLabel}</Text>
        </Button>
      ) : null}
    </View>
  );
}
