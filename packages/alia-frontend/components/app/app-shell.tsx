import { StatusBadge } from '@/components/data/status-badge';
import { ThemeToggle } from '@/components/app/theme-toggle';
import { Button } from '@/components/ui/button';
import { Icon } from '@/components/ui/icon';
import { Separator } from '@/components/ui/separator';
import { Text } from '@/components/ui/text';
import { useHealth } from '@/lib/api/hooks';
import { useSession } from '@/lib/session/provider';
import { useTranslation } from '@/lib/session/use-translation';
import { cn } from '@/lib/utils';
import { Link, Redirect, router, usePathname, type Href } from 'expo-router';
import {
  Brain,
  Cable,
  ChevronLeft,
  Database,
  HeartPulse,
  Home,
  LogOut,
  Radar,
  Settings,
  SlidersHorizontal,
  Sparkles,
  Wrench,
  type LucideIcon,
} from 'lucide-react-native';
import * as React from 'react';
import { Animated, Easing, Pressable, StyleSheet, useWindowDimensions, View } from 'react-native';
import { SafeAreaView, useSafeAreaInsets } from 'react-native-safe-area-context';

const mainNavItems = [
  { href: '/runtime', label: 'nav.runtime', icon: Home },
  { href: '/memories', label: 'nav.memories', icon: Database },
  { href: '/connectors', label: 'nav.workspace', icon: Sparkles },
  { href: '/settings', label: 'nav.settings', icon: Settings },
] as const;

const routeMeta = {
  '/runtime': { label: 'nav.runtime' },
  '/memories': { label: 'nav.memories' },
  '/connectors': { label: 'nav.workspace' },
  '/settings': { label: 'nav.settings' },
  '/skills': { label: 'nav.skills', parent: '/settings' },
  '/tools': { label: 'nav.tools', parent: '/settings' },
  '/expression': { label: 'nav.expression', parent: '/settings' },
  '/signals': { label: 'nav.sensors', parent: '/settings' },
} as const;

type AppHref = keyof typeof routeMeta;
type RouteMeta = { label: string; parent?: Href };

const sidebarSections = [
  {
    label: 'navGroup.overview',
    items: [{ href: '/runtime', label: 'nav.runtime', icon: Home }],
  },
  {
    label: 'navGroup.personal',
    items: [{ href: '/memories', label: 'nav.memories', icon: Database }],
  },
  {
    label: 'navGroup.workspace',
    items: [{ href: '/connectors', label: 'nav.workspace', icon: Sparkles }],
  },
  {
    label: 'navGroup.settings',
    items: [
      { href: '/settings', label: 'nav.settings', icon: Settings },
      { href: '/skills', label: 'nav.skills', icon: Brain },
      { href: '/tools', label: 'nav.tools', icon: Wrench },
      { href: '/expression', label: 'nav.expression', icon: HeartPulse },
      { href: '/signals', label: 'nav.sensors', icon: Radar },
    ],
  },
] as const;

export function AppShell({ children }: { children: React.ReactNode }) {
  const session = useSession();
  const { width } = useWindowDimensions();
  const desktop = width >= 900;
  const health = useHealth();
  const tr = useTranslation();
  const pathname = usePathname();
  const currentRoute = getRouteMeta(pathname);
  const parentHref = currentRoute?.parent ?? null;
  const showBack = Boolean(parentHref);
  const online =
    health.data?.status &&
    !['offline', 'error', 'unhealthy'].includes(health.data.status.toLowerCase());

  if (!session.hydrated) return null;
  if (session.effectiveBaseUrl === null) return <Redirect href={'/connect' as Href} />;
  if (!session.accessToken) return <Redirect href={'/login' as Href} />;

  return (
    <SafeAreaView edges={['top', 'left', 'right']} className="bg-background" style={styles.shell}>
      {desktop ? <Sidebar /> : null}
      <View className="flex-1">
        <View className="border-border bg-background/95 h-16 flex-row items-center justify-between border-b px-4 md:px-6">
          <View className="min-w-0 flex-1 flex-row items-center gap-2">
            <BackSlot visible={showBack} fallback={(parentHref ?? '/settings') as Href} />
            <View className="min-w-0 flex-1 gap-0.5">
              <View className="flex-row items-center gap-2">
                {!desktop && !showBack ? (
                  <Icon as={Sparkles} className="text-primary size-4" />
                ) : null}
                <Text className="font-semibold">
                  {showBack && currentRoute ? tr(currentRoute.label) : tr('app.name')}
                </Text>
              </View>
              <Text className="text-muted-foreground text-xs" numberOfLines={1}>
                {showBack
                  ? tr('nav.backToSettings')
                  : online
                    ? tr('app.status.ready')
                    : health.isLoading
                      ? tr('app.status.checking')
                      : tr('app.status.attention')}
              </Text>
            </View>
          </View>
          <View className="flex-row items-center gap-2">
            {desktop ? (
              <StatusBadge
                status={health.data?.status ?? (health.isLoading ? 'checking' : 'offline')}
              />
            ) : null}
            {desktop ? <ThemeToggle /> : null}
            {desktop ? (
              <Button
                variant="ghost"
                size="icon"
                onPress={() => session.clearAuth()}
                className="rounded-full">
                <Icon as={LogOut} className="size-5" />
              </Button>
            ) : null}
          </View>
        </View>
        <View className="flex-1">{children}</View>
        {!desktop ? <BottomNav /> : null}
      </View>
    </SafeAreaView>
  );
}

function Sidebar() {
  const tr = useTranslation();
  const session = useSession();

  return (
    <View className="border-border bg-sidebar w-64 border-r p-4">
      <View className="mb-5 flex-row items-center gap-3">
        <View className="bg-primary/10 size-10 items-center justify-center rounded-lg">
          <Icon as={Sparkles} className="text-primary size-5" />
        </View>
        <View className="min-w-0 flex-1">
          <Text className="font-semibold">{tr('app.name')}</Text>
          <Text className="text-muted-foreground text-xs">{tr('app.subtitle')}</Text>
        </View>
      </View>
      <View className="gap-5">
        {sidebarSections.map((section) => (
          <View key={section.label} className="gap-1">
            <Text className="text-muted-foreground px-3 text-xs font-medium uppercase">
              {tr(section.label)}
            </Text>
            {section.items.map((item) => (
              <NavLink key={item.href} href={item.href} label={tr(item.label)} icon={item.icon} />
            ))}
          </View>
        ))}
      </View>
      <View className="mt-auto gap-3">
        <Separator />
        <Button
          variant="outline"
          onPress={() => session.resetConnection()}
          className="justify-start">
          <Icon as={SlidersHorizontal} className="size-4" />
          <Text>Server</Text>
        </Button>
      </View>
    </View>
  );
}

function NavLink({ href, label, icon }: { href: string; label: string; icon: LucideIcon }) {
  const pathname = usePathname();
  const active = pathname === href || (href === '/settings' && pathname.startsWith('/settings/'));

  return (
    <Link href={href as Href} replace asChild>
      <Pressable
        className={cn(
          'h-10 flex-row items-center gap-3 rounded-md px-3',
          active ? 'bg-sidebar-accent' : 'web:hover:bg-sidebar-accent/70'
        )}>
        <Icon
          as={icon}
          className={cn('size-4', active ? 'text-foreground' : 'text-muted-foreground')}
        />
        <Text className={cn('text-sm', active ? 'font-medium' : 'text-muted-foreground')}>
          {label}
        </Text>
      </Pressable>
    </Link>
  );
}

function BottomNav() {
  const tr = useTranslation();
  const insets = useSafeAreaInsets();

  return (
    <View
      className="border-border bg-background absolute inset-x-0 bottom-0 border-t"
      style={[styles.bottomNav, { height: 64 + insets.bottom, paddingBottom: insets.bottom }]}>
      <View className="flex-1 flex-row items-start justify-around px-2 pt-2">
        {mainNavItems.map((item) => (
          <BottomNavLink key={item.href} href={item.href} label={tr(item.label)} icon={item.icon} />
        ))}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  shell: {
    flex: 1,
    flexDirection: 'row',
  },
  bottomNav: {
    minHeight: 64,
  },
});

function BottomNavLink({ href, label, icon }: { href: string; label: string; icon: LucideIcon }) {
  const pathname = usePathname();
  const currentRoute = getRouteMeta(pathname);
  const parentHref = currentRoute?.parent ?? null;
  const active = pathname === href || parentHref === href;
  return (
    <Link href={href as Href} replace asChild>
      <Pressable className="min-w-16 flex-1 items-center gap-1 rounded-md px-2 py-2">
        <Icon
          as={icon}
          className={cn('size-5', active ? 'text-foreground' : 'text-muted-foreground')}
        />
        <Text
          className={cn('text-[11px]', active ? 'font-medium' : 'text-muted-foreground')}
          numberOfLines={1}>
          {label}
        </Text>
      </Pressable>
    </Link>
  );
}

function getRouteMeta(pathname: string): RouteMeta | undefined {
  if (pathname.startsWith('/settings/')) {
    return { label: 'nav.settings', parent: '/settings' as Href };
  }
  return routeMeta[pathname as AppHref];
}

function BackSlot({ fallback, visible }: { fallback: Href; visible: boolean }) {
  const progress = React.useRef(new Animated.Value(visible ? 1 : 0)).current;

  React.useEffect(() => {
    Animated.timing(progress, {
      duration: 180,
      easing: Easing.out(Easing.cubic),
      toValue: visible ? 1 : 0,
      useNativeDriver: false,
    }).start();
  }, [progress, visible]);

  const width = progress.interpolate({ inputRange: [0, 1], outputRange: [0, 40] });
  const opacity = progress;
  const translateX = progress.interpolate({ inputRange: [0, 1], outputRange: [-8, 0] });

  return (
    <Animated.View
      pointerEvents={visible ? 'auto' : 'none'}
      style={{ opacity, transform: [{ translateX }], width }}>
      <BackButton fallback={fallback} />
    </Animated.View>
  );
}

function BackButton({ fallback }: { fallback: Href }) {
  function goBack() {
    if (router.canGoBack()) {
      router.back();
      return;
    }
    router.replace(fallback);
  }

  return (
    <Button
      accessibilityLabel="Back"
      variant="ghost"
      size="icon"
      onPress={goBack}
      className="-ml-2 rounded-full">
      <Icon as={ChevronLeft} className="size-5" />
    </Button>
  );
}
