import { ThemeToggle } from '@/components/app/theme-toggle';
import { Field } from '@/components/forms/field';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Icon } from '@/components/ui/icon';
import { Input } from '@/components/ui/input';
import { Text } from '@/components/ui/text';
import { useAuthLogin, useAuthRegister } from '@/lib/api/hooks';
import { useSession } from '@/lib/session/provider';
import { useTranslation } from '@/lib/session/use-translation';
import { Redirect, router, type Href } from 'expo-router';
import { ChevronLeft, Sparkles } from 'lucide-react-native';
import * as React from 'react';
import { Platform, Pressable, StyleSheet, View } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';

export default function LoginScreen() {
  const session = useSession();
  const tr = useTranslation();
  const [mode, setMode] = React.useState<'login' | 'register'>('login');
  const [email, setEmail] = React.useState('');
  const [name, setName] = React.useState('');
  const [password, setPassword] = React.useState('');
  const login = useAuthLogin();
  const register = useAuthRegister();
  const pending = login.isMutating || register.isMutating;
  const error = login.error ?? register.error;

  if (!session.hydrated) return null;
  if (session.effectiveBaseUrl === null) return <Redirect href={'/connect' as Href} />;
  if (session.accessToken) return <Redirect href={'/runtime' as Href} />;

  async function submit() {
    const response =
      mode === 'login'
        ? await login.trigger({ email, password })
        : await register.trigger({ email, name: name || email.split('@')[0], password });
    await session.setAuth({
      accessToken: response.access_token,
      refreshToken: response.refresh_token,
      user: response.user,
    });
    router.replace('/runtime' as Href);
  }

  return (
    <SafeAreaView
      edges={['top', 'bottom', 'left', 'right']}
      className="bg-background"
      style={styles.safeArea}>
      {Platform.OS === 'web' ? (
        <View className="absolute top-4 right-4">
          <ThemeToggle />
        </View>
      ) : null}
      <View className="absolute top-4 left-4 z-10">
        <Button
          accessibilityLabel="Back to connection"
          variant="ghost"
          size="icon"
          onPress={() => router.replace('/connect' as Href)}
          className="rounded-full">
          <Icon as={ChevronLeft} className="size-5" />
        </Button>
      </View>
      <View className="w-full max-w-md gap-4">
        <View className="items-center gap-3">
          <View className="bg-primary/10 size-12 items-center justify-center rounded-xl">
            <Icon as={Sparkles} className="text-primary size-6" />
          </View>
          <View className="items-center gap-1">
            <Text className="text-2xl font-semibold tracking-normal">Welcome to Alia</Text>
            <Text className="text-muted-foreground text-center text-sm">
              Sign in to continue with your personal assistant.
            </Text>
          </View>
        </View>

        <Card>
          <CardHeader>
            <CardTitle>{mode === 'login' ? tr('auth.login') : tr('auth.register')}</CardTitle>
            <CardDescription>
              {mode === 'login'
                ? 'Use the account on your Alia server.'
                : 'Create the first profile for this assistant.'}
            </CardDescription>
          </CardHeader>
          <CardContent className="gap-4">
            <View className="bg-muted flex-row rounded-md p-1">
              {(['login', 'register'] as const).map((item) => (
                <Pressable
                  key={item}
                  onPress={() => setMode(item)}
                  className={`h-9 flex-1 items-center justify-center rounded ${mode === item ? 'bg-background' : ''}`}>
                  <Text className="text-sm font-medium">
                    {item === 'login' ? tr('auth.login') : tr('auth.register')}
                  </Text>
                </Pressable>
              ))}
            </View>
            <Field label={tr('auth.email')}>
              <Input
                autoCapitalize="none"
                autoCorrect={false}
                inputMode="email"
                value={email}
                onChangeText={setEmail}
              />
            </Field>
            {mode === 'register' ? (
              <Field label={tr('auth.name')}>
                <Input value={name} onChangeText={setName} />
              </Field>
            ) : null}
            <Field label={tr('auth.password')}>
              <Input secureTextEntry value={password} onChangeText={setPassword} />
            </Field>
            {error ? <Text className="text-destructive text-sm">{error.message}</Text> : null}
            <Button disabled={pending || !email || password.length < 8} onPress={submit}>
              <Text>
                {pending ? 'Working...' : mode === 'login' ? tr('auth.login') : tr('auth.register')}
              </Text>
            </Button>
            <Button variant="ghost" onPress={() => router.replace('/connect' as Href)}>
              <Text>Change server</Text>
            </Button>
            <Text className="text-muted-foreground text-center text-xs" numberOfLines={1}>
              {session.effectiveBaseUrl || 'Same-origin backend'}
            </Text>
          </CardContent>
        </Card>
      </View>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  safeArea: {
    alignItems: 'center',
    flex: 1,
    justifyContent: 'center',
    padding: 16,
  },
});
