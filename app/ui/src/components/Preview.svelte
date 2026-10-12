<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { save } from '@tauri-apps/plugin-dialog';
  import { api, contentUrl, rad } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import { renderMarkdown } from '../lib/markdown';
  import { anchorLabel, formatBytes, formatTime, timeAgo } from '../lib/util';
  import type { Anchor, Chunk, Document, Id } from '../lib/types';
  import PdfView from './PdfView.svelte';
  import { Download, Info, Loader, TriangleAlert } from '@lucide/svelte';

  let { documentId, mime, title, anchor = {}, chunkId = null }: { documentId: Id; mime: string; title: string; anchor?: Anchor; chunkId?: Id | null } = $props();

  let doc = $state<Document | null>(null);
  let chunks = $state<Chunk[]>([]);
  let note = $state<string | null>(null);
  let loading = $state(true);
  let media: HTMLMediaElement | undefined = $state();
  let chunkBox: HTMLDivElement | undefined = $state();

  const isPdf = $derived(mime === 'application/pdf');
  const isImage = $derived(mime.startsWith('image/'));
  const isAudio = $derived(mime.startsWith('audio/'));
  const isVideo = $derived(mime.startsWith('video/'));

  onMount(async () => {
    try {
      doc = await api.get<Document>(`/v1/documents/${documentId}`);
      if (doc.source_kind === 'note') {
        note = (await api.get<{ content: string }>(`/v1/notes/${documentId}`)).content;
      }
      if (!isPdf) chunks = await api.get<Chunk[]>(`/v1/documents/${documentId}/chunks`);
    } catch (e) {
      store.error(e);
    } finally {
      loading = false;
    }
    await tick();
    chunkBox?.querySelector('.hit')?.scrollIntoView({ block: 'center' });
  });

  function onMediaReady() {
    if (media && anchor.time != null) {
      media.currentTime = anchor.time;
    }
  }

  function seek(t?: number) {
    if (media && t != null) {
      media.currentTime = t;
      media.play().catch(() => {});
    }
  }

  async function exportFile() {
    const dest = await save({ defaultPath: doc?.original_name ?? title });
    if (!dest) return;
    try {
      await rad.exportDocument(documentId, dest);
      store.toast('success', 'Exported', 'A decrypted copy was saved where you chose.');
    } catch (e) {
      store.error(e, 'Export failed');
    }
  }
</script>

<div class="preview">
  {#if doc}
    <div class="meta">
      <span>{formatBytes(doc.size)}</span>
      {#if doc.page_count}<span>· {doc.page_count} {isPdf ? 'pages' : 'parts'}</span>{/if}
      <span>· {timeAgo(doc.created_at)}</span>
      {#if anchorLabel(anchor)}<span class="pill accent">{anchorLabel(anchor)}</span>{/if}
      <span class="spacer"></span>
      <button class="btn sm ghost" onclick={exportFile} title="Save a decrypted copy"><Download size={13} /> Export</button>
    </div>
    {#if doc.notice}<div class="notice"><Info size={14} /> {doc.notice}</div>{/if}
    {#if doc.error}<div class="notice bad"><TriangleAlert size={14} /> {doc.error}</div>{/if}
  {/if}

  {#if isPdf}
    <PdfView url={contentUrl(documentId)} page={anchor.page ?? 1} />
  {:else}
    <div class="body">
      {#if isImage}
        <div class="img-wrap"><img src={contentUrl(documentId)} alt={title} /></div>
      {:else if isAudio}
        <audio bind:this={media} controls src={contentUrl(documentId)} onloadedmetadata={onMediaReady}></audio>
      {:else if isVideo}
        <!-- svelte-ignore a11y_media_has_caption -->
        <video bind:this={media} controls src={contentUrl(documentId)} onloadedmetadata={onMediaReady}></video>
      {/if}

      {#if loading}
        <div class="state"><Loader size={16} class="spin" /></div>
      {:else if note !== null}
        <div class="md selectable note">{@html renderMarkdown(note)}</div>
      {:else if chunks.length}
        <div class="chunks" bind:this={chunkBox}>
          {#if isImage}<div class="section-title">What RAD sees in this image</div>{/if}
          {#if isAudio || isVideo}<div class="section-title">Transcript</div>{/if}
          {#each chunks as c (c.id)}
            <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
            <div class="chunk selectable" class:hit={c.id === chunkId} onclick={() => seek(c.anchor.time)}>
              {#if anchorLabel(c.anchor)}<div class="where">{#if c.anchor.time != null}▶ {formatTime(c.anchor.time)}{:else}{anchorLabel(c.anchor)}{/if}</div>{/if}
              <div class="txt">{c.text}</div>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .preview {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .meta {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 12px 8px 16px;
    font-size: 12px;
    color: var(--text-3);
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  .notice {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    margin: 10px 12px 0;
    padding: 9px 11px;
    border-radius: var(--r-sm);
    background: var(--warning-soft);
    font-size: 12.5px;
    color: var(--text-2);
  }
  .notice.bad {
    background: var(--danger-soft);
    color: var(--danger);
  }
  .notice :global(svg) {
    flex: none;
    margin-top: 2px;
  }
  .body {
    flex: 1;
    overflow: auto;
    padding: 14px;
  }
  .img-wrap {
    border-radius: var(--r);
    overflow: hidden;
    /* Light checkerboard: most screenshots and scans are dark-on-transparent. */
    background: repeating-conic-gradient(#ececf1 0% 25%, #ffffff 0% 50%) 50% / 16px 16px;
    border: 1px solid var(--border);
    margin-bottom: 14px;
  }
  .img-wrap img {
    display: block;
    width: 100%;
    height: auto;
  }
  audio,
  video {
    width: 100%;
    border-radius: var(--r);
    margin-bottom: 14px;
    background: black;
  }
  audio {
    background: transparent;
  }
  .chunks {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .section-title {
    margin: 4px 2px 2px;
  }
  .chunk {
    padding: 12px 14px;
    border-radius: var(--r);
    background: var(--surface);
    border: 1px solid var(--border);
    font-size: 13px;
    line-height: 1.6;
    transition: border-color var(--t);
  }
  .chunk.hit {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-soft);
    background: color-mix(in oklab, var(--accent) 4%, var(--surface));
  }
  .where {
    font-size: 11px;
    font-weight: 600;
    color: var(--accent);
    margin-bottom: 4px;
  }
  .txt {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .note {
    padding: 4px 6px;
  }
  .state {
    display: grid;
    place-items: center;
    padding: 30px;
    color: var(--text-3);
  }
</style>
