import '@/global.css';

import { NAV_THEME } from '@/lib/theme';
import { ThemeProvider } from '@react-navigation/native';
import { PortalHost } from '@rn-primitives/portal';
import { Stack } from 'expo-router';
import { StatusBar } from 'expo-status-bar';
import { SWRConfig } from 'swr';
import { useUniwind } from 'uniwind';
import { SessionProvider } from '@/lib/session/provider';
import { SafeAreaProvider } from 'react-native-safe-area-context';

export {
  // Catch any errors thrown by the Layout component.
  ErrorBoundary,
} from 'expo-router';

export default function RootLayout() {
  const { theme } = useUniwind();

  return (
    <SafeAreaProvider>
      <ThemeProvider value={NAV_THEME[theme ?? 'light']}>
        <SessionProvider>
          <SWRConfig value={{ shouldRetryOnError: false }}>
            <StatusBar style={theme === 'dark' ? 'light' : 'dark'} />
            <Stack screenOptions={{ headerShown: false }} />
            <PortalHost />
          </SWRConfig>
        </SessionProvider>
      </ThemeProvider>
    </SafeAreaProvider>
  );
}
