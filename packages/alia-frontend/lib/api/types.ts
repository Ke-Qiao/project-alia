import type { SessionUser } from '@/lib/session/provider';

export type JsonValue =
  | string
  | number
  | boolean
  | null
  | JsonValue[]
  | { [key: string]: JsonValue };

export type ApiErrorPayload = {
  error?: string;
  message?: string;
  code?: string;
};

export class ApiError extends Error {
  status: number;
  payload: ApiErrorPayload | JsonValue | null;

  constructor(status: number, message: string, payload: ApiErrorPayload | JsonValue | null = null) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.payload = payload;
  }
}

export type HealthResponse = {
  status: string;
  version?: string;
  mode?: string;
  modules?: Record<string, { status: string; message?: string }>;
};

export type AuthResponse = {
  access_token: string;
  refresh_token: string;
  user: SessionUser;
};

export type ChannelInfo = {
  id: string;
  channel_type: string;
  active: boolean;
  health: string;
};

export type DisabledChannelInfo = {
  id: string;
  channel_type: string;
  reason: string;
  disabled_at?: string;
};

export type ChannelsResponse = {
  active: ChannelInfo[];
  disabled: DisabledChannelInfo[];
};

export type DeviceInfo = {
  device_id?: string | null;
  device_name?: string | null;
  status: string;
  endpoint?: string | null;
  provisioning_mode?: boolean | null;
  bind_required?: boolean | null;
};

export type SettingSchema = {
  label?: string;
  description?: string;
  type?: string;
  default?: JsonValue;
  requires_restart?: boolean;
  secret?: boolean;
  options?: string[];
  min?: number;
  max?: number;
};

export type SettingEntry = {
  value: JsonValue;
  schema?: SettingSchema | null;
  source?: string;
  updated_at?: string;
};

export type SettingsResponse = Record<string, SettingEntry>;

export type SettingsUpdateResponse = {
  success: boolean;
  changed: string[];
  applied: string[];
  requires_restart: string[];
  ignored_runtime: string[];
  module_reports: Array<{ module: string; applied: string[]; error?: string | null }>;
  errors: string[];
};

export type Skill = {
  id: string;
  name: string;
  display_name: string;
  description?: string | null;
  category: string;
  intent_patterns: JsonValue;
  tools: string[];
  prompt_template?: string | null;
  cacheable: boolean;
  enabled: boolean;
  priority: number;
  source: string;
  created_at: string;
  updated_at: string;
};

export type MatchSkillResponse = {
  matched: boolean;
  skill?: string;
  confidence?: number;
};

export type ToolDefinition = {
  name: string;
  description: string;
  parameters: JsonValue;
  routing_hints?: string[];
  source: string;
  require_confirmation: boolean;
  required_role?: string | null;
};

export type ToolResult = {
  call_id: string;
  success: boolean;
  output: JsonValue;
  // Opaque typed content is untrusted; clients must not dereference resource URIs.
  presentation?: JsonValue[] | null;
  display?: string | null;
};

export type EffectRecord = {
  effect_id: string;
  key: { user_id: string; request_id: string; invocation_key: string };
  model_call_id: string;
  tool_name: string;
  arguments: JsonValue;
  intent_digest: string;
  status: 'proposed' | 'approved' | 'dispatched' | 'succeeded' | 'failed' | 'unknown' | 'cancelled';
  requires_confirmation: boolean;
  completion?:
    | { status: 'succeeded'; receipt: ToolResult }
    | { status: 'failed'; evidence: JsonValue }
    | { status: 'unknown'; reason: string }
    | null;
};

export type ExecuteToolRequest = {
  request_id: string;
  tool_name: string;
  arguments: Record<string, JsonValue>;
  call_id?: string;
};

export type MemoryInfo = {
  id: string;
  memory_layer: string;
  node_type: string;
  kind: string;
  scope: string;
  content: string;
  importance: number;
  confidence: number;
  salience: number;
  visibility: string;
  sensitive: boolean;
  status: string;
  topic_tags: string[];
  affect_tags: string[];
  evidence_summary?: string | null;
  metadata: JsonValue;
};

export type MemoryPrivacy = {
  enabled: boolean;
  capture_mode: string;
  automatic_capture_enabled: boolean;
  paused_until?: string | null;
  explicit_memory_enabled: boolean;
  sensitive_auto_capture_allowed: boolean;
  sensitive_direct_entry_allowed: boolean;
  supported_capture_modes: string[];
};

export type MemoryCapabilities = {
  system: string;
  abi_version: number;
  supports_store: boolean;
  supports_list: boolean;
  supports_get: boolean;
  supports_retrieve: boolean;
  supports_update: boolean;
  supports_delete: boolean;
  supports_clear_user: boolean;
  supports_metadata: boolean;
  supports_search_filters: boolean;
  supports_layer_filters: boolean;
  supports_similarity_threshold: boolean;
  supports_graph: boolean;
  supports_relationship_edges: boolean;
  async_writes: boolean;
  max_content_chars?: number | null;
  diagnostics?: Record<string, JsonValue>;
};

export type MemoryConsolidationResult = {
  scanned: number;
  deduplicated: number;
  promoted: number;
  demoted: number;
  superseded: number;
  contradictions: number;
  transformed_affective_guidance: number;
};

export type RelationshipProfile = {
  user_id: string;
  trust: number;
  familiarity: number;
  summary: string;
  preference_summary: string;
  preferences: Record<string, string>;
  signal_counts: Record<string, number>;
  privacy: {
    user_visible: boolean;
    stores_preferences: boolean;
    stores_raw_evidence: boolean;
    clearable: boolean;
  };
  updated_at: string;
};

export type SensorSummary = {
  sensor_id: string;
  display_name: string;
  source: string;
  unit: string;
  value_type: string;
  last_seen_at?: string | null;
  latest?: {
    sensor_id: string;
    value: JsonValue;
    timestamp: string;
    quality: number;
  } | null;
  metadata?: JsonValue;
};
