import { ListPage } from '@/components/app/page';
import { EmptyState } from '@/components/data/empty-state';
import { InfoRows } from '@/components/data/friendly-data';
import { StatusBadge } from '@/components/data/status-badge';
import { Field } from '@/components/forms/field';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Icon } from '@/components/ui/icon';
import { Input } from '@/components/ui/input';
import { Switch } from '@/components/ui/switch';
import { Text } from '@/components/ui/text';
import { useApiAction, useMatchSkill, useSkills } from '@/lib/api/hooks';
import { useTranslation } from '@/lib/session/use-translation';
import { Brain, Search } from 'lucide-react-native';
import * as React from 'react';
import { View } from 'react-native';

export default function SkillsScreen() {
  const tr = useTranslation();
  const skills = useSkills();
  const match = useMatchSkill();
  const action = useApiAction();
  const [intent, setIntent] = React.useState('');

  async function toggleSkill(name: string, enabled: boolean) {
    await action.trigger({
      path: `/api/v1/skills/${encodeURIComponent(name)}`,
      method: 'PUT',
      body: { enabled },
    });
    await skills.mutate();
  }

  async function matchIntent() {
    if (!intent.trim()) return;
    await match.trigger({ intent });
  }

  return (
    <ListPage
      title={tr('skills.title')}
      description="Skills are intent-scoped capabilities the assistant can route into. They remain server-owned artifacts."
      actions={
        <>
          <Input
            value={intent}
            onChangeText={setIntent}
            placeholder="Summarize my recent notes"
            className="w-72"
          />
          <Button variant="outline" onPress={matchIntent} disabled={!intent || match.isMutating}>
            <Icon as={Search} className="size-4" />
            <Text>{tr('skills.match')}</Text>
          </Button>
        </>
      }
      data={skills.data ?? []}
      keyExtractor={(skill) => skill.id}
      empty={
        <EmptyState
          title="No skills found"
          description="Builtin or API-defined skills will appear here after the server loads them."
          icon={Brain}
        />
      }
      header={
        match.data ? (
          <IntentMatchCard
            matched={match.data.matched}
            skill={match.data.skill}
            confidence={match.data.confidence}
          />
        ) : null
      }
      footer={
        skills.error || action.error || match.error ? (
          <Text className="text-destructive text-sm">
            {(skills.error ?? action.error ?? match.error)?.message}
          </Text>
        ) : null
      }
      renderItem={({ item: skill }) => (
        <Card>
          <CardHeader>
            <View className="gap-2 sm:flex-row sm:items-start sm:justify-between">
              <View className="min-w-0 flex-1 gap-1">
                <CardTitle numberOfLines={1}>{skill.display_name}</CardTitle>
                <CardDescription numberOfLines={1}>
                  {skill.name} · {skill.category} · {skill.source}
                </CardDescription>
              </View>
              <StatusBadge status={skill.enabled ? 'enabled' : 'disabled'} />
            </View>
          </CardHeader>
          <CardContent className="gap-3">
            {skill.description ? (
              <Text className="text-muted-foreground text-sm leading-5">{skill.description}</Text>
            ) : null}
            <View className="flex-row flex-wrap gap-2">
              {skill.tools.map((tool) => (
                <StatusBadge key={tool} status={tool} />
              ))}
            </View>
            <View className="flex-row items-center justify-between">
              <View>
                <Text className="text-sm font-medium">Enabled</Text>
                <Text className="text-muted-foreground text-xs">Priority {skill.priority}</Text>
              </View>
              <Switch
                checked={skill.enabled}
                onCheckedChange={(checked) => toggleSkill(skill.name, checked)}
              />
            </View>
          </CardContent>
        </Card>
      )}
    />
  );
}

function IntentMatchCard({
  confidence,
  matched,
  skill,
}: {
  matched: boolean;
  skill?: string;
  confidence?: number;
}) {
  return (
    <Card>
      <CardHeader>
        <View className="gap-2 sm:flex-row sm:items-start sm:justify-between">
          <View className="gap-1">
            <CardTitle>Intent match</CardTitle>
            <CardDescription>
              {matched
                ? 'The assistant found the closest skill for this request.'
                : 'No confident skill match yet.'}
            </CardDescription>
          </View>
          <StatusBadge status={matched ? 'matched' : 'no match'} />
        </View>
      </CardHeader>
      <CardContent>
        <InfoRows
          rows={[
            { label: 'Skill', value: skill, hidden: !matched },
            {
              label: 'Confidence',
              value:
                typeof confidence === 'number' ? `${Math.round(confidence * 100)}%` : undefined,
            },
          ]}
        />
      </CardContent>
    </Card>
  );
}
