import { ThemeToggle } from '@/components/app/theme-toggle';
import { Field } from '@/components/forms/field';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Text } from '@/components/ui/text';
import { Textarea } from '@/components/ui/textarea';
import { useSaveSetupConfig, useSetupTemplate } from '@/lib/api/hooks';
import { useTranslation } from '@/lib/session/use-translation';
import { router, type Href } from 'expo-router';
import * as React from 'react';
import { StyleSheet, View } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';

export default function SetupScreen() {
  const tr = useTranslation();
  const template = useSetupTemplate();
  const save = useSaveSetupConfig();
  const [config, setConfig] = React.useState('');
  const [path, setPath] = React.useState('');
  const [message, setMessage] = React.useState<string | null>(null);

  async function loadTemplate() {
    const response = await template.trigger(undefined);
    setConfig(response.template);
  }

  async function submit() {
    const response = await save.trigger({ config, path: path || undefined });
    if (response.success) {
      setMessage(`Saved to ${response.path}. Restarting runtime may take a moment.`);
      setTimeout(() => router.replace('/' as Href), 1200);
    } else {
      setMessage(response.error ?? 'Unable to save config.');
    }
  }

  return (
    <SafeAreaView edges={['top', 'bottom', 'left', 'right']} className="bg-background" style={styles.safeArea}>
      <View className="absolute right-4 top-4 z-10">
        <ThemeToggle />
      </View>
      <View className="mx-auto w-full max-w-4xl flex-1 justify-center">
        <Card>
          <CardHeader>
            <CardTitle>{tr('setup.title')}</CardTitle>
            <CardDescription>{tr('setup.description')}</CardDescription>
          </CardHeader>
          <CardContent className="gap-4">
            <View className="flex-row flex-wrap gap-2">
              <Button variant="outline" onPress={loadTemplate} disabled={template.isMutating}>
                <Text>{tr('setup.generate')}</Text>
              </Button>
            </View>
            <Field label="Target path" description="Leave blank to let the server choose config/server.toml or user config dir.">
              <Input value={path} onChangeText={setPath} placeholder="config/server.toml" />
            </Field>
            <Field label="server.toml">
              <Textarea value={config} onChangeText={setConfig} className="min-h-96 font-mono" />
            </Field>
            {message ? <Text className="text-muted-foreground text-sm">{message}</Text> : null}
            {template.error || save.error ? (
              <Text className="text-destructive text-sm">{(template.error ?? save.error)?.message}</Text>
            ) : null}
            <Button onPress={submit} disabled={!config || save.isMutating}>
              <Text>{save.isMutating ? 'Saving...' : tr('setup.save')}</Text>
            </Button>
          </CardContent>
        </Card>
      </View>
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  safeArea: {
    flex: 1,
    padding: 16,
  },
});
