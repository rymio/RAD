<script lang="ts" module>
  export interface MenuItem {
    label: string;
    icon?: any;
    danger?: boolean;
    disabled?: boolean;
    hint?: string;
    onclick: () => void;
  }
</script>

<script lang="ts">
  import type { Snippet } from 'svelte';

  let { items, align = 'right', trigger, width = 210 }: { items: (MenuItem | null)[]; align?: 'left' | 'right'; trigger: Snippet<[() => void]>; width?: number } = $props();
  let open = $state(false);
  let el: HTMLDivElement | undefined = $state();
  let up = $state(false);

  function toggle() {
    open = !open;
    if (open && el) {
      const r = el.getBoundingClientRect();
      up = r.bottom + 260 > window.innerHeight;
    }
  }
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && (open = false)} />

<div class="menu-wrap" bind:this={el}>
  {@render trigger(toggle)}
  {#if open}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="scrim" onclick={() => (open = false)}></div>
    <div class="menu {align}" class:up style="width:{width}px" role="menu">
      {#each items as it}
        {#if it === null}
          <div class="sep"></div>
        {:else}
          <button class="item" class:danger={it.danger} disabled={it.disabled} role="menuitem" onclick={() => ((open = false), it.onclick())}>
            {#if it.icon}<it.icon size={15} />{/if}
            <span class="spacer">{it.label}</span>
            {#if it.hint}<span class="hint-k">{it.hint}</span>{/if}
          </button>
        {/if}
      {/each}
    </div>
  {/if}
</div>

<style>
  .menu-wrap {
    position: relative;
    display: inline-flex;
  }
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 90;
  }
  .menu {
    position: absolute;
    top: calc(100% + 6px);
    z-index: 100;
    padding: 5px;
    border-radius: var(--r);
    background: var(--glass);
    backdrop-filter: blur(28px) saturate(1.6);
    -webkit-backdrop-filter: blur(28px) saturate(1.6);
    border: 1px solid var(--border-strong);
    box-shadow: var(--shadow-lg);
    animation: pop 140ms var(--ease) both;
  }
  .menu.up {
    top: auto;
    bottom: calc(100% + 6px);
  }
  .menu.right {
    right: 0;
  }
  .menu.left {
    left: 0;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    height: 32px;
    padding: 0 10px;
    border: none;
    border-radius: var(--r-xs);
    background: transparent;
    color: var(--text);
    font-size: 13px;
    text-align: left;
  }
  .item :global(svg) {
    color: var(--text-3);
  }
  .item:hover:not(:disabled) {
    background: var(--accent);
    color: var(--on-accent);
  }
  .item:hover :global(svg) {
    color: inherit;
  }
  .item.danger {
    color: var(--danger);
  }
  .item.danger :global(svg) {
    color: var(--danger);
  }
  .item.danger:hover {
    background: var(--danger);
    color: white;
  }
  .item:disabled {
    opacity: 0.45;
  }
  .sep {
    height: 1px;
    margin: 4px 6px;
    background: var(--border);
  }
  .hint-k {
    font-size: 11px;
    opacity: 0.6;
  }
</style>
