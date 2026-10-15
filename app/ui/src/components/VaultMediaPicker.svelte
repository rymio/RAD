<script lang="ts">
  import { api } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import { playableDoc } from '../lib/media.svelte';
  import type * as T from '../lib/types';
  import Modal from './Modal.svelte';
  import { Search, Loader, Music, Film, Check } from '@lucide/svelte';

  let { open = $bindable(false), kind, multiple = true, onpick }: {
    open?: boolean;
    kind: 'audio' | 'video';
    multiple?: boolean;
    onpick: (docs: T.Document[]) => void;
  } = $props();

  let docs = $state<T.Document[]>([]);
  let loading = $state(false);
  let query = $state('');
  let picked = $state<Set<T.Id>>(new Set());

  const projectName = (id: T.Id) => store.projectMap.get(id)?.name ?? '';
  const shown = $derived.by(() => {
    const q = query.trim().toLowerCase();
    return q ? docs.filter((d) => `${d.title} ${d.original_name} ${projectName(d.project_id)}`.toLowerCase().includes(q)) : docs;
  });

  // Every project's playable files, newest first.
  async function load() {
    loading = true;
    picked = new Set();
    try {
      const lists = await Promise.all(store.projects.map((p) => api.get<T.Document[]>(`/v1/projects/${p.id}/documents`).catch(() => [])));
      docs = lists.flat().filter((d) => playableDoc(d, kind)).sort((a, b) => b.created_at - a.created_at);
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    if (open) load();
  });

  function toggle(d: T.Document) {
    if (!multiple) {
      onpick([d]);
      open = false;
      return;
    }
    const s = new Set(picked);
    if (s.has(d.id)) s.delete(d.id);
    else s.add(d.id);
    picked = s;
  }

  function done() {
    onpick(docs.filter((d) => picked.has(d.id)));
    open = false;
  }

  const size = (n: number) => (n > 1 << 30 ? `${(n / (1 << 30)).toFixed(1)} GB` : `${(n / (1 << 20)).toFixed(1)} MB`);
</script>

<Modal bind:open title={kind === 'audio' ? 'Add music from the vault' : 'Open a video from the vault'} width={560}>
  <div class="search">
    <Search size={15} />
    <!-- svelte-ignore a11y_autofocus -->
    <input autofocus placeholder="Filter by name or project" bind:value={query} />
  </div>
  <div class="list">
    {#if loading}
      <div class="empty"><Loader size={16} class="spin" /> Looking through your projects…</div>
    {:else if !shown.length}
      <div class="empty">
        {docs.length ? 'Nothing matches.' : `No ${kind === 'audio' ? 'audio' : 'video'} files in the vault yet. Drop some into a project to add them.`}
      </div>
    {:else}
      {#each shown as d (d.id)}
        <button class="row" class:on={picked.has(d.id)} onclick={() => toggle(d)} ondblclick={() => multiple && (picked = new Set([d.id]), done())}>
          <span class="ic">
            {#if picked.has(d.id)}<Check size={15} />{:else if kind === 'audio'}<Music size={15} />{:else}<Film size={15} />{/if}
          </span>
          <span class="name truncate">{d.title || d.original_name}</span>
          <span class="proj truncate">{projectName(d.project_id)}</span>
          <span class="size">{size(d.size)}</span>
        </button>
      {/each}
    {/if}
  </div>
  {#snippet footer()}
    {#if multiple}
      <button class="btn" onclick={() => (open = false)}>Cancel</button>
      <button class="btn primary" disabled={!picked.size} onclick={done}>Add {picked.size || ''}</button>
    {/if}
  {/snippet}
</Modal>

<style>
  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 10px;
    height: 36px;
    border: 1px solid var(--border-strong);
    border-radius: var(--r-sm);
    color: var(--text-3, var(--text));
    margin-bottom: 10px;
  }
  .search input {
    flex: 1;
    border: 0;
    background: transparent;
    outline: none;
    color: var(--text);
    font: inherit;
  }
  .list {
    max-height: 52vh;
    overflow: auto;
    margin: 0 -6px;
  }
  .empty {
    display: flex;
    gap: 8px;
    align-items: center;
    justify-content: center;
    padding: 36px 12px;
    color: var(--text-2, var(--text));
    text-align: center;
  }
  .row {
    width: 100%;
    display: grid;
    grid-template-columns: 22px 1fr minmax(0, 140px) 70px;
    gap: 10px;
    align-items: center;
    padding: 7px 8px;
    border-radius: var(--r-sm);
    text-align: left;
    color: var(--text);
  }
  .row:hover {
    background: var(--surface-hover);
  }
  .row.on {
    background: var(--accent-soft);
  }
  .ic {
    display: flex;
    color: var(--accent);
  }
  .proj,
  .size {
    color: var(--text-3, var(--text));
    font-size: 12px;
  }
  .size {
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
</style>
