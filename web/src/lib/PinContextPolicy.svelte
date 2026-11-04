<script lang="ts">
  import { createPinPolicyWorkflow, type PinPolicyState, type PolicyRequest } from './pinPolicy';
  let { sessionId, revision, request }: { sessionId: string; revision: string; request: PolicyRequest } = $props();
  let view = $state<PinPolicyState>({ kind: 'empty' });
  const workflow = createPinPolicyWorkflow({ request: (path, init) => request(path, init), onChange: (next) => { view = next; } });
  $effect(() => {
    // New session or graph revision requires a fresh preview and acknowledgement.
    const scope = { sessionId, revision };
    void workflow.load(scope.sessionId);
    return () => workflow.clear();
  });
</script>

<section aria-label="PinContext reinforcement policy" class="policy">
  <div class="heading">
    <h3>PinContext reinforcement</h3>
    <button onclick={() => workflow.load(sessionId)} disabled={view.kind === 'saving'}>Refresh preview</button>
    <button onclick={() => workflow.disable()} disabled={!sessionId || view.kind === 'saving'}>Disable reinforcement</button>
  </div>
  {#if view.kind === 'loading' || view.kind === 'saving'}
    <p role="status">{view.kind === 'loading' ? 'Loading consent and activation preview…' : 'Updating session consent…'}</p>
  {:else if view.kind === 'error'}
    <p role="alert">Consent state unavailable: {view.message}</p>
    <p>Enable is unavailable until a valid preview is loaded. Disable remains available to request revocation.</p>
  {:else if view.kind === 'ready'}
    {@const preview = view.preview}
    <p><strong>Consent: {preview.enabled ? 'Enabled (session grant stored)' : 'Disabled'}</strong></p>
    <p class="warning">Experimental, potentially harmful. {preview.warning}</p>
    <p>{preview.scope}</p>
    {#if preview.terminated}<p role="status">This session has ended and cannot be re-enabled.</p>{/if}
    {#if !preview.supported}<p role="status">Reinforcement is unsupported for this session provider. Gemini remains unqualified.</p>{/if}
    <p>Configured automatic policies may create future pins after opt-in: <strong>{preview.mayGenerate ? 'Yes, when context qualifies' : 'No under current configuration'}</strong>.</p>
    <p>Automatic reminder configured: {preview.autoPinConfigured ? 'yes' : 'no'}. Low-health recovery configured: {preview.recoveryConfigured ? `yes (${preview.recoveryStrategy})` : 'no'}.</p>
    <details open>
      <summary>Stored pins ({preview.pins.length}) and latest-request eligibility</summary>
      <p>{preview.basis}</p>
      {#each preview.pins as pin (pin.id)}
        <article>
          <strong>{pin.label}</strong>
          <p>Stored status: {pin.status}. Eligible in latest-request preview after opt-in: {pin.eligible === null ? 'not available' : pin.eligible ? 'yes' : 'no'}.</p>
          <pre>{pin.content}</pre>
        </article>
      {:else}<p>No stored PinContext rules.</p>{/each}
    </details>
    <p>Stored or committed rules, session authorization, and preview eligibility are separate from actual provider-request application. This preview does not establish actual application or measured savings.</p>
    <label><input type="checkbox" checked={view.acknowledged} disabled={preview.enabled || preview.terminated || !preview.supported}
      onchange={(event) => workflow.acknowledge(event.currentTarget.checked)} />
      I reviewed the stored contents, session-wide scope and risks, including possible future automatically generated pins.
    </label>
    <button onclick={() => workflow.enable()} disabled={!view.acknowledged || preview.enabled || preview.terminated || !preview.supported}>Enable reinforcement</button>
    {#if view.notice}<p role="status">{view.notice}</p>{/if}
  {/if}
</section>

<style>
  .policy { border: 1px solid #45403a; border-radius: 6px; background: #141418; padding: 12px; margin-bottom: 16px; color: #d4d4d8; font-size: 12px; }
  .heading { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
  h3 { margin: 0 auto 0 0; font-size: 13px; }
  button { background: #27272a; color: #f4f4f5; border: 1px solid #52525b; border-radius: 4px; padding: 5px 8px; cursor: pointer; }
  button:disabled { opacity: 0.45; cursor: default; }
  .warning { color: #fbbf24; }
  details { max-height: 320px; overflow-y: auto; }
  label { display: block; margin: 10px 0; }
  article { border-top: 1px solid #3f3f46; margin-top: 8px; padding-top: 8px; }
  pre { white-space: pre-wrap; overflow-wrap: anywhere; max-height: 180px; overflow-y: auto; }
</style>
