import { Button } from '@/components/ui/button';
import { Icon } from '@/components/ui/icon';
import { MoonStarIcon, SunIcon } from 'lucide-react-native';
import { Uniwind, useUniwind } from 'uniwind';

const THEME_ICONS = {
  light: SunIcon,
  dark: MoonStarIcon,
};

export function ThemeToggle() {
  const { theme } = useUniwind();
  const nextTheme = theme === 'dark' ? 'light' : 'dark';

  return (
    <Button
      accessibilityLabel="Toggle theme"
      size="icon"
      variant="ghost"
      onPress={() => Uniwind.setTheme(nextTheme)}
      className="rounded-full">
      <Icon as={THEME_ICONS[theme ?? 'light']} className="size-5" />
    </Button>
  );
}
