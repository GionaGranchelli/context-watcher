import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import ts from 'typescript';
const source = await readFile(new URL('../src/lib/pinPolicy.ts', import.meta.url), 'utf8');
const { outputText } = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext } });
const { createPinPolicyWorkflow } = await import(`data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`);
const fixture = (id = 'session-a', enabled = false) => ({
  session_id: id, enabled, session_terminated: false, reinforcement_supported: true,
  risk_warning: 'PinContext can increase latency, cache disruption, cost and behavioral errors.',
  activation_scope: 'All eligible stored and future pins in this session until revoked or ended.',
  preview_basis: 'Latest request only; not provider execution.',
  automatic_generation: { may_generate_after_opt_in: true, auto_cache_safe_pin: true, auto_heal_low_health: false, heal_strategy: 'surgical_log_diet' },
  stored_pins: [{ rule_id: 'draft-1', label: 'Early constraint', content: 'Preserve the public API.', status: 'draft', eligible_in_latest_request_preview_after_opt_in: false }],
});
const response = (body, status = 200) => new Response(JSON.stringify(body), { status });
function ready(flow) { const state = flow.getState(); assert.equal(state.kind, 'ready'); return state; }

test('preview exposes inactive contents and future automatic scope without a write', async () => {
  const calls = [];
  const flow = createPinPolicyWorkflow({ request: async (path, init) => { calls.push({ path, init }); return response(fixture()); }, onChange() {} });
  await flow.load('session-a');
  const state = ready(flow);
  assert.equal(state.preview.enabled, false);
  assert.equal(state.preview.pins[0].content, 'Preserve the public API.');
  assert.equal(state.preview.pins[0].status, 'draft');
  assert.equal(state.preview.mayGenerate, true);
  assert.match(state.preview.scope, /future pins/);
  assert.match(state.preview.warning, /latency.*cache.*cost.*behavior/);
  await flow.enable();
  assert.equal(calls.length, 1, 'No POST without deliberate acknowledgement');
});

test('explicit acknowledgement authorizes; revocation never deletes rules', async () => {
  const calls = [];
  const flow = createPinPolicyWorkflow({ request: async (path, init) => {
    calls.push({ path, init });
    return response(fixture('session-a', init ? JSON.parse(init.body).enabled : false));
  }, onChange() {} });
  await flow.load('session-a'); flow.acknowledge(true); await flow.enable();
  assert.equal(ready(flow).preview.enabled, true);
  assert.equal(ready(flow).acknowledged, false);
  assert.match(ready(flow).notice, /Provider-request application is not established/);
  assert.deepEqual(JSON.parse(calls[1].init.body), { enabled: true, acknowledge_risk: true });
  await flow.disable();
  assert.equal(ready(flow).preview.enabled, false);
  assert.equal(ready(flow).preview.pins.length, 1);
  assert.match(ready(flow).notice, /Stored rules were not deleted/);
  assert.deepEqual(JSON.parse(calls[2].init.body), { enabled: false, acknowledge_risk: false });
  assert.ok(calls.every(call => !call.init || call.init.method === 'POST'));
});

test('session switching and refresh clear acknowledgement and ignore old responses', async () => {
  let finishOld;
  const calls = [];
  const flow = createPinPolicyWorkflow({ request: async (path, init) => {
    calls.push({ path, init });
    if (path.includes('/old-session/')) return new Promise(resolve => { finishOld = resolve; });
    return response(fixture(path.split('/')[4]));
  }, onChange() {} });
  await flow.load('session-a'); flow.acknowledge(true);
  const pending = flow.load('old-session');
  await flow.load('session-b');
  finishOld(response(fixture('old-session'))); await pending;
  assert.equal(ready(flow).sessionId, 'session-b');
  assert.equal(ready(flow).acknowledged, false);
  await flow.enable();
  assert.equal(calls.filter(call => call.init).length, 0);
  flow.acknowledge(true); await flow.load('session-b');
  assert.equal(ready(flow).acknowledged, false);
});

test('terminated and Gemini sessions cannot be enabled by the workflow', async () => {
  for (const patch of [{ session_terminated: true }, { reinforcement_supported: false }]) {
    const calls = [];
    const flow = createPinPolicyWorkflow({ request: async (path, init) => { calls.push({ path, init }); return response({ ...fixture(), ...patch }); }, onChange() {} });
    await flow.load('session-a'); flow.acknowledge(true); await flow.enable();
    assert.equal(ready(flow).acknowledged, false);
    assert.equal(calls.length, 1);
  }
});

test('invalid session, unavailable API and HTTP failures cannot become disabled or authorized state', async () => {
  for (const request of [async () => { throw new Error('Network unavailable'); }, ...[401, 404, 500].map(status => async () => response({}, status))]) {
    const flow = createPinPolicyWorkflow({ request, onChange() {} });
    await flow.load('session-a');
    assert.equal(flow.getState().kind, 'error');
    await flow.enable();
    assert.equal(flow.getState().kind, 'error');
  }
  let called = false;
  const flow = createPinPolicyWorkflow({ request: async () => { called = true; return response(fixture()); }, onChange() {} });
  await flow.load('bad/session');
  assert.equal(called, false);
  assert.equal(flow.getState().kind, 'error');
});

test('malformed or cross-session policy responses fail closed', async () => {
  for (const body of [null, {}, fixture('another-session'), { ...fixture(), risk_warning: '' }, { ...fixture(), enabled: 'false' }, { ...fixture(), stored_pins: [{}] }]) {
    const flow = createPinPolicyWorkflow({ request: async () => response(body), onChange() {} });
    await flow.load('session-a');
    assert.equal(flow.getState().kind, 'error');
  }
});

test('failed or unconfirmed update clears known state rather than claiming success', async () => {
  for (const outcome of [() => response({}, 409), () => response({}, 500), () => response(fixture('session-a', false)), () => new Response('bad JSON')]) {
    const flow = createPinPolicyWorkflow({ request: async (_path, init) => init ? outcome() : response(fixture()), onChange() {} });
    await flow.load('session-a'); flow.acknowledge(true); await flow.enable();
    assert.equal(flow.getState().kind, 'error');
  }
});

test('an in-flight acknowledgement for one session cannot update another session', async () => {
  let finishEnable;
  const calls = [];
  const flow = createPinPolicyWorkflow({ request: async (path, init) => {
    calls.push({ path, init });
    if (init) return new Promise(resolve => { finishEnable = resolve; });
    return response(fixture(path.split('/')[4]));
  }, onChange() {} });
  await flow.load('session-a'); flow.acknowledge(true);
  const pending = flow.enable();
  await flow.load('session-b'); finishEnable(response(fixture('session-a', true))); await pending;
  assert.equal(ready(flow).sessionId, 'session-b');
  assert.equal(ready(flow).preview.enabled, false);
  assert.equal(ready(flow).acknowledged, false);
  assert.equal(calls.filter(call => call.init)[0].path, '/api/v1/sessions/session-a/pin-context-policy');
});

test('revocation remains available while a preview request is pending', async () => {
  let finishPreview;
  const flow = createPinPolicyWorkflow({ request: async (_path, init) => init
    ? response(fixture('session-a', false))
    : new Promise(resolve => { finishPreview = resolve; }), onChange() {} });
  const pending = flow.load('session-a');
  await flow.disable();
  assert.equal(ready(flow).preview.enabled, false);
  finishPreview(response(fixture('session-a', true))); await pending;
  assert.equal(ready(flow).preview.enabled, false);
});


test('malformed JSON on GET and POST shows actionable unconfirmed state', async () => {
  for (const method of ['GET', 'POST']) {
    const flow = createPinPolicyWorkflow({ request: async (_path, init) =>
      (init ? 'POST' : 'GET') === method ? new Response('invalid JSON') : response(fixture()), onChange() {} });
    await flow.load('session-a');
    if (method === 'POST') { flow.acknowledge(true); await flow.enable(); }
    const state = flow.getState();
    assert.equal(state.kind, 'error');
    assert.equal(state.message, 'Policy API returned malformed JSON. Consent state is unconfirmed; refresh before retrying.');
  }
});
