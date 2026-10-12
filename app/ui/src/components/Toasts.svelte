<script lang="ts">
  import { store } from '../lib/state.svelte';
  import { CircleCheck, CircleAlert, Info, X } from '@lucide/svelte';
</script>

<div class="toasts">
  {#each store.toasts as t (t.id)}
    <div class="toast {t.kind}">
      <span class="ic">
        {#if t.kind === 'success'}<CircleCheck size={17} />{:else if t.kind === 'error'}<CircleAlert size={17} />{:else}<Info size={17} />{/if}
      </span>
      <div class="txt">
        <div class="title">{t.title}</div>
        {#if t.body}<div class="body selectable">{t.body}</div>{/if}
      </div>
      <button class="icon-btn sm" onclick={() => (store.toasts = store.toasts.filter((x) => x.id !== t.id))}><X size={14} /></button>
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    right: 18px;
    bottom: 18px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    z-index: 1000;
    pointer-events: none;
  }
  .toast {
    pointer-events: auto;
    display: flex;
    gap: 10px;
    align-items: flex-start;
    width: 360px;
    padding: 12px 10px 12px 14px;
    border-radius: var(--r);
    background: var(--glass);
    backdrop-filter: blur(24px) saturate(1.6);
    -webkit-backdrop-filter: blur(24px) saturate(1.6);
    border: 1px solid var(--border-strong);
    box-shadow: var(--shadow-lg);
    animation: pop 220ms var(--ease) both;
  }
  .ic {
    margin-top: 1px;
    color: var(--accent);
  }
  .success .ic {
    color: var(--success);
  }
  .error .ic {
    color: var(--danger);
  }
  .txt {
    flex: 1;
    min-width: 0;
  }
  .title {
    font-weight: 600;
    font-size: 13.5px;
  }
  .body {
    color: var(--text-2);
    font-size: 12.5px;
    margin-top: 2px;
    overflow-wrap: anywhere;
  }
</style>
