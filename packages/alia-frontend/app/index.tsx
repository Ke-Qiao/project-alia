import { EmptyState } from '@/components/data/empty-state';
import { useHealth } from '@/lib/api/hooks';
import { useSession } from '@/lib/session/provider';
import { Redirect, type Href } from 'expo-router';
import { Server } from 'lucide-react-native';
import { View } from 'react-native';

export default function Index() {
  const session = useSession();
  const health = useHealth();

  if (!session.hydrated) {
    return <Loading />;
  }

  if (session.effectiveBaseUrl === null) {
    return <Redirect href={'/connect' as Href} />;
  }

  if (health.error) {
    return <Redirect href={'/connect' as Href} />;
  }

  const mode = health.data?.mode ?? health.data?.status;
  if (mode === 'oobe' || mode === 'setup') {
    return <Redirect href={'/setup' as Href} />;
  }

  if (!health.data) {
    return <Loading />;
  }

  if (!session.accessToken) {
    return <Redirect href={'/login' as Href} />;
  }

  return <Redirect href={'/runtime' as Href} />;
}

function Loading() {
  return (
    <View className="bg-background flex-1 items-center justify-center p-6">
      <EmptyState title="Loading Alia" description="Checking local session and server health." icon={Server} />
    </View>
  );
}
