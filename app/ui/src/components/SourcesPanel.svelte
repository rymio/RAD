<script lang="ts">
  import { store } from '../lib/state.svelte';
  import { anchorLabel, docKind } from '../lib/util';
  import type { Citation } from '../lib/types';
  import DocIcon from './DocIcon.svelte';
  import Preview from './Preview.svelte';
  import { X, ChevronLeft, FolderOpen } from '@lucide/svelte';

  let { citations, selected, onclose, onselect }: { citations: Citation[]; selected: Citation | null; onclose: () => void; onselect: (c: Citation | null) => void } = $props();
</script>

<aside class="panel" class:wide={!!selected}>
  <header class="drag">
    {#if selected}
      <button class="icon-btn no-drag" onclick={() => onselect(null)} aria-label="Back"><ChevronLeft size={17} /></button>
      <span class="truncate t">{selected.title}</span>
      <button class="icon-btn no-drag" title="Open project" onclick={() => store.navigate({ kind: 'project', id: selected!.project_id, doc: selected!.document_id })}><FolderOpen size={16} /></button>
    {:else}
      <span class="t">Sources <span class="muted">{citations.length || ''}</span></span>
    {/if}
    <span class="spacer"></span>
    <button class="icon-btn no-drag" onclick={onclose} aria-label="Close"><X size={17} /></button>
  </header>

  {#if selected}
    {#key selected.chunk_id}
      <Preview documentId={selected.document_id} mime={selected.mime} title={selected.title} anchor={selected.anchor} chunkId={selected.chunk_id} />
    {/key}
  {:else}
    <div class="list">
      {#each citations as c (c.n + '-' + c.chunk_id)}
        <button class="card src" onclick={() => onselect(c)}>
          <div class="h">
            <span class="n">{c.n}</span>
            <DocIcon kind={docKind({ mime: c.mime, original_name: c.title, source_kind: c.source_kind })} size={15} />
            <span class="truncate title">{c.title}</span>
          </div>
          <div class="meta"><span class="pdot" style="background:{store.projectMap.get(c.project_id)?.color ?? 'var(--text-4)'}"></span>{c.project_name}{#if anchorLabel(c.anchor)} · {anchorLabel(c.anchor)}{/if}</div>
          <div class="snip">{c.snippet}</div>
        </button>
      {:else}
        <div class="empty">Sources for the answer appear here. Click a <span class="pill accent">1</span> citation to preview the exact passage.</div>
      {/each}
    </div>
  {/if}
</aside>

<style>
  .panel {
    width: var(--panel-w);
    flex: none;
    display: flex;
    flex-direction: column;
    border-left: 1px solid var(--border);
    background: var(--surface-2);
    min-height: 0;
    animation: slide 200ms var(--ease) both;
    transition: width var(--t) var(--ease);
  }
  .panel.wide {
    width: min(560px, 45vw);
  }
  @keyframes slide {
    from {
      transform: translateX(16px);
      opacity: 0;
    }
  }
  header {
    display: flex;
    align-items: center;
    gap: 6px;
    height: var(--titlebar-h);
    padding: 0 10px 0 16px;
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  .t {
    font-weight: 600;
    font-size: 13.5px;
    min-width: 0;
  }
  .list {
    flex: 1;
    overflow: auto;
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .src {
    padding: 12px 14px;
    text-align: left;
    transition: all var(--t-fast) var(--ease);
  }
  .src:hover {
    border-color: var(--accent-ring);
    transform: translateY(-1px);
    box-shadow: var(--shadow);
  }
  .h {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .n {
    display: grid;
    place-items: center;
    min-width: 20px;
    height: 20px;
    border-radius: 6px;
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 700;
    font-size: 11px;
  }
  .title {
    font-weight: 600;
    font-size: 13px;
  }
  .meta {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 4px 0 6px;
    font-size: 11.5px;
    color: var(--text-3);
  }
  .pdot {
    width: 7px;
    height: 7px;
    border-radius: 2px;
  }
  .snip {
    font-size: 12.5px;
    color: var(--text-2);
    line-height: 1.5;
    display: -webkit-box;
    -webkit-line-clamp: 4;
    line-clamp: 4;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .empty {
    padding: 30px 16px;
    text-align: center;
    color: var(--text-3);
    font-size: 13px;
    line-height: 1.6;
  }
</style>
