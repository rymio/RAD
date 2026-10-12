<script lang="ts">
  import type { Snippet } from 'svelte';
  import { X } from '@lucide/svelte';

  let { open = $bindable(false), title = '', width = 520, children, footer, onclose }: {
    open?: boolean;
    title?: string;
    width?: number;
    children: Snippet;
    footer?: Snippet;
    onclose?: () => void;
  } = $props();

  function close() {
    open = false;
    onclose?.();
  }
</script>

<svelte:window onkeydown={(e) => open && e.key === 'Escape' && close()} />

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="backdrop" onclick={close}>
    <div class="modal" style="width:{width}px" onclick={(e) => e.stopPropagation()} role="dialog" aria-modal="true" tabindex="-1">
      {#if title}
        <header>
          <h2>{title}</h2>
          <button class="icon-btn" onclick={close} aria-label="Close"><X size={17} /></button>
        </header>
      {/if}
      <div class="body">{@render children()}</div>
      {#if footer}<footer>{@render footer()}</footer>{/if}
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 500;
    display: grid;
    place-items: center;
    background: rgba(8, 8, 16, 0.38);
    backdrop-filter: blur(3px);
    animation: fade 160ms var(--ease) both;
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
  .modal {
    max-width: calc(100vw - 48px);
    max-height: calc(100vh - 64px);
    display: flex;
    flex-direction: column;
    border-radius: var(--r-xl);
    background: var(--surface);
    border: 1px solid var(--border-strong);
    box-shadow: var(--shadow-lg);
    animation: pop 220ms var(--ease-spring) both;
    overflow: hidden;
  }
  header {
    display: flex;
    align-items: center;
    padding: 18px 18px 6px 22px;
  }
  h2 {
    flex: 1;
    margin: 0;
    font-size: 16px;
    font-weight: 650;
    letter-spacing: -0.01em;
  }
  .body {
    padding: 12px 22px 20px;
    overflow: auto;
  }
  footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 14px 22px;
    border-top: 1px solid var(--border);
    background: var(--surface-2);
  }
</style>
