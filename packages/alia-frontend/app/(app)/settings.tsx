import { Page } from '@/components/app/page';
import { StatusBadge } from '@/components/data/status-badge';
import {
  groupSettingsSections,
  settingsSectionHref,
  type SettingsSection,
} from '@/components/settings/settings-model';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import { Icon } from '@/components/ui/icon';
import { Text } from '@/components/ui/text';
import { useTranslation } from '@/lib/session/use-translation';
import { Link } from 'expo-router';
import { ChevronRight } from 'lucide-react-native';
import { View } from 'react-native';

export default function SettingsScreen() {
  const tr = useTranslation();
  const groups = groupSettingsSections();

  return (
    <Page title={tr('settings.title')} description={tr('settings.description')}>
      <Card className="border-primary/20 bg-primary/5">
        <CardHeader>
          <CardTitle>Settings are organized by task</CardTitle>
          <CardDescription>
            Choose one area first. Detailed controls and server configuration live one level deeper
            so this page stays calm on mobile.
          </CardDescription>
        </CardHeader>
      </Card>

      {groups.map((group) => (
        <View key={group.title} className="gap-3">
          <Text className="text-muted-foreground text-xs font-medium uppercase">{group.title}</Text>
          <View className="gap-3">
            {group.sections.map((section) => (
              <SettingsSectionLink key={section.id} section={section} />
            ))}
          </View>
        </View>
      ))}
    </Page>
  );
}

function SettingsSectionLink({ section }: { section: SettingsSection }) {
  return (
    <Link href={settingsSectionHref(section.id)} asChild>
      <Button variant="outline" className="h-auto min-h-20 justify-between p-4">
        <View className="min-w-0 flex-1 flex-row items-start gap-3">
          <View className="bg-secondary rounded-lg p-2">
            <Icon as={section.icon} className="size-5" />
          </View>
          <View className="min-w-0 flex-1 gap-1">
            <View className="flex-row flex-wrap items-center gap-2">
              <Text className="font-medium">{section.title}</Text>
              {section.status ? <StatusBadge status={section.status} /> : null}
            </View>
            <Text className="text-muted-foreground text-sm leading-5" numberOfLines={2}>
              {section.description}
            </Text>
          </View>
        </View>
        <Icon as={ChevronRight} className="text-muted-foreground size-4" />
      </Button>
    </Link>
  );
}
