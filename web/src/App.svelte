<script lang="ts">
  import { hierarchy, treemap } from 'd3-hierarchy';
  import HelpTip from './lib/HelpTip.svelte';
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
  let costAndPerf = $state<CostAndPerfReport | null>(null);
  let localModelAnalysis = $state<LocalModelAnalysis | null>(null);
  let autopilotConfig = $state<AutoPilotConfig | null>(null);
  let transcriptSourcePath = $state<string | null>(null);

  let activeTab = $state<'story' | 'optimize' | 'local' | 'radix' | 'treemap' | 'tools' | 'replay'>('story');
  let selectedNodeId = $state<string | null>(null);
  let wsConnected = $state<boolean>(false);
  let statusBanner = $state<string>('');
  let replayComparison = $state<{ original_turn: ContextTurn; forked_turn: ContextTurn; saved_tokens: number } | null>(null);

  // Project & Session Sidebar filters
  let searchQuery = $state<string>('');
  let selectedProjectFilter = $state<string>('ALL');
  let collapsedProjects = $state<Record<string, boolean>>({});

  // Interactive hover states for Treemap & Attention Candles
  let hoveredTreemapName = $state<string | null>(null);
  let hoveredCandleNodeId = $state<string | null>(null);

  // Fold form state
  let foldStart = $state<number>(1);
  let foldEnd = $state<number>(2);
  let foldSummary = $state<string>('Tried debugging branch; resolved root cause in config.');

  // Pin form state
  let pinLabel = $state<string>('Architecture Rule');
  let pinContent = $state<string>('Preserve lossless JSON pointers and KV cache prefix boundaries.');

  function projectNameFromWorkspace(ws: string | null | undefined): string {
    if (!ws || !ws.trim()) return 'General Chat (No Project)';
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
      minute: '2-digit',
      second: '2-digit'
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

  // Group sessions by project name
  let projectGroups = $derived.by(() => {
    const q = searchQuery.trim().toLowerCase();
    const groups = new Map<string, { projectName: string; workspacePath: string | null; sessions: Session[]; latestUpdated: string }>();

    for (const s of sessions) {
      const proj = projectNameFromWorkspace(s.workspace_root);
      if (selectedProjectFilter !== 'ALL' && proj !== selectedProjectFilter) continue;
      if (q) {
        const hay = `${s.title || ''} ${proj} ${s.workspace_root || ''} ${s.id} ${s.model} ${s.harness}`.toLowerCase();
        if (!hay.includes(q)) continue;
      }
      if (!groups.has(proj)) {
        groups.set(proj, {
          projectName: proj,
          workspacePath: s.workspace_root,
          sessions: [],
          latestUpdated: s.updated_at
        });
      }
      const g = groups.get(proj)!;
      g.sessions.push(s);
      if (s.updated_at > g.latestUpdated) {
        g.latestUpdated = s.updated_at;
      }
    }

    return Array.from(groups.values()).sort((a, b) => {
      // Put real project folders before "General Chat (No Project)"
      if (a.projectName === 'General Chat (No Project)' && b.projectName !== 'General Chat (No Project)') return 1;
      if (b.projectName === 'General Chat (No Project)' && a.projectName !== 'General Chat (No Project)') return -1;
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

  let activeSession = $derived<Session | null>(sessions.find((s) => s.id === activeSessionId) ?? null);

  let selectedNode = $derived<ContextNode | null>(
    latestTurn?.nodes.find((n) => n.id === selectedNodeId) ?? latestTurn?.nodes[0] ?? null
  );

  let draftMutations = $derived(mutations.filter((m) => m.status === 'draft'));

  // Group conversation nodes into chronological Human Story Turn Cards
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
        color: '#eab308',
        whatMatters: 'Only the latest command output or error summary matters; old verbose logs/stack traces from past turns are prime candidates to cut!',
        canCut: 'High (Safe to prune old logs)'
      },
      {
        name: 'Assistant Thinking',
        value: cb.assistant_thinking,
        color: '#ec4899',
        whatMatters: 'Internal model reasoning helped produce past answers, but old thinking blocks from earlier turns rarely help future turns.',
        canCut: 'High (Safe to prune old thoughts)'
      },
      {
        name: 'File Reads (Fresh)',
        value: cb.file_reads_fresh,
        color: '#22c55e',
        whatMatters: 'Latest code state of files the agent is actively working on. Keep these unless you moved to an unrelated task.',
        canCut: 'Low (Keep active files)'
      },
      {
        name: 'File Reads (Stale/Dup)',
        value: cb.file_reads_stale_or_duplicate,
        color: '#f97316',
        whatMatters: 'Outdated or duplicate file contents that were superseded by later edits or re-reads. Keeping them confuses the model!',
        canCut: '100% Safe to Cut (Shadow-Dedupe)'
      },
      {
        name: 'MCP & Native Tools',
        value: cb.mcp_and_native_tools,
        color: '#6366f1',
        whatMatters: 'JSON schemas for tools. Gate off MCP servers you are not using in this session.',
        canCut: 'High (Gate unused MCP tools)'
      },
      {
        name: 'Rules & Instructions',
        value: cb.rules_and_instructions,
        color: '#14b8a6',
        whatMatters: 'Project guidelines (AGENTS.md, CLAUDE.md, user rules). Essential for coding standards—pin if attention drops.',
        canCut: 'Keep (Core instructions)'
      },
      {
        name: 'System & Identity',
        value: cb.system_and_identity,
        color: '#0ea5e9',
        whatMatters: 'Base harness system prompt and identity instructions.',
        canCut: 'Keep (System core)'
      },
      {
        name: 'Skills Catalog',
        value: cb.skills_catalog,
        color: '#8b5cf6',
        whatMatters: 'Descriptions of available agent skills.',
        canCut: 'Low'
      },
      {
        name: 'Pinned & Folded',
        value: cb.pinned_and_folded,
        color: '#10b981',
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
    const layout = treemap<any>().size([520, 300]).paddingInner(4).paddingOuter(4);
    return layout(root).leaves();
  });

  let activeCandleNode = $derived<ContextNode | null>(
    latestTurn?.nodes.find((n) => n.id === hoveredCandleNodeId) ?? selectedNode
  );

  // Smart Prune & Cut Advisor: evaluates every node in the context to tell the human what matters vs what can be cut
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
          verdictBadge: '🔴 Safe to Cut (Stale / Bloated)',
          reason: 'Outdated or duplicate file read / oversized tool output. Cutting this prevents model confusion and saves tokens immediately.'
        });
      } else if (k === 'tool_result' && (n.turn_seq < maxTurn || tok > 120)) {
        items.push({
          node: n,
          label: humanToolActionLabel(n),
          preview: text.slice(0, 140),
          tokens: tok,
          verdict: n.turn_seq < maxTurn ? 'cut_recommended' : 'can_compact',
          verdictBadge: n.turn_seq < maxTurn ? '🟠 Old Tool Output (Safe to Cut)' : '🟡 Latest Tool Output (Can Compact)',
          reason:
            n.turn_seq < maxTurn
              ? `From earlier Turn #${n.turn_seq} ( model already acted on this output). Safe to replace with a 1-line stub.`
              : 'From the latest turn. Keep if the model still needs to read these details, or prune if it is noisy log output.'
        });
      } else if (k === 'assistant_thinking' && (n.turn_seq < maxTurn || tok > 80)) {
        items.push({
          node: n,
          label: `🧠 Assistant Thinking (Turn #${n.turn_seq})`,
          preview: text.slice(0, 140),
          tokens: tok,
          verdict: 'can_compact',
          verdictBadge: '🟠 Old Reasoning (Safe to Cut)',
          reason: `Internal reasoning block from Turn #${n.turn_seq}. Once the assistant has taken its action, raw thinking blocks can be pruned.`
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
      const res = await fetch('/api/v1/sessions');
      sessions = await res.json();
      if (!activeSessionId && sessions.length > 0) {
        // Default to the first session that has a real project workspace if available
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
    const res = await fetch(`/api/v1/sessions/${sessionId}/graph`);
    if (!res.ok) return;
    const data = await res.json();
    latestTurn = data.latest_turn;
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
    uncheckedItemIds = {};
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
    await fetch(`/api/v1/sessions/${activeSessionId}/mutations`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ op, status })
    });
    statusBanner = msg;
    await fetchSessionGraph(activeSessionId);
  }

  async function commitDrafts() {
    if (!activeSessionId) return;
    await fetch(`/api/v1/sessions/${activeSessionId}/mutations/commit-drafts`, { method: 'POST' });
    statusBanner = 'Committed all staged draft mutations (single KV cache rebuild).';
    await fetchSessionGraph(activeSessionId);
  }

  async function deleteMutation(mutationId: string) {
    if (!activeSessionId) return;
    await fetch(`/api/v1/sessions/${activeSessionId}/mutations/${mutationId}`, { method: 'DELETE' });
    statusBanner = 'Reverted mutation overlay.';
    await fetchSessionGraph(activeSessionId);
  }

  async function oneClickOptimize() {
    activeTab = 'optimize';
  }

  async function executeSelectedStrategy(status: 'committed' | 'draft' = 'committed') {
    if (!activeSessionId || !activeStrategyPlan) return;
    const selectedIds = checkedStrategyItems.map((i) => i.item_id);
    if (selectedIds.length === 0) return;
    const res = await fetch(`/api/v1/sessions/${activeSessionId}/optimize`, {
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
          ? `🕒 Staged ${result.applied_count} optimizations from "${activeStrategyPlan.name}" into Cache-Safe Draft Queue (~${result.estimated_saved_tokens.toLocaleString()} tok).`
          : `⚡ Applied ${result.applied_count} optimizations from "${activeStrategyPlan.name}"! Saved ~${result.estimated_saved_tokens.toLocaleString()} tokens.`;
      await fetchSessionGraph(activeSessionId);
    }
  }

  async function bulkCutSafeItems() {
    if (!activeSessionId) return;
    const toCut = pruneAdvisorItems.filter((i) => i.verdict === 'cut_recommended' || i.verdict === 'can_compact');
    for (const item of toCut) {
      await fetch(`/api/v1/sessions/${activeSessionId}/mutations`, {
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
    statusBanner = `Cut ${toCut.length} prunable tool outputs & old thoughts (saved ~${totalSafeToCutTokens.toLocaleString()} tokens).`;
    await fetchSessionGraph(activeSessionId);
  }

  async function triggerForkReplay() {
    if (!activeSessionId) return;
    const res = await fetch(`/api/v1/sessions/${activeSessionId}/replay`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        from_turn_id: latestTurn?.id ?? null,
        additional_mutations: [{ op: 'shadow_dedupe', file_path: null }]
      })
    });
    if (res.ok) {
      replayComparison = await res.json();
      statusBanner = `Created Time-Travel Fork & Replay (saved ${replayComparison?.saved_tokens ?? 0} tokens).`;
      await fetchSessionGraph(activeSessionId);
    }
  }

  async function resolveIntercept(interceptId: string, action: 'approve' | 'abort') {
    await fetch(`/api/v1/firewall/intercepts/${interceptId}/resolve`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ action, edited_payload: null })
    });
    intercepts = intercepts.filter((i) => i.intercept_id !== interceptId);
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

    await fetch('/api/v1/ingest', {
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
    statusBanner = 'Loaded interactive multi-turn demo session under Project "contextWatcher".';
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

    await fetch('/api/v1/ingest', {
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
    activeTab = 'local';
    await fetchSessions();
    statusBanner = '🖥️ Loaded Local LLM session (Ollama qwen2.5-coder:32b) — inspect n_keep, KV VRAM & Auto-Pilot below!';
  }

  async function syncTranscriptToDisk() {
    if (!activeSessionId) return;
    const res = await fetch(`/api/v1/sessions/${activeSessionId}/sync-back`, {
      method: 'POST'
    });
    const data = await res.json();
    if (res.ok) {
      statusBanner = `💾 Synced ${data.applied_mutations} mutation overlays back to disk! Backup saved at ${data.backup_path}`;
      await fetchSessionGraph(activeSessionId);
    } else {
      statusBanner = `⚠️ Sync-back note: ${data.error || 'Session is proxy-only or has no on-disk transcript.'}`;
    }
  }

  async function saveAutoPilotConfig(updated: AutoPilotConfig) {
    const res = await fetch('/api/v1/autopilot', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(updated)
    });
    if (res.ok) {
      const data = await res.json();
      autopilotConfig = data.autopilot_config;
      statusBanner = '🤖 Updated Hands-Free Auto-Pilot background policy configuration.';
    }
  }

  $effect(() => {
    fetchSessions();
    const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const ws = new WebSocket(`${proto}//${window.location.host}/api/v1/ws`);
    ws.onopen = () => (wsConnected = true);
    ws.onclose = () => (wsConnected = false);
    ws.onmessage = (ev) => {
      try {
        const msg = JSON.parse(ev.data);
        if (msg.event === 'turn_completed' || msg.event === 'mutations_updated') {
          fetchSessions();
        } else if (msg.event === 'firewall_intercepted') {
          intercepts = [...intercepts, msg.intercept];
        }
      } catch {}
    };
    return () => ws.close();
  });

  function kvColor(status: string): string {
    switch (status) {
      case 'cached_hit':
        return '#22c55e';
      case 'newly_cached':
        return '#eab308';
      case 'cache_invalidated':
        return '#ef4444';
      default:
        return '#64748b';
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
      if (sem?.semantic_type === 'file_read') return `📖 Called Read File: ${sem.path}`;
      if (sem?.semantic_type === 'file_write') return `✏️ Called Edit/Write File: ${sem.path}`;
      if (sem?.semantic_type === 'command_exec') return `💻 Ran Command: ${sem.command}`;
      return `🛠️ Called Tool: ${k.tool_name}`;
    }
    if (k.kind === 'tool_result') {
      const sem = k.semantic_meta;
      if (sem?.semantic_type === 'file_read') return `📄 File Content Returned: ${sem.path}`;
      if (sem?.semantic_type === 'file_write') return `✅ File Updated: ${sem.path}`;
      if (sem?.semantic_type === 'command_exec') return `🖥️ Command Output (exit ${sem.exit_code ?? 0}): ${sem.command}`;
      return `📥 Tool Output: ${k.tool_name}`;
    }
    return k.kind;
  }
</script>

<div style="display: flex; flex-direction: column; height: 100vh; overflow: hidden;">
  <!-- Top Telemetry Ribbon -->
  <header style="display: flex; align-items: center; justify-content: space-between; padding: 10px 18px; background: #111827; border-bottom: 1px solid #1f2937; gap: 16px;">
    <div style="display: flex; align-items: center; gap: 12px;">
      <span style="font-weight: 800; font-size: 18px; color: #38bdf8; letter-spacing: -0.02em;">contextWatcher</span>
      <span style="font-size: 11px; padding: 2px 8px; border-radius: 999px; background: {wsConnected ? '#064e3b' : '#7f1d1d'}; color: {wsConnected ? '#34d399' : '#fca5a5'};">
        {wsConnected ? '● Live Sync' : '○ Offline'}
      </span>

      {#if activeSession}
        <div style="display: flex; align-items: center; gap: 8px; background: #1e293b; padding: 4px 10px; border-radius: 6px; border: 1px solid #334155; font-size: 12px;">
          <span style="color: #38bdf8; font-weight: 700;">📁 {projectNameFromWorkspace(activeSession.workspace_root)}</span>
          <span style="color: #64748b;">•</span>
          <span style="color: #f8fafc; max-width: 280px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;">
            {activeSession.title || activeSession.id}
          </span>
          <span style="color: #94a3b8; font-size: 11px;">({formatTimestamp(activeSession.updated_at)})</span>
        </div>
      {/if}

      <div style="display: flex; align-items: center; gap: 6px;">
        <button
          onclick={seedRichDemoSession}
          style="background: #1e293b; color: #93c5fd; border: 1px solid #3b82f6; border-radius: 6px; padding: 5px 10px; font-size: 12px; cursor: pointer;"
        >
          + Cloud Demo
        </button>
        <HelpTip text="Creates a sample multi-turn cloud coding session (Claude Sonnet 4) with Stale File Reads, Duplicate Reads, and MCP Tool schemas." />

        <button
          onclick={seedLocalLlmDemoSession}
          style="background: #1e293b; color: #c4b5fd; border: 1px solid #8b5cf6; border-radius: 6px; padding: 5px 10px; font-size: 12px; cursor: pointer;"
        >
          🖥️ + Local LLM Demo
        </button>
        <HelpTip text="Creates a sample Local LLM session (Ollama qwen2.5-coder:32b) so you can inspect KV-Cache Slot Pinning (n_keep), Quantized KV VRAM (FP16/Q8_0/Q4_0), and Prefill Latency savings." />
      </div>
    </div>

    {#if latestTurn}
      <div style="display: flex; align-items: center; gap: 14px; font-size: 12.5px; flex-wrap: wrap;">
        <div style="display: flex; align-items: center;">
          <span style="color: #94a3b8; margin-right: 4px;">Context:</span>
          <strong style="color: #f8fafc;">{latestTurn.token_usage.input_tokens.toLocaleString()}</strong>
          <span style="color: #64748b; margin-left: 3px;">/ {(latestTurn.token_usage.context_limit || 200000).toLocaleString()} tok</span>
          {#if latestTurn.token_usage.saved_by_mutations > 0}
            <span style="color: #34d399; margin-left: 6px;">(-{latestTurn.token_usage.saved_by_mutations} saved)</span>
          {/if}
          <HelpTip text="Total tokens currently occupying the LLM's context window on this turn (including hidden system prompts, rules, MCP tool definitions, and conversation history)." />
        </div>

        <div style="display: flex; align-items: center; gap: 8px;">
          <span style="color: #22c55e;">🟢 {latestTurn.kv_cache_report.hit_tokens.toLocaleString()} Hit</span>
          <span style="color: #eab308;">🟡 {latestTurn.kv_cache_report.newly_cached_tokens.toLocaleString()} New</span>
          {#if latestTurn.kv_cache_report.invalidated_tokens > 0}
            <span style="color: #ef4444; font-weight: 700;">🔴 {latestTurn.kv_cache_report.invalidated_tokens.toLocaleString()} Brk</span>
          {/if}
          <HelpTip text="KV Cache X-Ray: Green = tokens reused from the provider's or local GPU's KV cache. Yellow = new tokens prefilled this turn. Red = previously cached tokens evicted because an earlier prefix changed." />
        </div>

        {#if costAndPerf}
          <div style="display: flex; align-items: center; background: #1e293b; padding: 3px 9px; border-radius: 6px; border: 1px solid #334155;">
            {#if costAndPerf.is_local_model && localModelAnalysis}
              <span style="color: #c4b5fd; font-weight: 700;">
                🖥️ KV VRAM: {localModelAnalysis.vram_q8_0_mb.toFixed(0)} MB (Q8) • Prefill: {costAndPerf.estimated_prefill_ms}ms (-{costAndPerf.saved_prefill_ms_by_kv + costAndPerf.saved_prefill_ms_by_overlays}ms saved)
              </span>
              <HelpTip text="Local GPU Metrics: Shows estimated KV-cache VRAM footprint (Q8_0 quantized), active prefill latency in milliseconds, and prefill time saved via KV slot reuse + cw compaction." />
            {:else}
              <span style="color: #34d399; font-weight: 700;">
                💲 Turn: ${costAndPerf.turn_cost_usd.toFixed(4)}
              </span>
              <span style="color: #94a3b8; margin-left: 6px; font-size: 11.5px;">
                (Saved ${(costAndPerf.saved_by_kv_cache_usd + costAndPerf.saved_by_cw_overlays_usd).toFixed(4)})
              </span>
              <HelpTip text="Live Cost & Savings Calculator: Shows the exact USD cost of this turn and how much money you saved via 90%-discounted KV Cache Hits + contextWatcher token cuts." />
            {/if}
          </div>
        {/if}

        <div style="display: flex; align-items: center;">
          <div style="padding: 4px 10px; border-radius: 6px; font-weight: 700; background: {latestTurn.health_score >= 80 ? '#064e3b' : latestTurn.health_score >= 55 ? '#78350f' : '#7f1d1d'};">
            Health: {latestTurn.health_score}/100
          </div>
          <HelpTip text="Context Health Score (0–100): Penalizes context rot such as stale file reads (including external edits on disk!), duplicate file reads, failed command loops, and bloated MCP tool schemas." />
        </div>

        {#if transcriptSourcePath}
          <div style="display: flex; align-items: center;">
            <button
              onclick={syncTranscriptToDisk}
              style="background: #059669; color: #fff; border: none; border-radius: 6px; padding: 6px 10px; font-size: 12px; font-weight: 600; cursor: pointer;"
            >
              💾 Sync to Disk (.bak)
            </button>
            <HelpTip position="left" text="Writes your active contextWatcher mutations back to the harness's on-disk .jsonl transcript (after creating a safe .jsonl.bak backup) so resuming the session in Antigravity, Claude Code, Codex, or OpenCode immediately uses the smaller context!" />
          </div>
        {/if}

        {#if draftMutations.length > 0}
          <div style="display: flex; align-items: center;">
            <button
              onclick={commitDrafts}
              style="background: #d97706; color: #fff; border: none; border-radius: 6px; padding: 6px 12px; font-weight: 600; cursor: pointer;"
            >
              Commit {draftMutations.length} Staged Drafts
            </button>
            <HelpTip position="left" text="Applies all queued draft edits at once so you only pay the KV Cache prefix rebuild cost a single time instead of on every turn." />
          </div>
        {/if}

        <div style="display: flex; align-items: center;">
          <button
            onclick={oneClickOptimize}
            style="background: #2563eb; color: #fff; border: none; border-radius: 6px; padding: 6px 12px; font-weight: 600; cursor: pointer;"
          >
            ⚡ Optimize & Cut Studio ({latestTurn.lint_findings.length})
          </button>
          <HelpTip position="left" text="Opens the Interactive Optimization & Cut Studio where you can compare 5 optimization strategies, read what & how each strategy works, preview exact Before → After token and KV cache results, and check/uncheck individual nodes." />
        </div>
      </div>
    {/if}
  </header>

  {#if statusBanner}
    <div style="background: #1e3a8a; color: #dbeafe; padding: 6px 18px; font-size: 12px; display: flex; justify-content: space-between;">
      <span>{statusBanner}</span>
      <button onclick={() => (statusBanner = '')} style="background: transparent; border: none; color: #93c5fd; cursor: pointer;">✕</button>
    </div>
  {/if}

  <!-- Main 3-Column Layout: [1] Left Project/Session Explorer | [2] Center Trace Views | [3] Right Inspector -->
  <div style="display: grid; grid-template-columns: 310px 1fr 410px; flex: 1; overflow: hidden;">
    <!-- [1] Left Sidebar: Project & Session Explorer -->
    <aside style="background: #0f172a; border-right: 1px solid #1e293b; display: flex; flex-direction: column; overflow: hidden;">
      <div style="padding: 12px; border-bottom: 1px solid #1e293b; display: flex; flex-direction: column; gap: 8px;">
        <div style="display: flex; align-items: center; justify-content: space-between;">
          <strong style="font-size: 13px; color: #f8fafc;">Projects & Sessions ({sessions.length})</strong>
          <HelpTip text="Sessions are automatically grouped by the project folder they touched on disk, and sorted by when the conversation happened. Click any conversation to inspect its trace." />
        </div>

        <input
          type="text"
          bind:value={searchQuery}
          placeholder="🔍 Search by prompt, project, or ID..."
          style="width: 100%; box-sizing: border-box; background: #1e293b; color: #f8fafc; border: 1px solid #334155; border-radius: 6px; padding: 6px 10px; font-size: 12px;"
        />

        <select
          bind:value={selectedProjectFilter}
          style="width: 100%; background: #1e293b; color: #cbd5e1; border: 1px solid #334155; border-radius: 6px; padding: 5px 8px; font-size: 12px;"
        >
          <option value="ALL">📁 All Projects ({allProjectNames.length})</option>
          {#each allProjectNames as proj}
            <option value={proj}>📁 {proj}</option>
          {/each}
        </select>
      </div>

      <div style="flex: 1; overflow-y: auto; padding: 8px;">
        {#each projectGroups as group}
          <div style="margin-bottom: 10px; border: 1px solid #1e293b; border-radius: 8px; overflow: hidden; background: #111827;">
            <!-- Project Folder Header -->
            <button
              onclick={() => (collapsedProjects[group.projectName] = !collapsedProjects[group.projectName])}
              style="width: 100%; display: flex; align-items: center; justify-content: space-between; padding: 8px 10px; background: #1e293b; color: #f8fafc; border: none; cursor: pointer; text-align: left;"
            >
              <div style="overflow: hidden;">
                <div style="font-size: 12.5px; font-weight: 700; color: #38bdf8; display: flex; align-items: center; gap: 6px;">
                  <span>{collapsedProjects[group.projectName] ? '▸' : '▾'}</span>
                  <span>{group.projectName === 'General Chat (No Project)' ? '💬' : '📁'} {group.projectName}</span>
                </div>
                {#if group.workspacePath}
                  <div style="font-size: 10px; color: #64748b; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; margin-left: 16px;">
                    {group.workspacePath}
                  </div>
                {/if}
              </div>
              <span style="font-size: 11px; padding: 2px 7px; border-radius: 999px; background: #0f172a; color: #94a3b8;">
                {group.sessions.length}
              </span>
            </button>

            <!-- Sessions inside this Project -->
            {#if !collapsedProjects[group.projectName]}
              <div style="display: flex; flex-direction: column;">
                {#each group.sessions as s}
                  <button
                    onclick={() => {
                      activeSessionId = s.id;
                      selectedNodeId = null;
                      fetchSessionGraph(s.id);
                    }}
                    style="width: 100%; text-align: left; padding: 9px 10px; border: none; border-top: 1px solid #1f2937; cursor: pointer; background: {activeSessionId === s.id ? '#1e3a8a' : 'transparent'}; color: #f1f5f9;"
                  >
                    <div style="font-size: 12.5px; font-weight: 600; color: {activeSessionId === s.id ? '#ffffff' : '#e2e8f0'}; line-height: 1.35; margin-bottom: 4px; display: -webkit-box; -webkit-line-clamp: 2; -webkit-box-orient: vertical; overflow: hidden;">
                      {s.title || `Session ${s.id}`}
                    </div>
                    <div style="display: flex; justify-content: space-between; align-items: center; font-size: 10.5px; color: {activeSessionId === s.id ? '#bfdbfe' : '#94a3b8'};">
                      <span>🕒 {formatTimestamp(s.updated_at)}</span>
                      <span>{formatRelativeTime(s.updated_at)}</span>
                    </div>
                    <div style="display: flex; justify-content: space-between; align-items: center; font-size: 10px; color: #64748b; margin-top: 3px;">
                      <span>{s.harness} • {s.model}</span>
                      <code>{s.id}</code>
                    </div>
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        {/each}
      </div>
    </aside>

    <!-- [2] Center Main Column: Navigation Tabs + Active View -->
    <div style="display: flex; flex-direction: column; overflow: hidden; background: #0b0f19;">
      <nav style="display: flex; align-items: center; gap: 6px; padding: 8px 14px; background: #0f172a; border-bottom: 1px solid #1e293b; flex-wrap: wrap;">
        {#each [
          ['story', '📖 1. Human Trace Story', 'Chronological, human-readable timeline of every turn: what you asked, what the model thought, which files/commands it ran, and when it happened.'],
          ['optimize', '⚡ 2. Optimize & Cut Studio', 'Interactive dry-run simulator for 5 context optimization strategies: explains What & How each cut works, shows Expected Results (Tokens, KV Cache break-even, Health Score), and lets you check/uncheck individual items.'],
          ['local', '🖥️ 3. Local LLM & Auto-Pilot', 'Deep KV-cache slot optimization for Ollama, llama.cpp, vLLM, SGLang & LM Studio (n_keep system prefix lock, VRAM calculator, sliding-window guard), Hands-Free Auto-Pilot policies, and Disk Transcript Sync-Back.'],
          ['radix', '🌿 4. Context Radix Tree', 'Low-level token prefix tree showing exact JSON pointers, KV cache hits/breaks, and lint warnings on every block in the context window.'],
          ['treemap', '📊 5. Treemap & Attention', 'Visual breakdown of where your tokens are spent (MCP tools vs File Reads vs Thinking) and the U-shaped Lost-in-the-Middle attention curve.'],
          ['tools', '🛠️ 6. Tool Gate & Files', 'Toggle off unused MCP tools to save tokens, and check which project files in the LLM context are Fresh vs Stale.'],
          ['replay', '⏪ 7. Overlays & Replay', 'Manage active context mutations and fork any turn to run an A/B comparison replay.']
        ] as [id, label, tip]}
          <div style="display: flex; align-items: center;">
            <button
              onclick={() => (activeTab = id as any)}
              style="padding: 6px 11px; border-radius: 6px; border: none; font-size: 12.5px; font-weight: 600; cursor: pointer; background: {activeTab === id ? '#2563eb' : 'transparent'}; color: {activeTab === id ? '#ffffff' : '#94a3b8'};"
            >
              {label}
            </button>
            <HelpTip text={tip} />
          </div>
        {/each}
      </nav>

      <main style="flex: 1; overflow-y: auto; padding: 16px;">
        {#if !latestTurn}
          <div style="text-align: center; padding: 80px 20px; color: #64748b;">
            <h2 style="color: #e2e8f0; margin-bottom: 8px;">Select a Conversation on the Left</h2>
            <p style="margin-bottom: 20px;">Choose any project session from the left sidebar, or load the interactive demo session.</p>
          </div>
        {:else if activeTab === 'story'}
          <!-- Human-Readable Trace Story & Chronological Timeline -->
          <div style="margin-bottom: 14px; padding: 12px 14px; background: #111827; border: 1px solid #1f2937; border-radius: 8px; display: flex; justify-content: space-between; align-items: center;">
            <div>
              <div style="font-size: 14px; font-weight: 700; color: #f8fafc;">
                {activeSession?.title || activeSession?.id}
              </div>
              <div style="font-size: 12px; color: #94a3b8; margin-top: 3px;">
                📁 Project: <strong style="color: #38bdf8;">{projectNameFromWorkspace(activeSession?.workspace_root)}</strong>
                {#if activeSession?.workspace_root}
                  <code>({activeSession.workspace_root})</code>
                {/if}
                • Started: {formatTimestamp(activeSession?.created_at)} • Last Activity: {formatTimestamp(activeSession?.updated_at)}
              </div>
            </div>
            <div style="font-size: 12px; color: #cbd5e1;">
              <strong>{storyTurns.length}</strong> Turns • <strong>{fileStates.length}</strong> Files Touched
            </div>
          </div>

          <div style="display: flex; flex-direction: column; gap: 14px;">
            {#each storyTurns as st}
              <div style="background: #111827; border: 1px solid #1f2937; border-radius: 10px; overflow: hidden;">
                <!-- Turn Header with Timestamp & Quick Fold Action -->
                <div style="display: flex; justify-content: space-between; align-items: center; padding: 8px 14px; background: #1e293b; border-bottom: 1px solid #334155;">
                  <div style="display: flex; align-items: center; gap: 10px;">
                    <span style="background: #2563eb; color: white; font-size: 11.5px; font-weight: 700; padding: 2px 8px; border-radius: 999px;">
                      Turn #{st.turnSeq}
                    </span>
                    {#if st.timestamp}
                      <span style="font-size: 12px; color: #cbd5e1;">🕒 {formatTimestamp(st.timestamp)} ({formatRelativeTime(st.timestamp)})</span>
                    {/if}
                    <span style="font-size: 11.5px; color: #94a3b8;">• {st.totalTokens.toLocaleString()} tokens in this turn</span>
                  </div>

                  <div style="display: flex; align-items: center;">
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
                      style="background: #334155; color: #e2e8f0; border: 1px solid #475569; border-radius: 5px; padding: 3px 9px; font-size: 11px; cursor: pointer;"
                    >
                      📁 Fold This Turn
                    </button>
                    <HelpTip position="left" text="Compresses the verbose tool outputs and thinking blocks in this turn into a short summary so the LLM doesn't waste tokens or get distracted by old logs." />
                  </div>
                </div>

                <div style="padding: 12px 14px; display: flex; flex-direction: column; gap: 10px;">
                  <!-- 1. User Prompts -->
                  {#each st.userPrompts as up}
                    <div
                      role="button"
                      tabindex="0"
                      onclick={() => (selectedNodeId = up.id)}
                      onkeydown={(e) => e.key === 'Enter' && (selectedNodeId = up.id)}
                      style="background: #0f172a; border-left: 4px solid #38bdf8; padding: 9px 12px; border-radius: 6px; cursor: pointer;"
                    >
                      <div style="font-size: 11px; font-weight: 700; color: #38bdf8; margin-bottom: 4px;">
                        👤 USER ASKED ({up.token_span.count} tok)
                      </div>
                      <div style="font-size: 13px; color: #f8fafc; white-space: pre-wrap;">{up.kind.text}</div>
                    </div>
                  {/each}

                  <!-- 2. Model Thinking -->
                  {#each st.thinkings as th}
                    <div
                      role="button"
                      tabindex="0"
                      onclick={() => (selectedNodeId = th.id)}
                      onkeydown={(e) => e.key === 'Enter' && (selectedNodeId = th.id)}
                      style="background: #0f172a; border-left: 4px solid #ec4899; padding: 8px 12px; border-radius: 6px; cursor: pointer;"
                    >
                      <div style="font-size: 11px; font-weight: 700; color: #f472b6; margin-bottom: 3px;">
                        🧠 MODEL REASONING / THINKING ({th.token_span.count} tok)
                      </div>
                      <div style="font-size: 12px; color: #cbd5e1; max-height: 75px; overflow: hidden; text-overflow: ellipsis;">
                        {th.kind.thought}
                      </div>
                    </div>
                  {/each}

                  <!-- 3. Tool Calls & Results -->
                  {#if st.toolActions.length > 0}
                    <div style="display: flex; flex-direction: column; gap: 6px;">
                      <div style="font-size: 11px; font-weight: 700; color: #94a3b8;">
                        🛠️ AGENT ACTIONS & TOOL OUTPUTS ({st.toolActions.length})
                      </div>
                      {#each st.toolActions as ta}
                        <div
                          role="button"
                          tabindex="0"
                          onclick={() => (selectedNodeId = ta.id)}
                          onkeydown={(e) => e.key === 'Enter' && (selectedNodeId = ta.id)}
                          style="display: flex; justify-content: space-between; align-items: center; background: #0f172a; border: 1px solid {selectedNodeId === ta.id ? '#38bdf8' : '#1e293b'}; border-left: 4px solid {kvColor(ta.kv_status)}; padding: 7px 11px; border-radius: 6px; cursor: pointer; font-size: 12px;"
                        >
                          <div style="overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 75%;">
                            <strong>{humanToolActionLabel(ta)}</strong>
                          </div>
                          <div style="display: flex; align-items: center; gap: 8px; flex-shrink: 0;">
                            <span style="color: #94a3b8;">{ta.token_span.count} tok</span>
                            <span style="font-size: 10px; color: {kvColor(ta.kv_status)};">{ta.kv_status.toUpperCase()}</span>
                          </div>
                        </div>
                      {/each}
                    </div>
                  {/if}

                  <!-- 4. Assistant Final Response -->
                  {#each st.assistantReplies as ar}
                    <div
                      role="button"
                      tabindex="0"
                      onclick={() => (selectedNodeId = ar.id)}
                      onkeydown={(e) => e.key === 'Enter' && (selectedNodeId = ar.id)}
                      style="background: #0f172a; border-left: 4px solid #22c55e; padding: 9px 12px; border-radius: 6px; cursor: pointer;"
                    >
                      <div style="font-size: 11px; font-weight: 700; color: #4ade80; margin-bottom: 4px;">
                        💬 ASSISTANT REPLIED ({ar.token_span.count} tok)
                      </div>
                      <div style="font-size: 12.5px; color: #e2e8f0; max-height: 140px; overflow-y: auto; white-space: pre-wrap;">
                        {ar.kind.text || ar.kind.summary}
                      </div>
                    </div>
                  {/each}
                </div>
              </div>
            {/each}
          </div>
        {:else if activeTab === 'optimize'}
          <!-- Interactive Optimization & Cut Studio -->
          <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 12px;">
            <div style="display: flex; align-items: center;">
              <h3 style="margin: 0; font-size: 16px; color: #f8fafc;">⚡ Interactive Optimization & Cut Studio (Dry-Run Simulator)</h3>
              <HelpTip text="Choose from 5 specialized optimization strategies below. Every strategy explains What & How it mutates your context, calculates Expected Results (Before → After tokens, KV Cache break-even turns, and Health Score), and lets you check or uncheck individual blocks before applying." />
            </div>
            <span style="font-size: 12px; color: #94a3b8;">
              Available across Web UI, CLI (<code>cw optimize</code>), and TUI (<code>[o]</code>)
            </span>
          </div>

          <!-- 1. 5 Strategy Selector Cards -->
          <div style="display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 10px; margin-bottom: 16px;">
            {#each optimizationPlans as plan}
              <button
                onclick={() => {
                  selectedStrategyId = plan.id;
                  uncheckedItemIds = {};
                }}
                style="text-align: left; padding: 11px 12px; border-radius: 10px; border: 2px solid {selectedStrategyId === plan.id ? '#38bdf8' : '#1f2937'}; background: {selectedStrategyId === plan.id ? '#1e293b' : '#111827'}; color: #f8fafc; cursor: pointer; display: flex; flex-direction: column; justify-content: space-between; gap: 8px;"
              >
                <div>
                  <div style="font-size: 13px; font-weight: 700; color: {selectedStrategyId === plan.id ? '#38bdf8' : '#f1f5f9'}; margin-bottom: 4px;">
                    {plan.name}
                  </div>
                  <div style="font-size: 11px; color: #94a3b8; line-height: 1.35;">
                    {plan.tagline}
                  </div>
                </div>

                <div style="display: flex; flex-direction: column; gap: 4px; border-top: 1px solid #334155; padding-top: 7px; font-size: 11px;">
                  <div style="display: flex; justify-content: space-between; align-items: center;">
                    <span style="color: #cbd5e1;">Items: <strong>{plan.items.length}</strong></span>
                    {#if plan.saved_tokens > 0}
                      <span style="color: #34d399; font-weight: 700;">-{plan.saved_tokens.toLocaleString()} tok (-{plan.saved_pct.toFixed(1)}%)</span>
                    {:else}
                      <span style="color: #60a5fa; font-weight: 700;">+Attention Boost</span>
                    {/if}
                  </div>
                  <div style="font-size: 10.5px; color: {plan.kv_cache_break_tokens === 0 ? '#4ade80' : '#fbbf24'};">
                    {plan.kv_badge}
                  </div>
                </div>
              </button>
            {/each}
          </div>

          {#if activeStrategyPlan}
            <!-- 2. Strategy Blueprint: What & How We Do It + Expected Results Scoreboard -->
            <div style="background: #111827; border: 1px solid #1f2937; border-radius: 10px; padding: 14px 16px; margin-bottom: 16px;">
              <div style="display: grid; grid-template-columns: 1.25fr 1fr; gap: 16px; align-items: start;">
                <!-- Left: What & How We Do It -->
                <div style="display: flex; flex-direction: column; gap: 10px;">
                  <div>
                    <div style="font-size: 12px; font-weight: 700; color: #38bdf8; text-transform: uppercase; letter-spacing: 0.03em; margin-bottom: 4px;">
                      📖 What & How We Do It ({activeStrategyPlan.name})
                    </div>
                    <div style="font-size: 12.5px; color: #e2e8f0; line-height: 1.5; background: #0b0f19; padding: 10px 12px; border-radius: 8px; border: 1px solid #1e293b;">
                      {activeStrategyPlan.what_and_how}
                    </div>
                  </div>

                  <div>
                    <div style="font-size: 12px; font-weight: 700; color: #34d399; text-transform: uppercase; letter-spacing: 0.03em; margin-bottom: 4px;">
                      🎯 Expected Quality & Model Impact
                    </div>
                    <div style="font-size: 12.5px; color: #cbd5e1; line-height: 1.45; background: #0b0f19; padding: 10px 12px; border-radius: 8px; border: 1px solid #1e293b;">
                      {activeStrategyPlan.expected_quality_impact}
                    </div>
                  </div>
                </div>

                <!-- Right: Live Recalculated Expected Results Scoreboard -->
                <div style="background: #0b0f19; border: 1px solid #334155; border-radius: 10px; padding: 12px 14px; display: flex; flex-direction: column; gap: 10px;">
                  <div style="display: flex; justify-content: space-between; align-items: center; border-bottom: 1px solid #1e293b; padding-bottom: 6px;">
                    <strong style="font-size: 13px; color: #f8fafc;">📊 Expected Results (Before → After)</strong>
                    <span style="font-size: 11px; color: #38bdf8;">
                      {checkedStrategyItems.length} of {activeStrategyPlan.items.length} items checked
                    </span>
                  </div>

                  <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 8px;">
                    <div style="background: #111827; padding: 8px 10px; border-radius: 6px; border: 1px solid #1f2937;">
                      <div style="font-size: 11px; color: #94a3b8;">Context Tokens</div>
                      <div style="font-size: 13.5px; font-weight: 700; color: #f8fafc; margin-top: 2px;">
                        {liveStrategyScoreboard.tokensBefore.toLocaleString()} → <span style="color: #34d399;">{liveStrategyScoreboard.tokensAfter.toLocaleString()} tok</span>
                      </div>
                      <div style="font-size: 11px; color: #34d399;">
                        {#if liveStrategyScoreboard.savedTokens > 0}
                          -{liveStrategyScoreboard.savedTokens.toLocaleString()} tok (-{liveStrategyScoreboard.savedPct}%)
                        {:else}
                          Tail Attention Pin (+35 tok)
                        {/if}
                      </div>
                    </div>

                    <div style="background: #111827; padding: 8px 10px; border-radius: 6px; border: 1px solid #1f2937;">
                      <div style="font-size: 11px; color: #94a3b8;">Context Health Score</div>
                      <div style="font-size: 13.5px; font-weight: 700; color: #f8fafc; margin-top: 2px;">
                        {liveStrategyScoreboard.healthBefore}/100 → <span style="color: #34d399;">{liveStrategyScoreboard.healthAfter}/100</span>
                      </div>
                      <div style="font-size: 11px; color: #93c5fd;">
                        +{Math.max(0, liveStrategyScoreboard.healthAfter - liveStrategyScoreboard.healthBefore)} pts improvement
                      </div>
                    </div>
                  </div>

                  <div style="background: #111827; padding: 8px 10px; border-radius: 6px; border: 1px solid #1f2937; font-size: 12px;">
                    <div style="color: #94a3b8; font-size: 11px; margin-bottom: 2px;">KV Cache Prefix Impact & Break-Even</div>
                    {#if liveStrategyScoreboard.kvBreakTokens === 0}
                      <div style="color: #4ade80; font-weight: 700;">
                        🟢 0 Cached Tokens Invalidated (100% KV Cache-Safe!)
                      </div>
                    {:else}
                      <div style="color: #fbbf24; font-weight: 700;">
                        🟡 Rebuilds ~{liveStrategyScoreboard.kvBreakTokens.toLocaleString()} cached prefix tokens once (Break-even in ~{liveStrategyScoreboard.breakEvenTurns} turn{liveStrategyScoreboard.breakEvenTurns === 1 ? '' : 's'})
                      </div>
                    {/if}
                  </div>

                  <div style="display: flex; gap: 8px; margin-top: 2px;">
                    <div style="display: flex; align-items: center; flex: 1;">
                      <button
                        disabled={checkedStrategyItems.length === 0}
                        onclick={() => executeSelectedStrategy('committed')}
                        style="flex: 1; background: {checkedStrategyItems.length === 0 ? '#334155' : '#2563eb'}; color: white; border: none; border-radius: 6px; padding: 8px 12px; font-size: 12.5px; font-weight: 700; cursor: {checkedStrategyItems.length === 0 ? 'not-allowed' : 'pointer'};"
                      >
                        ⚡ Apply {checkedStrategyItems.length} Checked Now
                      </button>
                      <HelpTip position="left" text="Immediately commits the checked optimization overlays to this session so the very next LLM turn uses the compacted context." />
                    </div>

                    <div style="display: flex; align-items: center;">
                      <button
                        disabled={checkedStrategyItems.length === 0}
                        onclick={() => executeSelectedStrategy('draft')}
                        style="background: {checkedStrategyItems.length === 0 ? '#334155' : '#d97706'}; color: white; border: none; border-radius: 6px; padding: 8px 12px; font-size: 12px; font-weight: 600; cursor: {checkedStrategyItems.length === 0 ? 'not-allowed' : 'pointer'};"
                      >
                        🕒 Stage as Draft
                      </button>
                      <HelpTip position="left" text="Queues these optimizations in the Cache-Safe Draft Queue so you can keep inspecting or wait until your next natural cache breakpoint before committing." />
                    </div>
                  </div>
                </div>
              </div>
            </div>

            <!-- 3. Granular Per-Node Checklist -->
            <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 8px;">
              <div style="display: flex; align-items: center;">
                <h4 style="margin: 0; font-size: 14px; color: #f8fafc;">
                  ☑️ Granular Operations in "{activeStrategyPlan.name}" ({activeStrategyPlan.items.length} items)
                </h4>
                <HelpTip text="Uncheck any specific file read, tool output, or turn below if you want to exclude it from this optimization pass. The Expected Results scoreboard above updates live as you toggle checkboxes!" />
              </div>

              {#if activeStrategyPlan.items.length > 0}
                <div style="display: flex; gap: 8px;">
                  <button
                    onclick={() => (uncheckedItemIds = {})}
                    style="background: #1e293b; color: #cbd5e1; border: 1px solid #334155; border-radius: 5px; padding: 3px 9px; font-size: 11px; cursor: pointer;"
                  >
                    Check All
                  </button>
                  <button
                    onclick={() => {
                      const next: Record<string, boolean> = {};
                      for (const item of activeStrategyPlan!.items) next[item.item_id] = true;
                      uncheckedItemIds = next;
                    }}
                    style="background: #1e293b; color: #cbd5e1; border: 1px solid #334155; border-radius: 5px; padding: 3px 9px; font-size: 11px; cursor: pointer;"
                  >
                    Uncheck All
                  </button>
                </div>
              {/if}
            </div>

            {#if activeStrategyPlan.items.length === 0}
              <div style="background: #064e3b; color: #a7f3d0; padding: 14px; border-radius: 8px; font-size: 13px;">
                ✅ Nothing left to cut or compact for this strategy! Your context is already fully optimized for "{activeStrategyPlan.name}".
              </div>
            {:else}
              <div style="display: flex; flex-direction: column; gap: 8px;">
                {#each activeStrategyPlan.items as item}
                  {@const isChecked = !uncheckedItemIds[item.item_id]}
                  <div
                    style="background: {isChecked ? '#111827' : '#0b0f19'}; border: 1px solid {isChecked ? '#334155' : '#1e293b'}; border-left: 4px solid {isChecked ? '#38bdf8' : '#475569'}; border-radius: 8px; padding: 10px 14px; display: flex; align-items: flex-start; gap: 12px; opacity: {isChecked ? '1' : '0.6'};"
                  >
                    <input
                      type="checkbox"
                      checked={isChecked}
                      onchange={() => {
                        uncheckedItemIds = {
                          ...uncheckedItemIds,
                          [item.item_id]: isChecked
                        };
                      }}
                      style="margin-top: 3px; width: 16px; height: 16px; cursor: pointer;"
                    />

                    <div style="flex: 1; overflow: hidden;">
                      <div style="display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 8px;">
                        <div style="display: flex; align-items: center; gap: 8px; flex-wrap: wrap;">
                          <span style="font-size: 11px; padding: 2px 7px; border-radius: 4px; background: #1e293b; color: #93c5fd; font-weight: 700;">
                            Turn #{item.turn_seq}
                          </span>
                          <strong style="font-size: 13px; color: #f8fafc;">{item.title}</strong>
                          {#if item.timestamp}
                            <span style="font-size: 11px; color: #64748b;">🕒 {formatTimestamp(item.timestamp)}</span>
                          {/if}
                        </div>

                        <div style="display: flex; align-items: center; gap: 10px; font-size: 12px;">
                          <span style="color: #cbd5e1;">
                            {item.tokens_before.toLocaleString()} → <strong>{item.tokens_after.toLocaleString()} tok</strong>
                          </span>
                          {#if item.saved_tokens > 0}
                            <span style="color: #34d399; font-weight: 700;">(-{item.saved_tokens.toLocaleString()} tok)</span>
                          {:else}
                            <span style="color: #60a5fa; font-weight: 700;">(+Attention Pin)</span>
                          {/if}
                        </div>
                      </div>

                      <div style="font-size: 12px; color: #e2e8f0; margin-top: 5px;">
                        🛠️ <strong>What we do:</strong> {item.what_we_do}
                      </div>
                      <div style="font-size: 11.5px; color: #94a3b8; margin-top: 3px;">
                        💡 <strong>Why it matters:</strong> {item.why_it_matters}
                      </div>
                      <div style="font-size: 11px; color: #64748b; margin-top: 4px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;">
                        Preview: <code>{item.content_preview}</code>
                      </div>
                    </div>

                    {#if item.node_id}
                      <button
                        onclick={() => (selectedNodeId = item.node_id)}
                        style="background: #1e293b; color: #93c5fd; border: 1px solid #334155; border-radius: 5px; padding: 4px 8px; font-size: 11px; cursor: pointer; flex-shrink: 0;"
                      >
                        Inspect →
                      </button>
                    {/if}
                  </div>
                {/each}
              </div>
            {/if}
          {/if}
        {:else if activeTab === 'local'}
          <!-- 🖥️ Local LLM KV-Cache Slot Studio, Auto-Pilot Policies & Disk Sync-Back -->
          <div style="display: flex; flex-direction: column; gap: 18px;">
            <!-- 1. Local Model Engine & KV-Cache Slot Studio -->
            <div style="background: #111827; border: 1px solid #1f2937; border-radius: 10px; padding: 14px 16px;">
              <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 12px;">
                <div style="display: flex; align-items: center;">
                  <h3 style="margin: 0; font-size: 15.5px; color: #f8fafc;">
                    🖥️ 1. Local Model Engine & KV-Cache Slot Optimization (Ollama / llama.cpp / vLLM / SGLang / LM Studio)
                  </h3>
                  <HelpTip text="Local inference engines have limited GPU VRAM and relatively slow prompt prefill speeds (150–800 tok/sec). If an edit or sliding-window shift evicts the system prompt or MCP tool schemas from the KV cache slot, local GPUs freeze for 15–60 seconds re-evaluating the prompt. contextWatcher automatically injects n_keep / num_keep, cache_prompt: true, and deterministic id_slot affinity into local proxy requests!" />
                </div>
                <span style="font-size: 11.5px; padding: 3px 9px; border-radius: 999px; background: #1e1b4b; color: #c4b5fd; font-weight: 700;">
                  CLI: <code>cw local</code>
                </span>
              </div>

              {#if localModelAnalysis}
                <div style="display: grid; grid-template-columns: repeat(auto-fit, minmax(210px, 1fr)); gap: 10px; margin-bottom: 14px;">
                  <!-- Card A: Engine & Slot Affinity -->
                  <div style="background: #0b0f19; border: 1px solid #1e293b; border-radius: 8px; padding: 10px 12px;">
                    <div style="font-size: 11px; color: #94a3b8; display: flex; align-items: center;">
                      <span>Engine & Slot Affinity</span>
                      <HelpTip text="Pins this conversation to a deterministic llama.cpp / Ollama KV slot (id_slot) so concurrent background requests never evict your coding session's KV cache." />
                    </div>
                    <div style="font-size: 14px; font-weight: 700; color: #c4b5fd; margin-top: 4px;">
                      {localModelAnalysis.engine_kind.toUpperCase()} • Slot #{localModelAnalysis.recommended_slot_id}
                    </div>
                    <div style="font-size: 11px; color: #64748b; margin-top: 3px;">
                      PagedAttention Block: {localModelAnalysis.paged_attention_block_size} tok ({localModelAnalysis.prefix_block_aligned_tokens.toLocaleString()} aligned)
                    </div>
                  </div>

                  <!-- Card B: System Prefix Pin (n_keep / num_keep) -->
                  <div style="background: #0b0f19; border: 1px solid #1e293b; border-radius: 8px; padding: 10px 12px;">
                    <div style="font-size: 11px; color: #94a3b8; display: flex; align-items: center;">
                      <span>System Prefix Lock (<code>n_keep</code>)</span>
                      <HelpTip text="Exact number of leading tokens (System Prompt + Rules + MCP Tool Schemas) that contextWatcher locks via options.num_keep / n_keep so context-shift never evicts your system prompt!" />
                    </div>
                    <div style="font-size: 14px; font-weight: 700; color: #38bdf8; margin-top: 4px;">
                      {localModelAnalysis.recommended_n_keep_tokens.toLocaleString()} tokens pinned
                    </div>
                    <div style="font-size: 11px; color: #34d399; margin-top: 3px;">
                      Auto-injected on <code>/ollama</code> & <code>/llamacpp</code> proxy routes
                    </div>
                  </div>

                  <!-- Card C: KV Cache VRAM Footprint -->
                  <div style="background: #0b0f19; border: 1px solid #1e293b; border-radius: 8px; padding: 10px 12px;">
                    <div style="font-size: 11px; color: #94a3b8; display: flex; align-items: center;">
                      <span>KV Cache GPU VRAM Footprint</span>
                      <HelpTip text="Estimated GPU/Unified Memory consumed solely by the KV cache tensors for this conversation across FP16, Q8_0 (-kvu q8_0), and Q4_0 quantizations." />
                    </div>
                    <div style="font-size: 13px; font-weight: 700; color: #f8fafc; margin-top: 4px;">
                      FP16: <span style="color: #fbbf24;">{localModelAnalysis.vram_fp16_mb.toFixed(0)} MB</span> •
                      Q8_0: <span style="color: #34d399;">{localModelAnalysis.vram_q8_0_mb.toFixed(0)} MB</span> •
                      Q4_0: <span style="color: #38bdf8;">{localModelAnalysis.vram_q4_0_mb.toFixed(0)} MB</span>
                    </div>
                    <div style="font-size: 11px; color: #94a3b8; margin-top: 3px;">
                      Tip: Set <code>OLLAMA_KV_CACHE_TYPE=q8_0</code> to halve VRAM
                    </div>
                  </div>

                  <!-- Card D: Sliding Window & Prefill Speed -->
                  <div style="background: #0b0f19; border: 1px solid {localModelAnalysis.sliding_window_overflow_risk ? '#ef4444' : '#1e293b'}; border-radius: 8px; padding: 10px 12px;">
                    <div style="font-size: 11px; color: #94a3b8; display: flex; align-items: center;">
                      <span>Window Utilization & Prefill</span>
                      <HelpTip text="When a prompt exceeds 85% of num_ctx, local engines perform a brute-force context shift unless old turns are folded cleanly first." />
                    </div>
                    <div style="font-size: 14px; font-weight: 700; color: {localModelAnalysis.sliding_window_overflow_risk ? '#ef4444' : '#34d399'}; margin-top: 4px;">
                      {localModelAnalysis.used_ctx_pct.toFixed(1)}% of {localModelAnalysis.num_ctx_limit.toLocaleString()} tok
                    </div>
                    {#if costAndPerf}
                      <div style="font-size: 11px; color: #cbd5e1; margin-top: 3px;">
                        Prefill: <strong>{costAndPerf.estimated_prefill_ms}ms</strong> (Saved <strong>{costAndPerf.saved_prefill_ms_by_kv + costAndPerf.saved_prefill_ms_by_overlays}ms</strong>)
                      </div>
                    {/if}
                  </div>
                </div>

                <!-- Actionable Local Engine Recommendations -->
                <div style="display: flex; flex-direction: column; gap: 8px;">
                  {#each localModelAnalysis.recommendations as rec}
                    <div style="background: #0b0f19; border: 1px solid #1e293b; border-left: 4px solid #8b5cf6; border-radius: 8px; padding: 10px 14px;">
                      <div style="display: flex; justify-content: space-between; align-items: center; gap: 8px; flex-wrap: wrap;">
                        <strong style="font-size: 13px; color: #f8fafc;">⚡ {rec.title}</strong>
                        <span style="font-size: 11.5px; color: #34d399; font-weight: 700;">{rec.severity.toUpperCase()}</span>
                      </div>
                      <div style="font-size: 12px; color: #cbd5e1; margin-top: 4px;">
                        🛠️ <strong>What & How:</strong> {rec.explanation}
                      </div>
                      <div style="margin-top: 6px; display: flex; justify-content: space-between; align-items: center; gap: 10px; background: #111827; padding: 6px 10px; border-radius: 6px; border: 1px solid #1f2937;">
                        <code style="font-size: 11px; color: #93c5fd; overflow-x: auto;">{rec.cli_or_api_fix}</code>
                        {#if rec.id === 'gate_mcp_for_local' || rec.id === 'local_mcp_schema_bloat'}
                          <button
                            onclick={() => {
                              selectedStrategyId = 'lean_tool_belt';
                              activeTab = 'optimize';
                            }}
                            style="background: #2563eb; color: white; border: none; border-radius: 5px; padding: 4px 10px; font-size: 11px; font-weight: 600; cursor: pointer; flex-shrink: 0;"
                          >
                            Open Lean Tool Belt →
                          </button>
                        {:else if rec.id === 'sliding_window_guard' || rec.id === 'local_sliding_window_overflow'}
                          <button
                            onclick={() => {
                              selectedStrategyId = 'surgical_log_diet';
                              activeTab = 'optimize';
                            }}
                            style="background: #dc2626; color: white; border: none; border-radius: 5px; padding: 4px 10px; font-size: 11px; font-weight: 600; cursor: pointer; flex-shrink: 0;"
                          >
                            Compact Before Shift →
                          </button>
                        {/if}
                      </div>
                    </div>
                  {/each}
                </div>
              {/if}

              <!-- Quick Proxy Routes Reference for Local Harnesses -->
              <div style="margin-top: 12px; padding: 10px 12px; background: #0b0f19; border: 1px solid #1e293b; border-radius: 8px; font-size: 11.5px; color: #94a3b8;">
                <strong style="color: #e2e8f0;">🔌 Zero-Config Local Proxy Routes (Auto-injects <code>n_keep</code>, <code>cache_prompt: true</code> & <code>id_slot</code>):</strong>
                <div style="display: grid; grid-template-columns: repeat(auto-fit, minmax(240px, 1fr)); gap: 8px; margin-top: 6px;">
                  <div>• <strong>Ollama:</strong> <code>OLLAMA_HOST=http://127.0.0.1:4401/ollama</code></div>
                  <div>• <strong>llama.cpp / LM Studio:</strong> <code>OPENAI_BASE_URL=http://127.0.0.1:4401/llamacpp/v1</code></div>
                  <div>• <strong>vLLM / SGLang:</strong> <code>OPENAI_BASE_URL=http://127.0.0.1:4401/vllm/v1</code></div>
                </div>
              </div>
            </div>

            <!-- 2. Hands-Free Auto-Pilot Background Policies -->
            {#if autopilotConfig}
              <div style="background: #111827; border: 1px solid #1f2937; border-radius: 10px; padding: 14px 16px;">
                <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 10px;">
                  <div style="display: flex; align-items: center;">
                    <h3 style="margin: 0; font-size: 15.5px; color: #f8fafc;">
                      🤖 2. Hands-Free Auto-Pilot Policies (Background Context Hygiene)
                    </h3>
                    <HelpTip text="When enabled, contextWatcher automatically applies safe, deterministic optimizations in the background on every turn so you don't have to click 'Cut' manually during a fast coding loop." />
                  </div>
                  <span style="font-size: 11.5px; padding: 3px 9px; border-radius: 999px; background: #064e3b; color: #6ee7b7; font-weight: 700;">
                    CLI: <code>cw autopilot</code>
                  </span>
                </div>

                <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 10px;">
                  <!-- Policy 1: Zero-Break Tail Hygiene -->
                  <label style="display: flex; align-items: flex-start; gap: 10px; background: #0b0f19; border: 1px solid {autopilotConfig.zero_break_tail_hygiene ? '#38bdf8' : '#1e293b'}; border-radius: 8px; padding: 10px 12px; cursor: pointer;">
                    <input
                      type="checkbox"
                      checked={autopilotConfig.zero_break_tail_hygiene}
                      onchange={(e) =>
                        saveAutoPilotConfig({
                          ...autopilotConfig!,
                          zero_break_tail_hygiene: (e.currentTarget as HTMLInputElement).checked
                        })}
                      style="margin-top: 3px; width: 16px; height: 16px;"
                    />
                    <div>
                      <div style="font-size: 13px; font-weight: 700; color: #f8fafc;">
                        1. Zero-Break Tail Hygiene (&gt;{autopilotConfig.tail_max_tool_tokens.toLocaleString()} tok → <code>/tmp/cw_logs/</code>)
                      </div>
                      <div style="font-size: 11.5px; color: #94a3b8; margin-top: 3px;">
                        Automatically truncates massive single command outputs to <code>head -n 30 ... tail -n 45</code> before they enter the KV cache, while saving the full raw output to <code>/tmp/cw_logs/&lt;call_id&gt;.log</code> so the agent can still grep it!
                      </div>
                    </div>
                  </label>

                  <!-- Policy 2: Local Model Sliding-Window Guard -->
                  <label style="display: flex; align-items: flex-start; gap: 10px; background: #0b0f19; border: 1px solid {autopilotConfig.local_model_window_guard ? '#38bdf8' : '#1e293b'}; border-radius: 8px; padding: 10px 12px; cursor: pointer;">
                    <input
                      type="checkbox"
                      checked={autopilotConfig.local_model_window_guard}
                      onchange={(e) =>
                        saveAutoPilotConfig({
                          ...autopilotConfig!,
                          local_model_window_guard: (e.currentTarget as HTMLInputElement).checked
                        })}
                      style="margin-top: 3px; width: 16px; height: 16px;"
                    />
                    <div>
                      <div style="font-size: 13px; font-weight: 700; color: #f8fafc;">
                        2. Local Model Sliding-Window Guard (&gt;85% <code>num_ctx</code>)
                      </div>
                      <div style="font-size: 11.5px; color: #94a3b8; margin-top: 3px;">
                        Automatically folds older debugging turns and deduplicates stale file reads before a local model hits its <code>num_ctx</code> ceiling—preventing catastrophic KV-cache context shifts.
                      </div>
                    </div>
                  </label>

                  <!-- Policy 3: Auto Cache-Safe Pin -->
                  <label style="display: flex; align-items: flex-start; gap: 10px; background: #0b0f19; border: 1px solid {autopilotConfig.auto_cache_safe_pin ? '#38bdf8' : '#1e293b'}; border-radius: 8px; padding: 10px 12px; cursor: pointer;">
                    <input
                      type="checkbox"
                      checked={autopilotConfig.auto_cache_safe_pin}
                      onchange={(e) =>
                        saveAutoPilotConfig({
                          ...autopilotConfig!,
                          auto_cache_safe_pin: (e.currentTarget as HTMLInputElement).checked
                        })}
                      style="margin-top: 3px; width: 16px; height: 16px;"
                    />
                    <div>
                      <div style="font-size: 13px; font-weight: 700; color: #f8fafc;">
                        3. Auto Cache-Safe Rule Pin (&gt;{autopilotConfig.pin_min_context_tokens.toLocaleString()} tok &amp; Attention &lt;40%)
                      </div>
                      <div style="font-size: 11.5px; color: #94a3b8; margin-top: 3px;">
                        When project rules (<code>AGENTS.md</code> / <code>CLAUDE.md</code>) sink into the Lost-in-the-Middle attention trough, appends a lightweight reminder at the prompt tail with 0% KV cache break.
                      </div>
                    </div>
                  </label>

                  <!-- Policy 4: Auto Diet at Breakpoint -->
                  <label style="display: flex; align-items: flex-start; gap: 10px; background: #0b0f19; border: 1px solid {autopilotConfig.auto_diet_at_breakpoint ? '#38bdf8' : '#1e293b'}; border-radius: 8px; padding: 10px 12px; cursor: pointer;">
                    <input
                      type="checkbox"
                      checked={autopilotConfig.auto_diet_at_breakpoint}
                      onchange={(e) =>
                        saveAutoPilotConfig({
                          ...autopilotConfig!,
                          auto_diet_at_breakpoint: (e.currentTarget as HTMLInputElement).checked
                        })}
                      style="margin-top: 3px; width: 16px; height: 16px;"
                    />
                    <div>
                      <div style="font-size: 13px; font-weight: 700; color: #f8fafc;">
                        4. Auto Diet at Breakpoint (&gt;{autopilotConfig.breakpoint_tokens.toLocaleString()} tok)
                      </div>
                      <div style="font-size: 11.5px; color: #94a3b8; margin-top: 3px;">
                        Triggers <em>Surgical Log Diet</em> + <em>Clean Code State</em> automatically when a session crosses {autopilotConfig.breakpoint_tokens.toLocaleString()} tokens.
                      </div>
                    </div>
                  </label>
                </div>
              </div>
            {/if}

            <!-- 3. Closing the Loop: Safe Transcript Disk Write-Back & Harness Integrations -->
            <div style="background: #111827; border: 1px solid #1f2937; border-radius: 10px; padding: 14px 16px;">
              <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 8px;">
                <div style="display: flex; align-items: center;">
                  <h3 style="margin: 0; font-size: 15.5px; color: #f8fafc;">
                    💾 3. Non-Proxy Harness Transcript Sync-Back &amp; MCP Self-Compaction
                  </h3>
                  <HelpTip text="For harnesses that read/resume sessions from disk transcripts (Antigravity, Claude Code --resume, Codex, OpenCode, Pi), 'Sync to Disk' writes a timestamped .jsonl.bak backup and applies your active contextWatcher cuts directly to the .jsonl transcript file on disk!" />
                </div>
                <button
                  onclick={syncTranscriptToDisk}
                  style="background: #059669; color: white; border: none; border-radius: 6px; padding: 7px 14px; font-size: 12.5px; font-weight: 700; cursor: pointer;"
                >
                  💾 Sync Active Mutations to Disk (.bak)
                </button>
              </div>

              <div style="font-size: 12px; color: #cbd5e1; display: flex; flex-direction: column; gap: 6px;">
                <div>
                  <strong>On-Disk Source Transcript:</strong>
                  <code>{transcriptSourcePath || 'In-memory / Proxy Session (Run cw install-integrations for MCP & Hooks)'}</code>
                </div>
                <div>
                  <strong>Install Live MCP &amp; PreToolUse/Stop Hooks across Antigravity, Claude Code, Codex, OpenCode, Pi &amp; Aider:</strong>
                  <code style="margin-left: 6px; color: #38bdf8;">cw install-integrations</code>
                </div>
              </div>
            </div>
          </div>
        {:else if activeTab === 'radix'}
          <div style="display: flex; flex-direction: column; gap: 8px;">
            <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 6px;">
              <div style="display: flex; align-items: center;">
                <h3 style="margin: 0; font-size: 15px; color: #f8fafc;">Context Radix Prefix Tree & Turn Spine</h3>
                <HelpTip text="Shows the exact order in which blocks are sent to the LLM provider. Blocks with a Green border are served from the KV cache; Yellow blocks are newly written; Orange badges flag stale or duplicate data." />
              </div>
              <span style="font-size: 12px; color: #94a3b8;">Click any node to inspect & mutate on the right →</span>
            </div>

            {#each graphNodes as gnode}
              <div
                role="button"
                tabindex="0"
                onclick={() => {
                  if (gnode.node_id) selectedNodeId = gnode.node_id;
                }}
                onkeydown={(e) => {
                  if (e.key === 'Enter' && gnode.node_id) selectedNodeId = gnode.node_id;
                }}
                style="display: flex; align-items: center; justify-content: space-between; padding: 9px 12px; border-radius: 8px; background: {selectedNodeId === gnode.node_id && gnode.node_id ? '#1e293b' : '#111827'}; border-left: 5px solid {kvColor(gnode.kv_status)}; border-top: 1px solid #1f2937; border-right: 1px solid #1f2937; border-bottom: 1px solid #1f2937; cursor: pointer; margin-left: {gnode.group === 'root' ? 0 : gnode.group === 'fork' ? 40 : 18}px;"
              >
                <div style="display: flex; align-items: center; gap: 8px; flex-wrap: wrap;">
                  <span style="font-size: 10.5px; padding: 2px 6px; border-radius: 4px; background: #1f2937; color: #94a3b8;">
                    {gnode.group}
                  </span>
                  <strong style="font-size: 12.5px; color: #f1f5f9;">{gnode.label}</strong>
                  {#if gnode.timestamp}
                    <span style="font-size: 11px; color: #64748b;">🕒 {formatTimestamp(gnode.timestamp)}</span>
                  {/if}
                  {#each gnode.lint_badges as badge}
                    <span style="font-size: 10.5px; padding: 2px 7px; border-radius: 999px; background: #78350f; color: #fde68a;">
                      ⚠️ {badge}
                    </span>
                  {/each}
                </div>
                <div style="display: flex; align-items: center; gap: 10px; font-size: 11.5px;">
                  {#if gnode.raw_json_pointer}
                    <code style="color: #64748b;">{gnode.raw_json_pointer}</code>
                  {/if}
                  <span style="color: {kvColor(gnode.kv_status)}; font-weight: 600;">
                    {gnode.kv_status.toUpperCase()}
                  </span>
                </div>
              </div>
            {/each}
          </div>
        {:else if activeTab === 'treemap'}
          <!-- 1. Proportional Token Treemap + Complete Legend Table -->
          <div style="display: flex; align-items: center; margin-bottom: 8px;">
            <h3 style="margin: 0; font-size: 15px;">1. Proportional Token Treemap & Full Category Legend</h3>
            <HelpTip text="Every rectangle is sized by token count. Smaller blocks that cannot fit text inside the rectangle are listed in the complete color-coded legend right next to the chart—hover any block or row to see what it contains and whether it can be cut." />
          </div>

          <div style="display: grid; grid-template-columns: 520px 1fr; gap: 14px; align-items: start;">
            <svg width="520" height="300" style="background: #111827; border-radius: 8px; border: 1px solid #1f2937; max-width: 100%;">
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
                    stroke={hoveredTreemapName === leaf.data.name ? '#ffffff' : '#0b0f19'}
                    stroke-width={hoveredTreemapName === leaf.data.name ? '2.5' : '1'}
                    rx="4"
                    opacity={hoveredTreemapName && hoveredTreemapName !== leaf.data.name ? '0.5' : '0.9'}
                  />
                  {#if leaf.x1 - leaf.x0 > 65 && leaf.y1 - leaf.y0 > 30}
                    <text x="7" y="17" fill="#ffffff" font-size="11.5" font-weight="bold">{leaf.data.name}</text>
                    <text x="7" y="32" fill="#f1f5f9" font-size="10.5">{leaf.value?.toLocaleString()} tok ({leaf.data.pct}%)</text>
                  {/if}
                </g>
              {/each}
            </svg>

            <!-- Complete Legend Table (Shows 100% of blocks including small unlabeled ones!) -->
            <div style="background: #111827; border: 1px solid #1f2937; border-radius: 8px; padding: 10px; display: flex; flex-direction: column; gap: 6px;">
              <div style="font-size: 11.5px; font-weight: 700; color: #94a3b8; border-bottom: 1px solid #1f2937; padding-bottom: 5px;">
                All Context Blocks (Hover to inspect what matters vs. what to cut)
              </div>
              {#each treemapCategories as cat}
                <div
                  role="button"
                  tabindex="0"
                  onmouseenter={() => (hoveredTreemapName = cat.name)}
                  onmouseleave={() => (hoveredTreemapName = null)}
                  onclick={() => (hoveredTreemapName = cat.name)}
                  onkeydown={(e) => e.key === 'Enter' && (hoveredTreemapName = cat.name)}
                  style="padding: 6px 8px; border-radius: 6px; background: {hoveredTreemapName === cat.name ? '#1e293b' : '#0b0f19'}; border-left: 4px solid {cat.color}; cursor: pointer; font-size: 12px;"
                >
                  <div style="display: flex; justify-content: space-between; align-items: center;">
                    <strong style="color: #f8fafc;">{cat.name}</strong>
                    <span style="color: #38bdf8; font-weight: 700;">{cat.value.toLocaleString()} tok ({cat.pct}%)</span>
                  </div>
                  <div style="font-size: 11px; color: #94a3b8; margin-top: 2px;">
                    {cat.whatMatters}
                  </div>
                  <div style="font-size: 10.5px; color: #fbbf24; margin-top: 2px;">
                    ✂️ Prunability: {cat.canCut}
                  </div>
                </div>
              {/each}
            </div>
          </div>

          <!-- 2. Interactive KV Cache Prefix Tape & "Lost in the Middle" Attention Candles -->
          <div style="display: flex; align-items: center; margin-top: 22px; margin-bottom: 8px;">
            <h3 style="margin: 0; font-size: 15px;">2. Interactive KV Cache Tape & "Lost in the Middle" Attention Candles</h3>
            <HelpTip text="Hover over ANY candle bar or KV tape segment below to see an instant X-Ray card of what message, file read, or command output is inside that candle, its exact attention score, and a 1-click button to Prune or Pin it!" />
          </div>

          <div style="background: #111827; padding: 14px; border-radius: 8px; border: 1px solid #1f2937;">
            <div style="font-size: 11px; color: #94a3b8; margin-bottom: 4px;">
              <strong>KV Cache Prefix Tape</strong> (Green = Cached Hit, Yellow = Newly Written, Red = Cache Invalidated) — <em>Hover any segment:</em>
            </div>
            <div style="display: flex; height: 24px; border-radius: 6px; overflow: hidden; margin-bottom: 14px;">
              {#each latestTurn.nodes as n}
                <div
                  role="button"
                  tabindex="0"
                  onmouseenter={() => {
                    hoveredCandleNodeId = n.id;
                    selectedNodeId = n.id;
                  }}
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
                  style="flex: {Math.max(1, n.token_span.count)}; background: {kvColor(n.kv_status)}; border-right: 1px solid #0b0f19; opacity: {activeCandleNode?.id === n.id ? '1' : '0.78'}; outline: {activeCandleNode?.id === n.id ? '2px solid #ffffff' : 'none'}; cursor: pointer;"
                ></div>
              {/each}
            </div>

            <div style="font-size: 11px; color: #94a3b8; margin-bottom: 4px;">
              <strong>Positional Attention Salience Candles</strong> (Blue = High Attention Zone, Orange = Middle Dead Zone &lt;40%) — <em>Hover any candle:</em>
            </div>
            <div style="display: flex; align-items: flex-end; height: 78px; gap: 3px; border-bottom: 1px solid #334155; padding-top: 6px;">
              {#each latestTurn.nodes as n}
                <div
                  role="button"
                  tabindex="0"
                  onmouseenter={() => {
                    hoveredCandleNodeId = n.id;
                    selectedNodeId = n.id;
                  }}
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
                  style="flex: 1; height: {Math.max(8, Math.round(n.attention_salience * 72))}px; background: {activeCandleNode?.id === n.id ? '#ffffff' : n.attention_salience < 0.4 ? '#f97316' : '#38bdf8'}; border-radius: 3px 3px 0 0; cursor: pointer; transition: background 0.1s;"
                ></div>
              {/each}
            </div>
            <div style="display: flex; justify-content: space-between; font-size: 11px; color: #94a3b8; margin-top: 6px;">
              <span>Token 0 (Primacy Zone — High Attention)</span>
              <span>Middle Trough (Attention Dead Zone &lt; 40%)</span>
              <span>Token {latestTurn.token_usage.input_tokens} (Recency Tail — Highest Attention)</span>
            </div>

            <!-- Live Hovered Candle X-Ray Card -->
            {#if activeCandleNode}
              <div style="margin-top: 12px; padding: 11px 14px; border-radius: 8px; background: #0f172a; border: 1px solid #38bdf8; display: flex; justify-content: space-between; align-items: flex-start; gap: 14px;">
                <div style="flex: 1; overflow: hidden;">
                  <div style="display: flex; align-items: center; gap: 8px; flex-wrap: wrap; margin-bottom: 4px;">
                    <span style="background: #1e3a8a; color: #93c5fd; font-size: 11px; font-weight: 700; padding: 2px 7px; border-radius: 4px;">
                      Turn #{activeCandleNode.turn_seq}
                    </span>
                    <strong style="font-size: 13px; color: #f8fafc;">{humanToolActionLabel(activeCandleNode)}</strong>
                    <span style="font-size: 11.5px; color: #38bdf8; font-weight: 700;">
                      {activeCandleNode.token_span.count.toLocaleString()} tok (span {activeCandleNode.token_span.start}..{activeCandleNode.token_span.end})
                    </span>
                    <span style="font-size: 11.5px; padding: 2px 7px; border-radius: 999px; background: {activeCandleNode.attention_salience < 0.4 ? '#7c2d12' : '#064e3b'}; color: {activeCandleNode.attention_salience < 0.4 ? '#fed7aa' : '#6ee7b7'};">
                      Attention: {(activeCandleNode.attention_salience * 100).toFixed(0)}% {activeCandleNode.attention_salience < 0.4 ? '(⚠️ Low Attention Trough)' : '(🔥 Strong Attention)'}
                    </span>
                  </div>
                  <div style="font-size: 12px; color: #cbd5e1; white-space: pre-wrap; max-height: 64px; overflow: hidden; text-overflow: ellipsis; background: #0b0f19; padding: 6px 8px; border-radius: 4px; border: 1px solid #1e293b;">
                    {nodeDisplayText(activeCandleNode).slice(0, 260)}
                  </div>
                </div>

                <div style="display: flex; flex-direction: column; gap: 6px; flex-shrink: 0;">
                  <button
                    onclick={() =>
                      applyMutation(
                        {
                          op: 'prune_or_stub',
                          target_fingerprint: activeCandleNode!.fingerprint,
                          target_call_id: null,
                          target_json_pointer: activeCandleNode!.raw_json_pointer,
                          stub_text: `[cw:pruned original_tokens=${activeCandleNode!.token_span.count}]`
                        },
                        'committed',
                        `Cut node ${activeCandleNode!.id} (-${activeCandleNode!.token_span.count} tok)`
                      )}
                    style="background: #dc2626; color: white; border: none; border-radius: 5px; padding: 5px 10px; font-size: 11.5px; font-weight: 600; cursor: pointer;"
                  >
                    ✂️ Cut This Node (-{activeCandleNode.token_span.count} tok)
                  </button>
                  <button
                    onclick={() =>
                      applyMutation(
                        {
                          op: 'pin_context',
                          label: humanToolActionLabel(activeCandleNode!),
                          content: nodeDisplayText(activeCandleNode!).slice(0, 180),
                          source_node_id: activeCandleNode!.id
                        },
                        'committed',
                        `Pinned ${activeCandleNode!.id} to high-attention tail`
                      )}
                    style="background: #059669; color: white; border: none; border-radius: 5px; padding: 5px 10px; font-size: 11.5px; font-weight: 600; cursor: pointer;"
                  >
                    📌 Pin to Tail (Boost Attention)
                  </button>
                </div>
              </div>
            {/if}
          </div>

          <!-- 3. Smart "What Can We Cut?" Prunability & Importance Advisor -->
          <div style="display: flex; justify-content: space-between; align-items: center; margin-top: 22px; margin-bottom: 10px;">
            <div style="display: flex; align-items: center;">
              <h3 style="margin: 0; font-size: 15px;">3. ✂️ What Can We Cut? (Tool Output & Context Prunability Advisor)</h3>
              <HelpTip text="Automatically scans all tool call outputs, file reads, and thinking blocks in the session to tell you what actually matters vs. what is safe to cut right now." />
            </div>

            {#if pruneAdvisorItems.length > 0}
              <div style="display: flex; align-items: center;">
                <button
                  onclick={bulkCutSafeItems}
                  style="background: #dc2626; color: white; border: none; border-radius: 6px; padding: 7px 14px; font-size: 12.5px; font-weight: 700; cursor: pointer;"
                >
                  ⚡ Cut All {pruneAdvisorItems.length} Prunable Items (Save ~{totalSafeToCutTokens.toLocaleString()} tok)
                </button>
                <HelpTip position="left" text="Replaces all stale file reads, old turn command outputs, and old thinking blocks listed below with compact 1-line stubs in a single click." />
              </div>
            {/if}
          </div>

          {#if pruneAdvisorItems.length === 0}
            <div style="background: #064e3b; color: #a7f3d0; padding: 12px 14px; border-radius: 8px; font-size: 12.5px;">
              ✅ Your context is super clean! No bloated tool outputs or redundant old blocks left to cut.
            </div>
          {:else}
            <div style="display: flex; flex-direction: column; gap: 8px;">
              {#each pruneAdvisorItems as item}
                <div style="background: #111827; border: 1px solid #1f2937; border-left: 4px solid {item.verdict === 'cut_recommended' ? '#ef4444' : '#f59e0b'}; border-radius: 8px; padding: 10px 14px; display: flex; justify-content: space-between; align-items: center; gap: 14px;">
                  <div style="flex: 1; overflow: hidden;">
                    <div style="display: flex; align-items: center; gap: 8px; flex-wrap: wrap;">
                      <span style="font-size: 11px; padding: 2px 7px; border-radius: 4px; background: #1e293b; color: #f8fafc; font-weight: 700;">
                        Turn #{item.node.turn_seq}
                      </span>
                      <span style="font-size: 11px; padding: 2px 7px; border-radius: 999px; background: {item.verdict === 'cut_recommended' ? '#7f1d1d' : '#78350f'}; color: #fde68a; font-weight: 600;">
                        {item.verdictBadge}
                      </span>
                      <strong style="font-size: 12.5px; color: #f8fafc;">{item.label}</strong>
                      <span style="font-size: 12px; color: #38bdf8; font-weight: 700;">{item.tokens.toLocaleString()} tok</span>
                    </div>
                    <div style="font-size: 11.5px; color: #94a3b8; margin-top: 4px;">
                      💡 <strong>Why you can cut this:</strong> {item.reason}
                    </div>
                    <div style="font-size: 11px; color: #64748b; margin-top: 3px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;">
                      Preview: <code>{item.preview}</code>
                    </div>
                  </div>

                  <div style="display: flex; align-items: center; gap: 6px; flex-shrink: 0;">
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
                          `Cut ${item.label} (saved ~${item.tokens} tok)`
                        )}
                      style="background: #dc2626; color: white; border: none; border-radius: 5px; padding: 6px 12px; font-size: 12px; font-weight: 600; cursor: pointer;"
                    >
                      ✂️ Cut (-{item.tokens} tok)
                    </button>
                  </div>
                </div>
              {/each}
            </div>
          {/if}
        {:else if activeTab === 'tools'}
          <div style="display: flex; align-items: center; margin-bottom: 10px;">
            <h3 style="margin: 0; font-size: 15px;">Dynamic MCP Tool & Schema Gate</h3>
            <HelpTip text="Harnesses inject every registered MCP tool schema into every turn. Gating off tools you don't need for the current task saves thousands of tokens and improves LLM tool-selection accuracy." />
          </div>
          <div style="display: flex; flex-direction: column; gap: 8px; margin-bottom: 24px;">
            {#if mcpToolsList.length === 0}
              <div style="color: #64748b; font-size: 12.5px;">No separate MCP tool schema blocks in this transcript.</div>
            {/if}
            {#each mcpToolsList as tool}
              <div style="display: flex; justify-content: space-between; align-items: center; padding: 10px 14px; background: #111827; border: 1px solid #1f2937; border-radius: 8px;">
                <div>
                  <span style="font-size: 11px; padding: 2px 6px; background: #1e293b; color: #38bdf8; border-radius: 4px; margin-right: 8px;">{tool.server}</span>
                  <strong>{tool.name}</strong>
                  <span style="color: #94a3b8; font-size: 12px; margin-left: 8px;">{tool.description}</span>
                </div>
                <div style="display: flex; align-items: center; gap: 8px;">
                  <span style="font-size: 12px; color: #cbd5e1;">{tool.tokens} tok</span>
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
                    style="background: #dc2626; color: white; border: none; border-radius: 4px; padding: 4px 10px; font-size: 12px; cursor: pointer;"
                  >
                    Gate Off
                  </button>
                  <HelpTip position="left" text="Removes this tool's JSON schema from outgoing proxy requests for the rest of this session." />
                </div>
              </div>
            {/each}
          </div>

          <div style="display: flex; align-items: center; margin-bottom: 10px;">
            <h3 style="margin: 0; font-size: 15px;">Workspace Files in Context (Synced with IntelliJ Plugin)</h3>
            <HelpTip text="Tracks every file read by the agent. If a file was modified after an earlier read, or read multiple times, earlier copies are marked Stale/Duplicate so you can deduplicate them with 1 click." />
          </div>
          <div style="display: flex; flex-direction: column; gap: 8px;">
            {#each fileStates as fs}
              <div style="display: flex; justify-content: space-between; align-items: center; padding: 10px 14px; background: #111827; border: 1px solid #1f2937; border-radius: 8px;">
                <div>
                  <span style="margin-right: 8px;">
                    {fs.freshness === 'fresh' ? '🟢 Fresh' : fs.freshness === 'stale' ? '🟠 Stale' : fs.freshness === 'duplicate' ? '🔁 Duplicate' : '📌 Pinned'}
                  </span>
                  <code>{fs.path}</code>
                  <span style="font-size: 12px; color: #94a3b8; margin-left: 8px;">Read {fs.read_count}x (Last: Turn #{fs.last_read_turn}) • {fs.total_tokens_in_context} tok</span>
                </div>
                {#if fs.freshness === 'stale' || fs.freshness === 'duplicate'}
                  <div style="display: flex; align-items: center;">
                    <button
                      onclick={() =>
                        applyMutation(
                          { op: 'shadow_dedupe', file_path: fs.path },
                          'committed',
                          `Shadow-deduplicated stale reads of ${fs.path}`
                        )}
                      style="background: #ea580c; color: white; border: none; border-radius: 4px; padding: 4px 10px; font-size: 12px; cursor: pointer;"
                    >
                      Shadow-Dedupe File
                    </button>
                    <HelpTip position="left" text="Replaces older reads of this file in earlier turns with a short pointer to the latest version, eliminating contradictory code versions from the model's context." />
                  </div>
                {/if}
              </div>
            {/each}
          </div>
        {:else if activeTab === 'replay'}
          <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 12px;">
            <div style="display: flex; align-items: center;">
              <h3 style="margin: 0; font-size: 15px;">Deterministic Mutation Ledger ({mutations.length} Rules)</h3>
              <HelpTip text="Because agent harnesses re-send their full unmutated history on every turn, contextWatcher stores your edits here as deterministic overlays keyed by BLAKE3 fingerprint and re-applies them automatically." />
            </div>
            <div style="display: flex; align-items: center;">
              <button
                onclick={triggerForkReplay}
                style="background: #7c3aed; color: white; border: none; border-radius: 6px; padding: 7px 14px; font-weight: 600; cursor: pointer;"
              >
                🌿 Fork Current Turn & Run A/B Replay
              </button>
              <HelpTip position="left" text="Clones the current turn into a parallel branch, applies deduplication/compaction overlays, and compares the resulting token count and health score side-by-side." />
            </div>
          </div>

          {#if replayComparison}
            <div style="display: grid; grid-template-columns: 1fr 1fr; gap: 12px; margin-bottom: 20px;">
              <div style="background: #111827; padding: 12px; border-radius: 8px; border: 1px solid #374151;">
                <h4 style="margin: 0 0 6px 0; color: #94a3b8;">Original Turn #{replayComparison.original_turn.sequence_number}</h4>
                <div>Tokens: <strong>{replayComparison.original_turn.token_usage.input_tokens}</strong></div>
                <div>Health Score: <strong>{replayComparison.original_turn.health_score}/100</strong></div>
                <div>Lint Warnings: <strong>{replayComparison.original_turn.lint_findings.length}</strong></div>
              </div>
              <div style="background: #064e3b; padding: 12px; border-radius: 8px; border: 1px solid #10b981;">
                <h4 style="margin: 0 0 6px 0; color: #6ee7b7;">Forked Replay ({replayComparison.forked_turn.id})</h4>
                <div>Tokens: <strong>{replayComparison.forked_turn.token_usage.input_tokens}</strong> (-{replayComparison.saved_tokens} tok)</div>
                <div>Health Score: <strong>{replayComparison.forked_turn.health_score}/100</strong></div>
                <div>Lint Warnings: <strong>{replayComparison.forked_turn.lint_findings.length}</strong></div>
              </div>
            </div>
          {/if}

          <div style="display: flex; flex-direction: column; gap: 8px;">
            {#each mutations as m}
              <div style="display: flex; justify-content: space-between; align-items: center; padding: 10px 14px; background: #111827; border: 1px solid #1f2937; border-radius: 8px;">
                <div>
                  <span style="font-size: 11px; padding: 2px 6px; border-radius: 4px; background: {m.status === 'committed' ? '#065f46' : '#78350f'}; color: white; margin-right: 8px;">
                    {m.status.toUpperCase()}
                  </span>
                  <strong>{m.op.op}</strong>
                  <span style="font-size: 12px; color: #94a3b8; margin-left: 8px;">
                    Est. Saved: {m.estimated_saved_tokens} tok | KV Rebuild Impact: {m.estimated_kv_break_tokens} tok
                  </span>
                </div>
                <button
                  onclick={() => deleteMutation(m.id)}
                  style="background: #374151; color: #fca5a5; border: none; border-radius: 4px; padding: 4px 10px; font-size: 12px; cursor: pointer;"
                >
                  Revert
                </button>
              </div>
            {/each}
          </div>
        {/if}
      </main>
    </div>

    <!-- [3] Right Inspector & Surgical Manipulation Drawer -->
    <aside style="background: #111827; border-left: 1px solid #1f2937; padding: 14px; overflow-y: auto; display: flex; flex-direction: column; gap: 14px;">
      {#if selectedNode}
        <div>
          <div style="display: flex; align-items: center; margin-bottom: 8px;">
            <h3 style="margin: 0; font-size: 14px; color: #38bdf8;">Node Inspector & Surgical Actions</h3>
            <HelpTip position="left" text="Inspect the exact raw payload of the selected context block and apply surgical edits (Prune, Stage, Dedupe) without restarting your agent." />
          </div>

          <div style="font-size: 11.5px; color: #94a3b8; display: flex; flex-direction: column; gap: 4px; background: #0b0f19; padding: 10px; border-radius: 6px; border: 1px solid #1f2937;">
            <div><strong>Node:</strong> <code>{selectedNode.id}</code> (Turn #{selectedNode.turn_seq})</div>
            {#if selectedNode.timestamp}
              <div><strong>Timestamp:</strong> {formatTimestamp(selectedNode.timestamp)} ({formatRelativeTime(selectedNode.timestamp)})</div>
            {/if}
            <div><strong>Fingerprint:</strong> <code>{selectedNode.fingerprint}</code></div>
            <div><strong>JSON Pointer:</strong> <code>{selectedNode.raw_json_pointer}</code></div>
            <div><strong>Token Span:</strong> {selectedNode.token_span.start}..{selectedNode.token_span.end} ({selectedNode.token_span.count} tok)</div>
            <div><strong>Attention Salience:</strong> {(selectedNode.attention_salience * 100).toFixed(0)}%</div>
          </div>

          <div style="display: flex; flex-direction: column; gap: 6px; margin-top: 10px;">
            <div style="display: flex; align-items: center; justify-content: space-between;">
              <button
                onclick={() =>
                  applyMutation(
                    {
                      op: 'prune_or_stub',
                      target_fingerprint: selectedNode!.fingerprint,
                      target_call_id: null,
                      target_json_pointer: selectedNode!.raw_json_pointer,
                      stub_text: `[cw:pruned original_tokens=${selectedNode!.token_span.count} summary="Compacted via Web UI"]`
                    },
                    'committed',
                    `Pruned node ${selectedNode!.id}`
                  )}
                style="flex: 1; background: #dc2626; color: white; border: none; border-radius: 5px; padding: 6px 10px; font-size: 12px; cursor: pointer; text-align: left;"
              >
                ✂️ Prune / Stub This Node Now
              </button>
              <HelpTip position="left" text="Replaces this block's heavy text with a 1-line compact stub while keeping valid tool_use / tool_result IDs intact so the API never errors." />
            </div>

            <div style="display: flex; align-items: center; justify-content: space-between;">
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
                    `Staged Prune for ${selectedNode!.id} in Cache-Safe Draft Queue`
                  )}
                style="flex: 1; background: #d97706; color: white; border: none; border-radius: 5px; padding: 6px 10px; font-size: 12px; cursor: pointer; text-align: left;"
              >
                🕒 Stage Prune in Draft Queue (Cache-Safe)
              </button>
              <HelpTip position="left" text="Editing an old turn immediately invalidates the KV cache after that turn. Staging queues the edit until you click 'Commit Staged Drafts' so you only rebuild the KV cache once." />
            </div>

            <div style="display: flex; align-items: center; justify-content: space-between;">
              <button
                onclick={() =>
                  applyMutation(
                    { op: 'shadow_dedupe', file_path: null },
                    'committed',
                    'Shadow-deduplicated all stale & duplicate file reads'
                  )}
                style="flex: 1; background: #ea580c; color: white; border: none; border-radius: 5px; padding: 6px 10px; font-size: 12px; cursor: pointer; text-align: left;"
              >
                🔁 Shadow-Dedupe All Stale File Reads
              </button>
              <HelpTip position="left" text="Finds every file that was read multiple times or edited after being read, and replaces the older copies with a lightweight pointer to the newest read." />
            </div>
          </div>

          <pre style="background: #0b0f19; color: #cbd5e1; padding: 10px; border-radius: 6px; border: 1px solid #1f2937; font-size: 11px; max-height: 175px; overflow: auto; white-space: pre-wrap; margin-top: 10px;">{nodeDisplayText(selectedNode)}</pre>
        </div>
      {/if}

      <!-- Subtree Folding Studio -->
      <div style="border-top: 1px solid #1f2937; padding-top: 12px;">
        <div style="display: flex; align-items: center; margin-bottom: 8px;">
          <h4 style="margin: 0; font-size: 13px; color: #f8fafc;">📁 Subtree Folding (Collapse Turn Range)</h4>
          <HelpTip position="left" text="When an agent spends 5 turns failing tests before finding the fix, folding those turns into a 1-sentence outcome prevents the model from imitating its own prior errors." />
        </div>
        <div style="display: flex; gap: 6px; margin-bottom: 6px; font-size: 12px; align-items: center;">
          <span>Turns:</span>
          <input type="number" bind:value={foldStart} min="1" style="width: 44px; background: #0b0f19; color: white; border: 1px solid #374151; border-radius: 4px; padding: 3px;" />
          <span>to</span>
          <input type="number" bind:value={foldEnd} min="1" style="width: 44px; background: #0b0f19; color: white; border: 1px solid #374151; border-radius: 4px; padding: 3px;" />
          <button
            onclick={() =>
              applyMutation(
                { op: 'fold_subtree', start_turn: foldStart, end_turn: foldEnd, summary_text: foldSummary },
                'committed',
                `Folded turns ${foldStart}..${foldEnd}`
              )}
            style="background: #2563eb; color: white; border: none; border-radius: 4px; padding: 4px 10px; font-size: 12px; cursor: pointer;"
          >
            Fold Subtree
          </button>
        </div>
        <input
          type="text"
          bind:value={foldSummary}
          style="width: 100%; box-sizing: border-box; background: #0b0f19; color: #e2e8f0; border: 1px solid #374151; border-radius: 4px; padding: 5px; font-size: 12px;"
        />
      </div>

      <!-- Cache-Safe Context Pinning -->
      <div style="border-top: 1px solid #1f2937; padding-top: 12px;">
        <div style="display: flex; align-items: center; margin-bottom: 8px;">
          <h4 style="margin: 0; font-size: 13px; color: #f8fafc;">📌 Pin to High-Attention Tail (0% KV Break)</h4>
          <HelpTip position="left" text="Appends a critical rule or reminder to the very end of the prompt (the highest-attention recency zone) without modifying the cached prefix—giving you maximum instruction adherence with zero KV cache penalty!" />
        </div>
        <div style="display: flex; gap: 6px; margin-bottom: 6px;">
          <input
            type="text"
            bind:value={pinLabel}
            placeholder="Label"
            style="width: 120px; background: #0b0f19; color: white; border: 1px solid #374151; border-radius: 4px; padding: 4px; font-size: 12px;"
          />
          <button
            onclick={() =>
              applyMutation(
                { op: 'pin_context', label: pinLabel, content: pinContent, source_node_id: selectedNode?.id ?? null },
                'committed',
                `Pinned '${pinLabel}' to prompt tail`
              )}
            style="background: #059669; color: white; border: none; border-radius: 4px; padding: 4px 10px; font-size: 12px; cursor: pointer;"
          >
            Pin Reminder
          </button>
        </div>
        <input
          type="text"
          bind:value={pinContent}
          style="width: 100%; box-sizing: border-box; background: #0b0f19; color: #e2e8f0; border: 1px solid #374151; border-radius: 4px; padding: 5px; font-size: 12px;"
        />
      </div>

      <!-- Active Context Health Linter Findings -->
      {#if latestTurn}
        <div style="border-top: 1px solid #1f2937; padding-top: 12px;">
          <div style="display: flex; align-items: center; margin-bottom: 8px;">
            <h4 style="margin: 0; font-size: 13px; color: #f8fafc;">🩺 Context Health Lints ({latestTurn.lint_findings.length})</h4>
            <HelpTip position="left" text="Automated diagnostics scanning for 7 context rot patterns (CW-L001 through CW-L007). Click 'Fix' on any finding to apply its recommended surgical mutation." />
          </div>
          {#each latestTurn.lint_findings as lint}
            <div style="background: #0b0f19; border-left: 3px solid {lint.severity === 'critical' ? '#ef4444' : '#f59e0b'}; padding: 8px 10px; border-radius: 4px; margin-bottom: 8px; font-size: 12px;">
              <div style="display: flex; justify-content: space-between; align-items: center;">
                <strong>[{lint.rule_id}] {lint.title}</strong>
                {#if lint.suggested_mutation}
                  <button
                    onclick={() => applyMutation(lint.suggested_mutation!, 'committed', `Fixed ${lint.rule_id}: ${lint.title}`)}
                    style="background: #2563eb; color: white; border: none; border-radius: 4px; padding: 2px 8px; font-size: 11px; cursor: pointer;"
                  >
                    Fix ({lint.wasted_tokens} tok)
                  </button>
                {/if}
              </div>
              <div style="color: #94a3b8; margin-top: 4px;">{lint.message}</div>
            </div>
          {/each}
        </div>
      {/if}
    </aside>
  </div>
</div>
