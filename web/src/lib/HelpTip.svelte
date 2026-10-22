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
  style="position: relative; display: inline-flex; align-items: center; justify-content: center; width: 14px; height: 14px; padding: 0; border-radius: 999px; background: {visible ? '#27272a' : 'transparent'}; color: {visible ? '#e4e4e7' : '#71717a'}; font-size: 10px; font-weight: 600; cursor: help; margin-left: 5px; user-select: none; border: 1px solid {visible ? '#52525b' : '#27272a'}; flex-shrink: 0; transition: all 0.15s;"
>
  ?
  {#if visible}
    <div
      role="tooltip"
      style="position: absolute; z-index: 9999; width: 260px; padding: 10px 12px; border-radius: 8px; background: #18181b; color: #e4e4e7; border: 1px solid #3f3f46; box-shadow: 0 12px 28px rgba(0,0,0,0.7); font-size: 11.5px; font-weight: 400; line-height: 1.5; text-align: left; pointer-events: none; {position === 'top' ? 'bottom: 22px; left: 50%; transform: translateX(-50%);' : position === 'left' ? 'right: 22px; top: -4px;' : 'top: 22px; left: 50%; transform: translateX(-50%);'}"
    >
      {text}
    </div>
  {/if}
</button>
