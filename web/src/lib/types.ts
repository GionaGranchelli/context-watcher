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

export type KvNodeStatus = 'cached_hit' | 'newly_cached' | 'cache_invalidated' | 'uncached';

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
