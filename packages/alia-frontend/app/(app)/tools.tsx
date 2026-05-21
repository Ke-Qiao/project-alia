import { ListPage } from '@/components/app/page';
import { EmptyState } from '@/components/data/empty-state';
import { JsonPreview } from '@/components/data/json-preview';
import { StatusBadge } from '@/components/data/status-badge';
import { Field } from '@/components/forms/field';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Text } from '@/components/ui/text';
import { Textarea } from '@/components/ui/textarea';
import { useExecuteTool, useTools } from '@/lib/api/hooks';
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
      empty={<EmptyState title="No tools found" description="Tool providers register executable functions during server startup." icon={Wrench} />}
      header={
      <Card>
        <CardHeader>
          <CardTitle>{tr('tools.execute')}</CardTitle>
          <CardDescription>Use with care. Tools may touch external systems depending on server configuration.</CardDescription>
        </CardHeader>
        <CardContent className="gap-4">
          <Field label="Tool name">
            <Input value={selected} onChangeText={setSelected} placeholder="tool.name" />
          </Field>
          <Field label={tr('tools.arguments')}>
            <Textarea value={argumentsJson} onChangeText={setArgumentsJson} className="min-h-32 font-mono" />
          </Field>
          <Button onPress={runTool} disabled={!selected || execute.isMutating}>
            <Text>{execute.isMutating ? 'Executing...' : tr('tools.execute')}</Text>
          </Button>
          {execute.data ? <JsonPreview value={execute.data} /> : null}
          {execute.error ? <Text className="text-destructive text-sm">{execute.error.message}</Text> : null}
        </CardContent>
      </Card>
      }
      footer={tools.error ? <Text className="text-destructive text-sm">{tools.error.message}</Text> : null}
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
            <JsonPreview value={tool.parameters} />
          </CardContent>
        </Card>
      )}
    />
  );
}
