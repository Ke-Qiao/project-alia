import { ThemeToggle } from '@/components/app/theme-toggle';
import { ListPage } from '@/components/app/page';
import { JsonPreview } from '@/components/data/json-preview';
import { StatusBadge } from '@/components/data/status-badge';
import { Field } from '@/components/forms/field';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Icon } from '@/components/ui/icon';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import { Text } from '@/components/ui/text';
import { Textarea } from '@/components/ui/textarea';
import {
  useMemoryPrivacy,
  usePatchSettings,
  useRelationshipProfile,
  useSettings,
  useUpdateMemoryPrivacy,
} from '@/lib/api/hooks';
import type { SettingEntry, SettingsResponse } from '@/lib/api/types';
import { useSession } from '@/lib/session/provider';
import { useTranslation } from '@/lib/session/use-translation';
import { Link, type Href } from 'expo-router';
import {
  BellRing,
  Brain,
  Cable,
  ChevronRight,
  HeartPulse,
  LogOut,
  Palette,
  Radar,
  Server,
  SlidersHorizontal,
  UserRound,
  Wrench,
  type LucideIcon,
} from 'lucide-react-native';
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
  const session = useSession();
  const [prefix, setPrefix] = React.useState('core');
  const [localError, setLocalError] = React.useState<string | null>(null);
  const [showAdvancedSettings, setShowAdvancedSettings] = React.useState(false);
  const settings = useSettings(prefix);
  const patch = usePatchSettings();
  const privacy = useMemoryPrivacy();
  const updatePrivacy = useUpdateMemoryPrivacy();
  const relationship = useRelationshipProfile();
  const [drafts, setDrafts] = React.useState<Record<string, string>>({});
  const rows = normalizeSettings(settings.data);
  const changed = Object.keys(drafts).filter((key) => rows.some((row) => key === row.key));

  async function apply() {
    setLocalError(null);
    try {
      const body: Record<string, unknown> = {};
      for (const key of changed) {
        const row = rows.find((item) => item.key === key);
        if (row) body[key] = parseValue(drafts[key], row.value);
      }
      await patch.trigger(body);
      setDrafts({});
      await settings.mutate();
    } catch (err) {
      setLocalError(err instanceof Error ? err.message : 'Unable to apply settings.');
    }
  }

  async function toggleMemory(enabled: boolean) {
    await updatePrivacy.trigger({ enabled });
    await privacy.mutate();
  }

  return (
    <ListPage
      title={tr('settings.title')}
      description={tr('settings.description')}
      data={showAdvancedSettings ? rows : []}
      keyExtractor={(row) => row.key}
      extraData={{ drafts, showAdvancedSettings }}
      empty={
        showAdvancedSettings ? (
          <Text className="text-muted-foreground text-sm">
            No advanced settings returned for this namespace.
          </Text>
        ) : null
      }
      header={
        <>
          <View className="gap-4 lg:flex-row">
            <Card className="flex-1">
              <CardHeader>
                <View className="flex-row items-start gap-3">
                  <View className="bg-secondary rounded-lg p-2">
                    <Icon as={UserRound} className="size-5" />
                  </View>
                  <View className="min-w-0 flex-1 gap-1">
                    <CardTitle>Account</CardTitle>
                    <CardDescription>Signed in to your personal Alia space.</CardDescription>
                  </View>
                </View>
              </CardHeader>
              <CardContent className="gap-4">
                <View className="gap-1">
                  <Text className="font-medium">
                    {session.user?.name || session.user?.email || 'Alia user'}
                  </Text>
                  <Text className="text-muted-foreground text-sm">
                    {session.user?.email || 'No email in session'}
                  </Text>
                </View>
                <Button variant="outline" onPress={() => session.clearAuth()}>
                  <Icon as={LogOut} className="size-4" />
                  <Text>{tr('auth.logout')}</Text>
                </Button>
              </CardContent>
            </Card>

            <Card className="flex-1">
              <CardHeader>
                <View className="flex-row items-start gap-3">
                  <View className="bg-secondary rounded-lg p-2">
                    <Icon as={Server} className="size-5" />
                  </View>
                  <View className="min-w-0 flex-1 gap-1">
                    <CardTitle>Server</CardTitle>
                    <CardDescription>Where this app sends Alia requests.</CardDescription>
                  </View>
                </View>
              </CardHeader>
              <CardContent className="gap-4">
                <View className="gap-1">
                  <Text className="font-medium">
                    {session.effectiveBaseUrl || 'Same-origin backend'}
                  </Text>
                  <Text className="text-muted-foreground text-sm">
                    Mobile apps can switch servers from the connection screen.
                  </Text>
                </View>
                <Button variant="outline" onPress={() => session.resetConnection()}>
                  <Icon as={Cable} className="size-4" />
                  <Text>Change server</Text>
                </Button>
              </CardContent>
            </Card>
          </View>

          <View className="gap-4 lg:flex-row">
            <Card className="flex-1">
              <CardHeader>
                <View className="flex-row items-start gap-3">
                  <View className="bg-secondary rounded-lg p-2">
                    <Icon as={Palette} className="size-5" />
                  </View>
                  <View className="min-w-0 flex-1 gap-1">
                    <CardTitle>Appearance</CardTitle>
                    <CardDescription>Theme controls live here on mobile.</CardDescription>
                  </View>
                </View>
              </CardHeader>
              <CardContent>
                <View className="flex-row items-center justify-between gap-4">
                  <View className="min-w-0 flex-1">
                    <Text className="text-sm font-medium">Theme</Text>
                    <Text className="text-muted-foreground text-sm">
                      Switch between light and dark mode.
                    </Text>
                  </View>
                  <ThemeToggle />
                </View>
              </CardContent>
            </Card>

            <Card className="flex-1">
              <CardHeader>
                <View className="flex-row items-start gap-3">
                  <View className="bg-secondary rounded-lg p-2">
                    <Icon as={Brain} className="size-5" />
                  </View>
                  <View className="min-w-0 flex-1 gap-1">
                    <CardTitle>Assistant</CardTitle>
                    <CardDescription>
                      Personal memory and communication preferences.
                    </CardDescription>
                  </View>
                </View>
              </CardHeader>
              <CardContent className="gap-4">
                <View className="flex-row items-center justify-between gap-4">
                  <View className="min-w-0 flex-1 gap-1">
                    <Text className="text-sm font-medium">Memory</Text>
                    <Text className="text-muted-foreground text-sm">
                      {privacy.data?.capture_mode
                        ? `Capture mode: ${privacy.data.capture_mode}`
                        : 'Personal context controls'}
                    </Text>
                  </View>
                  <Switch
                    checked={privacy.data?.enabled ?? false}
                    disabled={updatePrivacy.isMutating || !privacy.data}
                    onCheckedChange={toggleMemory}
                  />
                </View>
                <View className="flex-row items-center justify-between gap-4">
                  <View className="min-w-0 flex-1 gap-1">
                    <Text className="text-sm font-medium">Assistant style</Text>
                    <Text className="text-muted-foreground text-sm" numberOfLines={2}>
                      {relationship.data?.summary || 'Alia is still learning your preferences.'}
                    </Text>
                  </View>
                  <StatusBadge status={relationship.data ? 'active' : 'learning'} />
                </View>
              </CardContent>
            </Card>
          </View>

          <Card>
            <CardHeader>
              <CardTitle>Advanced workspace</CardTitle>
              <CardDescription>
                Less common assistant controls are still available here without crowding the mobile
                home screen.
              </CardDescription>
            </CardHeader>
            <CardContent className="gap-2 md:flex-row">
              <AdvancedLink
                href="/skills"
                title="Skills"
                description="Intent matching and prompt-backed abilities"
                icon={Brain}
              />
              <AdvancedLink
                href="/tools"
                title="Tools"
                description="Callable server tools"
                icon={Wrench}
              />
              <AdvancedLink
                href="/expression"
                title="Assistant style"
                description="Relationship and preference model"
                icon={HeartPulse}
              />
              <AdvancedLink
                href="/signals"
                title="Signals"
                description="Sensor and event summaries"
                icon={Radar}
              />
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <View className="gap-3 sm:flex-row sm:items-start sm:justify-between">
                <View className="min-w-0 flex-1 gap-1">
                  <View className="flex-row items-center gap-2">
                    <Icon as={SlidersHorizontal} className="size-5" />
                    <CardTitle>Advanced settings</CardTitle>
                  </View>
                  <CardDescription>
                    Edit raw server settings only when you know the namespace and expected value
                    shape.
                  </CardDescription>
                </View>
                <StatusBadge
                  status={showAdvancedSettings ? `${changed.length} changed` : 'hidden'}
                />
              </View>
            </CardHeader>
            <CardContent className="gap-4">
              <View className="gap-3 md:flex-row md:items-center md:justify-between">
                <Text className="text-muted-foreground flex-1 text-sm leading-5">
                  Most people never need this area. Open it when you are changing server modules,
                  namespaces, or raw configuration values.
                </Text>
                <Button
                  variant="outline"
                  onPress={() => setShowAdvancedSettings((current) => !current)}>
                  <Icon as={SlidersHorizontal} className="size-4" />
                  <Text>{showAdvancedSettings ? 'Hide raw settings' : 'Show raw settings'}</Text>
                </Button>
              </View>
              {showAdvancedSettings ? (
                <View className="gap-3 md:flex-row md:items-end">
                  <View className="flex-1">
                    <Field
                      label={tr('settings.prefix')}
                      description="Examples: core, memory, channels">
                      <Input value={prefix} onChangeText={setPrefix} placeholder="core" />
                    </Field>
                  </View>
                  <Button onPress={apply} disabled={!changed.length || patch.isMutating}>
                    <Icon as={BellRing} className="size-4" />
                    <Text>{patch.isMutating ? 'Applying...' : tr('settings.apply')}</Text>
                  </Button>
                </View>
              ) : null}
              {showAdvancedSettings && patch.data ? <JsonPreview value={patch.data} /> : null}
            </CardContent>
          </Card>
        </>
      }
      footer={
        localError || settings.error || patch.error || privacy.error || updatePrivacy.error ? (
          <Text className="text-destructive text-sm">
            {localError ??
              (settings.error ?? patch.error ?? privacy.error ?? updatePrivacy.error)?.message}
          </Text>
        ) : null
      }
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
                  {schema?.requires_restart ? (
                    <StatusBadge status="restart" />
                  ) : (
                    <StatusBadge status="runtime" />
                  )}
                </View>
              </View>
            </CardHeader>
            <CardContent className="gap-3">
              {schema?.description ? (
                <Text className="text-muted-foreground text-sm">{schema.description}</Text>
              ) : null}
              {typeof row.value === 'boolean' ? (
                <Switch
                  checked={(drafts[row.key] ?? String(row.value)) === 'true'}
                  onCheckedChange={(checked) =>
                    setDrafts((current) => ({ ...current, [row.key]: String(checked) }))
                  }
                />
              ) : typeof row.value === 'object' ? (
                <Textarea
                  value={value}
                  onChangeText={(next) => setDrafts((current) => ({ ...current, [row.key]: next }))}
                  className="min-h-24 font-mono"
                />
              ) : (
                <Input
                  value={
                    secret && !drafts[row.key]
                      ? '********'
                      : String(drafts[row.key] ?? row.value ?? '')
                  }
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

function AdvancedLink({
  description,
  href,
  icon,
  title,
}: {
  description: string;
  href: string;
  icon: LucideIcon;
  title: string;
}) {
  return (
    <Link href={href as Href} asChild>
      <Button variant="outline" className="h-auto min-h-20 flex-1 items-start justify-between p-3">
        <View className="min-w-0 flex-1 flex-row items-start gap-3">
          <View className="bg-secondary rounded-md p-2">
            <Icon as={icon} className="size-4" />
          </View>
          <View className="min-w-0 flex-1 gap-1">
            <Text className="font-medium">{title}</Text>
            <Text className="text-muted-foreground text-xs" numberOfLines={2}>
              {description}
            </Text>
          </View>
        </View>
        <Icon as={ChevronRight} className="text-muted-foreground size-4" />
      </Button>
    </Link>
  );
}
