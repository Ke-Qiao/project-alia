import { Text } from '@/components/ui/text';
import { cn } from '@/lib/utils';
import { ScrollView, View } from 'react-native';

type JsonPreviewProps = {
  value: unknown;
  className?: string;
};

export function JsonPreview({ value, className }: JsonPreviewProps) {
  return (
    <View className={cn('bg-muted rounded-md border border-transparent p-3', className)}>
      <ScrollView horizontal showsHorizontalScrollIndicator={false}>
        <Text className="font-mono text-xs leading-5">
          {typeof value === 'string' ? value : JSON.stringify(value ?? null, null, 2)}
        </Text>
      </ScrollView>
    </View>
  );
}
