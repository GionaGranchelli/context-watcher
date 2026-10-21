<script lang="ts">
  import { hierarchy, treemap } from 'd3-hierarchy';
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

  let activeTab = $state<'radix' | 'treemap' | 'tools' | 'replay'>('radix');
  let selectedNodeId = $state<string | null>(null);
  let wsConnected = $state<boolean>(false);
  let statusBanner = $state<string>('');
  let replayComparison = $state<{ original_turn: ContextTurn; forked_turn: ContextTurn; saved_tokens: number } | null>(null);

  // Fold form state
  let foldStart = $state<number>(1);
  let foldEnd = $state<number>(2);
  let foldSummary = $state<string>('Tried debugging branch; resolved root cause in config.');

  // Pin form state
  let pinLabel = $state<string>('Architecture Rule');
  let pinContent = $state<string>('Preserve lossless JSON pointers and KV cache prefix boundaries.');

  let selectedNode = $derived<ContextNode | null>(
    latestTurn?.nodes.find((n) => n.id === selectedNodeId) ?? latestTurn?.nodes[0] ?? null
  );

  let draftMutations = $derived(mutations.filter((m) => m.status === 'draft'));
  let committedMutations = $derived(mutations.filter((m) => m.status === 'committed'));

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
    const layout = treemap<any>().size([760, 340]).paddingInner(4).paddingOuter(4);
    return layout(root).leaves();
  });

  // Extract MCP tools from latestTurn
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
        activeSessionId = sessions[0].id;
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
        'You are Antigravity, an agentic coding assistant.\n<RULE[/workspace/AGENTS.md]>\nZero-Drop Proxy Transparency, KV-Cache Awareness First, and Deterministic Mutation Overlays.\n</RULE[/workspace/AGENTS.md]>\n<skills>\n- ponytail (/skills/ponytail/SKILL.md): Write minimal, direct code.\n- chrome-devtools (/skills/chrome-devtools/SKILL.md): Debug web pages.\n</skills>',
      tools: [
        { name: 'read_file', description: 'Read file from disk', input_schema: { type: 'object', properties: { Path: { type: 'string' } } } },
        { name: 'edit_file', description: 'Edit file on disk', input_schema: { type: 'object', properties: { Path: { type: 'string' }, Replacement: { type: 'string' } } } },
        { name: 'run_command', description: 'Execute bash command', input_schema: { type: 'object', properties: { CommandLine: { type: 'string' } } } },
        { name: 'mcp_chrome_click', description: 'Click DOM node in Chrome DevTools MCP', input_schema: { type: 'object', properties: { uid: { type: 'string' } } } },
        { name: 'mcp_chrome_take_snapshot', description: 'Take accessibility tree snapshot', input_schema: { type: 'object', properties: {} } }
      ],
      messages: [
        { role: 'user', content: 'Inspect crates/cw-core/src/radix.rs and optimize KV cache prefix diffing.' },
        {
          role: 'assistant',
          content: [
            { type: 'thinking', thinking: 'Let me read crates/cw-core/src/radix.rs first to inspect the LCP loop.', signature: 'sig_demo_1' },
            { type: 'tool_use', id: 'call_read_1', name: 'read_file', input: { Path: 'crates/cw-core/src/radix.rs', StartLine: 1, EndLine: 120 } }
          ]
        },
        {
          role: 'user',
          content: [
            {
              type: 'tool_result',
              tool_use_id: 'call_read_1',
              content: 'pub fn analyze_kv_cache(prev_nodes: Option<&[ContextNode]>, curr_nodes: &mut [ContextNode]) -> KvCacheReport {\n    // v1 implementation before edit\n    KvCacheReport::default()\n}\n'.repeat(12)
            }
          ]
        },
        {
          role: 'assistant',
          content: [
            { type: 'tool_use', id: 'call_edit_1', name: 'edit_file', input: { Path: 'crates/cw-core/src/radix.rs', Replacement: 'pub fn analyze_kv_cache_v2() {}' } }
          ]
        },
        {
          role: 'user',
          content: [{ type: 'tool_result', tool_use_id: 'call_edit_1', content: 'Updated crates/cw-core/src/radix.rs' }]
        },
        {
          role: 'assistant',
          content: [
            { type: 'tool_use', id: 'call_read_2', name: 'read_file', input: { Path: 'crates/cw-core/src/radix.rs', StartLine: 1, EndLine: 140 } }
          ]
        },
        {
          role: 'user',
          content: [
            {
              type: 'tool_result',
              tool_use_id: 'call_read_2',
              content: 'pub fn analyze_kv_cache(prev_nodes: Option<&[ContextNode]>, curr_nodes: &mut [ContextNode]) -> KvCacheReport {\n    // v2 updated implementation with full Longest Common Prefix diffing\n}\n'.repeat(10)
            }
          ]
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
    statusBanner = 'Loaded interactive multi-turn session with Stale File, Duplicate Read, and MCP Tools.';
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
</script>

<div style="display: flex; flex-direction: column; height: 100vh; overflow: hidden;">
  <!-- Top Telemetry Ribbon -->
  <header style="display: flex; align-items: center; justify-content: space-between; padding: 10px 18px; background: #111827; border-bottom: 1px solid #1f2937; gap: 16px;">
    <div style="display: flex; align-items: center; gap: 12px;">
      <span style="font-weight: 800; font-size: 18px; color: #38bdf8; letter-spacing: -0.02em;">contextWatcher</span>
      <span style="font-size: 11px; padding: 2px 8px; border-radius: 999px; background: {wsConnected ? '#064e3b' : '#7f1d1d'}; color: {wsConnected ? '#34d399' : '#fca5a5'};">
        {wsConnected ? '● Live WS' : '○ Offline'}
      </span>

      <select
        value={activeSessionId}
        onchange={(e) => {
          activeSessionId = (e.target as HTMLSelectElement).value;
          fetchSessionGraph(activeSessionId);
        }}
        style="background: #1f2937; color: #f8fafc; border: 1px solid #374151; border-radius: 6px; padding: 5px 10px; font-size: 13px;"
      >
        {#if sessions.length === 0}
          <option value="">No active sessions</option>
        {/if}
        {#each sessions as s}
          <option value={s.id}>{s.id} ({s.harness} • {s.model})</option>
        {/each}
      </select>

      <button
        onclick={seedRichDemoSession}
        style="background: #1e293b; color: #93c5fd; border: 1px solid #3b82f6; border-radius: 6px; padding: 5px 10px; font-size: 12px; cursor: pointer;"
      >
        + Load Demo Session
      </button>
    </div>

    {#if latestTurn}
      <div style="display: flex; align-items: center; gap: 18px; font-size: 13px;">
        <div>
          <span style="color: #94a3b8;">Context:</span>
          <strong style="color: #f8fafc;">{latestTurn.token_usage.input_tokens.toLocaleString()}</strong>
          <span style="color: #64748b;">/ {(latestTurn.token_usage.context_limit || 200000).toLocaleString()} tok</span>
          {#if latestTurn.token_usage.saved_by_mutations > 0}
            <span style="color: #34d399; margin-left: 6px;">(-{latestTurn.token_usage.saved_by_mutations} saved)</span>
          {/if}
        </div>

        <div style="display: flex; align-items: center; gap: 8px;">
          <span style="color: #22c55e;">🟢 {latestTurn.kv_cache_report.hit_tokens} Hit</span>
          <span style="color: #eab308;">🟡 {latestTurn.kv_cache_report.newly_cached_tokens} New</span>
          {#if latestTurn.kv_cache_report.invalidated_tokens > 0}
            <span style="color: #ef4444; font-weight: 700;">🔴 {latestTurn.kv_cache_report.invalidated_tokens} Invalidated</span>
          {/if}
        </div>

        <div style="padding: 4px 10px; border-radius: 6px; font-weight: 700; background: {latestTurn.health_score >= 80 ? '#064e3b' : latestTurn.health_score >= 55 ? '#78350f' : '#7f1d1d'};">
          Health: {latestTurn.health_score}/100
        </div>

        {#if draftMutations.length > 0}
          <button
            onclick={commitDrafts}
            style="background: #d97706; color: #fff; border: none; border-radius: 6px; padding: 6px 12px; font-weight: 600; cursor: pointer;"
          >
            Commit {draftMutations.length} Staged Drafts
          </button>
        {/if}

        <button
          onclick={oneClickOptimize}
          style="background: #2563eb; color: #fff; border: none; border-radius: 6px; padding: 6px 12px; font-weight: 600; cursor: pointer;"
        >
          ⚡ 1-Click Optimize ({latestTurn.lint_findings.length})
        </button>
      </div>
    {/if}
  </header>

  {#if statusBanner}
    <div style="background: #1e3a8a; color: #dbeafe; padding: 6px 18px; font-size: 12px; display: flex; justify-content: space-between;">
      <span>{statusBanner}</span>
      <button onclick={() => (statusBanner = '')} style="background: transparent; border: none; color: #93c5fd; cursor: pointer;">✕</button>
    </div>
  {/if}

  <!-- Firewall Interceptor Alert Banner -->
  {#each intercepts as item}
    <div style="background: #7f1d1d; color: #fecaca; padding: 10px 18px; display: flex; justify-content: space-between; align-items: center; border-bottom: 1px solid #ef4444;">
      <div>
        <strong>🛑 Context Firewall Paused In-Flight Request:</strong> Rule <code>{item.rule_name}</code> triggered on Turn #{item.turn_seq}
      </div>
      <div style="display: flex; gap: 8px;">
        <button onclick={() => resolveIntercept(item.intercept_id, 'approve')} style="background: #16a34a; color: white; border: none; padding: 5px 12px; border-radius: 4px; cursor: pointer;">Approve & Forward</button>
        <button onclick={() => resolveIntercept(item.intercept_id, 'abort')} style="background: #dc2626; color: white; border: none; padding: 5px 12px; border-radius: 4px; cursor: pointer;">Abort Request</button>
      </div>
    </div>
  {/each}

  <!-- View Navigation Tabs -->
  <nav style="display: flex; gap: 4px; padding: 8px 18px; background: #0f172a; border-bottom: 1px solid #1e293b;">
    {#each [
      ['radix', '🌿 1. Context Radix Tree & DAG'],
      ['treemap', '📊 2. Token Treemap & KV Flamegraph'],
      ['tools', '🛠️ 3. Tool Gate & File Freshness (IDE Sync)'],
      ['replay', '⏪ 4. Mutation Ledger & Time-Travel Replay']
    ] as [id, label]}
      <button
        onclick={() => (activeTab = id as any)}
        style="padding: 7px 14px; border-radius: 6px; border: none; font-size: 13px; font-weight: 600; cursor: pointer; background: {activeTab === id ? '#2563eb' : 'transparent'}; color: {activeTab === id ? '#ffffff' : '#94a3b8'};"
      >
        {label}
      </button>
    {/each}
  </nav>

  <!-- Main Workspace Split -->
  <div style="display: grid; grid-template-columns: 1fr 430px; flex: 1; overflow: hidden;">
    <!-- Left Canvas Area -->
    <main style="overflow-y: auto; padding: 18px; background: #0b0f19;">
      {#if !latestTurn}
        <div style="text-align: center; padding: 80px 20px; color: #64748b;">
          <h2 style="color: #e2e8f0; margin-bottom: 8px;">No Active LLM Session Selected</h2>
          <p style="margin-bottom: 20px;">Point your harness to <code>http://127.0.0.1:4401</code> via <code>cw wrap agy</code> / <code>cw wrap codex</code>, or click below to load a live demo trajectory.</p>
          <button
            onclick={seedRichDemoSession}
            style="background: #2563eb; color: white; border: none; padding: 10px 20px; border-radius: 8px; font-weight: 600; cursor: pointer;"
          >
            Load Interactive Multi-Turn Demo Session
          </button>
        </div>
      {:else if activeTab === 'radix'}
        <div style="display: flex; flex-direction: column; gap: 10px;">
          <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 6px;">
            <h3 style="margin: 0; font-size: 15px; color: #f8fafc;">Context Radix Prefix Tree & Turn Spine</h3>
            <span style="font-size: 12px; color: #94a3b8;">Click any node to inspect raw JSON pointer & apply surgical mutations</span>
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
              style="display: flex; align-items: center; justify-content: space-between; padding: 10px 14px; border-radius: 8px; background: {selectedNodeId === gnode.node_id && gnode.node_id ? '#1e293b' : '#111827'}; border-left: 5px solid {kvColor(gnode.kv_status)}; border-top: 1px solid #1f2937; border-right: 1px solid #1f2937; border-bottom: 1px solid #1f2937; cursor: pointer; margin-left: {gnode.group === 'root' ? 0 : gnode.group === 'fork' ? 48 : 24}px;"
            >
              <div style="display: flex; align-items: center; gap: 10px; flex-wrap: wrap;">
                <span style="font-size: 11px; padding: 2px 6px; border-radius: 4px; background: #1f2937; color: #94a3b8;">
                  {gnode.group}
                </span>
                <strong style="font-size: 13px; color: #f1f5f9;">{gnode.label}</strong>
                {#each gnode.lint_badges as badge}
                  <span style="font-size: 11px; padding: 2px 8px; border-radius: 999px; background: #78350f; color: #fde68a;">
                    ⚠️ {badge}
                  </span>
                {/each}
              </div>
              <div style="display: flex; align-items: center; gap: 12px; font-size: 12px;">
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
        <h3 style="margin-top: 0; font-size: 15px;">Proportional Token Treemap (d3-hierarchy)</h3>
        <svg width="760" height="340" style="background: #111827; border-radius: 8px; border: 1px solid #1f2937;">
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

        <h3 style="margin-top: 24px; font-size: 15px;">KV Cache Prefix Tape & "Lost in the Middle" Attention Curve</h3>
        <div style="background: #111827; padding: 14px; border-radius: 8px; border: 1px solid #1f2937;">
          <div style="display: flex; height: 28px; border-radius: 6px; overflow: hidden; margin-bottom: 12px;">
            {#each latestTurn.nodes as n}
              <div
                title="{n.id}: {n.token_span.count} tok ({n.kv_status}) | Attention: {(n.attention_salience * 100).toFixed(0)}%"
                style="flex: {n.token_span.count}; background: {kvColor(n.kv_status)}; border-right: 1px solid #0b0f19;"
              ></div>
            {/each}
          </div>
          <div style="display: flex; align-items: flex-end; height: 70px; gap: 2px; border-bottom: 1px solid #334155;">
            {#each latestTurn.nodes as n}
              <div
                title="Attention Salience: {(n.attention_salience * 100).toFixed(0)}%"
                style="flex: 1; height: {Math.round(n.attention_salience * 68)}px; background: {n.attention_salience < 0.4 ? '#f97316' : '#38bdf8'}; border-radius: 2px 2px 0 0;"
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
        <h3 style="margin-top: 0; font-size: 15px;">Dynamic MCP Tool & Schema Gate</h3>
        <div style="display: flex; flex-direction: column; gap: 8px; margin-bottom: 24px;">
          {#each mcpToolsList as tool}
            <div style="display: flex; justify-content: space-between; align-items: center; padding: 10px 14px; background: #111827; border: 1px solid #1f2937; border-radius: 8px;">
              <div>
                <span style="font-size: 11px; padding: 2px 6px; background: #1e293b; color: #38bdf8; border-radius: 4px; margin-right: 8px;">{tool.server}</span>
                <strong>{tool.name}</strong>
                <span style="color: #94a3b8; font-size: 12px; margin-left: 8px;">{tool.description}</span>
              </div>
              <div style="display: flex; align-items: center; gap: 12px;">
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
              </div>
            </div>
          {/each}
        </div>

        <h3 style="font-size: 15px;">Workspace Files in Context (Synced with IntelliJ Plugin)</h3>
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
              {/if}
            </div>
          {/each}
        </div>
      {:else if activeTab === 'replay'}
        <div style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 12px;">
          <h3 style="margin: 0; font-size: 15px;">Deterministic Mutation Ledger ({mutations.length} Rules)</h3>
          <button
            onclick={triggerForkReplay}
            style="background: #7c3aed; color: white; border: none; border-radius: 6px; padding: 7px 14px; font-weight: 600; cursor: pointer;"
          >
            🌿 Fork Current Turn & Run A/B Replay
          </button>
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

    <!-- Right Inspector & Surgical Manipulation Drawer -->
    <aside style="background: #111827; border-left: 1px solid #1f2937; padding: 16px; overflow-y: auto; display: flex; flex-direction: column; gap: 16px;">
      {#if selectedNode}
        <div>
          <h3 style="margin: 0 0 8px 0; font-size: 14px; color: #38bdf8;">Node Inspector & Surgical Actions</h3>
          <div style="font-size: 12px; color: #94a3b8; display: flex; flex-direction: column; gap: 4px; background: #0b0f19; padding: 10px; border-radius: 6px; border: 1px solid #1f2937;">
            <div><strong>ID:</strong> <code>{selectedNode.id}</code> (Turn #{selectedNode.turn_seq})</div>
            <div><strong>Fingerprint:</strong> <code>{selectedNode.fingerprint}</code></div>
            <div><strong>JSON Pointer:</strong> <code>{selectedNode.raw_json_pointer}</code></div>
            <div><strong>Token Span:</strong> {selectedNode.token_span.start}..{selectedNode.token_span.end} ({selectedNode.token_span.count} tok)</div>
            <div><strong>Attention Salience:</strong> {(selectedNode.attention_salience * 100).toFixed(0)}%</div>
          </div>

          <div style="display: flex; flex-wrap: wrap; gap: 6px; margin-top: 10px;">
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
              style="background: #dc2626; color: white; border: none; border-radius: 5px; padding: 6px 10px; font-size: 12px; cursor: pointer;"
            >
              ✂️ Prune / Stub Node
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
                  `Staged Prune for ${selectedNode!.id} in Cache-Safe Draft Queue`
                )}
              style="background: #d97706; color: white; border: none; border-radius: 5px; padding: 6px 10px; font-size: 12px; cursor: pointer;"
            >
              🕒 Stage in Draft Queue
            </button>

            <button
              onclick={() =>
                applyMutation(
                  { op: 'shadow_dedupe', file_path: null },
                  'committed',
                  'Shadow-deduplicated all stale & duplicate file reads'
                )}
              style="background: #ea580c; color: white; border: none; border-radius: 5px; padding: 6px 10px; font-size: 12px; cursor: pointer;"
            >
              🔁 Shadow-Dedupe Reads
            </button>
          </div>

          <pre style="background: #0b0f19; color: #cbd5e1; padding: 10px; border-radius: 6px; border: 1px solid #1f2937; font-size: 11px; max-height: 190px; overflow: auto; white-space: pre-wrap;">{nodeDisplayText(selectedNode)}</pre>
        </div>
      {/if}

      <!-- Subtree Folding Studio -->
      <div style="border-top: 1px solid #1f2937; padding-top: 12px;">
        <h4 style="margin: 0 0 8px 0; font-size: 13px; color: #f8fafc;">📁 Subtree Folding (Trajectory Compression)</h4>
        <div style="display: flex; gap: 6px; margin-bottom: 6px; font-size: 12px; align-items: center;">
          <span>Turns:</span>
          <input type="number" bind:value={foldStart} min="1" style="width: 48px; background: #0b0f19; color: white; border: 1px solid #374151; border-radius: 4px; padding: 3px;" />
          <span>to</span>
          <input type="number" bind:value={foldEnd} min="1" style="width: 48px; background: #0b0f19; color: white; border: 1px solid #374151; border-radius: 4px; padding: 3px;" />
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
        <h4 style="margin: 0 0 8px 0; font-size: 13px; color: #f8fafc;">📌 Pin to High-Attention Tail (0% KV Break)</h4>
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
          <h4 style="margin: 0 0 8px 0; font-size: 13px; color: #f8fafc;">🩺 Context Health Lints ({latestTurn.lint_findings.length})</h4>
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
