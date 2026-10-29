<script lang="ts">
  import { hierarchy, treemap } from 'd3-hierarchy';
  import HelpTip from './lib/HelpTip.svelte';
  import MarkdownText from './lib/MarkdownText.svelte';
  import type {
    Session,
    ContextTurn,
    ContextNode,
    RadixGraphNode,
    RadixGraphEdge,
    MutationRule,
    FileContextState,
    PendingInterceptInfo,
    OptimizationStrategyPlan,
    CostAndPerfReport,
    LocalModelAnalysis,
    AutoPilotConfig
  } from './lib/types';

  let controlToken = $state(sessionStorage.getItem('cw-control-token') || '');
  let tokenInput = $state('');
  let loadedSessionId = $state('');
  let loadedRevision = $state('');
  let graphRequest = 0;

  async function apiFetch(url: string, init: RequestInit = {}) {
    const headers = new Headers(init.headers);
    headers.set('Authorization', `Bearer ${controlToken}`);
    if (init.method && init.method !== 'GET' && url.includes('/sessions/') && (loadedSessionId !== activeSessionId || decodeURIComponent(url.split('/sessions/')[1].split('/')[0].split('?')[0]) !== loadedSessionId)) {
      statusBanner = 'Wait for the selected session to load before making changes.';
      throw new Error(statusBanner);
    }
    if (init.method && init.method !== 'GET' && url.includes('/sessions/')) headers.set('If-Match', loadedRevision);
    const response = await fetch(url, { ...init, headers });
    if (!response.ok) {
      statusBanner = response.status === 401 ? 'Connect using the token from cw auth-token.' : `Request failed (${response.status}). Refresh before retrying.`;
      throw new Error(statusBanner);
    }
    const revision = response.headers.get('x-cw-revision');
    if (revision && url.includes(`/sessions/${activeSessionId}/`)) loadedRevision = revision;
    return response;
  }

  let sessions = $state<Session[]>([]);
  let activeSessionId = $state<string>('');
  let latestTurn = $state<ContextTurn | null>(null);
  let turns = $state<ContextTurn[]>([]);
  let graphNodes = $state<RadixGraphNode[]>([]);
  let graphEdges = $state<RadixGraphEdge[]>([]);
  let mutations = $state<MutationRule[]>([]);
  let fileStates = $state<FileContextState[]>([]);
  let intercepts = $state<PendingInterceptInfo[]>([]);
  let optimizationPlans = $state<OptimizationStrategyPlan[]>([]);
  let selectedStrategyId = $state<string>('max_token_diet');
  let uncheckedItemIds = $state<Record<string, boolean>>({});
  let expandedOptItemIds = $state<Record<string, boolean>>({});
  let costAndPerf = $state<CostAndPerfReport | null>(null);
  let localModelAnalysis = $state<LocalModelAnalysis | null>(null);
  let autopilotConfig = $state<AutoPilotConfig | null>(null);
  let transcriptSourcePath = $state<string | null>(null);

  // 4 Primary Views + Focused Sub-Views (Linear/Vercel style progressive disclosure)
  let activeTab = $state<'timeline' | 'optimize' | 'analytics' | 'settings'>('timeline');
  let analyticsSubView = $state<'treemap' | 'advisor' | 'radix'>('treemap');
  let settingsSubView = $state<'local' | 'autopilot' | 'tools' | 'overlays'>('local');

  // Slide-over Right Inspector Drawer (Hidden by default to eliminate visual clutter)
  let inspectorOpen = $state<boolean>(false);
  let selectedNodeId = $state<string | null>(null);

  // Collapsible turn tool-action steps in Timeline
  let expandedTurnSteps = $state<Record<number, boolean>>({});
  let expandAllTurns = $state<boolean>(false);
  let rawMarkdownMode = $state<boolean>(false);

  // Strategy explanation collapsible in Optimize tab
  let showStrategyDetails = $state<boolean>(false);

  let wsConnected = $state<boolean>(false);
  let statusBanner = $state<string>('');
  let replayComparison = $state<{ original_turn: ContextTurn; forked_turn: ContextTurn; saved_tokens: number } | null>(null);

  // Sidebar filters & 2-level grouping
  let searchQuery = $state<string>('');
  let selectedProjectFilter = $state<string>('ALL');
  let selectedHarnessFilter = $state<string>('ALL');
  let groupByMode = $state<'project_first' | 'harness_first'>('project_first');
  let collapsedProjects = $state<Record<string, boolean>>({});

  // Interactive hover states for Treemap & Attention Candles
  let hoveredTreemapName = $state<string | null>(null);
  let hoveredCandleNodeId = $state<string | null>(null);
  let hoveredTabId = $state<string | null>(null);

  // Fold form state
  let foldStart = $state<number>(1);
  let foldEnd = $state<number>(2);
  let foldSummary = $state<string>('Tried debugging branch; resolved root cause in config.');

  // Pin form state
  let pinLabel = $state<string>('Architecture Rule');
  let pinContent = $state<string>('Preserve lossless JSON pointers and KV cache prefix boundaries.');

  function inspectNode(nodeId: string | null) {
    if (!nodeId) return;
    selectedNodeId = nodeId;
    inspectorOpen = true;
  }

  function projectNameFromWorkspace(ws: string | null | undefined): string {
    if (!ws || !ws.trim()) return 'General Chat';
    const parts = ws.replace(/\/+$/, '').split('/').filter(Boolean);
    return parts[parts.length - 1] || ws;
  }

  function formatTimestamp(iso: string | null | undefined): string {
    if (!iso) return 'Unknown time';
    const d = new Date(iso);
    if (isNaN(d.getTime())) return iso;
    return d.toLocaleString(undefined, {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit'
    });
  }

  function formatRelativeTime(iso: string | null | undefined): string {
    if (!iso) return '';
    const d = new Date(iso);
    if (isNaN(d.getTime())) return '';
    const diffSec = Math.round((Date.now() - d.getTime()) / 1000);
    if (diffSec < 60) return 'just now';
    if (diffSec < 3600) return `${Math.floor(diffSec / 60)}m ago`;
    if (diffSec < 86400) return `${Math.floor(diffSec / 3600)}h ago`;
    return `${Math.floor(diffSec / 86400)}d ago`;
  }

  function isRecentSession(iso: string | null | undefined): boolean {
    if (!iso) return false;
    const d = new Date(iso);
    if (isNaN(d.getTime())) return false;
    return Date.now() - d.getTime() < 86400 * 1000;
  }

  function shortModelName(model: string | null | undefined): string {
    if (!model) return '';
    return model
      .replace(/^claude-/, '')
      .replace(/-\d{8}$/, '')
      .replace(/^ollama\//, '')
      .slice(0, 18);
  }

  function projectAccentColor(name: string): string {
    if (name === 'General Chat') return '#fbbf24';
    const palette = ['#60a5fa', '#34d399', '#a78bfa', '#38bdf8', '#fb923c', '#f472b6', '#2dd4bf'];
    let hash = 0;
    for (let i = 0; i < name.length; i++) {
      hash = (hash * 31 + name.charCodeAt(i)) >>> 0;
    }
    return palette[hash % palette.length];
  }

  function harnessBadgeStyle(h: string): { bg: string; fg: string; border: string; dot: string; label: string } {
    switch (h) {
      case 'omp':
        return { bg: 'rgba(236, 72, 153, 0.15)', fg: '#f472b6', border: 'rgba(236, 72, 153, 0.34)', dot: '#ec4899', label: 'omp' };
      case 'pi':
        return { bg: 'rgba(168, 85, 247, 0.15)', fg: '#d8b4fe', border: 'rgba(168, 85, 247, 0.34)', dot: '#a855f7', label: 'pi' };
      case 'opencode':
        return { bg: 'rgba(14, 165, 233, 0.15)', fg: '#38bdf8', border: 'rgba(14, 165, 233, 0.34)', dot: '#0ea5e9', label: 'opencode' };
      case 'hermes':
        return { bg: 'rgba(245, 158, 11, 0.14)', fg: '#fbbf24', border: 'rgba(245, 158, 11, 0.32)', dot: '#f59e0b', label: 'hermes' };
      case 'codex':
        return { bg: 'rgba(16, 185, 129, 0.14)', fg: '#34d399', border: 'rgba(16, 185, 129, 0.32)', dot: '#10b981', label: 'codex' };
      case 'antigravity':
        return { bg: 'rgba(59, 130, 246, 0.15)', fg: '#93c5fd', border: 'rgba(59, 130, 246, 0.34)', dot: '#3b82f6', label: 'antigravity' };
      case 'claude_code':
        return { bg: 'rgba(249, 115, 22, 0.15)', fg: '#fdba74', border: 'rgba(249, 115, 22, 0.34)', dot: '#f97316', label: 'claude' };
      default:
        return { bg: 'rgba(168, 85, 247, 0.15)', fg: '#d8b4fe', border: 'rgba(168, 85, 247, 0.34)', dot: '#a855f7', label: h };
    }
  }

  // 2-Level Sidebar Hierarchy: supports both `Project -> Agent` and `Agent -> Project`
  let sidebarGroups = $derived.by(() => {
    const q = searchQuery.trim().toLowerCase();
    const topMap = new Map<
      string,
      {
        topKey: string;
        topLabel: string;
        topSubtitle: string | null;
        latestUpdated: string;
        totalSessions: number;
        subGroups: Map<string, { subKey: string; subLabel: string; sessions: Session[] }>;
      }
    >();

    for (const s of sessions) {
      const proj = projectNameFromWorkspace(s.workspace_root);
      const h = s.harness || 'generic_proxy';
      if (selectedProjectFilter !== 'ALL' && proj !== selectedProjectFilter) continue;
      if (selectedHarnessFilter !== 'ALL' && h !== selectedHarnessFilter) continue;
      if (q) {
        const hay = `${s.title || ''} ${proj} ${s.workspace_root || ''} ${s.id} ${s.model} ${h}`.toLowerCase();
        if (!hay.includes(q)) continue;
      }

      const topKey = groupByMode === 'project_first' ? proj : h;
      const topLabel = groupByMode === 'project_first' ? proj : h;
      const topSubtitle =
        groupByMode === 'project_first'
          ? s.workspace_root
          : h === 'hermes'
            ? '~/.hermes/state.db'
            : h === 'codex'
              ? '~/.codex/sessions'
              : h === 'antigravity'
                ? '~/.gemini/antigravity-cli'
                : h === 'claude_code'
                  ? '~/.claude/projects'
                  : h === 'omp'
                    ? '~/.omp/agent/sessions'
                    : h === 'pi'
                      ? '~/.pi/agent/sessions'
                      : h === 'opencode'
                        ? '~/.local/share/opencode'
                        : 'Local / Proxy';
      const subKey = groupByMode === 'project_first' ? h : proj;
      const subLabel = groupByMode === 'project_first' ? h : proj;

      if (!topMap.has(topKey)) {
        topMap.set(topKey, {
          topKey,
          topLabel,
          topSubtitle,
          latestUpdated: s.updated_at,
          totalSessions: 0,
          subGroups: new Map()
        });
      }
      const tg = topMap.get(topKey)!;
      tg.totalSessions += 1;
      if (s.updated_at > tg.latestUpdated) tg.latestUpdated = s.updated_at;

      if (!tg.subGroups.has(subKey)) {
        tg.subGroups.set(subKey, { subKey, subLabel, sessions: [] });
      }
      tg.subGroups.get(subKey)!.sessions.push(s);
    }

    return Array.from(topMap.values())
      .map((tg) => ({
        ...tg,
        subGroupsList: Array.from(tg.subGroups.values()).sort((a, b) =>
          (b.sessions[0]?.updated_at || '').localeCompare(a.sessions[0]?.updated_at || '')
        )
      }))
      .sort((a, b) => {
        if (a.topKey === 'General Chat' && b.topKey !== 'General Chat') return 1;
        if (b.topKey === 'General Chat' && a.topKey !== 'General Chat') return -1;
        return b.latestUpdated.localeCompare(a.latestUpdated);
      });
  });

  let allProjectNames = $derived.by(() => {
    const set = new Set<string>();
    for (const s of sessions) {
      set.add(projectNameFromWorkspace(s.workspace_root));
    }
    return Array.from(set).sort();
  });

  let allHarnessNames = $derived.by(() => {
    const set = new Set<string>();
    for (const s of sessions) {
      set.add(s.harness);
    }
    return Array.from(set).sort();
  });

  let activeSession = $derived<Session | null>(sessions.find((s) => s.id === activeSessionId) ?? null);

  let selectedNode = $derived<ContextNode | null>(
    latestTurn?.nodes.find((n) => n.id === selectedNodeId) ?? latestTurn?.nodes[0] ?? null
  );

  let draftMutations = $derived(mutations.filter((m) => m.status === 'draft'));

  // Group conversation nodes into chronological Turn Cards
  let storyTurns = $derived.by(() => {
    if (!latestTurn) return [];
    const byTurn = new Map<
      number,
      {
        turnSeq: number;
        timestamp: string | null;
        userPrompts: ContextNode[];
        thinkings: ContextNode[];
        toolActions: ContextNode[];
        assistantReplies: ContextNode[];
        totalTokens: number;
      }
    >();

    for (const node of latestTurn.nodes) {
      const k = node.kind.kind;
      if (k === 'mcp_tool_schema' || k === 'system_prompt' || k === 'injected_rule' || k === 'skill_catalog') {
        continue;
      }
      const t = node.turn_seq || 1;
      if (!byTurn.has(t)) {
        byTurn.set(t, {
          turnSeq: t,
          timestamp: node.timestamp ?? null,
          userPrompts: [],
          thinkings: [],
          toolActions: [],
          assistantReplies: [],
          totalTokens: 0
        });
      }
      const entry = byTurn.get(t)!;
      if (!entry.timestamp && node.timestamp) entry.timestamp = node.timestamp;
      entry.totalTokens += node.token_span.count;

      if (k === 'user_message') entry.userPrompts.push(node);
      else if (k === 'assistant_thinking') entry.thinkings.push(node);
      else if (k === 'tool_call' || k === 'tool_result') entry.toolActions.push(node);
      else entry.assistantReplies.push(node);
    }

    return Array.from(byTurn.values()).sort((a, b) => a.turnSeq - b.turnSeq);
  });

  // Compute D3 Treemap layout from CategoryBreakdown
  let treemapCategories = $derived.by(() => {
    if (!latestTurn) return [];
    const cb = latestTurn.category_breakdown;
    const total = Math.max(
      1,
      cb.mcp_and_native_tools +
        cb.system_and_identity +
        cb.rules_and_instructions +
        cb.skills_catalog +
        cb.file_reads_fresh +
        cb.file_reads_stale_or_duplicate +
        cb.command_outputs +
        cb.assistant_thinking +
        cb.assistant_responses_and_calls +
        cb.user_prompts +
        cb.pinned_and_folded
    );
    return [
      {
        name: 'Conversation & Calls',
        value: cb.assistant_responses_and_calls + cb.user_prompts,
        color: '#3b82f6',
        whatMatters: 'Keep recent turns; older debugging back-and-forth can be folded.',
        canCut: 'Medium (Fold old turns)'
      },
      {
        name: 'Command Outputs',
        value: cb.command_outputs,
        color: '#d97706',
        whatMatters: 'Review older outputs before removing them; they may contain unique facts.',
        canCut: 'High (Safe to prune old logs)'
      },
      {
        name: 'Assistant Thinking',
        value: cb.assistant_thinking,
        color: '#a855f7',
        whatMatters: 'Past internal reasoning blocks rarely help future turns once the assistant acted.',
        canCut: 'High (Safe to prune old thoughts)'
      },
      {
        name: 'File Reads (Fresh)',
        value: cb.file_reads_fresh,
        color: '#10b981',
        whatMatters: 'Latest code state of files the agent is actively working on.',
        canCut: 'Low (Keep active files)'
      },
      {
        name: 'File Reads (Stale/Dup)',
        value: cb.file_reads_stale_or_duplicate,
        color: '#ea580c',
        whatMatters: 'Outdated or duplicate file contents superseded by later edits or re-reads.',
        canCut: 'Review range coverage before dedupe'
      },
      {
        name: 'MCP & Native Tools',
        value: cb.mcp_and_native_tools,
        color: '#6366f1',
        whatMatters: 'JSON schemas for tools. Gate off MCP servers you are not using.',
        canCut: 'High (Gate unused MCP tools)'
      },
      {
        name: 'Rules & Instructions',
        value: cb.rules_and_instructions,
        color: '#14b8a6',
        whatMatters: 'Project guidelines (AGENTS.md, CLAUDE.md). Pin if attention drops.',
        canCut: 'Keep (Core instructions)'
      },
      {
        name: 'System & Identity',
        value: cb.system_and_identity,
        color: '#0284c7',
        whatMatters: 'Base harness system prompt and identity instructions.',
        canCut: 'Keep (System core)'
      },
      {
        name: 'Skills Catalog',
        value: cb.skills_catalog,
        color: '#7c3aed',
        whatMatters: 'Descriptions of available agent skills.',
        canCut: 'Low'
      },
      {
        name: 'Pinned & Folded',
        value: cb.pinned_and_folded,
        color: '#059669',
        whatMatters: 'Compact summaries of folded turns and high-attention tail reminders.',
        canCut: 'Keep (Already optimized)'
      }
    ]
      .filter((d) => d.value > 0)
      .map((d) => ({
        ...d,
        pct: ((d.value / total) * 100).toFixed(1)
      }))
      .sort((a, b) => b.value - a.value);
  });

  let treemapLeaves = $derived.by(() => {
    if (treemapCategories.length === 0) return [];
    const data = { name: 'root', children: treemapCategories };
    const root = hierarchy<any>(data).sum((d: any) => d.value || 0).sort((a, b) => (b.value || 0) - (a.value || 0));
    const layout = treemap<any>().size([540, 290]).paddingInner(3).paddingOuter(3);
    return layout(root).leaves();
  });

  let activeCandleNode = $derived<ContextNode | null>(
    latestTurn?.nodes.find((n) => n.id === hoveredCandleNodeId) ?? selectedNode
  );

  // Smart Prune & Cut Advisor
  let pruneAdvisorItems = $derived.by(() => {
    if (!latestTurn) return [];
    const maxTurn = Math.max(1, ...latestTurn.nodes.map((n) => n.turn_seq || 1));
    const staleNodeIds = new Set<string>();
    for (const f of latestTurn.lint_findings) {
      if (f.rule_id === 'CW-L001' || f.rule_id === 'CW-L002' || f.rule_id === 'CW-L003') {
        for (const id of f.affected_node_ids) staleNodeIds.add(id);
      }
    }

    const items: {
      node: ContextNode;
      label: string;
      preview: string;
      tokens: number;
      verdict: 'cut_recommended' | 'can_compact' | 'keep';
      verdictBadge: string;
      reason: string;
    }[] = [];

    for (const n of latestTurn.nodes) {
      const k = n.kind.kind;
      const tok = n.token_span.count;
      const text = nodeDisplayText(n);
      if (text.startsWith('[cw:pruned') || text.startsWith('[cw:shadow_deduped') || text.startsWith('[cw:folded')) {
        continue;
      }

      if (staleNodeIds.has(n.id)) {
        items.push({
          node: n,
          label: humanToolActionLabel(n),
          preview: text.slice(0, 140),
          tokens: tok,
          verdict: 'cut_recommended',
          verdictBadge: 'Stale / Duplicate',
          reason: 'Outdated or duplicate file read / oversized tool output.'
        });
      } else if (k === 'tool_result' && (n.turn_seq < maxTurn || tok > 120)) {
        items.push({
          node: n,
          label: humanToolActionLabel(n),
          preview: text.slice(0, 140),
          tokens: tok,
          verdict: n.turn_seq < maxTurn ? 'cut_recommended' : 'can_compact',
          verdictBadge: n.turn_seq < maxTurn ? 'Old Tool Output' : 'Large Output',
          reason:
            n.turn_seq < maxTurn
              ? `From earlier Turn #${n.turn_seq}. Safe to replace with a 1-line stub.`
              : 'From the latest turn. Compact if noisy.'
        });
      } else if (k === 'assistant_thinking' && (n.turn_seq < maxTurn || tok > 80)) {
        items.push({
          node: n,
          label: `Reasoning (Turn #${n.turn_seq})`,
          preview: text.slice(0, 140),
          tokens: tok,
          verdict: 'can_compact',
          verdictBadge: 'Old Reasoning',
          reason: `Internal reasoning block from Turn #${n.turn_seq}.`
        });
      }
    }

    return items.sort((a, b) => {
      if (a.verdict === 'cut_recommended' && b.verdict !== 'cut_recommended') return -1;
      if (b.verdict === 'cut_recommended' && a.verdict !== 'cut_recommended') return 1;
      return b.tokens - a.tokens;
    });
  });

  let totalSafeToCutTokens = $derived(
    pruneAdvisorItems
      .filter((i) => i.verdict === 'cut_recommended' || i.verdict === 'can_compact')
      .reduce((acc, i) => acc + Math.max(0, i.tokens - 12), 0)
  );

  let mcpToolsList = $derived.by(() => {
    if (!latestTurn) return [];
    return latestTurn.nodes
      .filter((n) => n.kind.kind === 'mcp_tool_schema')
      .map((n) => ({
        id: n.id,
        server: n.kind.server_name || 'native',
        name: n.kind.tool_name,
        description: n.kind.description,
        tokens: n.token_span.count
      }));
  });

  let activeStrategyPlan = $derived<OptimizationStrategyPlan | null>(
    optimizationPlans.find((p) => p.id === selectedStrategyId) ?? optimizationPlans[0] ?? null
  );

  let checkedStrategyItems = $derived.by(() => {
    if (!activeStrategyPlan) return [];
    return activeStrategyPlan.items.filter((i) => !uncheckedItemIds[i.item_id]);
  });

  let liveStrategyScoreboard = $derived.by(() => {
    if (!activeStrategyPlan) {
      return {
        tokensBefore: 0,
        tokensAfter: 0,
        savedTokens: 0,
        savedPct: '0.0',
        kvBreakTokens: 0,
        breakEvenTurns: 0,
        healthBefore: 100,
        healthAfter: 100
      };
    }
    const before = activeStrategyPlan.tokens_before;
    const rawSaved = checkedStrategyItems.reduce((acc, i) => acc + i.saved_tokens, 0);
    const saved = Math.max(0, rawSaved);
    const after = Math.max(0, before - rawSaved);
    const pct = before > 0 ? ((saved / before) * 100).toFixed(1) : '0.0';
    const kvBreak = checkedStrategyItems.reduce((acc, i) => Math.max(acc, i.kv_break_tokens), 0);
    const breakEven = kvBreak === 0 ? 0 : Math.ceil(kvBreak / Math.max(1, saved));
    const ratio = activeStrategyPlan.items.length > 0 ? checkedStrategyItems.length / activeStrategyPlan.items.length : 0;
    const healthAfter = Math.round(
      activeStrategyPlan.health_score_before +
        (activeStrategyPlan.health_score_after - activeStrategyPlan.health_score_before) * ratio
    );
    return {
      tokensBefore: before,
      tokensAfter: after,
      savedTokens: saved,
      savedPct: pct,
      kvBreakTokens: kvBreak,
      breakEvenTurns: breakEven,
      healthBefore: activeStrategyPlan.health_score_before,
      healthAfter
    };
  });

  async function fetchSessions() {
    try {
      const res = await apiFetch('/api/v1/sessions');
      sessions = await res.json();
      if (!activeSessionId && sessions.length > 0) {
        const withWorkspace = sessions.find((s) => s.workspace_root && s.title);
        activeSessionId = withWorkspace ? withWorkspace.id : sessions[0].id;
      }
      if (activeSessionId) {
        await fetchSessionGraph(activeSessionId);
      }
    } catch (e) {
      console.error(e);
    }
  }

  async function fetchSessionGraph(sessionId: string) {
    if (!sessionId) return;
    const request = ++graphRequest;
    const res = await apiFetch(`/api/v1/sessions/${sessionId}/graph`);
    const data = await res.json();
    if (request !== graphRequest || activeSessionId !== sessionId) return;
    const changedSession = loadedSessionId !== sessionId;
    loadedSessionId = sessionId;
    loadedRevision = data.revision;
    latestTurn = data.mutation_preview || data.latest_turn;
    turns = data.turns || [];
    graphNodes = data.radix_graph?.nodes || [];
    graphEdges = data.radix_graph?.edges || [];
    mutations = data.mutations || [];
    fileStates = data.file_states || [];
    optimizationPlans = data.optimization_plans || [];
    costAndPerf = data.cost_and_perf || null;
    localModelAnalysis = data.local_model_analysis || null;
    autopilotConfig = data.autopilot_config || null;
    transcriptSourcePath = data.transcript_source_path || null;
    if (changedSession) uncheckedItemIds = {};
    if (optimizationPlans.length > 0 && !optimizationPlans.some((p) => p.id === selectedStrategyId)) {
      const nonEmpty = optimizationPlans.find((p) => p.items.length > 0);
      selectedStrategyId = nonEmpty ? nonEmpty.id : optimizationPlans[0].id;
    }
    if (latestTurn && latestTurn.nodes.length > 0 && !selectedNodeId) {
      selectedNodeId = latestTurn.nodes[0].id;
    }
  }

  async function applyMutation(op: Record<string, any>, status: 'committed' | 'draft' = 'committed', msg = 'Mutation applied') {
    if (!activeSessionId) return;
    await apiFetch(`/api/v1/sessions/${activeSessionId}/mutations`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ op, status })
    });
    statusBanner = msg;
    await fetchSessionGraph(activeSessionId);
  }

  async function commitDrafts() {
    if (!activeSessionId) return;
    await apiFetch(`/api/v1/sessions/${activeSessionId}/mutations/commit-drafts`, { method: 'POST' });
    statusBanner = 'Committed all staged draft mutations (cache impact estimated).';
    await fetchSessionGraph(activeSessionId);
  }

  async function deleteMutation(mutationId: string) {
    if (!activeSessionId) return;
    await apiFetch(`/api/v1/sessions/${activeSessionId}/mutations/${mutationId}`, { method: 'DELETE' });
    statusBanner = 'Reverted mutation overlay.';
    await fetchSessionGraph(activeSessionId);
  }

  async function executeSelectedStrategy(status: 'committed' | 'draft' = 'committed') {
    if (!activeSessionId || !activeStrategyPlan) return;
    const selectedIds = checkedStrategyItems.map((i) => i.item_id);
    if (selectedIds.length === 0) return;
    const res = await apiFetch(`/api/v1/sessions/${activeSessionId}/optimize`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        strategy_id: activeStrategyPlan.id,
        selected_item_ids: selectedIds,
        status
      })
    });
    if (res.ok) {
      const result = await res.json();
      statusBanner =
        status === 'draft'
          ? `Staged ${result.applied_count} optimizations from "${activeStrategyPlan.name}" (~${result.estimated_saved_tokens.toLocaleString()} tok).`
          : `Applied ${result.applied_count} optimizations from "${activeStrategyPlan.name}" — saved ~${result.estimated_saved_tokens.toLocaleString()} tokens.`;
      await fetchSessionGraph(activeSessionId);
    }
  }

  async function bulkCutSafeItems() {
    if (!activeSessionId) return;
    const targetSession = activeSessionId;
    const toCut = pruneAdvisorItems.filter((i) => i.verdict === 'cut_recommended' || i.verdict === 'can_compact');
    for (const item of toCut) {
      await apiFetch(`/api/v1/sessions/${targetSession}/mutations`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          op: {
            op: 'prune_or_stub',
            target_fingerprint: item.node.fingerprint,
            target_call_id: null,
            target_json_pointer: item.node.raw_json_pointer,
            stub_text: `[cw:pruned original_tokens=${item.tokens} label="${item.label}"]`
          },
          status: 'committed'
        })
      });
    }
    statusBanner = `Cut ${toCut.length} prunable items (saved ~${totalSafeToCutTokens.toLocaleString()} tokens).`;
    await fetchSessionGraph(activeSessionId);
  }

  async function triggerForkReplay() {
    if (!activeSessionId) return;
    const res = await apiFetch(`/api/v1/sessions/${activeSessionId}/replay`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        from_turn_id: latestTurn?.id ?? null,
        additional_mutations: [{ op: 'shadow_dedupe', file_path: null }]
      })
    });
    if (res.ok) {
      replayComparison = await res.json();
      statusBanner = `Created mutation preview (saved ${replayComparison?.saved_tokens ?? 0} tokens).`;
      await fetchSessionGraph(activeSessionId);
    }
  }

  async function seedRichDemoSession() {
    const sessId = 'demo_agy_session';
    const turn1Payload = {
      model: 'claude-sonnet-4-20250514',
      max_tokens: 8192,
      system:
        'You are Antigravity, an agentic coding assistant.\n<RULE[/home/gionag/Development/contextWatcher/AGENTS.md]>\nZero-Drop Proxy Transparency, KV-Cache Awareness First, and Deterministic Mutation Overlays.\n</RULE[/home/gionag/Development/contextWatcher/AGENTS.md]>\n<skills>\n- ponytail (/skills/ponytail/SKILL.md): Write minimal, direct code.\n- chrome-devtools (/skills/chrome-devtools/SKILL.md): Debug web pages.\n</skills>',
      tools: [
        { name: 'read_file', description: 'Read file from disk', input_schema: { type: 'object', properties: { Path: { type: 'string' } } } },
        { name: 'edit_file', description: 'Edit file on disk', input_schema: { type: 'object', properties: { Path: { type: 'string' }, Replacement: { type: 'string' } } } },
        { name: 'run_command', description: 'Execute bash command', input_schema: { type: 'object', properties: { CommandLine: { type: 'string' } } } },
        { name: 'mcp_chrome_click', description: 'Click DOM node in Chrome DevTools MCP', input_schema: { type: 'object', properties: { uid: { type: 'string' } } } },
        { name: 'mcp_chrome_take_snapshot', description: 'Take accessibility tree snapshot', input_schema: { type: 'object', properties: {} } }
      ],
      messages: [
        {
          role: 'user',
          content: 'Inspect crates/cw-core/src/radix.rs and optimize KV cache prefix diffing.',
          cw_timestamp: new Date(Date.now() - 180000).toISOString(),
          cw_turn_seq: 1
        },
        {
          role: 'assistant',
          content: [
            { type: 'thinking', thinking: 'Let me read crates/cw-core/src/radix.rs first to inspect the LCP loop.', signature: 'sig_demo_1' },
            { type: 'tool_use', id: 'call_read_1', name: 'read_file', input: { Path: 'crates/cw-core/src/radix.rs', StartLine: 1, EndLine: 120 } }
          ],
          cw_timestamp: new Date(Date.now() - 165000).toISOString(),
          cw_turn_seq: 1
        },
        {
          role: 'user',
          content: [
            {
              type: 'tool_result',
              tool_use_id: 'call_read_1',
              content: 'pub fn analyze_kv_cache(prev_nodes: Option<&[ContextNode]>, curr_nodes: &mut [ContextNode]) -> KvCacheReport {\n    // v1 implementation before edit\n    KvCacheReport::default()\n}\n'.repeat(12)
            }
          ],
          cw_timestamp: new Date(Date.now() - 160000).toISOString(),
          cw_turn_seq: 1
        },
        {
          role: 'user',
          content: 'Now update analyze_kv_cache to support Longest Common Prefix diffing and re-read the file.',
          cw_timestamp: new Date(Date.now() - 90000).toISOString(),
          cw_turn_seq: 2
        },
        {
          role: 'assistant',
          content: [
            { type: 'tool_use', id: 'call_edit_1', name: 'edit_file', input: { Path: 'crates/cw-core/src/radix.rs', Replacement: 'pub fn analyze_kv_cache_v2() {}' } },
            { type: 'tool_use', id: 'call_read_2', name: 'read_file', input: { Path: 'crates/cw-core/src/radix.rs', StartLine: 1, EndLine: 140 } }
          ],
          cw_timestamp: new Date(Date.now() - 60000).toISOString(),
          cw_turn_seq: 2
        },
        {
          role: 'user',
          content: [
            { type: 'tool_result', tool_use_id: 'call_edit_1', content: 'Updated crates/cw-core/src/radix.rs' },
            {
              type: 'tool_result',
              tool_use_id: 'call_read_2',
              content: 'pub fn analyze_kv_cache(prev_nodes: Option<&[ContextNode]>, curr_nodes: &mut [ContextNode]) -> KvCacheReport {\n    // v2 updated implementation with full Longest Common Prefix diffing\n}\n'.repeat(10)
            }
          ],
          cw_timestamp: new Date(Date.now() - 45000).toISOString(),
          cw_turn_seq: 2
        }
      ]
    };

    await apiFetch('/api/v1/ingest', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        session_id: sessId,
        harness: 'antigravity',
        provider: 'anthropic',
        workspace_root: '/home/gionag/Development/contextWatcher',
        model: 'claude-sonnet-4-20250514',
        payload: turn1Payload
      })
    });
    activeSessionId = sessId;
    await fetchSessions();
    statusBanner = 'Loaded Cloud Demo Session under project "contextWatcher".';
  }

  async function seedLocalLlmDemoSession() {
    const sessId = 'demo_ollama_qwen32b';
    const payload = {
      model: 'ollama/qwen2.5-coder:32b',
      messages: [
        {
          role: 'system',
          content:
            'You are OpenCode running on local Ollama (qwen2.5-coder:32b).\n<RULE[/home/gionag/Development/contextWatcher/AGENTS.md]>\nPreserve KV cache prefix and keep prompt under local num_ctx window.\n</RULE[/home/gionag/Development/contextWatcher/AGENTS.md]>'
        },
        {
          role: 'user',
          content: 'Read crates/cw-core/src/local_llm.rs and run cargo test to check KV slot alignment.',
          cw_timestamp: new Date(Date.now() - 120000).toISOString(),
          cw_turn_seq: 1
        },
        {
          role: 'assistant',
          content: null,
          tool_calls: [
            {
              id: 'call_local_1',
              type: 'function',
              function: {
                name: 'read_file',
                arguments: JSON.stringify({ Path: 'crates/cw-core/src/local_llm.rs', StartLine: 1, EndLine: 90 })
              }
            },
            {
              id: 'call_local_2',
              type: 'function',
              function: {
                name: 'run_command',
                arguments: JSON.stringify({ CommandLine: 'cargo test --workspace' })
              }
            }
          ],
          cw_timestamp: new Date(Date.now() - 95000).toISOString(),
          cw_turn_seq: 1
        },
        {
          role: 'tool',
          tool_call_id: 'call_local_1',
          content: 'pub fn analyze_local_model_kv() {\n  // initial version\n}\n'.repeat(25),
          cw_timestamp: new Date(Date.now() - 90000).toISOString(),
          cw_turn_seq: 1
        },
        {
          role: 'tool',
          tool_call_id: 'call_local_2',
          content: 'running 18 tests\ntest result: ok. 18 passed; 0 failed;\n'.repeat(40),
          cw_timestamp: new Date(Date.now() - 85000).toISOString(),
          cw_turn_seq: 1
        }
      ]
    };

    await apiFetch('/api/v1/ingest', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        session_id: sessId,
        harness: 'opencode-local',
        provider: 'ollama',
        workspace_root: '/home/gionag/Development/contextWatcher',
        model: 'ollama/qwen2.5-coder:32b',
        payload
      })
    });
    activeSessionId = sessId;
    activeTab = 'settings';
    settingsSubView = 'local';
    await fetchSessions();
    statusBanner = 'Loaded Local LLM session (Ollama qwen2.5-coder:32b).';
  }

  async function syncTranscriptToDisk() {
    if (!activeSessionId) return;
    const res = await apiFetch(`/api/v1/sessions/${activeSessionId}/sync-back`, {
      method: 'POST'
    });
    const data = await res.json();
    if (res.ok) {
      statusBanner = `Exported ${data.steps_compacted} records to ${data.export_path}. Original transcript unchanged.`;
      await fetchSessionGraph(activeSessionId);
    } else {
      statusBanner = `Sync-back note: ${data.error || 'Session is proxy-only or has no on-disk transcript.'}`;
    }
  }

  async function saveAutoPilotConfig(updated: AutoPilotConfig) {
    const res = await apiFetch('/api/v1/autopilot', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(updated)
    });
    if (res.ok) {
      const data = await res.json();
      autopilotConfig = data.autopilot_config;
      statusBanner = 'Updated Auto-Pilot background policy configuration.';
    }
  }

  $effect(() => {
    const token = controlToken;
    if (!token) return;
    sessionStorage.setItem('cw-control-token', token);
    let disposed = false;
    let timer: ReturnType<typeof setTimeout>;
    let socket: WebSocket | undefined;
    function connect() {
      if (disposed) return;
      fetchSessions();
      const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
      socket = new WebSocket(`${proto}//${window.location.host}/api/v1/ws`, ['cw', `cw-token.${token}`]);
      socket.onopen = () => { wsConnected = true; };
      socket.onclose = () => {
        wsConnected = false;
        if (!disposed) timer = setTimeout(connect, 2000);
      };
      socket.onmessage = (event) => {
        try {
          const message = JSON.parse(event.data);
          if (message.event === 'turn_completed' || message.event === 'mutations_updated') fetchSessions();
          else if (message.event === 'firewall_intercepted') intercepts = [...intercepts, message.intercept];
        } catch { statusBanner = 'Received an unreadable update. Refresh the session.'; }
      };
    }
    connect();
    return () => { disposed = true; clearTimeout(timer); socket?.close(); };
  });

  function kvColor(status: string): string {
    switch (status) {
      case 'cached_hit':
        return '#10b981';
      case 'newly_cached':
        return '#f59e0b';
      case 'cache_invalidated':
        return '#ef4444';
      default:
        return '#52525b';
    }
  }

  function nodeDisplayText(node: ContextNode): string {
    const k = node.kind;
    return (
      k.text ||
      k.content ||
      k.thought ||
      k.summary ||
      k.raw_text ||
      JSON.stringify(k.schema_json || k.arguments || k, null, 2)
    );
  }

  function humanToolActionLabel(node: ContextNode): string {
    const k = node.kind;
    if (k.kind === 'tool_call') {
      const sem = k.semantic_meta;
      if (sem?.semantic_type === 'file_read') return `Read ${sem.path}`;
      if (sem?.semantic_type === 'file_write') return `Edit ${sem.path}`;
      if (sem?.semantic_type === 'command_exec') return `Run ${sem.command}`;
      return `Call ${k.tool_name}`;
    }
    if (k.kind === 'tool_result') {
      const sem = k.semantic_meta;
      if (sem?.semantic_type === 'file_read') return `Result: ${sem.path}`;
      if (sem?.semantic_type === 'file_write') return `Updated: ${sem.path}`;
      if (sem?.semantic_type === 'command_exec') return `Output (exit ${sem.exit_code ?? 0}): ${sem.command}`;
      return `Output: ${k.tool_name}`;
    }
    return k.kind;
  }
</script>

<div style="display: flex; flex-direction: column; height: 100vh; overflow: hidden; background: #09090b; color: #f4f4f5; font-family: -apple-system, BlinkMacSystemFont, 'Inter', 'Segoe UI', Roboto, sans-serif;">
  <!-- Calm Linear-Style Header Bar -->
  <header style="display: flex; align-items: center; justify-content: space-between; height: 50px; padding: 0 18px; background: #0f0f12; border-bottom: 1px solid #1f1f26; gap: 16px; flex-shrink: 0;">
    <div style="display: flex; align-items: center; gap: 12px; min-width: 0;">
      <div style="display: flex; align-items: center; gap: 8px;">
        <span style="width: 8px; height: 8px; border-radius: 999px; background: {wsConnected ? '#10b981' : '#ef4444'};"></span>
        <span style="font-weight: 700; font-size: 14.5px; color: #f4f4f5; letter-spacing: -0.02em;">contextWatcher</span>
      </div>

      {#if activeSession}
        <span style="color: #3f3f46;">/</span>
        <div style="display: flex; align-items: center; gap: 8px; font-size: 12.5px; min-width: 0;">
          <span style="color: #a1a1aa; font-weight: 500;">{projectNameFromWorkspace(activeSession.workspace_root)}</span>
          <span style="color: #3f3f46;">›</span>
          <span style="color: #f4f4f5; font-weight: 500; max-width: 320px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;">
            {activeSession.title || activeSession.id}
          </span>
          <span style="padding: 1px 7px; border-radius: 4px; background: #18181b; border: 1px solid #27272a; color: #a1a1aa; font-size: 11px;">
            {activeSession.harness} · {activeSession.model}
          </span>
        </div>
      {/if}
    </div>

    {#if latestTurn}
      <div style="display: flex; align-items: center; gap: 12px; font-size: 12px; flex-shrink: 0;">
        <!-- Metric 1: Context Usage -->
        <div style="display: flex; align-items: center; background: #141418; padding: 4px 10px; border-radius: 6px; border: 1px solid #1f1f26;">
          <span style="color: #71717a; margin-right: 6px;">Tokens</span>
          <strong style="color: #f4f4f5;">{(latestTurn.token_usage.input_tokens / 1000).toFixed(1)}k</strong>
          <span style="color: #52525b; margin-left: 3px;">/ {((latestTurn.token_usage.context_limit || 200000) / 1000).toFixed(0)}k</span>
          {#if latestTurn.token_usage.saved_by_mutations > 0}
            <span style="color: #10b981; margin-left: 6px; font-weight: 600;">(-{(latestTurn.token_usage.saved_by_mutations / 1000).toFixed(1)}k)</span>
          {/if}
          <HelpTip text={`Exact context window usage: ${latestTurn.token_usage.input_tokens.toLocaleString()} / ${(latestTurn.token_usage.context_limit || 200000).toLocaleString()} tokens.`} />
        </div>

        <!-- Metric 2: KV Cache Hit Rate -->
        <div style="display: flex; align-items: center; background: #141418; padding: 4px 10px; border-radius: 6px; border: 1px solid #1f1f26;">
          <span style="width: 6px; height: 6px; border-radius: 999px; background: {latestTurn.kv_cache_report.invalidated_tokens > 0 ? '#ef4444' : '#10b981'}; margin-right: 6px;"></span>
          <span style="color: #71717a; margin-right: 5px;">KV Cache</span>
          <strong style="color: #f4f4f5;">{(latestTurn.kv_cache_report.hit_ratio * 100).toFixed(0)}%</strong>
          <HelpTip text={`Cached Hit: ${latestTurn.kv_cache_report.hit_tokens.toLocaleString()} tok · Newly Written: ${latestTurn.kv_cache_report.newly_cached_tokens.toLocaleString()} tok · Invalidated: ${latestTurn.kv_cache_report.invalidated_tokens.toLocaleString()} tok`} />
        </div>

        <!-- Metric 3: Cost or Local VRAM -->
        {#if costAndPerf}
          <div style="display: flex; align-items: center; background: #141418; padding: 4px 10px; border-radius: 6px; border: 1px solid #1f1f26;">
            {#if costAndPerf.is_local_model && localModelAnalysis}
              <span style="color: #71717a; margin-right: 5px;">VRAM (Q8)</span>
              <strong style="color: #c084fc;">{(localModelAnalysis.vram_q8_0_mb / 1024).toFixed(1)} GB</strong>
              <HelpTip text={`Local GPU KV Cache: ${localModelAnalysis.vram_q8_0_mb.toFixed(0)} MB (Q8_0) · Prefill: ${costAndPerf.estimated_prefill_ms}ms`} />
            {:else}
              <span style="color: #71717a; margin-right: 5px;">Turn</span>
              <strong style="color: #10b981;">${costAndPerf.turn_cost_usd.toFixed(3)}</strong>
              <HelpTip text={`Saved $${(costAndPerf.saved_by_kv_cache_usd + costAndPerf.saved_by_cw_overlays_usd).toFixed(3)} via KV Cache & cw overlays on this turn.`} />
            {/if}
          </div>
        {/if}

        <!-- Metric 4: Health Score -->
        <div style="display: flex; align-items: center; background: #141418; padding: 4px 10px; border-radius: 6px; border: 1px solid #1f1f26;">
          <span style="color: #71717a; margin-right: 5px;">Health</span>
          <strong style="color: {latestTurn.health_score >= 80 ? '#10b981' : latestTurn.health_score >= 55 ? '#f59e0b' : '#ef4444'};">
            {latestTurn.health_score}
          </strong>
          <HelpTip text={`Context Health Score (${latestTurn.health_score}/100) based on ${latestTurn.lint_findings.length} lint findings.`} />
        </div>

        {#if draftMutations.length > 0}
          <button
            onclick={commitDrafts}
            style="background: #d97706; color: #fff; border: none; border-radius: 6px; padding: 5px 10px; font-size: 12px; font-weight: 600; cursor: pointer;"
          >
            Commit {draftMutations.length} Drafts
          </button>
        {/if}

        {#if transcriptSourcePath && mutations.length > 0}
          <button
            onclick={syncTranscriptToDisk}
            style="background: #18181b; color: #10b981; border: 1px solid #065f46; border-radius: 6px; padding: 5px 10px; font-size: 12px; font-weight: 500; cursor: pointer;"
          >
            Export Copy
          </button>
        {/if}

        <button
          onclick={() => (activeTab = 'optimize')}
          style="background: #2563eb; color: #ffffff; border: none; border-radius: 6px; padding: 5px 12px; font-size: 12px; font-weight: 600; cursor: pointer;"
        >
          Optimize {latestTurn.lint_findings.length > 0 ? `(${latestTurn.lint_findings.length})` : ''}
        </button>

        <button
          onclick={() => (inspectorOpen = !inspectorOpen)}
          style="background: {inspectorOpen ? '#27272a' : '#141418'}; color: #e4e4e7; border: 1px solid #27272a; border-radius: 6px; padding: 5px 10px; font-size: 12px; cursor: pointer;"
        >
          {inspectorOpen ? 'Hide Inspector' : 'Inspector'}
        </button>
      </div>
    {/if}
  </header>

  <form onsubmit={(event) => { event.preventDefault(); controlToken = tokenInput.trim(); tokenInput = ''; }} style="display:flex;gap:8px;padding:8px">
    <label for="control-token">Local connection token (run cw auth-token)</label>
    <input id="control-token" type="password" autocomplete="off" required pattern={'[A-Za-z0-9_-]{32,128}'} bind:value={tokenInput} />
    <button type="submit">Connect</button>
  </form>

  {#if statusBanner}
    <div style="background: #172554; color: #bfdbfe; padding: 6px 18px; font-size: 12px; display: flex; justify-content: space-between; align-items: center; border-bottom: 1px solid #1e3a8a;">
      <span>{statusBanner}</span>
      <button onclick={() => (statusBanner = '')} style="background: transparent; border: none; color: #93c5fd; cursor: pointer; font-size: 13px;">✕</button>
    </div>
  {/if}

  <!-- Main Workspace: 2-Column Layout by Default (Drawer opens on demand) -->
  <div style="display: grid; grid-template-columns: 280px 1fr {inspectorOpen ? '380px' : '0px'}; flex: 1; overflow: hidden; transition: grid-template-columns 0.15s ease;">
    <!-- [1] Left Sidebar: Clean Session Explorer -->
    <aside style="background: #0f0f12; border-right: 1px solid #1f1f26; display: flex; flex-direction: column; overflow: hidden;">
      <div style="padding: 12px; border-bottom: 1px solid #1f1f26; display: flex; flex-direction: column; gap: 8px; background: #0f0f12;">
        <!-- Segmented Control: By Project vs By Agent -->
        <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 2px; background: #18181b; padding: 3px; border-radius: 6px; border: 1px solid #27272a;">
          <button
            onclick={() => (groupByMode = 'project_first')}
            style="padding: 5px 8px; font-size: 11.5px; font-weight: {groupByMode === 'project_first' ? '600' : '500'}; border-radius: 4px; border: none; cursor: pointer; background: {groupByMode === 'project_first' ? '#27272a' : '#18181b'}; color: {groupByMode === 'project_first' ? '#ffffff' : '#a1a1aa'};"
          >
            By Project
          </button>
          <button
            onclick={() => (groupByMode = 'harness_first')}
            style="padding: 5px 8px; font-size: 11.5px; font-weight: {groupByMode === 'harness_first' ? '600' : '500'}; border-radius: 4px; border: none; cursor: pointer; background: {groupByMode === 'harness_first' ? '#27272a' : '#18181b'}; color: {groupByMode === 'harness_first' ? '#ffffff' : '#a1a1aa'};"
          >
            By Agent
          </button>
        </div>

        <input
          type="text"
          bind:value={searchQuery}
          placeholder="Search project, prompt, or agent..."
          style="width: 100%; box-sizing: border-box; background: #18181b; color: #f4f4f5; border: 1px solid #27272a; border-radius: 6px; padding: 6px 10px; font-size: 12px; outline: none;"
        />

        <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 6px;">
          <select
            bind:value={selectedHarnessFilter}
            style="width: 100%; background: #18181b; color: #a1a1aa; border: 1px solid #27272a; border-radius: 5px; padding: 4px 6px; font-size: 11px;"
          >
            <option value="ALL">All Agents ({allHarnessNames.length})</option>
            {#each allHarnessNames as h}
              <option value={h}>{h}</option>
            {/each}
          </select>

          <select
            bind:value={selectedProjectFilter}
            style="width: 100%; background: #18181b; color: #a1a1aa; border: 1px solid #27272a; border-radius: 5px; padding: 4px 6px; font-size: 11px;"
          >
            <option value="ALL">All Projects ({allProjectNames.length})</option>
            {#each allProjectNames as proj}
              <option value={proj}>{proj}</option>
            {/each}
          </select>
        </div>
      </div>

      <!-- Session Tree: Grey Container + Selected/Unselected Contrast (Matching By Project / By Agent) -->
      <div style="flex: 1; overflow-y: auto; padding: 10px 8px;">
        {#each sidebarGroups as group}
          {@const isCollapsed = collapsedProjects[group.topKey]}
          {@const isGroupActive = group.subGroupsList.some((sg) => sg.sessions.some((s) => s.id === activeSessionId))}
          <div style="margin-bottom: 10px; background: #18181b; padding: 4px; border-radius: 7px; border: 1px solid #27272a;">
            <!-- Top-Level Project / Agent Button (Selected = #27272a + white text, Unselected = #18181b + light grey text) -->
            <button
              onclick={() => (collapsedProjects[group.topKey] = !isCollapsed)}
              style="width: 100%; display: flex; align-items: center; justify-content: space-between; padding: 7px 10px; background: {isGroupActive ? '#27272a' : '#18181b'}; color: {isGroupActive ? '#ffffff' : '#a1a1aa'}; border: none; border-radius: 5px; cursor: pointer; text-align: left;"
            >
              <div style="display: flex; align-items: center; gap: 7px; min-width: 0;">
                <span style="font-size: 10px; color: {isGroupActive ? '#ffffff' : '#a1a1aa'};">{isCollapsed ? '▸' : '▾'}</span>
                <span style="font-size: 12.5px; font-weight: {isGroupActive ? '600' : '500'}; color: {isGroupActive ? '#ffffff' : '#a1a1aa'}; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;">
                  {group.topLabel}
                </span>
              </div>
              <span style="font-size: 10.5px; background: {isGroupActive ? '#3f3f46' : '#27272a'}; color: {isGroupActive ? '#ffffff' : '#a1a1aa'}; padding: 1px 7px; border-radius: 999px; font-weight: 600; flex-shrink: 0;">
                {group.totalSessions}
              </span>
            </button>

            {#if !isCollapsed}
              <div style="display: flex; flex-direction: column; gap: 4px; padding: 4px 2px 2px 6px; margin-top: 4px; border-top: 1px solid #27272a;">
                {#each group.subGroupsList as sub}
                  {#if group.subGroupsList.length > 1 || groupByMode === 'harness_first'}
                    {@const subStyle = groupByMode === 'project_first' ? harnessBadgeStyle(sub.subKey) : null}
                    <div style="display: flex; align-items: center; gap: 6px; padding: 3px 6px 1px 4px;">
                      {#if subStyle}
                        <span style="width: 6px; height: 6px; border-radius: 999px; background: {subStyle.dot};"></span>
                        <span style="font-size: 10.5px; color: #a1a1aa; font-weight: 600; text-transform: uppercase; letter-spacing: 0.04em;">
                          {sub.subLabel}
                        </span>
                      {:else}
                        <span style="font-size: 11px; color: #a1a1aa; font-weight: 600;">
                          {sub.subLabel}
                        </span>
                      {/if}
                    </div>
                  {/if}

                  {#each sub.sessions as s}
                    {@const hb = harnessBadgeStyle(s.harness)}
                    {@const isSelected = activeSessionId === s.id}
                    <button
                      onclick={() => {
                        activeSessionId = s.id;
                        selectedNodeId = null;
                        fetchSessionGraph(s.id);
                      }}
                      style="width: 100%; text-align: left; padding: 8px 10px; border: 1px solid {isSelected ? '#3f3f46' : 'transparent'}; border-radius: 5px; cursor: pointer; background: {isSelected ? '#27272a' : '#18181b'}; color: {isSelected ? '#ffffff' : '#a1a1aa'}; transition: background 0.1s;"
                    >
                      <div style="font-size: 12px; font-weight: {isSelected ? '600' : '400'}; color: {isSelected ? '#ffffff' : '#a1a1aa'}; line-height: 1.38; margin-bottom: 5px; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden;">
                        {s.title || s.id}
                      </div>
                      <div style="display: flex; justify-content: space-between; align-items: center; gap: 6px; font-size: 10.5px;">
                        <div style="display: flex; align-items: center; gap: 5px; min-width: 0; overflow: hidden;">
                          <span style="display: inline-flex; align-items: center; gap: 4px; padding: 1px 6px; border-radius: 4px; background: {isSelected ? '#3f3f46' : '#27272a'}; color: {isSelected ? '#ffffff' : '#d4d4d8'}; font-weight: 500; font-size: 10px; flex-shrink: 0;">
                            <span style="width: 5px; height: 5px; border-radius: 999px; background: {hb.dot};"></span>
                            {hb.label}
                          </span>
                          {#if s.model}
                            <span style="color: {isSelected ? '#d4d4d8' : '#71717a'}; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; font-size: 10px;">
                              {shortModelName(s.model)}
                            </span>
                          {/if}
                        </div>
                        <span style="color: {isSelected ? '#e4e4e7' : '#71717a'}; flex-shrink: 0;">
                          {formatRelativeTime(s.updated_at)}
                        </span>
                      </div>
                    </button>
                  {/each}
                {/each}
              </div>
            {/if}
          </div>
        {/each}
      </div>

      <!-- Subtle Footer for Sample Demos -->
      <div style="padding: 8px 12px; border-top: 1px solid #1f1f26; display: flex; justify-content: space-between; align-items: center; font-size: 11px; color: #71717a;">
        <span>Sample Demos:</span>
        <div style="display: flex; gap: 6px;">
          <button
            onclick={seedRichDemoSession}
            style="background: #141418; color: #a1a1aa; border: 1px solid #27272a; border-radius: 4px; padding: 3px 7px; font-size: 11px; cursor: pointer;"
          >
            Cloud
          </button>
          <button
            onclick={seedLocalLlmDemoSession}
            style="background: #141418; color: #a1a1aa; border: 1px solid #27272a; border-radius: 4px; padding: 3px 7px; font-size: 11px; cursor: pointer;"
          >
            Local GPU
          </button>
        </div>
      </div>
    </aside>

    <!-- [2] Center Workspace: 4 Clean Views -->
    <div style="display: flex; flex-direction: column; overflow: hidden; background: #09090b;">
      <!-- Clean 4-Tab Navigation Bar (Hover shows help text without '?' icon) -->
      <nav style="display: flex; align-items: center; justify-content: space-between; padding: 0 24px; height: 44px; background: #0f0f12; border-bottom: 1px solid #1f1f26; flex-shrink: 0;">
        <div style="display: flex; align-items: center; gap: 20px; height: 100%;">
          {#each [
            ['timeline', 'Timeline', 'Chronological conversation trace with collapsible tool actions and reasoning.'],
            ['optimize', 'Optimize', 'Dry-run context compaction studio with 5 strategies and live Before/After scoreboard.'],
            ['analytics', 'Analytics & Tree', 'Token treemap, U-shaped attention curve, prunability advisor, and Radix prefix tree.'],
            ['settings', 'Local GPU & Settings', 'Local LLM KV-cache slot studio (Ollama/llama.cpp), Auto-Pilot policies, Tool Gate, and Overlays.']
          ] as [id, label, tip]}
            <div
              role="presentation"
              onmouseenter={() => (hoveredTabId = id)}
              onmouseleave={() => (hoveredTabId = null)}
              style="position: relative; display: flex; align-items: center; height: 100%; border-bottom: 2px solid {activeTab === id ? '#3b82f6' : 'transparent'};"
            >
              <button
                onclick={() => (activeTab = id as any)}
                style="padding: 0 4px; height: 100%; border: none; background: transparent; font-size: 13px; font-weight: {activeTab === id ? '600' : '400'}; color: {activeTab === id ? '#f4f4f5' : '#a1a1aa'}; cursor: pointer;"
              >
                {label}
              </button>
              {#if hoveredTabId === id}
                <div
                  style="position: absolute; top: 46px; left: 0; width: 260px; background: #18181b; color: #e4e4e7; border: 1px solid #27272a; border-radius: 6px; padding: 8px 10px; font-size: 11.5px; line-height: 1.4; box-shadow: 0 10px 25px rgba(0,0,0,0.65); z-index: 9999; pointer-events: none;"
                >
                  {tip}
                </div>
              {/if}
            </div>
          {/each}
        </div>

        <!-- Contextual Sub-Navigation Controls on the Right of the Tab Bar -->
        {#if activeTab === 'timeline'}
          <div style="display: flex; align-items: center; gap: 8px;">
            <div style="display: flex; gap: 2px; background: #141418; padding: 2px; border-radius: 6px; border: 1px solid #27272a;">
              <button
                onclick={() => (rawMarkdownMode = false)}
                style="padding: 3px 8px; font-size: 11px; border-radius: 4px; border: none; cursor: pointer; background: {!rawMarkdownMode ? '#27272a' : 'transparent'}; color: {!rawMarkdownMode ? '#f4f4f5' : '#71717a'};"
              >
                Formatted MD
              </button>
              <button
                onclick={() => (rawMarkdownMode = true)}
                style="padding: 3px 8px; font-size: 11px; border-radius: 4px; border: none; cursor: pointer; background: {rawMarkdownMode ? '#27272a' : 'transparent'}; color: {rawMarkdownMode ? '#f4f4f5' : '#71717a'};"
              >
                Raw Text
              </button>
            </div>

            <button
              onclick={() => (expandAllTurns = !expandAllTurns)}
              style="background: #141418; color: #a1a1aa; border: 1px solid #27272a; border-radius: 5px; padding: 4px 10px; font-size: 11.5px; cursor: pointer;"
            >
              {expandAllTurns ? 'Collapse Tool Steps' : 'Expand All Tool Steps'}
            </button>
          </div>
        {:else if activeTab === 'analytics'}
          <div style="display: flex; gap: 4px; background: #141418; padding: 2px; border-radius: 6px; border: 1px solid #27272a;">
            {#each [
              ['treemap', 'Token & Attention'],
              ['advisor', `Prunable (${pruneAdvisorItems.length})`],
              ['radix', 'Radix Tree']
            ] as [sid, slabel]}
              <button
                onclick={() => (analyticsSubView = sid as any)}
                style="padding: 3px 9px; font-size: 11.5px; border-radius: 4px; border: none; cursor: pointer; background: {analyticsSubView === sid ? '#27272a' : 'transparent'}; color: {analyticsSubView === sid ? '#f4f4f5' : '#71717a'};"
              >
                {slabel}
              </button>
            {/each}
          </div>
        {:else if activeTab === 'settings'}
          <div style="display: flex; gap: 4px; background: #141418; padding: 2px; border-radius: 6px; border: 1px solid #27272a;">
            {#each [
              ['local', 'Local GPU & KV'],
              ['autopilot', 'Auto-Pilot'],
              ['tools', `Tools & Files (${fileStates.length})`],
              ['overlays', `Overlays (${mutations.length})`]
            ] as [sid, slabel]}
              <button
                onclick={() => (settingsSubView = sid as any)}
                style="padding: 3px 9px; font-size: 11.5px; border-radius: 4px; border: none; cursor: pointer; background: {settingsSubView === sid ? '#27272a' : 'transparent'}; color: {settingsSubView === sid ? '#f4f4f5' : '#71717a'};"
              >
                {slabel}
              </button>
            {/each}
          </div>
        {/if}
      </nav>

      <!-- Main Scrollable Content Area -->
      <main style="flex: 1; overflow-y: auto; padding: 24px; max-width: 1120px; width: 100%; margin: 0 auto; box-sizing: border-box;">
        {#if !latestTurn}
          <div style="text-align: center; padding: 90px 20px; color: #71717a;">
            <h2 style="color: #e4e4e7; font-size: 16px; font-weight: 600; margin-bottom: 6px;">Select a Session</h2>
            <p style="font-size: 13px;">Choose a project or agent conversation from the left sidebar.</p>
          </div>

        {:else if activeTab === 'timeline'}
          <!-- VIEW 1: CLEAN CHRONOLOGICAL TIMELINE -->
          <div style="display: flex; flex-direction: column; gap: 16px;">
            {#each storyTurns as st, idx}
              {@const isLastTurn = idx === storyTurns.length - 1}
              {@const stepsExpanded = expandAllTurns || expandedTurnSteps[st.turnSeq] || (isLastTurn && st.toolActions.length <= 4)}
              <div style="background: #141418; border: 1px solid #1f1f26; border-radius: 10px; overflow: hidden;">
                <!-- Turn Header -->
                <div style="display: flex; justify-content: space-between; align-items: center; padding: 10px 16px; border-bottom: 1px solid #1f1f26; background: #0f0f12;">
                  <div style="display: flex; align-items: center; gap: 10px; font-size: 12px;">
                    <span style="font-weight: 600; color: #e4e4e7;">Turn {st.turnSeq}</span>
                    {#if st.timestamp}
                      <span style="color: #71717a;">· {formatTimestamp(st.timestamp)} ({formatRelativeTime(st.timestamp)})</span>
                    {/if}
                    <span style="color: #52525b;">· {st.totalTokens.toLocaleString()} tok</span>
                  </div>

                  <button
                    onclick={() =>
                      applyMutation(
                        {
                          op: 'fold_subtree',
                          start_turn: st.turnSeq,
                          end_turn: st.turnSeq,
                          summary_text: `Collapsed Turn #${st.turnSeq} tool outputs`
                        },
                        'committed',
                        `Folded Turn #${st.turnSeq}`
                      )}
                    style="background: transparent; color: #71717a; border: 1px solid #27272a; border-radius: 5px; padding: 3px 8px; font-size: 11px; cursor: pointer;"
                  >
                    Fold turn
                  </button>
                </div>

                <div style="padding: 16px; display: flex; flex-direction: column; gap: 12px;">
                  <!-- User Prompts -->
                  {#each st.userPrompts as up}
                    <div
                      role="button"
                      tabindex="0"
                      onclick={() => inspectNode(up.id)}
                      onkeydown={(e) => e.key === 'Enter' && inspectNode(up.id)}
                      style="background: #171923; border: 1px solid #262938; border-left: 3px solid #3b82f6; padding: 12px 14px; border-radius: 8px; cursor: pointer;"
                    >
                      <div style="display: flex; justify-content: space-between; font-size: 11px; color: #60a5fa; font-weight: 600; margin-bottom: 6px;">
                        <span>User Prompt</span>
                        <span style="color: #94a3b8; font-weight: 400;">{up.token_span.count.toLocaleString()} tok</span>
                      </div>
                      <MarkdownText text={up.kind.text || ''} rawMode={rawMarkdownMode} maxHeight="280px" />
                    </div>
                  {/each}

                  <!-- Collapsible Intermediate Steps (Reasoning + Tool Calls) -->
                  {#if st.thinkings.length > 0 || st.toolActions.length > 0}
                    <div style="border: 1px solid #1f1f26; border-radius: 8px; background: #0f0f12; overflow: hidden;">
                      <button
                        onclick={() => (expandedTurnSteps[st.turnSeq] = !stepsExpanded)}
                        style="width: 100%; display: flex; justify-content: space-between; align-items: center; padding: 8px 12px; background: transparent; border: none; color: #a1a1aa; font-size: 12px; cursor: pointer; text-align: left;"
                      >
                        <div style="display: flex; align-items: center; gap: 8px;">
                          <span style="color: #52525b; font-size: 10px;">{stepsExpanded ? '▾' : '▸'}</span>
                          <span>
                            {st.toolActions.length} tool step{st.toolActions.length === 1 ? '' : 's'}
                            {st.thinkings.length > 0 ? ` · ${st.thinkings.length} reasoning block${st.thinkings.length === 1 ? '' : 's'}` : ''}
                          </span>
                        </div>
                        <span style="font-size: 11px; color: #71717a;">
                          {stepsExpanded ? 'Hide steps' : 'Show steps'}
                        </span>
                      </button>

                      {#if stepsExpanded}
                        <div style="padding: 8px 12px 12px 12px; border-top: 1px solid #1f1f26; display: flex; flex-direction: column; gap: 6px;">
                          {#each st.thinkings as th}
                            <div
                              role="button"
                              tabindex="0"
                              onclick={() => inspectNode(th.id)}
                              onkeydown={(e) => e.key === 'Enter' && inspectNode(th.id)}
                              style="padding: 8px 10px; border-radius: 6px; background: #141418; border: 1px solid #1f1f26; border-left: 3px solid #a855f7; cursor: pointer; font-size: 12px; color: #a1a1aa;"
                            >
                              <div style="display: flex; justify-content: space-between; font-size: 11px; color: #c084fc; margin-bottom: 4px;">
                                <span>Reasoning</span>
                                <span style="color: #71717a;">{th.token_span.count} tok</span>
                              </div>
                              <MarkdownText text={th.kind.thought || ''} rawMode={rawMarkdownMode} maxHeight="100px" compact={true} />
                            </div>
                          {/each}

                          {#each st.toolActions as ta}
                            <div
                              role="button"
                              tabindex="0"
                              onclick={() => inspectNode(ta.id)}
                              onkeydown={(e) => e.key === 'Enter' && inspectNode(ta.id)}
                              style="display: flex; justify-content: space-between; align-items: center; padding: 6px 10px; border-radius: 6px; background: #141418; border: 1px solid {selectedNodeId === ta.id ? '#3b82f6' : '#1f1f26'}; cursor: pointer; font-size: 12px;"
                            >
                              <div style="display: flex; align-items: center; gap: 8px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 78%;">
                                <span style="width: 6px; height: 6px; border-radius: 999px; background: {kvColor(ta.kv_status)}; flex-shrink: 0;"></span>
                                <span style="color: #d4d4d8;">{humanToolActionLabel(ta)}</span>
                              </div>
                              <span style="color: #71717a; font-size: 11px; flex-shrink: 0;">{ta.token_span.count.toLocaleString()} tok</span>
                            </div>
                          {/each}
                        </div>
                      {/if}
                    </div>
                  {/if}

                  <!-- Assistant Replies -->
                  {#each st.assistantReplies as ar}
                    <div
                      role="button"
                      tabindex="0"
                      onclick={() => inspectNode(ar.id)}
                      onkeydown={(e) => e.key === 'Enter' && inspectNode(ar.id)}
                      style="background: #12151c; border: 1px solid #1f2430; border-left: 3px solid #10b981; padding: 12px 14px; border-radius: 8px; cursor: pointer;"
                    >
                      <div style="display: flex; justify-content: space-between; font-size: 11px; color: #34d399; font-weight: 600; margin-bottom: 6px;">
                        <span>Assistant Response</span>
                        <span style="color: #94a3b8; font-weight: 400;">{ar.token_span.count.toLocaleString()} tok</span>
                      </div>
                      <MarkdownText text={ar.kind.text || ar.kind.summary || ''} rawMode={rawMarkdownMode} maxHeight="360px" />
                    </div>
                  {/each}
                </div>
              </div>
            {/each}
          </div>

        {:else if activeTab === 'optimize'}
          <!-- VIEW 2: CLEAN OPTIMIZATION STUDIO -->
          <!-- Section 1: Strategy Presets -->
          <div style="margin-bottom: 10px;">
            <h3 style="margin: 0; font-size: 13.5px; font-weight: 600; color: #f4f4f5;">
              1. Select an Optimization Strategy
            </h3>
            <p style="margin: 2px 0 0 0; font-size: 11.5px; color: #a1a1aa;">
              Choose a compaction preset below to preview how many tokens can be cut and how it impacts your KV cache.
            </p>
          </div>
          <div style="display: grid; grid-template-columns: repeat(5, 1fr); gap: 10px; margin-bottom: 22px;">
            {#each optimizationPlans as plan}
              {@const isSel = selectedStrategyId === plan.id}
              <button
                onclick={() => {
                  selectedStrategyId = plan.id;
                  uncheckedItemIds = {};
                }}
                style="text-align: left; padding: 12px; border-radius: 8px; border: 1px solid {isSel ? '#3b82f6' : '#1f1f26'}; background: {isSel ? '#141418' : '#0f0f12'}; color: #f4f4f5; cursor: pointer; display: flex; flex-direction: column; justify-content: space-between; gap: 10px;"
              >
                <div>
                  <div style="font-size: 12.5px; font-weight: 600; color: {isSel ? '#60a5fa' : '#e4e4e7'}; margin-bottom: 4px;">
                    {plan.name.replace(/^[^\w]+/, '')}
                  </div>
                  <div style="font-size: 11px; color: #71717a; line-height: 1.35;">
                    {plan.tagline}
                  </div>
                </div>

                <div style="display: flex; justify-content: space-between; align-items: center; border-top: 1px solid #1f1f26; padding-top: 8px; font-size: 11px;">
                  <span style="color: #a1a1aa;">{plan.items.length} items</span>
                  {#if plan.saved_tokens > 0}
                    <span style="color: #10b981; font-weight: 600;">-{(plan.saved_tokens / 1000).toFixed(1)}k tok</span>
                  {:else}
                    <span style="color: #60a5fa; font-weight: 600;">+Salience</span>
                  {/if}
                </div>
              </button>
            {/each}
          </div>

          {#if activeStrategyPlan}
            <!-- Section 2: Scoreboard & Action Bar -->
            <div style="margin-bottom: 10px;">
              <h3 style="margin: 0; font-size: 13.5px; font-weight: 600; color: #f4f4f5;">
                2. Projected Impact & Execution
              </h3>
              <p style="margin: 2px 0 0 0; font-size: 11.5px; color: #a1a1aa;">
                Live simulation of your context window, health score, and KV cache prefix before and after applying the selected items.
              </p>
            </div>
            <div style="background: #141418; border: 1px solid #1f1f26; border-radius: 10px; padding: 16px; margin-bottom: 22px;">
              <div style="display: flex; justify-content: space-between; align-items: center; gap: 20px; flex-wrap: wrap;">
                <div style=" display: flex; align-items: center; gap: 24px;">
                  <div>
                    <div style="font-size: 11px; color: #71717a;">Projected Tokens</div>
                    <div style="font-size: 16px; font-weight: 600; color: #f4f4f5; margin-top: 2px;">
                      {liveStrategyScoreboard.tokensBefore.toLocaleString()} → <span style="color: #10b981;">{liveStrategyScoreboard.tokensAfter.toLocaleString()}</span>
                      {#if liveStrategyScoreboard.savedTokens > 0}
                        <span style="font-size: 12px; color: #10b981; margin-left: 4px;">(-{liveStrategyScoreboard.savedPct}%)</span>
                      {/if}
                    </div>
                  </div>

                  <div style="height: 28px; width: 1px; background: #1f1f26;"></div>

                  <div>
                    <div style="font-size: 11px; color: #71717a;">Health Score</div>
                    <div style="font-size: 16px; font-weight: 600; color: #f4f4f5; margin-top: 2px;">
                      {liveStrategyScoreboard.healthBefore} → <span style="color: #10b981;">{liveStrategyScoreboard.healthAfter}</span>
                    </div>
                  </div>

                  <div style="height: 28px; width: 1px; background: #1f1f26;"></div>

                  <div>
                    <div style="font-size: 11px; color: #71717a;">KV Cache Impact</div>
                    <div style="font-size: 13px; font-weight: 500; color: {liveStrategyScoreboard.kvBreakTokens === 0 ? '#10b981' : '#f59e0b'}; margin-top: 4px;">
                      {liveStrategyScoreboard.kvBreakTokens === 0
                        ? 'Estimated break: 0 tokens (not a provider guarantee)'
                        : `Rebuilds ~${liveStrategyScoreboard.kvBreakTokens.toLocaleString()} tok (break-even: ${liveStrategyScoreboard.breakEvenTurns} turn${liveStrategyScoreboard.breakEvenTurns === 1 ? '' : 's'})`}
                    </div>
                  </div>
                </div>

                <div style="display: flex; align-items: center; gap: 8px;">
                  <button
                    onclick={() => (showStrategyDetails = !showStrategyDetails)}
                    style="background: transparent; color: #a1a1aa; border: 1px solid #27272a; border-radius: 6px; padding: 7px 12px; font-size: 12px; cursor: pointer;"
                  >
                    {showStrategyDetails ? 'Hide explanation' : 'How it works'}
                  </button>

                  <button
                    disabled={checkedStrategyItems.length === 0}
                    onclick={() => executeSelectedStrategy('draft')}
                    style="background: #18181b; color: #e4e4e7; border: 1px solid #27272a; border-radius: 6px; padding: 7px 12px; font-size: 12px; font-weight: 500; cursor: {checkedStrategyItems.length === 0 ? 'not-allowed' : 'pointer'};"
                  >
                    Stage Draft
                  </button>

                  <button
                    disabled={checkedStrategyItems.length === 0}
                    onclick={() => executeSelectedStrategy('committed')}
                    style="background: {checkedStrategyItems.length === 0 ? '#27272a' : '#2563eb'}; color: #ffffff; border: none; border-radius: 6px; padding: 7px 14px; font-size: 12px; font-weight: 600; cursor: {checkedStrategyItems.length === 0 ? 'not-allowed' : 'pointer'};"
                  >
                    Apply {checkedStrategyItems.length} Selected
                  </button>
                </div>
              </div>

              {#if showStrategyDetails}
                <div style="margin-top: 14px; padding-top: 14px; border-top: 1px solid #1f1f26; display: grid; grid-template-columns: 1fr 1fr; gap: 16px; font-size: 12.5px; color: #a1a1aa; line-height: 1.5;">
                  <div>
                    <strong style="color: #e4e4e7; display: block; margin-bottom: 4px;">What & How We Do It</strong>
                    {activeStrategyPlan.what_and_how}
                  </div>
                  <div>
                    <strong style="color: #e4e4e7; display: block; margin-bottom: 4px;">Expected Quality & Model Impact</strong>
                    {activeStrategyPlan.expected_quality_impact}
                  </div>
                </div>
              {/if}
            </div>

            <!-- Section 3: Compact 1-Line Checklist Rows (Expandable on click) -->
            <div style="display: flex; justify-content: space-between; align-items: flex-end; margin-bottom: 10px;">
              <div>
                <h3 style="margin: 0; font-size: 13.5px; font-weight: 600; color: #f4f4f5;">
                  3. Candidate Operations Breakdown ({checkedStrategyItems.length} of {activeStrategyPlan.items.length} selected)
                </h3>
                <p style="margin: 2px 0 0 0; font-size: 11.5px; color: #a1a1aa;">
                  Uncheck any individual item you want to keep untouched, or click Details to inspect its exact content.
                </p>
              </div>
              {#if activeStrategyPlan.items.length > 0}
                <div style="display: flex; gap: 6px;">
                  <button
                    onclick={() => (uncheckedItemIds = {})}
                    style="background: transparent; color: #71717a; border: 1px solid #27272a; border-radius: 4px; padding: 2px 8px; font-size: 11px; cursor: pointer;"
                  >
                    Select all
                  </button>
                  <button
                    onclick={() => {
                      const next: Record<string, boolean> = {};
                      for (const item of activeStrategyPlan!.items) next[item.item_id] = true;
                      uncheckedItemIds = next;
                    }}
                    style="background: transparent; color: #71717a; border: 1px solid #27272a; border-radius: 4px; padding: 2px 8px; font-size: 11px; cursor: pointer;"
                  >
                    Clear
                  </button>
                </div>
              {/if}
            </div>

            {#if activeStrategyPlan.items.length === 0}
              <div style="background: #141418; border: 1px solid #1f1f26; color: #10b981; padding: 16px; border-radius: 8px; font-size: 13px;">
                Context is already clean for this strategy.
              </div>
            {:else}
              <div style="display: flex; flex-direction: column; gap: 4px;">
                {#each activeStrategyPlan.items as item}
                  {@const isChecked = !uncheckedItemIds[item.item_id]}
                  {@const isExpanded = !!expandedOptItemIds[item.item_id]}
                  <div style="background: #141418; border: 1px solid #1f1f26; border-radius: 7px; padding: 8px 12px; opacity: {isChecked ? '1' : '0.55'};">
                    <div style="display: flex; align-items: center; justify-content: space-between; gap: 12px;">
                      <div style="display: flex; align-items: center; gap: 10px; min-width: 0; flex: 1;">
                        <input
                          type="checkbox"
                          checked={isChecked}
                          onchange={() => {
                            uncheckedItemIds = { ...uncheckedItemIds, [item.item_id]: isChecked };
                          }}
                          style="width: 14px; height: 14px; cursor: pointer;"
                        />
                        <span style="font-size: 11px; color: #71717a; flex-shrink: 0;">Turn {item.turn_seq}</span>
                        <span style="font-size: 12.5px; color: #e4e4e7; font-weight: 500; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;">
                          {item.title}
                        </span>
                      </div>

                      <div style="display: flex; align-items: center; gap: 12px; font-size: 12px; flex-shrink: 0;">
                        <span style="color: #71717a;">
                          {item.tokens_before.toLocaleString()} → {item.tokens_after.toLocaleString()} tok
                        </span>
                        {#if item.saved_tokens > 0}
                          <span style="color: #10b981; font-weight: 600; min-width: 70px; text-align: right;">
                            -{item.saved_tokens.toLocaleString()} tok
                          </span>
                        {/if}
                        <button
                          onclick={() => (expandedOptItemIds[item.item_id] = !isExpanded)}
                          style="background: transparent; color: #71717a; border: none; font-size: 11px; cursor: pointer;"
                        >
                          {isExpanded ? 'Less' : 'Details'}
                        </button>
                      </div>
                    </div>

                    {#if isExpanded}
                      <div style="margin-top: 8px; padding-top: 8px; border-top: 1px solid #1f1f26; font-size: 12px; color: #a1a1aa; display: flex; flex-direction: column; gap: 4px;">
                        <div><strong style="color: #d4d4d8;">Action:</strong> {item.what_we_do}</div>
                        <div><strong style="color: #d4d4d8;">Reason:</strong> {item.why_it_matters}</div>
                        <div style="color: #71717a; font-family: monospace; font-size: 11px; white-space: pre-wrap; word-break: break-word;">
                          {item.content_preview}
                        </div>
                      </div>
                    {/if}
                  </div>
                {/each}
              </div>
            {/if}
          {/if}

        {:else if activeTab === 'analytics'}
          <!-- VIEW 3: ANALYTICS & RADIX TREE (Focused Sub-Views) -->
          {#if analyticsSubView === 'treemap'}
            <div style="display: grid; grid-template-columns: 540px 1fr; gap: 20px; align-items: start; margin-bottom: 24px;">
              <div>
                <div style="font-size: 13px; font-weight: 600; color: #e4e4e7; margin-bottom: 10px;">
                  Proportional Token Treemap
                </div>
                <svg width="540" height="290" style="background: #141418; border-radius: 8px; border: 1px solid #1f1f26; max-width: 100%;">
                  {#each treemapLeaves as leaf}
                    <g
                      role="button"
                      tabindex="0"
                      transform="translate({leaf.x0},{leaf.y0})"
                      onmouseenter={() => (hoveredTreemapName = leaf.data.name)}
                      onmouseleave={() => (hoveredTreemapName = null)}
                      onclick={() => (hoveredTreemapName = leaf.data.name)}
                      onkeydown={(e) => e.key === 'Enter' && (hoveredTreemapName = leaf.data.name)}
                      style="cursor: pointer;"
                    >
                      <rect
                        width={Math.max(0, leaf.x1 - leaf.x0)}
                        height={Math.max(0, leaf.y1 - leaf.y0)}
                        fill={leaf.data.color}
                        stroke={hoveredTreemapName === leaf.data.name ? '#ffffff' : '#09090b'}
                        stroke-width={hoveredTreemapName === leaf.data.name ? '2' : '1'}
                        rx="4"
                        opacity={hoveredTreemapName && hoveredTreemapName !== leaf.data.name ? '0.45' : '0.88'}
                      />
                      {#if leaf.x1 - leaf.x0 > 70 && leaf.y1 - leaf.y0 > 32}
                        <text x="8" y="18" fill="#ffffff" font-size="11.5" font-weight="600">{leaf.data.name}</text>
                        <text x="8" y="33" fill="#e4e4e7" font-size="10.5">{leaf.value?.toLocaleString()} tok ({leaf.data.pct}%)</text>
                      {/if}
                    </g>
                  {/each}
                </svg>
              </div>

              <div>
                <div style="font-size: 13px; font-weight: 600; color: #e4e4e7; margin-bottom: 10px;">
                  Category Breakdown
                </div>
                <div style="background: #141418; border: 1px solid #1f1f26; border-radius: 8px; padding: 8px; display: flex; flex-direction: column; gap: 4px;">
                  {#each treemapCategories as cat}
                    <div
                      role="button"
                      tabindex="0"
                      onmouseenter={() => (hoveredTreemapName = cat.name)}
                      onmouseleave={() => (hoveredTreemapName = null)}
                      style="padding: 7px 10px; border-radius: 6px; background: {hoveredTreemapName === cat.name ? '#1f1f26' : 'transparent'}; display: flex; justify-content: space-between; align-items: center; font-size: 12px;"
                    >
                      <div style="display: flex; align-items: center; gap: 8px;">
                        <span style="width: 8px; height: 8px; border-radius: 2px; background: {cat.color};"></span>
                        <span style="color: #e4e4e7; font-weight: 500;">{cat.name}</span>
                      </div>
                      <div style="display: flex; align-items: center; gap: 10px;">
                        <span style="color: #a1a1aa;">{cat.value.toLocaleString()} tok</span>
                        <span style="color: #71717a; width: 42px; text-align: right;">{cat.pct}%</span>
                      </div>
                    </div>
                  {/each}
                </div>
              </div>
            </div>

            <!-- KV Cache Tape & Attention Curve (Click-Only Selection + Wrapping Detail Card) -->
            <div style="background: #141418; padding: 16px; border-radius: 10px; border: 1px solid #1f1f26;">
              <div style="font-size: 13px; font-weight: 600; color: #e4e4e7; margin-bottom: 10px;">
                KV Cache Prefix Tape & Positional Attention Curve (Click any bar to select and inspect below)
              </div>

              <div style="display: flex; height: 22px; border-radius: 4px; overflow: hidden; margin-bottom: 14px;">
                {#each latestTurn.nodes as n}
                  <div
                    role="button"
                    tabindex="0"
                    onclick={() => {
                      hoveredCandleNodeId = n.id;
                      selectedNodeId = n.id;
                    }}
                    onkeydown={(e) => {
                      if (e.key === 'Enter') {
                        hoveredCandleNodeId = n.id;
                        selectedNodeId = n.id;
                      }
                    }}
                    title={`${humanToolActionLabel(n)} (${n.token_span.count} tok)`}
                    style="flex: {Math.max(1, n.token_span.count)}; background: {kvColor(n.kv_status)}; border-right: 1px solid #09090b; outline: {activeCandleNode?.id === n.id ? '2px solid #ffffff' : 'none'}; opacity: {activeCandleNode?.id === n.id ? '1' : '0.78'}; cursor: pointer;"
                  ></div>
                {/each}
              </div>

              <div style="display: flex; align-items: flex-end; height: 74px; gap: 2px; border-bottom: 1px solid #27272a;">
                {#each latestTurn.nodes as n}
                  <div
                    role="button"
                    tabindex="0"
                    onclick={() => {
                      hoveredCandleNodeId = n.id;
                      selectedNodeId = n.id;
                    }}
                    onkeydown={(e) => {
                      if (e.key === 'Enter') {
                        hoveredCandleNodeId = n.id;
                        selectedNodeId = n.id;
                      }
                    }}
                    title={`Turn ${n.turn_seq}: ${humanToolActionLabel(n)} (${n.token_span.count} tok · Salience ${(n.attention_salience * 100).toFixed(0)}%)`}
                    style="flex: 1; height: {Math.max(8, Math.round(n.attention_salience * 68))}px; background: {activeCandleNode?.id === n.id ? '#ffffff' : n.attention_salience < 0.4 ? '#d97706' : '#3b82f6'}; border-radius: 2px 2px 0 0; cursor: pointer;"
                  ></div>
                {/each}
              </div>

              {#if activeCandleNode}
                <div style="margin-top: 14px; padding: 14px 16px; border-radius: 8px; background: #0f0f12; border: 1px solid #27272a; display: flex; flex-direction: column; gap: 10px; font-size: 12.5px;">
                  <div style="display: flex; justify-content: space-between; align-items: center; gap: 12px; flex-wrap: wrap; border-bottom: 1px solid #1f1f26; padding-bottom: 8px;">
                    <div style="display: flex; align-items: center; gap: 8px; flex-wrap: wrap;">
                      <span style="padding: 2px 7px; border-radius: 4px; background: #27272a; color: #f4f4f5; font-size: 11px; font-weight: 600;">
                        Turn {activeCandleNode.turn_seq}
                      </span>
                      <strong style="color: #ffffff; font-size: 13px;">{humanToolActionLabel(activeCandleNode)}</strong>
                      <span style="color: #60a5fa; font-weight: 600;">{activeCandleNode.token_span.count.toLocaleString()} tok</span>
                      <span style="color: #a1a1aa;">· Salience {(activeCandleNode.attention_salience * 100).toFixed(0)}%</span>
                      <span style="color: {kvColor(activeCandleNode.kv_status)}; font-size: 11.5px;">· {activeCandleNode.kv_status}</span>
                    </div>
                    <button
                      onclick={() => inspectNode(activeCandleNode!.id)}
                      style="background: #27272a; color: #ffffff; border: 1px solid #3f3f46; border-radius: 5px; padding: 5px 10px; font-size: 11.5px; cursor: pointer; flex-shrink: 0;"
                    >
                      Open in Inspector Drawer
                    </button>
                  </div>
                  <div style="color: #d4d4d8; line-height: 1.55; white-space: pre-wrap; word-break: break-word; overflow-wrap: anywhere;">
                    <MarkdownText text={nodeDisplayText(activeCandleNode)} rawMode={rawMarkdownMode} maxHeight="none" compact={true} />
                  </div>
                </div>
              {/if}
            </div>

          {:else if analyticsSubView === 'advisor'}
            <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 12px;">
              <span style="font-size: 13px; font-weight: 600; color: #e4e4e7;">
                Prunable Tool Outputs & Redundant Blocks ({pruneAdvisorItems.length})
              </span>
              {#if pruneAdvisorItems.length > 0}
                <button
                  onclick={bulkCutSafeItems}
                  style="background: #dc2626; color: white; border: none; border-radius: 6px; padding: 6px 12px; font-size: 12px; font-weight: 600; cursor: pointer;"
                >
                  Cut All {pruneAdvisorItems.length} Items (-{totalSafeToCutTokens.toLocaleString()} tok)
                </button>
              {/if}
            </div>

            <div style="display: flex; flex-direction: column; gap: 6px;">
              {#each pruneAdvisorItems as item}
                <div style="background: #141418; border: 1px solid #1f1f26; border-radius: 7px; padding: 10px 14px; display: flex; justify-content: space-between; align-items: center; gap: 14px;">
                  <div style="min-width: 0; flex: 1;">
                    <div style="display: flex; align-items: center; gap: 8px; font-size: 12.5px;">
                      <span style="color: #71717a;">Turn {item.node.turn_seq}</span>
                      <span style="font-size: 11px; padding: 1px 6px; border-radius: 4px; background: #27272a; color: #fbbf24;">{item.verdictBadge}</span>
                      <strong style="color: #e4e4e7;">{item.label}</strong>
                      <span style="color: #60a5fa;">{item.tokens.toLocaleString()} tok</span>
                    </div>
                    <div style="font-size: 11.5px; color: #71717a; margin-top: 3px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;">
                      {item.reason} — <code>{item.preview}</code>
                    </div>
                  </div>
                  <button
                    onclick={() =>
                      applyMutation(
                        {
                          op: 'prune_or_stub',
                          target_fingerprint: item.node.fingerprint,
                          target_call_id: null,
                          target_json_pointer: item.node.raw_json_pointer,
                          stub_text: `[cw:pruned original_tokens=${item.tokens} label="${item.label}"]`
                        },
                        'committed',
                        `Cut ${item.label} (-${item.tokens} tok)`
                      )}
                    style="background: #18181b; color: #fca5a5; border: 1px solid #7f1d1d; border-radius: 5px; padding: 5px 10px; font-size: 11.5px; cursor: pointer; flex-shrink: 0;"
                  >
                    Cut (-{item.tokens} tok)
                  </button>
                </div>
              {/each}
            </div>

          {:else if analyticsSubView === 'radix'}
            <div style="display: flex; flex-direction: column; gap: 4px;">
              {#each graphNodes as gnode}
                <div
                  role="button"
                  tabindex="0"
                  onclick={() => inspectNode(gnode.node_id)}
                  onkeydown={(e) => e.key === 'Enter' && inspectNode(gnode.node_id)}
                  style="display: flex; align-items: center; justify-content: space-between; padding: 8px 12px; border-radius: 6px; background: #141418; border: 1px solid {selectedNodeId === gnode.node_id && gnode.node_id ? '#3b82f6' : '#1f1f26'}; cursor: pointer; margin-left: {gnode.group === 'root' ? 0 : gnode.group === 'fork' ? 32 : 14}px; font-size: 12px;"
                >
                  <div style="display: flex; align-items: center; gap: 8px;">
                    <span style="width: 6px; height: 6px; border-radius: 999px; background: {kvColor(gnode.kv_status)};"></span>
                    <span style="color: #71717a; font-size: 11px;">{gnode.group}</span>
                    <span style="color: #e4e4e7; font-weight: 500;">{gnode.label}</span>
                    {#each gnode.lint_badges as badge}
                      <span style="font-size: 10.5px; padding: 1px 6px; border-radius: 4px; background: #272011; color: #fbbf24;">
                        {badge}
                      </span>
                    {/each}
                  </div>
                  <div style="display: flex; align-items: center; gap: 10px; font-size: 11px; color: #71717a;">
                    {#if gnode.raw_json_pointer}
                      <code>{gnode.raw_json_pointer}</code>
                    {/if}
                    <span style="color: {kvColor(gnode.kv_status)};">{gnode.kv_status}</span>
                  </div>
                </div>
              {/each}
            </div>
          {/if}

        {:else if activeTab === 'settings'}
          <!-- VIEW 4: LOCAL GPU, AUTO-PILOT, TOOLS & OVERLAYS (Focused Sub-Views) -->
          {#if settingsSubView === 'local'}
            {#if localModelAnalysis}
              <div style="display: grid; grid-template-columns: repeat(4, 1fr); gap: 12px; margin-bottom: 18px;">
                <div style="background: #141418; border: 1px solid #1f1f26; border-radius: 8px; padding: 12px;">
                  <div style="font-size: 11px; color: #71717a;">Engine & Slot Affinity</div>
                  <div style="font-size: 15px; font-weight: 600; color: #f4f4f5; margin-top: 4px;">
                    {localModelAnalysis.engine_kind.toUpperCase()} · Slot #{localModelAnalysis.recommended_slot_id}
                  </div>
                  <div style="font-size: 11px; color: #71717a; margin-top: 4px;">
                    Block size: {localModelAnalysis.paged_attention_block_size} tok
                  </div>
                </div>

                <div style="background: #141418; border: 1px solid #1f1f26; border-radius: 8px; padding: 12px;">
                  <div style="font-size: 11px; color: #71717a;">System Prefix Lock (n_keep)</div>
                  <div style="font-size: 15px; font-weight: 600; color: #60a5fa; margin-top: 4px;">
                    {localModelAnalysis.recommended_n_keep_tokens.toLocaleString()} tok pinned
                  </div>
                  <div style="font-size: 11px; color: #10b981; margin-top: 4px;">
                    Auto-injected on /ollama & /llamacpp
                  </div>
                </div>

                <div style="background: #141418; border: 1px solid #1f1f26; border-radius: 8px; padding: 12px;">
                  <div style="font-size: 11px; color: #71717a;">KV Cache VRAM Footprint</div>
                  <div style="font-size: 13.5px; font-weight: 600; color: #f4f4f5; margin-top: 4px;">
                    FP16: {localModelAnalysis.vram_fp16_mb.toFixed(0)}M · Q8: <span style="color: #10b981;">{localModelAnalysis.vram_q8_0_mb.toFixed(0)}M</span> · Q4: {localModelAnalysis.vram_q4_0_mb.toFixed(0)}M
                  </div>
                  <div style="font-size: 11px; color: #71717a; margin-top: 4px;">
                    OLLAMA_KV_CACHE_TYPE=q8_0
                  </div>
                </div>

                <div style="background: #141418; border: 1px solid #1f1f26; border-radius: 8px; padding: 12px;">
                  <div style="font-size: 11px; color: #71717a;">Window Utilization</div>
                  <div style="font-size: 15px; font-weight: 600; color: {localModelAnalysis.sliding_window_overflow_risk ? '#ef4444' : '#10b981'}; margin-top: 4px;">
                    {localModelAnalysis.used_ctx_pct.toFixed(1)}% of {localModelAnalysis.num_ctx_limit.toLocaleString()}
                  </div>
                  <div style="font-size: 11px; color: #71717a; margin-top: 4px;">
                    Recommended num_ctx: {localModelAnalysis.recommended_num_ctx.toLocaleString()}
                  </div>
                </div>
              </div>

              <div style="display: flex; flex-direction: column; gap: 8px;">
                {#each localModelAnalysis.recommendations as rec}
                  <div style="background: #141418; border: 1px solid #1f1f26; border-radius: 8px; padding: 12px 14px;">
                    <div style="display: flex; justify-content: space-between; align-items: center;">
                      <strong style="font-size: 13px; color: #e4e4e7;">{rec.title}</strong>
                      <span style="font-size: 11px; color: #71717a; text-transform: uppercase;">{rec.severity}</span>
                    </div>
                    <div style="font-size: 12px; color: #a1a1aa; margin-top: 4px; line-height: 1.45;">
                      {rec.explanation}
                    </div>
                    <div style="margin-top: 8px; background: #0f0f12; padding: 6px 10px; border-radius: 5px; border: 1px solid #1f1f26; font-family: monospace; font-size: 11px; color: #60a5fa;">
                      {rec.cli_or_api_fix}
                    </div>
                  </div>
                {/each}
              </div>
            {/if}

          {:else if settingsSubView === 'autopilot'}
            {#if autopilotConfig}
              <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 12px; margin-bottom: 18px;">
                {#each [
                  ['local_model_window_guard', 'Local Model Sliding-Window Guard (>85% num_ctx)', 'Advisory window threshold; automatic trajectory folds are disabled.'],
                  ['auto_cache_safe_pin', `Optional Rule Reminder (>${autopilotConfig.pin_min_context_tokens.toLocaleString()} tok)`, 'Adds a heuristic reminder when explicitly enabled; it may change the cached prefix.'],
                  ['auto_diet_at_breakpoint', `Auto Diet at Breakpoint (>${autopilotConfig.breakpoint_tokens.toLocaleString()} tok)`, 'Triggers Surgical Log Diet + Clean Code State automatically on deep sessions.']
                ] as [key, title, desc]}
                  <label style="display: flex; align-items: flex-start; gap: 10px; background: #141418; border: 1px solid #1f1f26; border-radius: 8px; padding: 12px 14px; cursor: pointer;">
                    <input
                      type="checkbox"
                      checked={(autopilotConfig as any)[key]}
                      onchange={(e) =>
                        saveAutoPilotConfig({
                          ...autopilotConfig!,
                          [key]: (e.currentTarget as HTMLInputElement).checked
                        })}
                      style="margin-top: 3px; width: 15px; height: 15px;"
                    />
                    <div>
                      <div style="font-size: 13px; font-weight: 600; color: #e4e4e7;">{title}</div>
                      <div style="font-size: 12px; color: #71717a; margin-top: 4px; line-height: 1.45;">{desc}</div>
                    </div>
                  </label>
                {/each}
              </div>
            {/if}

            <div style="background: #141418; border: 1px solid #1f1f26; border-radius: 8px; padding: 14px; display: flex; justify-content: space-between; align-items: center;">
              <div>
                <div style="font-size: 13px; font-weight: 600; color: #e4e4e7;">On-Disk Transcript Sync-Back</div>
                <div style="font-size: 12px; color: #71717a; margin-top: 2px;">
                  Source: <code>{transcriptSourcePath || 'In-memory / Proxy Session'}</code>
                </div>
              </div>
              <button
                onclick={syncTranscriptToDisk}
                style="background: #18181b; color: #10b981; border: 1px solid #065f46; border-radius: 6px; padding: 6px 12px; font-size: 12px; font-weight: 500; cursor: pointer;"
              >
                Sync Mutations to Disk (.bak)
              </button>
            </div>

          {:else if settingsSubView === 'tools'}
            <div style="font-size: 13px; font-weight: 600; color: #e4e4e7; margin-bottom: 10px;">
              Workspace Files in Context ({fileStates.length})
            </div>
            <div style="display: flex; flex-direction: column; gap: 6px; margin-bottom: 24px;">
              {#each fileStates as fs}
                <div style="display: flex; justify-content: space-between; align-items: center; padding: 9px 14px; background: #141418; border: 1px solid #1f1f26; border-radius: 7px; font-size: 12px;">
                  <div style="display: flex; align-items: center; gap: 10px;">
                    <span style="width: 6px; height: 6px; border-radius: 999px; background: {fs.freshness === 'fresh' ? '#10b981' : '#f59e0b'};"></span>
                    <code style="color: #e4e4e7;">{fs.path}</code>
                    <span style="color: #71717a;">Read {fs.read_count}x (Turn {fs.last_read_turn}) · {fs.total_tokens_in_context} tok</span>
                  </div>
                  {#if fs.freshness === 'stale' || fs.freshness === 'duplicate'}
                    <button
                      onclick={() =>
                        applyMutation(
                          { op: 'shadow_dedupe', file_path: fs.path },
                          'committed',
                          `Shadow-deduplicated ${fs.path}`
                        )}
                      style="background: #18181b; color: #fb923c; border: 1px solid #7c2d12; border-radius: 5px; padding: 4px 10px; font-size: 11.5px; cursor: pointer;"
                    >
                      Deduplicate
                    </button>
                  {/if}
                </div>
              {/each}
            </div>

            <div style="font-size: 13px; font-weight: 600; color: #e4e4e7; margin-bottom: 10px;">
              MCP & Native Tool Schemas ({mcpToolsList.length})
            </div>
            <div style="display: flex; flex-direction: column; gap: 6px;">
              {#each mcpToolsList as tool}
                <div style="display: flex; justify-content: space-between; align-items: center; padding: 9px 14px; background: #141418; border: 1px solid #1f1f26; border-radius: 7px; font-size: 12px;">
                  <div>
                    <span style="color: #60a5fa; margin-right: 8px;">{tool.server}</span>
                    <strong style="color: #e4e4e7;">{tool.name}</strong>
                    <span style="color: #71717a; margin-left: 8px;">{tool.description}</span>
                  </div>
                  <div style="display: flex; align-items: center; gap: 10px;">
                    <span style="color: #71717a;">{tool.tokens} tok</span>
                    <button
                      onclick={() =>
                        applyMutation(
                          {
                            op: 'gate_tools_and_skills',
                            disabled_tools: [tool.name],
                            disabled_mcp_servers: [],
                            disabled_skills: []
                          },
                          'committed',
                          `Disabled tool '${tool.name}'`
                        )}
                      style="background: #18181b; color: #fca5a5; border: 1px solid #7f1d1d; border-radius: 5px; padding: 4px 10px; font-size: 11.5px; cursor: pointer;"
                    >
                      Gate Off
                    </button>
                  </div>
                </div>
              {/each}
            </div>

          {:else if settingsSubView === 'overlays'}
            <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 14px;">
              <span style="font-size: 13px; font-weight: 600; color: #e4e4e7;">
                Active Mutation Overlays ({mutations.length})
              </span>
              <button
                onclick={triggerForkReplay}
                style="background: #18181b; color: #c084fc; border: 1px solid #3b0764; border-radius: 6px; padding: 6px 12px; font-size: 12px; font-weight: 500; cursor: pointer;"
              >
                Fork & Run A/B Replay
              </button>
            </div>

            {#if replayComparison}
              <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 12px; margin-bottom: 16px; font-size: 12.5px;">
                <div style="background: #141418; padding: 12px; border-radius: 8px; border: 1px solid #1f1f26;">
                  <div style="color: #71717a; margin-bottom: 4px;">Original Turn #{replayComparison.original_turn.sequence_number}</div>
                  <div>Tokens: <strong>{replayComparison.original_turn.token_usage.input_tokens.toLocaleString()}</strong> · Health: <strong>{replayComparison.original_turn.health_score}</strong></div>
                </div>
                <div style="background: #0d281e; padding: 12px; border-radius: 8px; border: 1px solid #065f46;">
                  <div style="color: #34d399; margin-bottom: 4px;">Forked Replay ({replayComparison.forked_turn.id})</div>
                  <div>Tokens: <strong>{replayComparison.forked_turn.token_usage.input_tokens.toLocaleString()}</strong> (-{replayComparison.saved_tokens.toLocaleString()}) · Health: <strong>{replayComparison.forked_turn.health_score}</strong></div>
                </div>
              </div>
            {/if}

            <div style="display: flex; flex-direction: column; gap: 6px;">
              {#each mutations as m}
                <div style="display: flex; justify-content: space-between; align-items: center; padding: 10px 14px; background: #141418; border: 1px solid #1f1f26; border-radius: 7px; font-size: 12px;">
                  <div>
                    <span style="font-size: 10.5px; padding: 2px 6px; border-radius: 4px; background: #18181b; color: {m.status === 'committed' ? '#10b981' : '#f59e0b'}; margin-right: 8px;">
                      {m.status}
                    </span>
                    <strong style="color: #e4e4e7;">{m.op.op}</strong>
                    <span style="color: #71717a; margin-left: 8px;">
                      Saved: {m.estimated_saved_tokens} tok · KV Impact: {m.estimated_kv_break_tokens} tok
                    </span>
                  </div>
                  <button
                    onclick={() => deleteMutation(m.id)}
                    style="background: transparent; color: #a1a1aa; border: 1px solid #27272a; border-radius: 5px; padding: 4px 10px; font-size: 11.5px; cursor: pointer;"
                  >
                    Revert
                  </button>
                </div>
              {/each}
            </div>
          {/if}
        {/if}
      </main>
    </div>

    <!-- [3] Slide-Over Right Inspector Drawer (Only visible when inspectorOpen is true) -->
    {#if inspectorOpen}
      <aside style="background: #0f0f12; border-left: 1px solid #1f1f26; padding: 16px; overflow-y: auto; display: flex; flex-direction: column; gap: 16px;">
        <div style="display: flex; justify-content: space-between; align-items: center; border-bottom: 1px solid #1f1f26; padding-bottom: 10px;">
          <strong style="font-size: 13px; color: #f4f4f5;">Inspector & Surgical Actions</strong>
          <button
            onclick={() => (inspectorOpen = false)}
            style="background: transparent; border: none; color: #71717a; cursor: pointer; font-size: 14px;"
          >
            ✕
          </button>
        </div>

        {#if selectedNode}
          <div style="font-size: 11.5px; color: #a1a1aa; display: flex; flex-direction: column; gap: 5px; background: #141418; padding: 10px 12px; border-radius: 6px; border: 1px solid #1f1f26;">
            <div><span style="color: #71717a;">Node:</span> <code>{selectedNode.id}</code> (Turn {selectedNode.turn_seq})</div>
            <div><span style="color: #71717a;">Span:</span> {selectedNode.token_span.start}..{selectedNode.token_span.end} ({selectedNode.token_span.count} tok)</div>
            <div><span style="color: #71717a;">Salience:</span> {(selectedNode.attention_salience * 100).toFixed(0)}%</div>
            <div><span style="color: #71717a;">Pointer:</span> <code>{selectedNode.raw_json_pointer}</code></div>
          </div>

          <div style="display: flex; flex-direction: column; gap: 6px;">
            <button
              onclick={() =>
                applyMutation(
                  {
                    op: 'prune_or_stub',
                    target_fingerprint: selectedNode!.fingerprint,
                    target_call_id: null,
                    target_json_pointer: selectedNode!.raw_json_pointer,
                    stub_text: `[cw:pruned original_tokens=${selectedNode!.token_span.count}]`
                  },
                  'committed',
                  `Pruned node ${selectedNode!.id}`
                )}
              style="background: #18181b; color: #fca5a5; border: 1px solid #7f1d1d; border-radius: 6px; padding: 7px 10px; font-size: 12px; cursor: pointer; text-align: left;"
            >
              Prune / Stub Node (-{selectedNode.token_span.count} tok)
            </button>

            <button
              onclick={() =>
                applyMutation(
                  {
                    op: 'prune_or_stub',
                    target_fingerprint: selectedNode!.fingerprint,
                    target_call_id: null,
                    target_json_pointer: selectedNode!.raw_json_pointer,
                    stub_text: `[cw:pruned original_tokens=${selectedNode!.token_span.count}]`
                  },
                  'draft',
                  `Staged Prune for ${selectedNode!.id}`
                )}
              style="background: #18181b; color: #fcd34d; border: 1px solid #78350f; border-radius: 6px; padding: 7px 10px; font-size: 12px; cursor: pointer; text-align: left;"
            >
              Stage Prune in Draft Queue
            </button>

            <button
              onclick={() =>
                applyMutation(
                  { op: 'shadow_dedupe', file_path: null },
                  'committed',
                  'Shadow-deduplicated stale file reads'
                )}
              style="background: #18181b; color: #e4e4e7; border: 1px solid #27272a; border-radius: 6px; padding: 7px 10px; font-size: 12px; cursor: pointer; text-align: left;"
            >
              Shadow-Dedupe Stale File Reads
            </button>
          </div>

          <div style="background: #141418; padding: 10px 12px; border-radius: 6px; border: 1px solid #1f1f26;">
            <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 6px; font-size: 11px; color: #71717a;">
              <span>Content Preview</span>
              <button
                onclick={() => (rawMarkdownMode = !rawMarkdownMode)}
                style="background: #1e202b; color: #cbd5e1; border: 1px solid #2e3244; border-radius: 4px; padding: 2px 6px; font-size: 10px; cursor: pointer;"
              >
                {rawMarkdownMode ? 'Show Formatted MD' : 'Show Raw'}
              </button>
            </div>
            <MarkdownText text={nodeDisplayText(selectedNode)} rawMode={rawMarkdownMode} maxHeight="260px" compact={true} />
          </div>
        {/if}

        <!-- Subtree Folding -->
        <div style="border-top: 1px solid #1f1f26; padding-top: 12px;">
          <div style="font-size: 12px; font-weight: 600; color: #e4e4e7; margin-bottom: 8px;">Fold Turn Range</div>
          <div style="display: flex; gap: 6px; margin-bottom: 6px; font-size: 12px; align-items: center;">
            <input type="number" bind:value={foldStart} min="1" style="width: 44px; background: #141418; color: white; border: 1px solid #27272a; border-radius: 4px; padding: 4px;" />
            <span style="color: #71717a;">to</span>
            <input type="number" bind:value={foldEnd} min="1" style="width: 44px; background: #141418; color: white; border: 1px solid #27272a; border-radius: 4px; padding: 4px;" />
            <button
              onclick={() =>
                applyMutation(
                  { op: 'fold_subtree', start_turn: foldStart, end_turn: foldEnd, summary_text: foldSummary },
                  'committed',
                  `Folded turns ${foldStart}..${foldEnd}`
                )}
              style="background: #2563eb; color: white; border: none; border-radius: 4px; padding: 5px 10px; font-size: 11.5px; cursor: pointer;"
            >
              Fold
            </button>
          </div>
          <input
            type="text"
            bind:value={foldSummary}
            style="width: 100%; box-sizing: border-box; background: #141418; color: #e4e4e7; border: 1px solid #27272a; border-radius: 4px; padding: 5px 8px; font-size: 11.5px;"
          />
        </div>

        <!-- Tail Pinning -->
        <div style="border-top: 1px solid #1f1f26; padding-top: 12px;">
          <div style="font-size: 12px; font-weight: 600; color: #e4e4e7; margin-bottom: 8px;">Pin to Tail (0% KV Break)</div>
          <div style="display: flex; gap: 6px; margin-bottom: 6px;">
            <input
              type="text"
              bind:value={pinLabel}
              placeholder="Label"
              style="flex: 1; background: #141418; color: white; border: 1px solid #27272a; border-radius: 4px; padding: 5px 8px; font-size: 11.5px;"
            />
            <button
              onclick={() =>
                applyMutation(
                  { op: 'pin_context', label: pinLabel, content: pinContent, source_node_id: selectedNode?.id ?? null },
                  'committed',
                  `Pinned '${pinLabel}' to prompt tail`
                )}
              style="background: #059669; color: white; border: none; border-radius: 4px; padding: 5px 10px; font-size: 11.5px; cursor: pointer;"
            >
              Pin
            </button>
          </div>
          <input
            type="text"
            bind:value={pinContent}
            style="width: 100%; box-sizing: border-box; background: #141418; color: #e4e4e7; border: 1px solid #27272a; border-radius: 4px; padding: 5px 8px; font-size: 11.5px;"
          />
        </div>

        <!-- Lints -->
        {#if latestTurn && latestTurn.lint_findings.length > 0}
          <div style="border-top: 1px solid #1f1f26; padding-top: 12px;">
            <div style="font-size: 12px; font-weight: 600; color: #e4e4e7; margin-bottom: 8px;">
              Diagnostics ({latestTurn.lint_findings.length})
            </div>
            {#each latestTurn.lint_findings as lint}
              <div style="background: #141418; border: 1px solid #1f1f26; padding: 8px 10px; border-radius: 6px; margin-bottom: 6px; font-size: 11.5px;">
                <div style="display: flex; justify-content: space-between; align-items: center;">
                  <strong style="color: #e4e4e7;">{lint.rule_id} · {lint.title}</strong>
                  {#if lint.suggested_mutation}
                    <button
                      onclick={() => applyMutation(lint.suggested_mutation!, 'committed', `Fixed ${lint.rule_id}`)}
                      style="background: #2563eb; color: white; border: none; border-radius: 4px; padding: 2px 7px; font-size: 10.5px; cursor: pointer;"
                    >
                      Fix
                    </button>
                  {/if}
                </div>
                <div style="color: #71717a; margin-top: 3px;">{lint.message}</div>
              </div>
            {/each}
          </div>
        {/if}
      </aside>
    {/if}
  </div>
</div>
