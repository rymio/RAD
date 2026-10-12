<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import { renderMarkdown } from '../lib/markdown';
  import { debounce } from '../lib/util';
  import type { Id, Note } from '../lib/types';
  import { X, Eye, PenLine, Check, Loader, Tag } from '@lucide/svelte';

  let { projectId, noteId = null, initial = '', onclose }: { projectId: Id; noteId?: Id | null; initial?: string; onclose: () => void } = $props();

  let id = $state<Id | null>(noteId);
  let title = $state('');
  let content = $state(initial);
  let tags = $state('');
  let mode = $state<'write' | 'preview'>('write');
  let saving = $state<'idle' | 'saving' | 'saved'>('idle');
  let loaded = $state(!noteId);
  let ta: HTMLTextAreaElement | undefined = $state();

  onMount(async () => {
    if (noteId) {
      try {
        const n = await api.get<Note>(`/v1/notes/${noteId}`);
        title = n.document.title;
        content = n.content;
        tags = n.document.tags.join(', ');
      } catch (e) {
        store.error(e);
      }
      loaded = true;
    }
    ta?.focus();
  });

  async function save() {
    if (!content.trim() && !title.trim()) return;
    saving = 'saving';
    const body = { title, content, tags: tags.split(',').map((t) => t.trim()).filter(Boolean) };
    try {
      if (id) {
        await api.put(`/v1/notes/${id}`, body);
      } else {
        const n = await api.post<Note>(`/v1/projects/${projectId}/notes`, body);
        id = n.document.id;
        if (!title) title = n.document.title;
      }
      saving = 'saved';
    } catch (e) {
      saving = 'idle';
      store.error(e, 'Saving note failed');
    }
  }
  const autosave = debounce(save, 1200);

  function onKey(e: KeyboardEvent) {
    if ((e.metaKey || e.ctrlKey) && e.key === 's') {
      e.preventDefault();
      save();
    }
    if (e.key === 'Escape') close();
  }

  async function close() {
    if (saving !== 'saved' && (content.trim() || title.trim())) await save();
    onclose();
  }

  function onInput() {
    saving = 'idle';
    autosave();
  }
</script>

<svelte:window onkeydown={onKey} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={close}>
  <div class="editor" onclick={(e) => e.stopPropagation()}>
    <header>
      <input class="title" placeholder="Untitled note" bind:value={title} oninput={onInput} />
      <div class="seg">
        <button class:on={mode === 'write'} onclick={() => (mode = 'write')}><PenLine size={13} /> Write</button>
        <button class:on={mode === 'preview'} onclick={() => (mode = 'preview')}><Eye size={13} /> Preview</button>
      </div>
      <span class="state">
        {#if saving === 'saving'}<Loader size={13} class="spin" /> Saving{:else if saving === 'saved'}<Check size={13} /> Saved & indexed{/if}
      </span>
      <button class="icon-btn" onclick={close} aria-label="Close"><X size={17} /></button>
    </header>
    {#if loaded}
      {#if mode === 'write'}
        <textarea bind:this={ta} bind:value={content} oninput={onInput} placeholder="Write or paste anything — it becomes part of what RAD knows. Markdown supported."></textarea>
      {:else}
        <div class="md selectable prev">{@html renderMarkdown(content || '*Nothing yet*')}</div>
      {/if}
    {:else}
      <div class="prev"><Loader size={16} class="spin" /></div>
    {/if}
    <footer>
      <Tag size={13} />
      <input class="tags" placeholder="tags, comma separated" bind:value={tags} oninput={onInput} />
      <span class="spacer"></span>
      <span class="muted">{content.trim() ? content.trim().split(/\s+/).length : 0} words · <span class="kbd">⌘S</span></span>
    </footer>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 450;
    display: grid;
    place-items: center;
    background: rgba(8, 8, 16, 0.38);
    backdrop-filter: blur(4px);
    animation: pop 160ms var(--ease) both;
  }
  .editor {
    width: min(820px, calc(100vw - 80px));
    height: min(720px, calc(100vh - 80px));
    display: flex;
    flex-direction: column;
    border-radius: var(--r-xl);
    background: var(--surface);
    border: 1px solid var(--border-strong);
    box-shadow: var(--shadow-lg);
    overflow: hidden;
  }
  header {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 14px 14px 10px 26px;
  }
  .title {
    flex: 1;
    border: none;
    outline: none;
    background: transparent;
    font-size: 22px;
    font-weight: 700;
    letter-spacing: -0.02em;
    user-select: text;
    -webkit-user-select: text;
  }
  .seg {
    display: flex;
    padding: 2px;
    border-radius: var(--r-sm);
    background: var(--surface-3);
  }
  .seg button {
    display: flex;
    align-items: center;
    gap: 5px;
    height: 26px;
    padding: 0 10px;
    border: none;
    border-radius: 6px;
    background: transparent;
    font-size: 12px;
    color: var(--text-3);
  }
  .seg button.on {
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow-sm);
  }
  .state {
    display: flex;
    align-items: center;
    gap: 5px;
    min-width: 120px;
    justify-content: flex-end;
    font-size: 12px;
    color: var(--success);
  }
  textarea {
    flex: 1;
    resize: none;
    border: none;
    outline: none;
    padding: 6px 26px 20px;
    background: transparent;
    font-size: 15px;
    line-height: 1.7;
    font-family: var(--font);
    user-select: text;
    -webkit-user-select: text;
  }
  .prev {
    flex: 1;
    overflow: auto;
    padding: 6px 26px 20px;
  }
  footer {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 18px 10px 26px;
    border-top: 1px solid var(--border);
    color: var(--text-3);
    font-size: 12px;
    background: var(--surface-2);
  }
  .tags {
    border: none;
    outline: none;
    background: transparent;
    font-size: 12.5px;
    width: 280px;
    user-select: text;
    -webkit-user-select: text;
  }
</style>
