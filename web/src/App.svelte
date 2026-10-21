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
    PendingInterceptInfo
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

  let activeTab = $state<'story' | 'radix' | 'treemap' | 'tools' | 'replay'>('story');
  let selectedNodeId = $state<string | null>(null);
  let wsConnected = $state<boolean>(false);
  let statusBanner = $state<string>('');
  let replayComparison = $state<{ original_turn: ContextTurn; forked_turn: ContextTurn; saved_tokens: number } | null>(null);

  // Project & Session Sidebar filters
  let searchQuery = $state<string>('');
  let selectedProjectFilter = $state<string>('ALL');
  let collapsedProjects = $state<Record<string, boolean>>({});

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
  let treemapLeaves = $derived.by(() => {
    if (!latestTurn) return [];
    const cb = latestTurn.category_breakdown;
    const data = {
      name: 'root',
      children: [
        { name: 'MCP & Native Tools', value: cb.mcp_and_native_tools, color: '#6366f1' },
        { name: 'System & Identity', value: cb.system_and_identity, color: '#0ea5e9' },
        { name: 'Rules & Instructions', value: cb.rules_and_instructions, color: '#14b8a6' },
        { name: 'Skills Catalog', value: cb.skills_catalog, color: '#8b5cf6' },
        { name: 'File Reads (Fresh)', value: cb.file_reads_fresh, color: '#22c55e' },
        { name: 'File Reads (Stale/Dup)', value: cb.file_reads_stale_or_duplicate, color: '#f97316' },
        { name: 'Command Outputs', value: cb.command_outputs, color: '#eab308' },
        { name: 'Assistant Thinking', value: cb.assistant_thinking, color: '#ec4899' },
        { name: 'Conversation & Calls', value: cb.assistant_responses_and_calls + cb.user_prompts, color: '#3b82f6' },
        { name: 'Pinned & Folded', value: cb.pinned_and_folded, color: '#10b981' }
      ].filter((d) => d.value > 0)
    };
    const root = hierarchy<any>(data).sum((d: any) => d.value || 0).sort((a, b) => (b.value || 0) - (a.value || 0));
    const layout = treemap<any>().size([740, 320]).paddingInner(4).paddingOuter(4);
    return layout(root).leaves();
  });

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
    if (!activeSessionId) return;
    await fetch(`/api/v1/sessions/${activeSessionId}/optimize`, { method: 'POST' });
    statusBanner = 'Applied 1-Click Context Optimization (Shadow-Dedupe + Prefix Normalization + Loop Folding).';
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

      <div style="display: flex; align-items: center;">
        <button
          onclick={seedRichDemoSession}
          style="background: #1e293b; color: #93c5fd; border: 1px solid #3b82f6; border-radius: 6px; padding: 5px 10px; font-size: 12px; cursor: pointer;"
        >
          + Load Demo Session
        </button>
        <HelpTip text="Creates a sample multi-turn coding session with Stale File Reads, Duplicate Reads, and MCP Tool schemas so you can safely test every button and view." />
      </div>
    </div>

    {#if latestTurn}
      <div style="display: flex; align-items: center; gap: 16px; font-size: 13px;">
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
          <HelpTip text="KV Cache X-Ray: Green = tokens reused from the provider's memory cache (fast & up to 90% cheaper). Yellow = new tokens added this turn. Red = previously cached tokens that were evicted because an earlier part of the prompt changed." />
        </div>

        <div style="display: flex; align-items: center;">
          <div style="padding: 4px 10px; border-radius: 6px; font-weight: 700; background: {latestTurn.health_score >= 80 ? '#064e3b' : latestTurn.health_score >= 55 ? '#78350f' : '#7f1d1d'};">
            Health: {latestTurn.health_score}/100
          </div>
          <HelpTip text="Context Health Score (0–100): Penalizes context rot such as stale file reads (where the file on disk changed after the LLM read it), duplicate file reads, failed command loops, and bloated MCP tool schemas." />
        </div>

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
            ⚡ 1-Click Optimize ({latestTurn.lint_findings.length})
          </button>
          <HelpTip position="left" text="Automatically fixes all detected Context Health issues: replaces older duplicate/stale file reads with lightweight pointers, folds failed debugging loops, and stabilizes prefix ordering." />
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
          ['radix', '🌿 2. Context Radix Tree', 'Low-level token prefix tree showing exact JSON pointers, KV cache hits/breaks, and lint warnings on every block in the context window.'],
          ['treemap', '📊 3. Treemap & Attention', 'Visual breakdown of where your tokens are spent (MCP tools vs File Reads vs Thinking) and the U-shaped Lost-in-the-Middle attention curve.'],
          ['tools', '🛠️ 4. Tool Gate & Files', 'Toggle off unused MCP tools to save tokens, and check which project files in the LLM context are Fresh vs Stale.'],
          ['replay', '⏪ 5. Overlays & Replay', 'Manage active context mutations and fork any turn to run an A/B comparison replay.']
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
          <div style="display: flex; align-items: center; margin-bottom: 8px;">
            <h3 style="margin: 0; font-size: 15px;">Proportional Token Treemap (d3-hierarchy)</h3>
            <HelpTip text="Sizes each rectangle proportionally to how many tokens that category consumes in the current context window. Helps you immediately spot if MCP Tool Schemas or Stale File Reads are dominating your budget." />
          </div>
          <svg width="740" height="320" style="background: #111827; border-radius: 8px; border: 1px solid #1f2937; max-width: 100%;">
            {#each treemapLeaves as leaf}
              <g transform="translate({leaf.x0},{leaf.y0})">
                <rect
                  width={Math.max(0, leaf.x1 - leaf.x0)}
                  height={Math.max(0, leaf.y1 - leaf.y0)}
                  fill={leaf.data.color}
                  rx="4"
                  opacity="0.85"
                />
                {#if leaf.x1 - leaf.x0 > 70 && leaf.y1 - leaf.y0 > 32}
                  <text x="8" y="18" fill="#ffffff" font-size="12" font-weight="bold">{leaf.data.name}</text>
                  <text x="8" y="34" fill="#e2e8f0" font-size="11">{leaf.value?.toLocaleString()} tok</text>
                {/if}
              </g>
            {/each}
          </svg>

          <div style="display: flex; align-items: center; margin-top: 22px; margin-bottom: 8px;">
            <h3 style="margin: 0; font-size: 15px;">KV Cache Prefix Tape & "Lost in the Middle" Attention Curve</h3>
            <HelpTip text="LLMs pay the strongest attention to the very beginning (System Prompt) and very end (Recency Tail) of the context window. Instructions in the middle trough (<40% salience, shown in orange) are much more likely to be forgotten unless Pinned to the tail." />
          </div>
          <div style="background: #111827; padding: 14px; border-radius: 8px; border: 1px solid #1f2937;">
            <div style="display: flex; height: 26px; border-radius: 6px; overflow: hidden; margin-bottom: 12px;">
              {#each latestTurn.nodes as n}
                <div
                  title="{n.id}: {n.token_span.count} tok ({n.kv_status}) | Attention: {(n.attention_salience * 100).toFixed(0)}%"
                  style="flex: {n.token_span.count}; background: {kvColor(n.kv_status)}; border-right: 1px solid #0b0f19;"
                ></div>
              {/each}
            </div>
            <div style="display: flex; align-items: flex-end; height: 68px; gap: 2px; border-bottom: 1px solid #334155;">
              {#each latestTurn.nodes as n}
                <div
                  title="Attention Salience: {(n.attention_salience * 100).toFixed(0)}%"
                  style="flex: 1; height: {Math.round(n.attention_salience * 65)}px; background: {n.attention_salience < 0.4 ? '#f97316' : '#38bdf8'}; border-radius: 2px 2px 0 0;"
                ></div>
              {/each}
            </div>
            <div style="display: flex; justify-content: space-between; font-size: 11px; color: #94a3b8; margin-top: 6px;">
              <span>Token 0 (Primacy Zone — High Attention)</span>
              <span>Middle Trough (Attention Dead Zone &lt; 40%)</span>
              <span>Token {latestTurn.token_usage.input_tokens} (Recency Tail — Highest Attention)</span>
            </div>
          </div>
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
