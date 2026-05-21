import { apiRequest, authedFetcher, normalizeApiPath } from '@/lib/api/client';
import type {
  AuthResponse,
  ChannelsResponse,
  DeviceInfo,
  HealthResponse,
  MatchSkillResponse,
  MemoryCapabilities,
  MemoryConsolidationResult,
  MemoryInfo,
  MemoryPrivacy,
  RelationshipProfile,
  SensorSummary,
  SettingsResponse,
  SettingsUpdateResponse,
  Skill,
  ToolDefinition,
  ToolResult,
} from '@/lib/api/types';
import { useSession } from '@/lib/session/provider';
import useSWR from 'swr';
import useSWRMutation from 'swr/mutation';

type MutationArg<TBody> = {
  arg: TBody;
};

function keyFor(baseUrl: string | null, token: string | null, path: string, auth = true) {
  if (baseUrl === null) return null;
  if (auth && !token) return null;
  return [baseUrl, auth ? token : 'public', path] as const;
}

export function useHealth() {
  const session = useSession();
  return useSWR(
    session.hydrated && session.effectiveBaseUrl !== null
      ? [session.effectiveBaseUrl, 'public', '/health']
      : null,
    ([baseUrl, , path]) => apiRequest<HealthResponse>(baseUrl, path),
    { refreshInterval: 15000 }
  );
}

export function useProbeServer() {
  return useSWRMutation('server-probe', (_key, { arg }: MutationArg<{ baseUrl: string | null }>) =>
    apiRequest<HealthResponse>(arg.baseUrl, '/health')
  );
}

export function usePublicMutation<TBody, TResponse>(path: string, method = 'POST') {
  const session = useSession();
  return useSWRMutation(
    session.effectiveBaseUrl !== null ? [session.effectiveBaseUrl, 'public', path] : null,
    ([baseUrl, , endpoint], { arg }: MutationArg<TBody>) =>
      apiRequest<TResponse>(baseUrl, endpoint, { method, body: arg })
  );
}

export function useApiMutation<TBody, TResponse>(path: string, method = 'POST') {
  const session = useSession();
  return useSWRMutation(
    keyFor(session.effectiveBaseUrl, session.accessToken, path),
    ([baseUrl, token, endpoint], { arg }: MutationArg<TBody>) =>
      apiRequest<TResponse>(baseUrl, endpoint, { method, body: arg, token })
  );
}

export function useApiAction<TResponse = unknown>() {
  const session = useSession();
  return useSWRMutation(
    keyFor(session.effectiveBaseUrl, session.accessToken, '__action__'),
    (
      [baseUrl, token],
      { arg }: MutationArg<{ path: string; method?: string; body?: unknown }>
    ) => apiRequest<TResponse>(baseUrl, arg.path, { method: arg.method ?? 'POST', body: arg.body, token })
  );
}

export function useAuthLogin() {
  return usePublicMutation<{ email: string; password: string }, AuthResponse>('/api/v1/auth/login');
}

export function useAuthRegister() {
  return usePublicMutation<{ email: string; name: string; password: string }, AuthResponse>(
    '/api/v1/auth/register'
  );
}

export function useSetupTemplate() {
  return usePublicMutation<undefined, { template: string }>('/api/v1/setup/generate-config');
}

export function useSaveSetupConfig() {
  return usePublicMutation<{ config: string; path?: string }, { success: boolean; path: string; error?: string }>(
    '/api/v1/setup/save-config'
  );
}

export function useChannels() {
  const session = useSession();
  return useSWR(keyFor(session.effectiveBaseUrl, session.accessToken, '/api/v1/channels'), ([, , path]) =>
    authedFetcher<ChannelsResponse>(session, path)
  );
}

export function useDeviceCandidate() {
  const session = useSession();
  return useSWR(keyFor(session.effectiveBaseUrl, session.accessToken, '/api/v1/device'), ([, , path]) =>
    authedFetcher<DeviceInfo | null>(session, path)
  );
}

export function useBindDevice() {
  return useApiMutation<
    { channel_id?: string; server_id?: string; server_name?: string; server_endpoint?: string; require_confirmation?: boolean },
    DeviceInfo
  >('/api/v1/device/bind');
}

export function useSettings(prefix?: string) {
  const session = useSession();
  const path = normalizeApiPath('/api/v1/settings', { prefix });
  return useSWR(keyFor(session.effectiveBaseUrl, session.accessToken, path), ([, , endpoint]) =>
    authedFetcher<SettingsResponse>(session, endpoint)
  );
}

export function usePatchSettings() {
  return useApiMutation<Record<string, unknown>, SettingsUpdateResponse>('/api/v1/settings', 'PATCH');
}

export function useSkills() {
  const session = useSession();
  return useSWR(keyFor(session.effectiveBaseUrl, session.accessToken, '/api/v1/skills'), ([, , path]) =>
    authedFetcher<Skill[]>(session, path)
  );
}

export function useMatchSkill() {
  return useApiMutation<{ intent: string }, MatchSkillResponse>('/api/v1/skills/match');
}

export function useTools() {
  const session = useSession();
  return useSWR(keyFor(session.effectiveBaseUrl, session.accessToken, '/api/v1/tools'), ([, , path]) =>
    authedFetcher<ToolDefinition[]>(session, path)
  );
}

export function useExecuteTool() {
  return useApiMutation<{ tool_name: string; arguments: unknown; call_id?: string }, ToolResult>(
    '/api/v1/tools/execute'
  );
}

export function useMemories(layer?: string) {
  const session = useSession();
  const path = normalizeApiPath('/api/v1/memories', { layer, limit: 100 });
  return useSWR(keyFor(session.effectiveBaseUrl, session.accessToken, path), ([, , endpoint]) =>
    authedFetcher<MemoryInfo[]>(session, endpoint)
  );
}

export function useMemoryPrivacy() {
  const session = useSession();
  return useSWR(
    keyFor(session.effectiveBaseUrl, session.accessToken, '/api/v1/memories/privacy'),
    ([, , path]) => authedFetcher<MemoryPrivacy>(session, path)
  );
}

export function useMemoryCapabilities() {
  const session = useSession();
  return useSWR(
    keyFor(session.effectiveBaseUrl, session.accessToken, '/api/v1/memories/capabilities'),
    ([, , path]) => authedFetcher<MemoryCapabilities>(session, path)
  );
}

export function useCreateMemory() {
  return useApiMutation<
    { content: string; memory_layer?: string; node_type?: string; kind?: string; importance?: number },
    MemoryInfo
  >('/api/v1/memories');
}

export function useSearchMemories() {
  return useApiMutation<{ query: string; max_results?: number; layers?: string[] }, MemoryInfo[]>(
    '/api/v1/memories/search'
  );
}

export function useUpdateMemoryPrivacy() {
  return useApiMutation<
    { enabled?: boolean; automatic_capture_enabled?: boolean; capture_mode?: string; pause_days?: number; clear_pause?: boolean },
    MemoryPrivacy
  >('/api/v1/memories/privacy', 'PATCH');
}

export function useConsolidateMemories() {
  return useApiMutation<undefined, MemoryConsolidationResult>('/api/v1/memories/consolidate');
}

export function useRelationshipProfile() {
  const session = useSession();
  return useSWR(
    keyFor(session.effectiveBaseUrl, session.accessToken, '/api/v1/expression/relationship'),
    ([, , path]) => authedFetcher<RelationshipProfile>(session, path)
  );
}

export function useSensors() {
  const session = useSession();
  return useSWR(keyFor(session.effectiveBaseUrl, session.accessToken, '/api/v1/sensors'), ([, , path]) =>
    authedFetcher<SensorSummary[]>(session, path)
  );
}
