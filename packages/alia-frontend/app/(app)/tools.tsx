import { ListPage } from '@/components/app/page';
import { EmptyState } from '@/components/data/empty-state';
import {
  FriendlyRecord,
  InfoRows,
  StatBadges,
  formatFriendlyValue,
  humanizeKey,
} from '@/components/data/friendly-data';
import { StatusBadge } from '@/components/data/status-badge';
import { Field } from '@/components/forms/field';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Text } from '@/components/ui/text';
import { Textarea } from '@/components/ui/textarea';
import { useApiAction, useExecuteTool, useTools } from '@/lib/api/hooks';
import {
  ApiError,
  type EffectRecord,
  type ExecuteToolRequest,
  type JsonValue,
} from '@/lib/api/types';
import { useSession } from '@/lib/session/provider';
import { useTranslation } from '@/lib/session/use-translation';
import { Wrench } from 'lucide-react-native';
import * as React from 'react';
import { View } from 'react-native';

export default function ToolsScreen() {
  const session = useSession();
  return <ToolsContents key={`${session.effectiveBaseUrl}:${session.user?.id ?? ''}`} />;
}

function ToolsContents() {
  const tr = useTranslation();
  const tools = useTools();
  const execute = useExecuteTool();
  const action = useApiAction<EffectRecord>();
  const [result, setResult] = React.useState<EffectRecord | null>(null);
  const [formError, setFormError] = React.useState<string | null>(null);
  // Retain the same intent and UUID after an ambiguous network response.
  const request = React.useRef<ExecuteToolRequest | null>(null);
  const [hasRequest, setHasRequest] = React.useState(false);
  const attempts = React.useRef(0);
  const [selected, setSelected] = React.useState('');
  const [argumentsJson, setArgumentsJson] = React.useState('{}');

  React.useEffect(() => {
    if (!selected && tools.data?.[0]) setSelected(tools.data[0].name);
  }, [selected, tools.data]);

  async function runTool() {
    setFormError(null);
    try {
      if (!request.current) {
        const args: JsonValue = JSON.parse(argumentsJson || '{}');
        if (!args || typeof args !== 'object' || Array.isArray(args)) {
          throw new Error('Arguments must be a JSON object.');
        }
        // This correlation key is not a credential. Prefer platform UUIDs;
        // the fallback supports native runtimes without Web Crypto.
        const id =
          globalThis.crypto?.randomUUID?.() ??
          'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, (part) => {
            const value = Math.floor(Math.random() * 16);
            return (part === 'x' ? value : (value & 3) | 8).toString(16);
          });
        request.current = { request_id: id, tool_name: selected, arguments: args };
        setHasRequest(true);
      }
      const reconcile = attempts.current > 0;
      attempts.current += 1;
      setResult(await execute.trigger({ request: request.current, reconcile }));
    } catch (error) {
      // A first-attempt schema rejection precedes proposal. After an ambiguous
      // response, keep the original request even if a later retry is rejected.
      if (attempts.current === 1 && error instanceof ApiError && error.status === 400) {
        request.current = null;
        setHasRequest(false);
        attempts.current = 0;
      }
      setFormError(error instanceof Error ? error.message : String(error));
    }
  }

  async function updateEffect(kind: 'refresh' | 'confirm' | 'cancel') {
    if (!result) return;
    setFormError(null);
    try {
      setResult(
        await action.trigger({
          path: `/api/v1/effects/${result.effect_id}${kind === 'refresh' ? '' : `/${kind}`}`,
          method: kind === 'refresh' ? 'GET' : 'POST',
          ...(kind === 'confirm' ? { body: { intent_digest: result.intent_digest } } : {}),
        })
      );
    } catch (error) {
      setFormError(error instanceof Error ? error.message : String(error));
    }
  }

  function newAction() {
    request.current = null;
    attempts.current = 0;
    setHasRequest(false);
    setResult(null);
    setFormError(null);
    execute.reset();
    action.reset();
  }
  const busy = execute.isMutating || action.isMutating;

  return (
    <ListPage
      title={tr('tools.title')}
      description="Choose a tool, review its arguments, and track its recorded execution status."
      data={tools.data ?? []}
      keyExtractor={(tool) => tool.name}
      empty={
        <EmptyState
          title="No tools found"
          description="Tool providers register executable functions during server startup."
          icon={Wrench}
        />
      }
      header={
        <Card>
          <CardHeader>
            <CardTitle>{tr('tools.execute')}</CardTitle>
            <CardDescription>
              Actions may require confirmation. A pending or unknown result does not mean the action
              succeeded.
            </CardDescription>
          </CardHeader>
          <CardContent className="gap-4">
            <Field label="Tool name">
              <Input
                value={selected}
                onChangeText={setSelected}
                editable={!hasRequest}
                placeholder="tool.name"
              />
            </Field>
            <Field label={tr('tools.arguments')}>
              <Textarea
                value={argumentsJson}
                onChangeText={setArgumentsJson}
                editable={!hasRequest}
                className="min-h-32 font-mono"
              />
            </Field>
            {!result ? (
              <Button onPress={runTool} disabled={!selected || busy}>
                <Text>
                  {execute.isMutating
                    ? 'Submitting...'
                    : hasRequest
                      ? 'Retry same request'
                      : tr('tools.execute')}
                </Text>
              </Button>
            ) : null}
            {result ? (
              <>
                <ToolResultCard result={result} />
                <View className="flex-row flex-wrap gap-2">
                  <Button onPress={() => updateEffect('refresh')} disabled={busy}>
                    <Text>Refresh status</Text>
                  </Button>
                  {result.status === 'proposed' ? (
                    <Button onPress={() => updateEffect('confirm')} disabled={busy}>
                      <Text>Confirm this action</Text>
                    </Button>
                  ) : null}
                  {['proposed', 'approved'].includes(result.status) ? (
                    <Button onPress={() => updateEffect('cancel')} disabled={busy}>
                      <Text>Cancel action</Text>
                    </Button>
                  ) : null}
                  {['succeeded', 'failed', 'cancelled', 'unknown'].includes(result.status) ? (
                    <Button onPress={newAction} disabled={busy}>
                      <Text>New action</Text>
                    </Button>
                  ) : null}
                </View>
              </>
            ) : null}
            {formError ? <Text className="text-destructive text-sm">{formError}</Text> : null}
          </CardContent>
        </Card>
      }
      footer={
        tools.error ? <Text className="text-destructive text-sm">{tools.error.message}</Text> : null
      }
      renderItem={({ item: tool }) => (
        <Card>
          <CardHeader>
            <View className="gap-2 sm:flex-row sm:items-start sm:justify-between">
              <View className="min-w-0 flex-1 gap-1">
                <CardTitle numberOfLines={1}>{tool.name}</CardTitle>
                <CardDescription>{tool.source}</CardDescription>
              </View>
              <StatusBadge status={tool.require_confirmation ? 'confirm' : 'direct'} />
            </View>
          </CardHeader>
          <CardContent className="gap-3">
            <Text className="text-muted-foreground text-sm leading-5">{tool.description}</Text>
            <View className="flex-row flex-wrap gap-2">
              {tool.required_role ? <StatusBadge status={tool.required_role} /> : null}
              {(tool.routing_hints ?? []).slice(0, 4).map((hint) => (
                <StatusBadge key={hint} status={hint} />
              ))}
            </View>
            <ToolParameters value={tool.parameters} />
          </CardContent>
        </Card>
      )}
    />
  );
}

function ToolResultCard({ result }: { result: EffectRecord }) {
  const receipt = result.completion?.status === 'succeeded' ? result.completion.receipt : null;
  return (
    <View className="border-border bg-muted/40 gap-3 rounded-md border p-3">
      <View className="flex-row items-start justify-between gap-3">
        <View className="min-w-0 flex-1 gap-1">
          <Text className="font-medium">{result.tool_name}</Text>
          <Text className="text-muted-foreground text-xs">Record {result.effect_id}</Text>
        </View>
        <StatusBadge status={result.status} />
      </View>
      <FriendlyRecord value={result.arguments} empty="This action has no arguments." />
      {result.status === 'unknown' ? (
        <Text className="text-destructive text-sm">
          The action may have taken effect. Inspect the external system before starting another
          action.
        </Text>
      ) : null}
      {result.status === 'proposed' ? (
        <Text className="text-sm">Review these exact arguments before confirming.</Text>
      ) : null}
      {receipt?.display ? <Text className="text-sm leading-5">{receipt.display}</Text> : null}
      <FriendlyRecord
        value={receipt?.output ?? result.completion ?? null}
        empty="No completion has been recorded. Refresh to check status."
      />
    </View>
  );
}

function ToolParameters({ value }: { value: JsonValue }) {
  const fields = schemaFields(value);
  const required = schemaRequired(value);

  if (!fields.length) {
    return (
      <InfoRows
        rows={[
          { label: 'Input shape', value: formatFriendlyValue(value) },
          { label: 'Required fields', value: required.length ? required.join(', ') : 'None' },
        ]}
      />
    );
  }

  return (
    <View className="gap-3">
      <StatBadges
        items={[
          { label: 'Fields', value: fields.length },
          { label: 'Required', value: required.length },
        ]}
      />
      <View className="gap-2">
        {fields.slice(0, 6).map(([name, field]) => (
          <View key={name} className="border-border rounded-md border p-3">
            <View className="flex-row items-start justify-between gap-3">
              <View className="min-w-0 flex-1">
                <Text className="text-sm font-medium">{humanizeKey(name)}</Text>
                <Text className="text-muted-foreground text-xs" numberOfLines={2}>
                  {schemaDescription(field) || 'No description provided.'}
                </Text>
              </View>
              <StatusBadge status={required.includes(name) ? 'required' : schemaType(field)} />
            </View>
          </View>
        ))}
      </View>
      {fields.length > 6 ? (
        <Text className="text-muted-foreground text-xs">+{fields.length - 6} more fields</Text>
      ) : null}
    </View>
  );
}

function schemaObject(value: JsonValue): Record<string, JsonValue> {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, JsonValue>)
    : {};
}

function schemaFields(value: JsonValue): Array<[string, JsonValue]> {
  const schema = schemaObject(value);
  const properties = schemaObject(schema.properties);
  return Object.entries(properties);
}

function schemaRequired(value: JsonValue): string[] {
  const required = schemaObject(value).required;
  return Array.isArray(required)
    ? required.filter((item): item is string => typeof item === 'string')
    : [];
}

function schemaType(value: JsonValue): string {
  const type = schemaObject(value).type;
  return typeof type === 'string' ? type : 'field';
}

function schemaDescription(value: JsonValue): string | undefined {
  const description = schemaObject(value).description;
  return typeof description === 'string' ? description : undefined;
}
