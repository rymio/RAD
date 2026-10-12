<script lang="ts">
  import { thumbUrl } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import { docKind, formatBytes, timeAgo, kindColor } from '../lib/util';
  import type { Document } from '../lib/types';
  import DocIcon from './DocIcon.svelte';
  import Menu from './Menu.svelte';
  import { MoreHorizontal, Pencil, Trash2, RefreshCw, Download, Pin, PinOff, Info, TriangleAlert, Loader, Camera, ClipboardPaste, FolderInput } from '@lucide/svelte';

  let { doc, onopen, onrename, ondelete, onreindex, onexport, onpin, onmove }: {
    doc: Document;
    onopen: () => void;
    onrename: () => void;
    ondelete: () => void;
    onreindex: () => void;
    onexport: () => void;
    onpin: () => void;
    onmove: () => void;
  } = $props();

  const live = $derived(store.docStatus[doc.id]);
  const status = $derived(live?.status ?? doc.status);
  const kind = $derived(docKind(doc));
  const busy = $derived(status === 'queued' || status === 'processing');
  let imgOk = $state(true);
</script>

<div class="card doc" class:busy>
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="thumb" onclick={onopen} style="--c:{kindColor[kind]}">
    {#if doc.has_thumb && imgOk}
      <img src={thumbUrl(doc.id, doc.updated_at)} alt="" loading="lazy" onerror={() => (imgOk = false)} />
    {:else}
      <div class="ph"><DocIcon {kind} size={26} /><span class="ext">{kind === 'note' ? 'NOTE' : (doc.original_name.split('.').pop() ?? '').toUpperCase().slice(0, 5)}</span></div>
    {/if}
    {#if busy}<div class="overlay shimmer"></div>{/if}
    {#if doc.pinned}<span class="pin"><Pin size={11} /></span>{/if}
    {#if doc.source_kind === 'screenshot'}<span class="src"><Camera size={11} /></span>{:else if doc.source_kind === 'paste'}<span class="src"><ClipboardPaste size={11} /></span>{/if}
  </div>
  <div class="info">
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="title truncate" onclick={onopen} title={doc.title}>{doc.title}</div>
    <div class="sub">
      {#if busy}
        <span class="pill accent"><Loader size={10} class="spin" /> {status === 'queued' ? 'Queued' : 'Indexing'}</span>
      {:else if status === 'error'}
        <span class="pill danger" title={live?.error ?? doc.error ?? ''}><TriangleAlert size={10} /> Failed</span>
      {:else}
        <span>{kind === 'note' ? 'Note' : formatBytes(doc.size)}</span><span>·</span><span>{timeAgo(doc.updated_at)}</span>
      {/if}
      <span class="spacer"></span>
      {#if doc.notice && !busy}<span class="notice" title={doc.notice}><Info size={13} /></span>{/if}
      <Menu
        items={[
          { label: doc.pinned ? 'Unpin' : 'Pin', icon: doc.pinned ? PinOff : Pin, onclick: onpin },
          { label: 'Rename', icon: Pencil, onclick: onrename },
          { label: 'Move to project…', icon: FolderInput, onclick: onmove },
          { label: 'Re-index', icon: RefreshCw, onclick: onreindex },
          { label: 'Export decrypted copy…', icon: Download, onclick: onexport },
          null,
          { label: 'Delete', icon: Trash2, danger: true, onclick: ondelete },
        ]}
      >
        {#snippet trigger(toggle)}<button class="icon-btn sm" onclick={toggle} aria-label="Document menu"><MoreHorizontal size={14} /></button>{/snippet}
      </Menu>
    </div>
  </div>
</div>

<style>
  .doc {
    display: flex;
    flex-direction: column;
    overflow: visible;
    transition: transform var(--t) var(--ease), box-shadow var(--t), border-color var(--t);
    animation: fade-up 260ms var(--ease) both;
  }
  .doc:hover {
    transform: translateY(-2px);
    box-shadow: var(--shadow);
    border-color: var(--border-strong);
  }
  .thumb {
    position: relative;
    height: 128px;
    border-radius: calc(var(--r-lg) - 1px) calc(var(--r-lg) - 1px) 0 0;
    overflow: hidden;
    background: color-mix(in oklab, var(--c) 7%, var(--surface-2));
    border-bottom: 1px solid var(--border);
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    object-position: top;
    background: white;
  }
  .ph {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
  }
  .ext {
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.08em;
    color: var(--c);
    opacity: 0.8;
  }
  .overlay {
    position: absolute;
    inset: 0;
    opacity: 0.55;
  }
  .pin,
  .src {
    position: absolute;
    top: 8px;
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    border-radius: 7px;
    background: var(--glass);
    backdrop-filter: blur(8px);
    color: var(--text-2);
  }
  .pin {
    left: 8px;
    color: var(--accent);
  }
  .src {
    right: 8px;
  }
  .info {
    padding: 10px 8px 8px 12px;
  }
  .title {
    font-weight: 600;
    font-size: 13px;
    margin-bottom: 4px;
  }
  .sub {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 11.5px;
    color: var(--text-3);
    height: 24px;
  }
  .notice {
    color: var(--warning);
    display: inline-flex;
  }
</style>
