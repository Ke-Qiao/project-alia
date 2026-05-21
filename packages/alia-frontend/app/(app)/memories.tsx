import { ListPage } from '@/components/app/page';
import { EmptyState } from '@/components/data/empty-state';
import { InfoRows, StatBadges } from '@/components/data/friendly-data';
import { StatusBadge } from '@/components/data/status-badge';
import { Field } from '@/components/forms/field';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import { Text } from '@/components/ui/text';
import { Textarea } from '@/components/ui/textarea';
import {
  useApiAction,
  useConsolidateMemories,
  useCreateMemory,
  useMemories,
  useMemoryCapabilities,
  useMemoryPrivacy,
  useSearchMemories,
  useUpdateMemoryPrivacy,
} from '@/lib/api/hooks';
import type { MemoryCapabilities, MemoryConsolidationResult } from '@/lib/api/types';
import { useTranslation } from '@/lib/session/use-translation';
import { Database } from 'lucide-react-native';
import * as React from 'react';
import { View } from 'react-native';

export default function MemoriesScreen() {
  const tr = useTranslation();
  const [layer, setLayer] = React.useState('');
  const [content, setContent] = React.useState('');
  const [query, setQuery] = React.useState('');
  const memories = useMemories(layer || undefined);
  const privacy = useMemoryPrivacy();
  const capabilities = useMemoryCapabilities();
  const create = useCreateMemory();
  const search = useSearchMemories();
  const updatePrivacy = useUpdateMemoryPrivacy();
  const consolidate = useConsolidateMemories();
  const action = useApiAction();
  const rows = search.data ?? memories.data ?? [];

  async function addMemory() {
    await create.trigger({ content, memory_layer: layer || 'episodic', importance: 0.5 });
    setContent('');
    await memories.mutate();
  }

  async function searchMemory() {
    if (!query.trim()) return;
    await search.trigger({ query, max_results: 20, layers: layer ? [layer] : undefined });
  }

  async function deleteMemory(id: string) {
    await action.trigger({ path: `/api/v1/memories/${encodeURIComponent(id)}`, method: 'DELETE' });
    await memories.mutate();
  }

  return (
    <ListPage
      title={tr('memories.title')}
      description="User-visible memory controls, retrieval, and maintenance. Memory remains a server capability behind HTTP APIs."
      actions={
        <Input
          value={layer}
          onChangeText={setLayer}
          placeholder="episodic / semantic / affective"
          className="w-72"
        />
      }
      data={rows}
      keyExtractor={(memory) => memory.id}
      empty={
        <EmptyState
          title="No memories visible"
          description="Directly added or captured memories will appear here."
          icon={Database}
        />
      }
      header={
        <>
          <View className="gap-4 lg:flex-row">
            <Card className="flex-1">
              <CardHeader>
                <CardTitle>{tr('memories.privacy')}</CardTitle>
                <CardDescription>{privacy.data?.capture_mode ?? 'unknown'}</CardDescription>
              </CardHeader>
              <CardContent className="gap-4">
                <View className="flex-row items-center justify-between">
                  <View>
                    <Text className="text-sm font-medium">Memory enabled</Text>
                    <Text className="text-muted-foreground text-xs">
                      Controls all memory capture and retrieval.
                    </Text>
                  </View>
                  <Switch
                    checked={privacy.data?.enabled ?? false}
                    onCheckedChange={(enabled) =>
                      updatePrivacy.trigger({ enabled }).then(() => privacy.mutate())
                    }
                  />
                </View>
                <View className="flex-row items-center justify-between">
                  <View>
                    <Text className="text-sm font-medium">Automatic capture</Text>
                    <Text className="text-muted-foreground text-xs">
                      Maps to LLM-selected capture mode.
                    </Text>
                  </View>
                  <Switch
                    checked={privacy.data?.automatic_capture_enabled ?? false}
                    onCheckedChange={(automatic_capture_enabled) =>
                      updatePrivacy
                        .trigger({ automatic_capture_enabled })
                        .then(() => privacy.mutate())
                    }
                  />
                </View>
                <MemoryCapabilitySummary capabilities={capabilities.data} />
              </CardContent>
            </Card>

            <Card className="flex-1">
              <CardHeader>
                <CardTitle>{tr('memories.add')}</CardTitle>
              </CardHeader>
              <CardContent className="gap-3">
                <Field label={tr('memories.content')}>
                  <Textarea value={content} onChangeText={setContent} className="min-h-32" />
                </Field>
                <Button onPress={addMemory} disabled={!content.trim() || create.isMutating}>
                  <Text>{tr('memories.add')}</Text>
                </Button>
              </CardContent>
            </Card>
          </View>

          <Card>
            <CardHeader>
              <CardTitle>{tr('common.search')}</CardTitle>
            </CardHeader>
            <CardContent className="gap-3">
              <View className="gap-2 sm:flex-row">
                <Input
                  value={query}
                  onChangeText={setQuery}
                  placeholder="What should Alia remember about me?"
                  className="flex-1"
                />
                <Button
                  variant="outline"
                  onPress={searchMemory}
                  disabled={!query.trim() || search.isMutating}>
                  <Text>{tr('common.search')}</Text>
                </Button>
                <Button
                  variant="outline"
                  onPress={() => consolidate.trigger(undefined)}
                  disabled={consolidate.isMutating}>
                  <Text>{tr('memories.consolidate')}</Text>
                </Button>
              </View>
              {consolidate.data ? <ConsolidationSummary result={consolidate.data} /> : null}
            </CardContent>
          </Card>
        </>
      }
      footer={
        privacy.error ||
        memories.error ||
        create.error ||
        search.error ||
        updatePrivacy.error ||
        action.error ? (
          <Text className="text-destructive text-sm">
            {
              (
                privacy.error ??
                memories.error ??
                create.error ??
                search.error ??
                updatePrivacy.error ??
                action.error
              )?.message
            }
          </Text>
        ) : null
      }
      renderItem={({ item: memory }) => (
        <Card>
          <CardHeader>
            <View className="gap-2 sm:flex-row sm:items-start sm:justify-between">
              <View className="min-w-0 flex-1 gap-1">
                <CardTitle className="text-base">{memory.kind}</CardTitle>
                <CardDescription>
                  {memory.memory_layer} · {memory.node_type} · {memory.scope}
                </CardDescription>
              </View>
              <View className="flex-row gap-2">
                <StatusBadge status={memory.status} />
                {memory.sensitive ? <StatusBadge status="sensitive" /> : null}
              </View>
            </View>
          </CardHeader>
          <CardContent className="gap-3">
            <Text className="leading-6">{memory.content}</Text>
            {memory.evidence_summary ? (
              <Text className="text-muted-foreground text-sm">{memory.evidence_summary}</Text>
            ) : null}
            <View className="flex-row flex-wrap gap-2">
              {memory.topic_tags.map((tag) => (
                <StatusBadge key={tag} status={tag} />
              ))}
              {memory.affect_tags.map((tag) => (
                <StatusBadge key={tag} status={tag} />
              ))}
            </View>
            <View className="flex-row justify-between gap-3">
              <Text className="text-muted-foreground text-xs">
                importance {memory.importance.toFixed(2)} · confidence{' '}
                {memory.confidence.toFixed(2)}
              </Text>
              <Button variant="ghost" size="sm" onPress={() => deleteMemory(memory.id)}>
                <Text>{tr('common.delete')}</Text>
              </Button>
            </View>
          </CardContent>
        </Card>
      )}
    />
  );
}

function MemoryCapabilitySummary({
  capabilities,
}: {
  capabilities: MemoryCapabilities | undefined;
}) {
  if (!capabilities) {
    return (
      <Text className="text-muted-foreground text-sm">Memory capabilities are still loading.</Text>
    );
  }

  return (
    <View className="gap-3">
      <InfoRows
        rows={[
          { label: 'Memory backend', value: capabilities.system },
          { label: 'ABI version', value: capabilities.abi_version },
          { label: 'Write mode', value: capabilities.async_writes ? 'Async' : 'Sync' },
          {
            label: 'Max content',
            value: capabilities.max_content_chars
              ? `${capabilities.max_content_chars} chars`
              : undefined,
          },
        ]}
      />
      <StatBadges
        items={[
          { label: 'Store', value: capabilities.supports_store ? 'Yes' : 'No' },
          { label: 'Search', value: capabilities.supports_search_filters ? 'Yes' : 'No' },
          { label: 'Graph', value: capabilities.supports_graph ? 'Yes' : 'No' },
          { label: 'Metadata', value: capabilities.supports_metadata ? 'Yes' : 'No' },
        ]}
      />
    </View>
  );
}

function ConsolidationSummary({ result }: { result: MemoryConsolidationResult }) {
  return (
    <View className="border-border bg-muted/40 gap-3 rounded-md border p-3">
      <Text className="font-medium">Consolidation complete</Text>
      <StatBadges
        items={[
          { label: 'Scanned', value: result.scanned },
          { label: 'Deduplicated', value: result.deduplicated },
          { label: 'Promoted', value: result.promoted },
          { label: 'Demoted', value: result.demoted },
          { label: 'Superseded', value: result.superseded },
          { label: 'Contradictions', value: result.contradictions },
        ]}
      />
    </View>
  );
}
