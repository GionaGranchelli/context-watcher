export type PolicyRequest = (path: string, init?: RequestInit) => Promise<Response>;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
function record(value: unknown): Record<string, unknown> {
  if (!isRecord(value)) throw new Error('Invalid policy preview');
  return value;
}
function text(value: unknown): string {
  if (typeof value !== 'string') throw new Error('Invalid policy preview text');
  return value;
}
function flag(value: unknown): boolean {
  if (typeof value !== 'boolean') throw new Error('Invalid policy preview state');
  return value;
}
function sessionPath(id: string): string {
  if (!/^[A-Za-z0-9_-]{1,128}$/.test(id)) throw new Error('Invalid session ID');
  return `/api/v1/sessions/${id}/pin-context-policy`;
}
export function parsePinPolicy(value: unknown, sessionId: string) {
  const body = record(value);
  if (body.session_id !== sessionId) throw new Error('Policy response belongs to a different session');
  const warning = text(body.risk_warning);
  const scope = text(body.activation_scope);
  if (!warning.trim() || !scope.trim()) throw new Error('Policy risk and scope disclosure unavailable');
  if (!Array.isArray(body.stored_pins)) throw new Error('Stored pins unavailable');
  const automatic = record(body.automatic_generation);
  return {
    sessionId,
    enabled: flag(body.enabled),
    terminated: flag(body.session_terminated),
    supported: flag(body.reinforcement_supported),
    warning, scope, basis: text(body.preview_basis),
    mayGenerate: flag(automatic.may_generate_after_opt_in),
    autoPinConfigured: flag(automatic.auto_cache_safe_pin),
    recoveryConfigured: flag(automatic.auto_heal_low_health),
    recoveryStrategy: text(automatic.heal_strategy),
    pins: body.stored_pins.map((value: unknown) => {
      const pin = record(value);
      const status = text(pin.status);
      if (!['draft', 'committed', 'disabled'].includes(status)) throw new Error('Invalid stored pin status');
      const eligible = pin.eligible_in_latest_request_preview_after_opt_in;
      if (eligible !== null && typeof eligible !== 'boolean') throw new Error('Pin eligibility unavailable');
      return { id: text(pin.rule_id), label: text(pin.label), content: text(pin.content), status, eligible };
    }),
  };
}
export type PinPolicyPreview = ReturnType<typeof parsePinPolicy>;
export type PinPolicyState =
  | { kind: 'empty' }
  | { kind: 'loading' | 'saving'; sessionId: string }
  | { kind: 'error'; sessionId: string; message: string }
  | { kind: 'ready'; sessionId: string; preview: PinPolicyPreview; acknowledged: boolean; notice: string };

export function createPinPolicyWorkflow({ request, onChange }: { request: PolicyRequest; onChange: (state: PinPolicyState) => void }) {
  let state: PinPolicyState = { kind: 'empty' };
  let selectedId = '';
  let epoch = 0;
  function publish(next: PinPolicyState) { state = next; onChange(next); }
  async function read(response: Response, id: string) {
    if (!response.ok) throw new Error(`Policy API failed (HTTP ${response.status}). Consent state is unconfirmed; refresh before retrying.`);
    const value: unknown = await response.json();
    return parsePinPolicy(value, id);
  }
  async function load(id: string) {
    selectedId = id;
    const current = ++epoch;
    if (!id) { publish({ kind: 'empty' }); return; }
    publish({ kind: 'loading', sessionId: id });
    try {
      const preview = await read(await request(sessionPath(id)), id);
      if (current === epoch) publish({ kind: 'ready', sessionId: id, preview, acknowledged: false, notice: '' });
    } catch (error) {
      if (current === epoch) publish({ kind: 'error', sessionId: id, message: error instanceof Error ? error.message : 'Policy API unavailable; consent state is unconfirmed.' });
    }
  }
  async function change(enabled: boolean) {
    const id = selectedId;
    if (!id || state.kind === 'saving') return;
    if (enabled && (state.kind !== 'ready' || !state.acknowledged || state.preview.enabled || state.preview.terminated || !state.preview.supported)) return;
    const current = ++epoch;
    publish({ kind: 'saving', sessionId: id });
    try {
      const preview = await read(await request(sessionPath(id), {
        method: 'POST', headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ enabled, acknowledge_risk: enabled }),
      }), id);
      if (preview.enabled !== enabled) throw new Error('The API did not confirm the requested consent state. Refresh to inspect it.');
      if (current === epoch) publish({ kind: 'ready', sessionId: id, preview, acknowledged: false,
        notice: enabled ? 'Reinforcement authorized for this session. Provider-request application is not established by this preview.' : 'Consent revoked. Stored rules were not deleted.' });
    } catch (error) {
      if (current === epoch) publish({ kind: 'error', sessionId: id, message: error instanceof Error ? error.message : 'Consent update is unconfirmed. Refresh to inspect it.' });
    }
  }
  return {
    load,
    clear() { selectedId = ''; ++epoch; publish({ kind: 'empty' }); },
    acknowledge(value: boolean) { if (state.kind === 'ready') publish({ ...state, acknowledged: value && !state.preview.enabled && !state.preview.terminated && state.preview.supported }); },
    enable: () => change(true), disable: () => change(false),
    getState: () => state,
  };
}
