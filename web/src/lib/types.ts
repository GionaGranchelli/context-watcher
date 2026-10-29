export interface Session {
  id: string;
  parent_session_id?: string | null;
  harness: string;
  workspace_root: string | null;
  model: string;
  provider: string;
  title: string | null;
  created_at: string;
  updated_at: string;
}

export interface TokenSpan {
  start: number;
  end: number;
  count: number;
}

export type KvNodeStatus = 'cached_hit' | 'newly_cached' | 'cache_invalidated' | 'uncached' | 'unknown';

export interface ContextNode {
  id: string;
  turn_seq: number;
  fingerprint: string;
  raw_json_pointer: string;
  role: string;
  kind: Record<string, any>;
  token_span: TokenSpan;
  kv_status: KvNodeStatus;
  attention_salience: number;
  timestamp?: string | null;
}

export interface KvCacheReport {
  hit_tokens: number;
  newly_cached_tokens: number;
  invalidated_tokens: number;
  uncached_tail_tokens: number;
  hit_ratio: number;
  divergence_token_index: number | null;
  invalidation_cause: Record<string, any> | null;
  telemetry_available?: boolean;
}

export interface LintFinding {
  rule_id: string;
  severity: 'info' | 'warning' | 'critical';
  title: string;
  message: string;
  affected_node_ids: string[];
  wasted_tokens: number;
  suggested_mutation: Record<string, any> | null;
}

export interface CategoryBreakdown {
  system_and_identity: number;
  rules_and_instructions: number;
  skills_catalog: number;
  mcp_and_native_tools: number;
  user_prompts: number;
  file_reads_fresh: number;
  file_reads_stale_or_duplicate: number;
  command_outputs: number;
  assistant_thinking: number;
  assistant_responses_and_calls: number;
  pinned_and_folded: number;
}

export interface ContextTurn {
  id: string;
  session_id: string;
  parent_turn_id: string | null;
  sequence_number: number;
  nodes: ContextNode[];
  token_usage: {
    input_tokens: number;
    output_tokens: number;
    cached_tokens: number;
    cache_write_tokens: number;
    context_limit: number;
    saved_by_mutations: number;
  };
  kv_cache_report: KvCacheReport;
  category_breakdown: CategoryBreakdown;
  lint_findings: LintFinding[];
  health_score: number;
  created_at: string;
}

export interface RadixGraphNode {
  id: string;
  label: string;
  group: string;
  turn_seq: number;
  tokens: number;
  kv_status: KvNodeStatus;
  lint_badges: string[];
  node_id: string | null;
  raw_json_pointer: string | null;
  timestamp?: string | null;
}

export interface RadixGraphEdge {
  id: string;
  source: string;
  target: string;
  animated: boolean;
}

export interface MutationRule {
  id: string;
  session_id: string;
  op: Record<string, any>;
  status: 'draft' | 'committed' | 'disabled';
  estimated_saved_tokens: number;
  estimated_kv_break_tokens: number;
}

export interface FileContextState {
  path: string;
  freshness: 'fresh' | 'stale' | 'duplicate' | 'pinned';
  last_read_turn: number;
  read_count: number;
  total_tokens_in_context: number;
  line_ranges: [number, number][];
  node_ids: string[];
}

export interface PendingInterceptInfo {
  intercept_id: string;
  session_id: string;
  turn_seq: number;
  rule_name: string;
  payload_preview: any;
}

export interface OptimizationCandidateItem {
  item_id: string;
  node_id: string | null;
  turn_seq: number;
  timestamp: string | null;
  title: string;
  action_kind: string;
  what_we_do: string;
  why_it_matters: string;
  content_preview: string;
  tokens_before: number;
  tokens_after: number;
  saved_tokens: number;
  kv_break_tokens: number;
  mutation_op: Record<string, any>;
}

export interface OptimizationStrategyPlan {
  id: string;
  name: string;
  tagline: string;
  kv_badge: string;
  what_and_how: string;
  expected_quality_impact: string;
  tokens_before: number;
  tokens_after: number;
  saved_tokens: number;
  saved_pct: number;
  kv_cache_break_tokens: number;
  break_even_turns: number;
  health_score_before: number;
  health_score_after: number;
  items: OptimizationCandidateItem[];
}

export interface CostAndPerfReport {
  model_family: string;
  is_local_model: boolean;
  turn_cost_usd: number;
  unoptimized_cost_usd: number;
  saved_by_kv_cache_usd: number;
  saved_by_cw_overlays_usd: number;
  estimated_prefill_ms: number;
  saved_prefill_ms_by_kv: number;
  saved_prefill_ms_by_overlays: number;
}

export interface LocalEngineRecommendation {
  id: string;
  severity: string;
  title: string;
  explanation: string;
  cli_or_api_fix: string;
}

export interface LocalModelAnalysis {
  is_local_model: boolean;
  engine_kind: string;
  model_name: string;
  num_ctx_limit: number;
  used_ctx_pct: number;
  recommended_n_keep_tokens: number;
  recommended_num_ctx: number;
  recommended_slot_id: number;
  paged_attention_block_size: number;
  prefix_block_aligned_tokens: number;
  prefix_block_waste_tokens: number;
  vram_fp16_mb: number;
  vram_q8_0_mb: number;
  vram_q4_0_mb: number;
  vram_saved_by_overlays_fp16_mb: number;
  sliding_window_overflow_risk: boolean;
  sliding_window_overflow_tokens: number;
  estimated_prefill_tps: number;
  prefill_latency_ms: number;
  prefill_saved_by_kv_ms: number;
  recommendations: LocalEngineRecommendation[];
}

export interface AutoPilotConfig {
  safety_version?: number;
  enabled: boolean;
  zero_break_tail_hygiene: boolean;
  tail_max_tool_tokens: number;
  auto_cache_safe_pin: boolean;
  pin_min_context_tokens: number;
  auto_diet_at_breakpoint: boolean;
  breakpoint_tokens: number;
  local_model_window_guard: boolean;
  local_window_threshold_tokens: number;
}


