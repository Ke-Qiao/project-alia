import { Page } from '@/components/app/page';
import { StatBadges, humanizeKey } from '@/components/data/friendly-data';
import { StatusBadge } from '@/components/data/status-badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Progress } from '@/components/ui/progress';
import { Text } from '@/components/ui/text';
import { useApiAction, useRelationshipProfile } from '@/lib/api/hooks';
import { useTranslation } from '@/lib/session/use-translation';
import { View } from 'react-native';

export default function ExpressionScreen() {
  const tr = useTranslation();
  const relationship = useRelationshipProfile();
  const action = useApiAction();
  const profile = relationship.data;

  async function clear() {
    await action.trigger({ path: '/api/v1/expression/relationship', method: 'DELETE' });
    await relationship.mutate();
  }

  return (
    <Page
      title={tr('expression.title')}
      description="Relationship and affect-facing controls exposed by the expression runtime. This is about assistant behavior, not adapter coupling.">
      <View className="gap-4 lg:flex-row">
        <Card className="flex-1">
          <CardHeader>
            <View className="gap-2 sm:flex-row sm:items-start sm:justify-between">
              <View className="gap-1">
                <CardTitle>Relationship Profile</CardTitle>
                <CardDescription>{profile?.updated_at ?? 'Not updated yet'}</CardDescription>
              </View>
              <StatusBadge status={profile?.privacy.user_visible ? 'visible' : 'private'} />
            </View>
          </CardHeader>
          <CardContent className="gap-5">
            <View className="gap-2">
              <View className="flex-row justify-between">
                <Text className="font-medium">Trust</Text>
                <Text className="text-muted-foreground">
                  {Math.round((profile?.trust ?? 0) * 100)}%
                </Text>
              </View>
              <Progress value={(profile?.trust ?? 0) * 100} />
            </View>
            <View className="gap-2">
              <View className="flex-row justify-between">
                <Text className="font-medium">Familiarity</Text>
                <Text className="text-muted-foreground">
                  {Math.round((profile?.familiarity ?? 0) * 100)}%
                </Text>
              </View>
              <Progress value={(profile?.familiarity ?? 0) * 100} />
            </View>
            <Text className="text-muted-foreground leading-6">
              {profile?.summary || 'No relationship summary has been built yet.'}
            </Text>
            {profile?.privacy.clearable ? (
              <Button variant="outline" onPress={clear} disabled={action.isMutating}>
                <Text>{tr('expression.clear')}</Text>
              </Button>
            ) : null}
          </CardContent>
        </Card>

        <Card className="flex-1">
          <CardHeader>
            <CardTitle>Preferences</CardTitle>
            <CardDescription>
              {profile?.preference_summary || 'No preference summary yet'}
            </CardDescription>
          </CardHeader>
          <CardContent className="gap-3">
            {Object.entries(profile?.preferences ?? {}).map(([key, value]) => (
              <View key={key} className="border-border rounded-md border p-3">
                <Text className="font-medium">{key}</Text>
                <Text className="text-muted-foreground text-sm">{value}</Text>
              </View>
            ))}
            {!Object.keys(profile?.preferences ?? {}).length ? (
              <Text className="text-muted-foreground text-sm">
                No stored preferences are visible.
              </Text>
            ) : null}
          </CardContent>
        </Card>
      </View>

      <Card>
        <CardHeader>
          <CardTitle>Signals</CardTitle>
          <CardDescription>
            How much relationship evidence Alia has seen by category.
          </CardDescription>
        </CardHeader>
        <CardContent className="gap-3">
          <StatBadges
            items={Object.entries(profile?.signal_counts ?? {}).map(([label, value]) => ({
              label: humanizeKey(label),
              value,
            }))}
          />
          {!Object.keys(profile?.signal_counts ?? {}).length ? (
            <Text className="text-muted-foreground text-sm">
              No signal evidence has been recorded yet.
            </Text>
          ) : null}
        </CardContent>
      </Card>
      {relationship.error || action.error ? (
        <Text className="text-destructive text-sm">
          {(relationship.error ?? action.error)?.message}
        </Text>
      ) : null}
    </Page>
  );
}
