<script lang="ts">
  import { store } from '../lib/state.svelte';
  import type { Id } from '../lib/types';
  import { Layers, Check } from '@lucide/svelte';

  let { scope, onchange }: { scope: Id[]; onchange: (s: Id[]) => void } = $props();
  let open = $state(false);

  const label = $derived.by(() => {
    if (!scope.length) return 'All projects';
    if (scope.length === 1) return store.projectMap.get(scope[0])?.name ?? '1 project';
    return `${scope.length} projects`;
  });

  function toggle(id: Id) {
    onchange(scope.includes(id) ? scope.filter((x) => x !== id) : [...scope, id]);
  }
</script>

<div class="wrap">
  <button class="chip" class:accent={scope.length > 0} onclick={() => (open = !open)} title="Which projects to search">
    {#if scope.length === 1}<span class="pdot" style="background:{store.projectMap.get(scope[0])?.color}"></span>{:else}<Layers size={13} />{/if}
    {label}
  </button>
  {#if open}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="scrim" onclick={() => (open = false)}></div>
    <div class="pop">
      <div class="h">Search in</div>
      <button class="opt" onclick={() => onchange([])}>
        <Layers size={14} /><span class="spacer">All projects</span>{#if !scope.length}<Check size={14} />{/if}
      </button>
      <div class="sep"></div>
      <div class="list">
        {#each store.projects.filter((p) => !p.archived) as p (p.id)}
          <button class="opt" onclick={() => toggle(p.id)}>
            <span class="pdot" style="background:{p.color}"></span>
            <span class="spacer truncate">{p.name}</span>
            {#if scope.includes(p.id)}<Check size={14} />{/if}
          </button>
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .wrap {
    position: relative;
  }
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 90;
  }
  .pop {
    position: absolute;
    top: calc(100% + 6px);
    left: 0;
    z-index: 100;
    width: 260px;
    padding: 5px;
    border-radius: var(--r);
    background: var(--glass);
    backdrop-filter: blur(28px) saturate(1.6);
    -webkit-backdrop-filter: blur(28px) saturate(1.6);
    border: 1px solid var(--border-strong);
    box-shadow: var(--shadow-lg);
    animation: pop 140ms var(--ease) both;
  }
  .h {
    padding: 6px 8px;
    font-size: 11px;
    font-weight: 650;
    color: var(--text-3);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .list {
    max-height: 300px;
    overflow: auto;
  }
  .opt {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    height: 32px;
    padding: 0 8px;
    border: none;
    border-radius: var(--r-xs);
    background: transparent;
    font-size: 13px;
    text-align: left;
  }
  .opt:hover {
    background: var(--surface-hover);
  }
  .opt :global(svg:last-child) {
    color: var(--accent);
  }
  .sep {
    height: 1px;
    margin: 4px 6px;
    background: var(--border);
  }
  .pdot {
    width: 9px;
    height: 9px;
    border-radius: 3px;
    flex: none;
  }
</style>
