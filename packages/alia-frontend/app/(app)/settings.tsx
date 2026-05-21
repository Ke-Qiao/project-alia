import { ListPage } from '@/components/app/page';
import { JsonPreview } from '@/components/data/json-preview';
import { StatusBadge } from '@/components/data/status-badge';
import { Field } from '@/components/forms/field';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import { Text } from '@/components/ui/text';
import { Textarea } from '@/components/ui/textarea';
import { usePatchSettings, useSettings } from '@/lib/api/hooks';
import type { SettingEntry, SettingsResponse } from '@/lib/api/types';
import { useTranslation } from '@/lib/session/use-translation';
import * as React from 'react';
import { View } from 'react-native';

type SettingRow = SettingEntry & { key: string };

function normalizeSettings(data: SettingsResponse | undefined): SettingRow[] {
  if (!data) return [];
  if (Array.isArray(data)) return data;
  return Object.entries(data).map(([key, entry]) => ({ key, ...entry }));
}

function parseValue(raw: string, current: unknown) {
  if (typeof current === 'number') return Number(raw);
  if (typeof current === 'boolean') return raw === 'true';
  if (typeof current === 'object') return JSON.parse(raw);
  return raw;
}

export default function SettingsScreen() {
  const tr = useTranslation();
  const [prefix, setPrefix] = React.useState('core');
  const settings = useSettings(prefix);
  const patch = usePatchSettings();
  const [drafts, setDrafts] = React.useState<Record<string, string>>({});
  const rows = normalizeSettings(settings.data);
  const changed = Object.keys(drafts).filter((key) => rows.some((row) => key === row.key));

  async function apply() {
    const body: Record<string, unknown> = {};
    for (const key of changed) {
      const row = rows.find((item) => item.key === key);
      if (row) body[key] = parseValue(drafts[key], row.value);
    }
    await patch.trigger(body);
    setDrafts({});
    await settings.mutate();
  }

  return (
    <ListPage
      title={tr('settings.title')}
      description="Runtime settings are flat, namespaced keys. Hot-applied values report module application status."
      actions={
        <>
          <Input value={prefix} onChangeText={setPrefix} placeholder={tr('settings.prefix')} className="w-56" />
          <Button onPress={apply} disabled={!changed.length || patch.isMutating}>
            <Text>{tr('settings.apply')}</Text>
          </Button>
        </>
      }
      data={rows}
      keyExtractor={(row) => row.key}
      extraData={drafts}
      empty={<Text className="text-muted-foreground text-sm">No settings returned for this prefix.</Text>}
      header={patch.data ? (
        <Card>
          <CardHeader>
            <View className="flex-row items-center justify-between">
              <CardTitle>Last Apply</CardTitle>
              <StatusBadge status={patch.data.success ? 'applied' : 'error'} />
            </View>
          </CardHeader>
          <CardContent>
            <JsonPreview value={patch.data} />
          </CardContent>
        </Card>
      ) : null}
      footer={settings.error || patch.error ? <Text className="text-destructive text-sm">{(settings.error ?? patch.error)?.message}</Text> : null}
      renderItem={({ item: row }) => {
        const schema = row.schema;
        const value = drafts[row.key] ?? JSON.stringify(row.value);
        const label = schema?.label ?? row.key;
        const secret = schema?.secret;
        const valueType = schema?.type ?? typeof row.value;
        return (
          <Card>
            <CardHeader>
              <View className="gap-2 sm:flex-row sm:items-start sm:justify-between">
                <View className="min-w-0 flex-1 gap-1">
                  <CardTitle className="text-base">{label}</CardTitle>
                  <CardDescription numberOfLines={2}>{row.key}</CardDescription>
                </View>
                <View className="flex-row gap-2">
                  {schema?.requires_restart ? <StatusBadge status="restart" /> : <StatusBadge status="runtime" />}
                </View>
              </View>
            </CardHeader>
            <CardContent className="gap-3">
              {schema?.description ? <Text className="text-muted-foreground text-sm">{schema.description}</Text> : null}
              {typeof row.value === 'boolean' ? (
                <Switch
                  checked={(drafts[row.key] ?? String(row.value)) === 'true'}
                  onCheckedChange={(checked) => setDrafts((current) => ({ ...current, [row.key]: String(checked) }))}
                />
              ) : typeof row.value === 'object' ? (
                <Textarea
                  value={value}
                  onChangeText={(next) => setDrafts((current) => ({ ...current, [row.key]: next }))}
                  className="min-h-24 font-mono"
                />
              ) : (
                <Input
                  value={secret && !drafts[row.key] ? '********' : String(drafts[row.key] ?? row.value ?? '')}
                  secureTextEntry={secret}
                  keyboardType={typeof row.value === 'number' ? 'numeric' : 'default'}
                  onChangeText={(next) => setDrafts((current) => ({ ...current, [row.key]: next }))}
                />
              )}
              <Text className="text-muted-foreground text-xs">{valueType}</Text>
            </CardContent>
          </Card>
        );
      }}
    />
  );
}
