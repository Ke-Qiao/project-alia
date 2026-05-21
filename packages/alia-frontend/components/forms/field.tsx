import { Text } from '@/components/ui/text';
import { View } from 'react-native';

type FieldProps = {
  label: string;
  description?: string;
  children: React.ReactNode;
};

export function Field({ label, description, children }: FieldProps) {
  return (
    <View className="gap-2">
      <View className="gap-0.5">
        <Text className="text-sm font-medium">{label}</Text>
        {description ? <Text className="text-muted-foreground text-xs leading-4">{description}</Text> : null}
      </View>
      {children}
    </View>
  );
}
