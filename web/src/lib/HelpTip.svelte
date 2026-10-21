<script lang="ts">
  let { text, position = 'bottom' }: { text: string; position?: 'top' | 'bottom' | 'left' } = $props();
  let visible = $state(false);
</script>

<button
  type="button"
  aria-label="Explanation tooltip"
  onmouseenter={() => (visible = true)}
  onmouseleave={() => (visible = false)}
  onfocus={() => (visible = true)}
  onblur={() => (visible = false)}
  onclick={(e) => {
    e.stopPropagation();
    visible = !visible;
  }}
  style="position: relative; display: inline-flex; align-items: center; justify-content: center; width: 16px; height: 16px; padding: 0; border-radius: 999px; background: #334155; color: #38bdf8; font-size: 11px; font-weight: 800; cursor: help; margin-left: 5px; user-select: none; border: 1px solid #475569; flex-shrink: 0;"
>
  ?
  {#if visible}
    <div
      role="tooltip"
      style="position: absolute; z-index: 9999; width: 270px; padding: 9px 11px; border-radius: 8px; background: #0f172a; color: #f1f5f9; border: 1px solid #38bdf8; box-shadow: 0 10px 25px rgba(0,0,0,0.65); font-size: 11.5px; font-weight: 400; line-height: 1.45; text-align: left; pointer-events: none; {position === 'top' ? 'bottom: 24px; left: 50%; transform: translateX(-50%);' : position === 'left' ? 'right: 24px; top: -4px;' : 'top: 24px; left: 50%; transform: translateX(-50%);'}"
    >
      {text}
    </div>
  {/if}
</button>
