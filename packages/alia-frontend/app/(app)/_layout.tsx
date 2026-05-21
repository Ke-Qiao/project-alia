import { AppShell } from '@/components/app/app-shell';
import { Stack } from 'expo-router';

export default function AppLayout() {
  return (
    <AppShell>
      <Stack screenOptions={{ headerShown: false }} />
    </AppShell>
  );
}
