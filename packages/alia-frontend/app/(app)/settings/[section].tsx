import { ListPage, Page } from '@/components/app/page';
import { ThemeToggle } from '@/components/app/theme-toggle';
import { FriendlyRecord, StatBadges } from '@/components/data/friendly-data';
import { StatusBadge } from '@/components/data/status-badge';
import { Field } from '@/components/forms/field';
import {
  findSettingsSection,
  type SettingsLink,
  type SettingsSection,
} from '@/components/settings/settings-model';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Icon } from '@/components/ui/icon';
import { Input } from '@/components/ui/input';
import { Separator } from '@/components/ui/separator';
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
import type { SettingEntry, SettingsResponse, SettingsUpdateResponse } from '@/lib/api/types';
import { useSession } from '@/lib/session/provider';
import { useTranslation } from '@/lib/session/use-translation';
import { Link, Redirect, useLocalSearchParams } from 'expo-router';
import { BellRing, Cable, ChevronRight, LogOut, SlidersHorizontal } from 'lucide-react-native';
import * as React from 'react';
import { View } from 'react-native';

type SettingRow = SettingEntry & { key: string };
type ServerSettingSchema = NonNullable<SettingEntry['schema']> & { value_type?: string };

export default function SettingsSectionScreen() {
  const { section: sectionId } = useLocalSearchParams<{ section?: string }>();
  const section = findSettingsSection(sectionId);

  if (!section) {
    return <Redirect href="/settings" />;
  }

  if (section.kind === 'config') {
    return <ConfigSettingsSection section={section} />;
  }

  return <LocalSettingsSection section={section} />;
}

function LocalSettingsSection({ section }: { section: SettingsSection }) {
  const tr = useTranslation();
  const session = useSession();
  const privacy = useMemoryPrivacy();
  const updatePrivacy = useUpdateMemoryPrivacy();
  const relationship = useRelationshipProfile();

  async function toggleMemory(enabled: boolean) {
    await updatePrivacy.trigger({ enabled });
    await privacy.mutate();
  }

  return (
    <Page title={section.title} description={section.description}>
      {section.kind === 'account' ? (
        <Card>
          <CardHeader>
            <SectionTitle section={section} />
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
      ) : null}

      {section.kind === 'server-connection' ? (
        <Card>
          <CardHeader>
            <SectionTitle section={section} />
          </CardHeader>
          <CardContent className="gap-4">
            <View className="gap-1">
              <Text className="font-medium">
                {session.effectiveBaseUrl || 'Same-origin backend'}
              </Text>
              <Text className="text-muted-foreground text-sm leading-5">
                Mobile apps can switch servers from the connection screen. Server-hosted web keeps
                using the same origin.
              </Text>
            </View>
            <Button variant="outline" onPress={() => session.resetConnection()}>
              <Icon as={Cable} className="size-4" />
              <Text>Change server</Text>
            </Button>
          </CardContent>
        </Card>
      ) : null}

      {section.kind === 'appearance' ? (
        <Card>
          <CardHeader>
            <SectionTitle section={section} />
          </CardHeader>
          <CardContent>
            <View className="flex-row items-center justify-between gap-4">
              <View className="min-w-0 flex-1 gap-1">
                <Text className="text-sm font-medium">Theme</Text>
                <Text className="text-muted-foreground text-sm">
                  Switch between light and dark mode.
                </Text>
              </View>
              <ThemeToggle />
            </View>
          </CardContent>
        </Card>
      ) : null}

      {section.kind === 'privacy-memory' ? (
        <Card>
          <CardHeader>
            <SectionTitle section={section} />
          </CardHeader>
          <CardContent className="gap-5">
            <View className="flex-row items-center justify-between gap-4">
              <View className="min-w-0 flex-1 gap-1">
                <Text className="text-sm font-medium">Memory capture</Text>
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
            <Separator />
            <View className="flex-row items-center justify-between gap-4">
              <View className="min-w-0 flex-1 gap-1">
                <Text className="text-sm font-medium">Assistant style</Text>
                <Text className="text-muted-foreground text-sm leading-5" numberOfLines={3}>
                  {relationship.data?.summary || 'Your assistant is still learning your preferences.'}
                </Text>
              </View>
              <StatusBadge status={relationship.data ? 'active' : 'learning'} />
            </View>
          </CardContent>
        </Card>
      ) : null}

      {section.kind === 'advanced-pages' ? (
        <Card>
          <CardHeader>
            <SectionTitle section={section} />
          </CardHeader>
          <CardContent className="gap-2">
            <SettingsLinks links={section.links ?? []} />
          </CardContent>
        </Card>
      ) : null}
    </Page>
  );
}

function ConfigSettingsSection({ section }: { section: SettingsSection }) {
  const editablePrefixes = section.editablePrefixes ?? [];
  const [prefix, setPrefix] = React.useState(editablePrefixes[0] ?? section.namespaces?.[0] ?? '');
  const [drafts, setDrafts] = React.useState<Record<string, string>>({});
  const [localError, setLocalError] = React.useState<string | null>(null);
  const settings = useSettings(prefix || undefined);
  const patch = usePatchSettings();
  const rows = editablePrefixes.length ? normalizeSettings(settings.data) : [];
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

  function changePrefix(nextPrefix: string) {
    setLocalError(null);
    setDrafts({});
    setPrefix(nextPrefix);
  }

  return (
    <ListPage
      title={section.title}
      description={section.description}
      data={rows}
      keyExtractor={(row) => row.key}
      extraData={{ drafts, prefix }}
      header={
        <>
          <ConfigOverviewCard
            section={section}
            prefix={prefix}
            editablePrefixes={editablePrefixes}
            onPrefixChange={changePrefix}
          />
          {editablePrefixes.length ? (
            <Card>
              <CardHeader>
                <View className="gap-3 sm:flex-row sm:items-start sm:justify-between">
                  <View className="min-w-0 flex-1 gap-1">
                    <View className="flex-row items-center gap-2">
                      <Icon as={SlidersHorizontal} className="size-5" />
                      <CardTitle>Editable values</CardTitle>
                    </View>
                    <CardDescription>
                      Editing namespace <Text className="font-medium">{prefix}</Text>. Apply only
                      the changed values below.
                    </CardDescription>
                  </View>
                  <StatusBadge status={`${changed.length} changed`} />
                </View>
              </CardHeader>
              <CardContent className="gap-4">
                {patch.data ? <SettingsApplySummary result={patch.data} /> : null}
                <Button onPress={apply} disabled={!changed.length || patch.isMutating}>
                  <Icon as={BellRing} className="size-4" />
                  <Text>{patch.isMutating ? 'Applying...' : 'Apply changes'}</Text>
                </Button>
              </CardContent>
            </Card>
          ) : null}
        </>
      }
      empty={
        editablePrefixes.length ? (
          <Text className="text-muted-foreground text-sm">
            {settings.isLoading
              ? 'Loading editable settings...'
              : 'No editable settings were returned for this group.'}
          </Text>
        ) : null
      }
      footer={
        localError || settings.error || patch.error ? (
          <Text className="text-destructive text-sm">
            {localError ?? (settings.error ?? patch.error)?.message}
          </Text>
        ) : null
      }
      renderItem={({ item: row }) => (
        <SettingEditorCard
          row={row}
          draft={drafts[row.key]}
          onDraftChange={(value) => setDrafts((current) => ({ ...current, [row.key]: value }))}
        />
      )}
    />
  );
}

function ConfigOverviewCard({
  editablePrefixes,
  onPrefixChange,
  prefix,
  section,
}: {
  editablePrefixes: string[];
  onPrefixChange: (prefix: string) => void;
  prefix: string;
  section: SettingsSection;
}) {
  return (
    <Card>
      <CardHeader>
        <View className="gap-3 sm:flex-row sm:items-start sm:justify-between">
          <SectionTitle section={section} />
          {section.status ? <StatusBadge status={section.status} /> : null}
        </View>
      </CardHeader>
      <CardContent className="gap-4">
        {section.source ? (
          <View className="gap-2">
            <Text className="text-muted-foreground text-xs font-medium uppercase">
              Config source
            </Text>
            <Text className="text-sm">{section.source}</Text>
          </View>
        ) : null}
        {section.namespaces?.length ? (
          <View className="flex-row flex-wrap gap-2">
            {section.namespaces.map((namespace) => (
              <View key={namespace} className="bg-muted rounded-md px-2.5 py-1">
                <Text className="text-muted-foreground text-xs">{namespace}</Text>
              </View>
            ))}
          </View>
        ) : null}
        {editablePrefixes.length ? (
          <View className="gap-2">
            <Text className="text-muted-foreground text-xs font-medium uppercase">
              Editable groups
            </Text>
            <View className="flex-row flex-wrap gap-2">
              {editablePrefixes.map((editablePrefix) => (
                <Button
                  key={editablePrefix}
                  size="sm"
                  variant={editablePrefix === prefix ? 'default' : 'outline'}
                  onPress={() => onPrefixChange(editablePrefix)}>
                  <Text>{editablePrefix.replace(/^core\./, '')}</Text>
                </Button>
              ))}
            </View>
          </View>
        ) : (
          <View className="border-border bg-muted/40 rounded-md border p-3">
            <Text className="text-muted-foreground text-sm leading-5">
              This area is mostly file-backed today. Keep it discoverable here, but edit it through
              server config fragments until the runtime registry exposes those keys.
            </Text>
          </View>
        )}
        {section.links?.length ? <SettingsLinks links={section.links} /> : null}
      </CardContent>
    </Card>
  );
}

function SectionTitle({ section }: { section: SettingsSection }) {
  return (
    <View className="min-w-0 flex-1 flex-row items-start gap-3">
      <View className="bg-secondary rounded-lg p-2">
        <Icon as={section.icon} className="size-5" />
      </View>
      <View className="min-w-0 flex-1 gap-1">
        <CardTitle>{section.title}</CardTitle>
        <CardDescription>{section.description}</CardDescription>
      </View>
    </View>
  );
}

function SettingsLinks({ links }: { links: SettingsLink[] }) {
  return (
    <View className="gap-2">
      {links.map((link) => (
        <Link key={link.title} href={link.href} asChild>
          <Button variant="outline" className="h-auto min-h-16 justify-between p-3">
            <View className="min-w-0 flex-1 flex-row items-start gap-3">
              <View className="bg-secondary rounded-md p-2">
                <Icon as={link.icon} className="size-4" />
              </View>
              <View className="min-w-0 flex-1 gap-0.5">
                <Text className="text-sm font-medium">{link.title}</Text>
                <Text className="text-muted-foreground text-xs leading-4" numberOfLines={2}>
                  {link.description}
                </Text>
              </View>
            </View>
            <Icon as={ChevronRight} className="text-muted-foreground size-4" />
          </Button>
        </Link>
      ))}
    </View>
  );
}

function SettingEditorCard({
  draft,
  onDraftChange,
  row,
}: {
  draft?: string;
  onDraftChange: (value: string) => void;
  row: SettingRow;
}) {
  const schema = row.schema as ServerSettingSchema | null | undefined;
  const valueType = schema?.type ?? schema?.value_type ?? friendlyType(row.value);
  const label = schema?.label ?? friendlySettingName(row.key);
  const options = schema?.options ?? [];
  const currentText = draft ?? valueToDraftString(row.value);
  const secret = schema?.secret;

  return (
    <Card>
      <CardHeader>
        <View className="gap-2 sm:flex-row sm:items-start sm:justify-between">
          <View className="min-w-0 flex-1 gap-1">
            <CardTitle className="text-base">{label}</CardTitle>
            <CardDescription numberOfLines={2}>{friendlySettingPath(row.key)}</CardDescription>
          </View>
          <View className="flex-row flex-wrap gap-2">
            <StatusBadge status={schema?.requires_restart ? 'restart needed' : 'live'} />
            <StatusBadge status={valueType} />
          </View>
        </View>
      </CardHeader>
      <CardContent className="gap-4">
        {schema?.description ? (
          <Text className="text-muted-foreground text-sm leading-5">{schema.description}</Text>
        ) : null}
        {options.length ? (
          <View className="gap-2">
            <Text className="text-sm font-medium">Choose a value</Text>
            <View className="flex-row flex-wrap gap-2">
              {options.map((option) => {
                const selected = String(draft ?? row.value) === option;
                return (
                  <Button
                    key={option}
                    size="sm"
                    variant={selected ? 'default' : 'outline'}
                    onPress={() => onDraftChange(option)}>
                    <Text>{option}</Text>
                  </Button>
                );
              })}
            </View>
          </View>
        ) : null}
        {typeof row.value === 'boolean' ? (
          <View className="flex-row items-center justify-between gap-4">
            <View className="min-w-0 flex-1 gap-1">
              <Text className="text-sm font-medium">
                {currentText === 'true' ? 'Enabled' : 'Disabled'}
              </Text>
              <Text className="text-muted-foreground text-sm">
                Toggle this server setting and apply changes.
              </Text>
            </View>
            <Switch
              checked={currentText === 'true'}
              onCheckedChange={(checked) => onDraftChange(String(checked))}
            />
          </View>
        ) : typeof row.value === 'object' ? (
          <View className="gap-3">
            <FriendlyRecord value={row.value} empty="No structured value." />
            <Field
              label="Structured value"
              description="Advanced JSON editing. Keep the value valid before applying.">
              <Textarea
                value={currentText}
                onChangeText={onDraftChange}
                className="min-h-28 font-mono"
              />
            </Field>
          </View>
        ) : (
          <Field
            label="Value"
            description={secret ? 'Stored as a secret on the server.' : undefined}>
            <Input
              value={secret && !draft ? '********' : currentText}
              secureTextEntry={secret}
              keyboardType={typeof row.value === 'number' ? 'numeric' : 'default'}
              onChangeText={onDraftChange}
            />
          </Field>
        )}
      </CardContent>
    </Card>
  );
}

function SettingsApplySummary({ result }: { result: SettingsUpdateResponse }) {
  return (
    <View className="border-border bg-muted/40 gap-3 rounded-md border p-3">
      <View className="flex-row items-start justify-between gap-3">
        <View className="min-w-0 flex-1 gap-1">
          <Text className="font-medium">Apply result</Text>
          <Text className="text-muted-foreground text-xs">
            {result.success
              ? 'Server accepted the settings update.'
              : 'Some settings could not be applied.'}
          </Text>
        </View>
        <StatusBadge status={result.success ? 'applied' : 'error'} />
      </View>
      <StatBadges
        items={[
          { label: 'Changed', value: result.changed.length },
          { label: 'Applied', value: result.applied.length },
          { label: 'Restart needed', value: result.requires_restart.length },
          { label: 'Errors', value: result.errors.length },
        ]}
      />
      <FriendlyRecord
        value={{
          changed: result.changed,
          requires_restart: result.requires_restart,
          ignored_runtime: result.ignored_runtime,
          errors: result.errors,
        }}
        empty="No additional apply details."
      />
    </View>
  );
}

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

function valueToDraftString(value: unknown) {
  if (typeof value === 'object') return JSON.stringify(value, null, 2);
  return String(value ?? '');
}

function friendlyType(value: unknown) {
  if (Array.isArray(value)) return 'array';
  if (value === null) return 'empty';
  return typeof value;
}

function friendlySettingPath(key: string) {
  return key
    .replace(/^core\./, '')
    .split('.')
    .map(friendlySettingPart)
    .join(' / ');
}

function friendlySettingName(key: string) {
  const parts = key.split('.');
  return friendlySettingPart(parts[parts.length - 1] ?? key);
}

function friendlySettingPart(part: string) {
  return part.replace(/_/g, ' ').replace(/\b\w/g, (letter) => letter.toUpperCase());
}
