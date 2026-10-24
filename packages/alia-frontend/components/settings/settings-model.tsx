import type { Href } from 'expo-router';
import {
  Bot,
  Brain,
  Cable,
  FolderCog,
  HeartPulse,
  Network,
  Palette,
  PlugZap,
  Radar,
  Server,
  Shield,
  Sparkles,
  UserRound,
  Wrench,
  type LucideIcon,
} from 'lucide-react-native';

export type SettingsSectionKind =
  | 'account'
  | 'server-connection'
  | 'appearance'
  | 'privacy-memory'
  | 'config'
  | 'advanced-pages';

export type SettingsLink = {
  href: Href;
  icon: LucideIcon;
  title: string;
  description: string;
};

export type SettingsSection = {
  id: string;
  title: string;
  description: string;
  group: string;
  icon: LucideIcon;
  kind: SettingsSectionKind;
  status?: string;
  source?: string;
  namespaces?: string[];
  editablePrefixes?: string[];
  links?: SettingsLink[];
};

export const SETTINGS_SECTIONS: SettingsSection[] = [
  {
    id: 'account',
    title: 'Account',
    description: 'Profile, session, and sign-out controls.',
    group: 'Personal',
    icon: UserRound,
    kind: 'account',
    status: 'session',
  },
  {
    id: 'server-connection',
    title: 'Server connection',
    description: 'Current backend address and server switching.',
    group: 'Personal',
    icon: Network,
    kind: 'server-connection',
    status: 'connection',
  },
  {
    id: 'appearance',
    title: 'Appearance',
    description: 'Theme and display preferences.',
    group: 'Personal',
    icon: Palette,
    kind: 'appearance',
    status: 'local',
  },
  {
    id: 'privacy-memory',
    title: 'Privacy and memory',
    description: 'Memory capture and assistant preference learning.',
    group: 'Personal',
    icon: Shield,
    kind: 'privacy-memory',
    status: 'privacy',
  },
  {
    id: 'server-foundation',
    title: 'Server foundation',
    description: 'Server identity, network binding, database pool, and auth lifecycle.',
    group: 'Server config',
    icon: Server,
    kind: 'config',
    status: 'runtime',
    source: 'server.toml: server, database, auth',
    namespaces: ['core.server', 'core.database', 'core.auth'],
    editablePrefixes: ['core.server', 'core.database'],
  },
  {
    id: 'connections',
    title: 'Connections',
    description: 'Platform adapters, embedded clients, future chat bridges, and pairing flows.',
    group: 'Server config',
    icon: PlugZap,
    kind: 'config',
    status: 'server.toml',
    source: 'server.toml: channels',
    namespaces: ['channels', 'embedded', 'discovery'],
    links: [
      {
        href: '/connectors' as Href,
        icon: Cable,
        title: 'Connection status',
        description: 'See which adapters are currently running.',
      },
    ],
  },
  {
    id: 'models-providers',
    title: 'Models and providers',
    description: 'LLM roles, named profiles, provider credentials, STT, and TTS modules.',
    group: 'Server config',
    icon: Brain,
    kind: 'config',
    status: 'sensitive',
    source: 'providers/* and profiles/llm.toml',
    namespaces: ['core.providers', 'core.llm', 'core.profiles.llm', 'core.modules'],
    editablePrefixes: ['core.providers', 'core.llm', 'core.profiles.llm', 'core.modules'],
  },
  {
    id: 'assistant-behavior',
    title: 'Assistant behavior',
    description: 'Conversation routing, memory capture, insights, cache, and tool selection.',
    group: 'Server config',
    icon: Bot,
    kind: 'config',
    status: 'policy',
    source: 'policies/conversation, routing, memory, insights, cache, tools',
    namespaces: [
      'core.policies.routing',
      'core.policies.conversation',
      'core.policies.memory',
      'core.policies.insights',
      'core.policies.cache',
      'core.policies.tools',
    ],
    editablePrefixes: [
      'core.policies.routing',
      'core.policies.conversation',
      'core.policies.memory',
      'core.policies.insights',
      'core.policies.cache',
      'core.policies.tools',
    ],
    links: [
      {
        href: '/tools' as Href,
        icon: Wrench,
        title: 'Tool registry',
        description: 'Review callable assistant tools.',
      },
    ],
  },
  {
    id: 'voice-expression',
    title: 'Voice and expression',
    description: 'Affect, regulation, expression limits, voice profile, and style presets.',
    group: 'Server config',
    icon: HeartPulse,
    kind: 'config',
    status: 'policy',
    source: 'policies/affect, regulation, expression and profiles/voice',
    namespaces: [
      'core.policies.affect',
      'core.policies.regulation',
      'core.policies.expression',
      'core.profiles.voice',
    ],
    editablePrefixes: [
      'core.policies.affect',
      'core.policies.regulation',
      'core.policies.expression',
      'core.profiles.voice',
    ],
    links: [
      {
        href: '/expression' as Href,
        icon: HeartPulse,
        title: 'Assistant style',
        description: 'Review learned relationship and preference signals.',
      },
    ],
  },
  {
    id: 'resources-plugins',
    title: 'Resources and plugins',
    description: 'Skills directory, expression plugins, local resources, and future plugins.',
    group: 'Server config',
    icon: FolderCog,
    kind: 'config',
    status: 'server.toml',
    source: 'resources/* and plugins/*',
    namespaces: ['resources', 'skills_dir', 'wasm_dir', 'plugin.*'],
    links: [
      {
        href: '/skills' as Href,
        icon: Sparkles,
        title: 'Skills',
        description: 'Review assistant abilities and intent matching.',
      },
      {
        href: '/signals' as Href,
        icon: Radar,
        title: 'Signals',
        description: 'Inspect summarized sensor and event inputs.',
      },
    ],
  },
  {
    id: 'assistant-operations',
    title: 'Assistant operations',
    description: 'Operational views for skills, tools, expression state, and sensor signals.',
    group: 'Advanced',
    icon: Sparkles,
    kind: 'advanced-pages',
    status: 'advanced',
    links: [
      {
        href: '/skills' as Href,
        icon: Sparkles,
        title: 'Skills',
        description: 'Prompt-backed abilities and intent matching.',
      },
      {
        href: '/tools' as Href,
        icon: Wrench,
        title: 'Tools',
        description: 'Callable actions available to the assistant.',
      },
      {
        href: '/expression' as Href,
        icon: HeartPulse,
        title: 'Assistant style',
        description: 'Relationship, preferences, and expression state.',
      },
      {
        href: '/signals' as Href,
        icon: Radar,
        title: 'Signals',
        description: 'Summarized environment and device events.',
      },
    ],
  },
];

export function settingsSectionHref(id: string) {
  return `/settings/${id}` as Href;
}

export function findSettingsSection(id: string | string[] | undefined) {
  const normalized = Array.isArray(id) ? id[0] : id;
  return SETTINGS_SECTIONS.find((section) => section.id === normalized);
}

export function groupSettingsSections() {
  const groups: Array<{ title: string; sections: SettingsSection[] }> = [];
  for (const section of SETTINGS_SECTIONS) {
    const group = groups.find((item) => item.title === section.group);
    if (group) {
      group.sections.push(section);
    } else {
      groups.push({ title: section.group, sections: [section] });
    }
  }
  return groups;
}
