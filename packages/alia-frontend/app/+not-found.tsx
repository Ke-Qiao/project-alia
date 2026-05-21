import { Link, type Href } from 'expo-router';
import { View } from 'react-native';
import { Text } from '@/components/ui/text';
import { Button } from '@/components/ui/button';

export default function NotFoundScreen() {
  return (
    <View className="bg-background flex-1 items-center justify-center gap-4 p-6">
      <View className="items-center gap-1">
        <Text className="text-xl font-semibold">Route not found</Text>
        <Text className="text-muted-foreground text-center text-sm">This Alia console route does not exist.</Text>
      </View>
      <Link href={'/' as Href} asChild>
        <Button variant="outline">
          <Text>Return to console</Text>
        </Button>
      </Link>
    </View>
  );
}
