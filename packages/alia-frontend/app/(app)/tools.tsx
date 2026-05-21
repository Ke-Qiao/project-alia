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
import { useExecuteTool, useTools } from '@/lib/api/hooks';
import type { JsonValue, ToolResult } from '@/lib/api/types';
import { useTranslation } from '@/lib/session/use-translation';
import { Wrench } from 'lucide-react-native';
import * as React from 'react';
import { View } from 'react-native';

export default function ToolsScreen() {
  const tr = useTranslation();
  const tools = useTools();
  const execute = useExecuteTool();
  const [selected, setSelected] = React.useState('');
  const [argumentsJson, setArgumentsJson] = React.useState('{}');

  React.useEffect(() => {
    if (!selected && tools.data?.[0]) setSelected(tools.data[0].name);
  }, [selected, tools.data]);

  async function runTool() {
    await execute.trigger({
      tool_name: selected,
      arguments: JSON.parse(argumentsJson || '{}'),
    });
  }

  return (
    <ListPage
      title={tr('tools.title')}
      description="Tool definitions are exposed by the server registry. Manual execution uses the same HTTP contract as other clients."
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
              Use with care. Tools may touch external systems depending on server configuration.
            </CardDescription>
          </CardHeader>
          <CardContent className="gap-4">
            <Field label="Tool name">
              <Input value={selected} onChangeText={setSelected} placeholder="tool.name" />
            </Field>
            <Field label={tr('tools.arguments')}>
              <Textarea
                value={argumentsJson}
                onChangeText={setArgumentsJson}
                className="min-h-32 font-mono"
              />
            </Field>
            <Button onPress={runTool} disabled={!selected || execute.isMutating}>
              <Text>{execute.isMutating ? 'Executing...' : tr('tools.execute')}</Text>
            </Button>
            {execute.data ? <ToolResultCard result={execute.data} /> : null}
            {execute.error ? (
              <Text className="text-destructive text-sm">{execute.error.message}</Text>
            ) : null}
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

function ToolResultCard({ result }: { result: ToolResult }) {
  return (
    <View className="border-border bg-muted/40 gap-3 rounded-md border p-3">
      <View className="flex-row items-start justify-between gap-3">
        <View className="min-w-0 flex-1 gap-1">
          <Text className="font-medium">Execution result</Text>
          <Text className="text-muted-foreground text-xs" numberOfLines={1}>
            Call {result.call_id}
          </Text>
        </View>
        <StatusBadge status={result.success ? 'success' : 'failed'} />
      </View>
      {result.display ? <Text className="text-sm leading-5">{result.display}</Text> : null}
      <FriendlyRecord value={result.output} empty="The tool did not return displayable output." />
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
