import { Page } from '@/components/app/page';
import { PendingEmbeddedCard } from '@/components/app/pending-embedded-card';
import { StatusBadge } from '@/components/data/status-badge';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Icon } from '@/components/ui/icon';
import { Progress } from '@/components/ui/progress';
import { Text } from '@/components/ui/text';
import { useChannels, useHealth, useMemoryPrivacy, useRelationshipProfile } from '@/lib/api/hooks';
import { useSession } from '@/lib/session/provider';
import { useTranslation } from '@/lib/session/use-translation';
import { Link, type Href } from 'expo-router';
import {
  Cable,
  ChevronRight,
  Database,
  HeartPulse,
  Server,
  Settings,
  Sparkles,
  type LucideIcon,
} from 'lucide-react-native';
import { View } from 'react-native';

export default function RuntimeScreen() {
  const tr = useTranslation();
  const session = useSession();
  const health = useHealth();
  const channels = useChannels();
  const memory = useMemoryPrivacy();
  const relationship = useRelationshipProfile();
  const modules = Object.entries(health.data?.modules ?? {});
  const activeConnectors = channels.data?.active ?? [];
  const disabledConnectors = channels.data?.disabled ?? [];
  const displayName = session.user?.name?.trim() || session.user?.email?.split('@')[0] || 'there';
  const serverReady = isReadyStatus(health.data?.status);
  const memoryReady = memory.data?.enabled ?? false;
  const relationshipScore = Math.round((relationship.data?.familiarity ?? 0) * 100);

  return (
    <Page title={tr('runtime.title')} description={tr('runtime.description')}>
      <PendingEmbeddedCard />

      <Card className="border-primary/20 bg-primary/5">
        <CardContent className="gap-5 p-5 md:p-6">
          <View className="gap-3 md:flex-row md:items-start md:justify-between">
            <View className="min-w-0 flex-1 gap-2">
              <Badge variant={serverReady ? 'default' : 'secondary'} className="self-start">
                <Text>
                  {serverReady ? 'Online' : health.isLoading ? 'Checking' : 'Needs attention'}
                </Text>
              </Badge>
              <Text className="text-3xl font-semibold tracking-normal">
                Good to see you, {displayName}
              </Text>
              <Text className="text-muted-foreground max-w-3xl text-base leading-6">
                Your assistant is organized around memory, connected places, and local devices. The
                technical controls are still here, just tucked behind calmer paths.
              </Text>
            </View>
            <View className="bg-background/70 rounded-lg p-3">
              <Icon as={Sparkles} className="text-primary size-7" />
            </View>
          </View>

          <View className="gap-3 lg:flex-row">
            <QuickAction
              href="/memories"
              icon={Database}
              title="Remember something"
              description="Add context your assistant should know"
            />
            <QuickAction
              href="/connectors"
              icon={Sparkles}
              title="Open workspace"
              description="Live chat clients and device bridges"
            />
            <QuickAction
              href="/settings"
              icon={Settings}
              title="Tune assistant"
              description="Theme, server, memory, and advanced tools"
            />
          </View>
        </CardContent>
      </Card>

      <View className="gap-3 md:flex-row">
        <AssistantSummaryCard
          title="Server"
          value={serverReady ? 'Ready' : health.isLoading ? 'Checking' : 'Attention needed'}
          detail={
            health.data?.version ? `Version ${health.data.version}` : 'Personal server connection'
          }
          icon={Server}
        />
        <AssistantSummaryCard
          title="Connections"
          value={activeConnectors.length ? `${activeConnectors.length} active` : 'Not connected'}
          detail={
            disabledConnectors.length
              ? `${disabledConnectors.length} need setup`
              : 'Platforms and clients'
          }
          icon={Cable}
        />
        <AssistantSummaryCard
          title="Memory"
          value={memoryReady ? 'On' : 'Paused'}
          detail={
            memory.data?.capture_mode ? `Capture: ${memory.data.capture_mode}` : 'Personal context'
          }
          icon={Database}
        />
        <AssistantSummaryCard
          title="Assistant style"
          value={relationshipScore ? `${relationshipScore}% learned` : 'Learning'}
          detail="Preferences and familiarity"
          icon={HeartPulse}
        />
      </View>

      <View className="gap-4 lg:flex-row">
        <Card className="flex-1">
          <CardHeader>
            <CardTitle>Things your assistant can remember</CardTitle>
            <CardDescription>Keep personal context useful and under your control.</CardDescription>
          </CardHeader>
          <CardContent className="gap-4">
            <View className="flex-row items-center justify-between gap-3">
              <View className="min-w-0 flex-1 gap-1">
                <Text className="text-sm font-medium">Memory capture</Text>
                <Text className="text-muted-foreground text-sm" numberOfLines={2}>
                  {memoryReady
                    ? 'Your assistant can use saved context when helping you.'
                    : 'Memory is currently paused or unavailable.'}
                </Text>
              </View>
              <StatusBadge status={memoryReady ? 'enabled' : 'disabled'} />
            </View>
            <Link href={'/memories' as Href} asChild>
              <Button variant="outline">
                <Icon as={Database} className="size-4" />
                <Text>Manage memories</Text>
              </Button>
            </Link>
          </CardContent>
        </Card>

        <Card className="flex-1">
          <CardHeader>
            <CardTitle>Connected places</CardTitle>
            <CardDescription>
              Channels are platform adapters for chat clients and future surfaces.
            </CardDescription>
          </CardHeader>
          <CardContent className="gap-3">
            {activeConnectors.slice(0, 4).map((channel) => (
              <View key={channel.id} className="flex-row items-center justify-between gap-3">
                <View className="min-w-0 flex-1">
                  <Text className="text-sm font-medium" numberOfLines={1}>
                    {channel.id}
                  </Text>
                  <Text className="text-muted-foreground text-xs" numberOfLines={1}>
                    {channel.channel_type}
                  </Text>
                </View>
                <StatusBadge status={channel.health} />
              </View>
            ))}
            {!activeConnectors.length ? (
              <Text className="text-muted-foreground text-sm leading-5">
                No chat platforms or clients are connected yet. Add adapters on the server when you
                are ready.
              </Text>
            ) : null}
            <Link href={'/connectors' as Href} asChild>
              <Button variant="outline">
                <Icon as={Cable} className="size-4" />
                <Text>Open connections</Text>
              </Button>
            </Link>
          </CardContent>
        </Card>
      </View>

      <View className="gap-4 lg:flex-row">
        <Card className="flex-1">
          <CardHeader>
            <CardTitle>Assistant style</CardTitle>
            <CardDescription>
              Your assistant gradually learns how to communicate and what matters to you.
            </CardDescription>
          </CardHeader>
          <CardContent className="gap-4">
            <View className="gap-2">
              <View className="flex-row justify-between">
                <Text className="text-sm font-medium">Familiarity</Text>
                <Text className="text-muted-foreground text-sm">
                  {Math.round((relationship.data?.familiarity ?? 0) * 100)}%
                </Text>
              </View>
              <Progress value={(relationship.data?.familiarity ?? 0) * 100} />
            </View>
            <View className="gap-2">
              <View className="flex-row justify-between">
                <Text className="text-sm font-medium">Trust</Text>
                <Text className="text-muted-foreground text-sm">
                  {Math.round((relationship.data?.trust ?? 0) * 100)}%
                </Text>
              </View>
              <Progress value={(relationship.data?.trust ?? 0) * 100} />
            </View>
            <Text className="text-muted-foreground text-sm leading-5">
              {relationship.data?.summary ||
                relationship.data?.preference_summary ||
                'Your assistant is still building a useful understanding of your preferences.'}
            </Text>
          </CardContent>
        </Card>

        <Card className="flex-1">
          <CardHeader>
            <CardTitle>System check</CardTitle>
            <CardDescription>
              Background services stay visible without making the home screen feel like a console.
            </CardDescription>
          </CardHeader>
          <CardContent className="gap-3">
            {modules.slice(0, 8).map(([name, module]) => (
              <View key={name} className="flex-row items-center justify-between gap-3">
                <View className="min-w-0 flex-1">
                  <Text className="text-sm font-medium" numberOfLines={1}>
                    {name}
                  </Text>
                  {module.message ? (
                    <Text className="text-muted-foreground text-xs" numberOfLines={1}>
                      {module.message}
                    </Text>
                  ) : null}
                </View>
                <StatusBadge status={module.status} />
              </View>
            ))}
            {!modules.length ? (
              <Text className="text-muted-foreground text-sm">
                No module health has been reported yet.
              </Text>
            ) : null}
            <Link href={'/settings' as Href} asChild>
              <Button variant="ghost" className="justify-start px-0">
                <Icon as={Settings} className="size-4" />
                <Text>Advanced assistant settings</Text>
              </Button>
            </Link>
          </CardContent>
        </Card>
      </View>
    </Page>
  );
}

function AssistantSummaryCard({
  detail,
  icon,
  title,
  value,
}: {
  detail: string;
  icon: LucideIcon;
  title: string;
  value: string;
}) {
  return (
    <Card className="flex-1">
      <CardContent className="gap-3 p-4">
        <View className="bg-secondary size-10 items-center justify-center rounded-lg">
          <Icon as={icon} className="size-5" />
        </View>
        <View className="gap-1">
          <Text className="text-muted-foreground text-xs font-medium uppercase">{title}</Text>
          <Text className="text-xl font-semibold">{value}</Text>
          <Text className="text-muted-foreground text-sm" numberOfLines={2}>
            {detail}
          </Text>
        </View>
      </CardContent>
    </Card>
  );
}

function QuickAction({
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
      <Button
        variant="outline"
        className="bg-background/70 h-auto min-h-20 flex-1 items-start justify-between p-3">
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

function isReadyStatus(status: string | undefined) {
  if (!status) return false;
  const normalized = status.toLowerCase();
  return (
    normalized === 'ok' ||
    normalized === 'healthy' ||
    normalized === 'ready' ||
    normalized === 'running'
  );
}
