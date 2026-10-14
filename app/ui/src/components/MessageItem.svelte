<script lang="ts">
  import { renderMarkdown } from '../lib/markdown';
  import { store } from '../lib/state.svelte';
  import { thumbUrl } from '../lib/api';
  import { anchorLabel, docKind } from '../lib/util';
  import type { Citation, Message } from '../lib/types';
  import DocIcon from './DocIcon.svelte';
  import { Copy, Check, RotateCcw, Pencil, CircleAlert, Sparkles } from '@lucide/svelte';

  let {
    message,
    streaming = false,
    status = '',
    isLast = false,
    oncite,
    onregenerate,
    onedit,
    onlink,
  }: {
    message: Message;
    streaming?: boolean;
    status?: string;
    isLast?: boolean;
    oncite?: (c: Citation, all: Citation[]) => void;
    onregenerate?: () => void;
    onedit?: (m: Message, text: string) => void;
    /** Handles web links instead of the system browser (e.g. in RAD's own browser). */
    onlink?: (href: string) => void;
  } = $props();

  let copied = $state(false);
  let editing = $state(false);
  let editText = $state('');

  // Throttle markdown rendering while streaming.
  let html = $state('');
  let raf = 0;
  $effect(() => {
    const src = message.content;
    const n = message.citations.length;
    cancelAnimationFrame(raf);
    raf = requestAnimationFrame(() => (html = renderMarkdown(src, n)));
  });

  function onClick(e: MouseEvent) {
    const t = e.target as HTMLElement;
    const cite = t.closest('[data-cite]') as HTMLElement | null;
    if (cite) {
      const n = Number(cite.dataset.cite);
      const c = message.citations.find((c) => c.n === n);
      if (c) oncite?.(c, message.citations);
      return;
    }
    const copyBtn = t.closest('[data-copy]') as HTMLElement | null;
    if (copyBtn) {
      const code = copyBtn.parentElement?.querySelector('code')?.textContent ?? '';
      navigator.clipboard.writeText(code);
      copyBtn.textContent = '✓';
      setTimeout(() => (copyBtn.textContent = '⧉'), 1200);
      return;
    }
    const a = t.closest('a') as HTMLAnchorElement | null;
    if (a?.href?.startsWith('http')) {
      e.preventDefault();
      if (onlink) return onlink(a.href);
      import('@tauri-apps/plugin-opener').then((m) => m.openUrl(a.href));
    }
  }

  async function copy() {
    await navigator.clipboard.writeText(message.content);
    copied = true;
    setTimeout(() => (copied = false), 1400);
  }

  const visibleCites = $derived.by(() => {
    // Unique documents, in citation order.
    const seen = new Set<number>();
    return message.citations.filter((c) => (seen.has(c.document_id) ? false : (seen.add(c.document_id), true)));
  });
</script>

{#if message.role === 'user'}
  <div class="msg user fade-up">
    <div class="ubody">
      {#if message.attachments.length}
        <div class="atts">
          {#each message.attachments as a}
            {#if a.mime.startsWith('image/')}
              <img class="att-img" src={thumbUrl(a.document_id)} alt={a.title} />
            {:else}
              <span class="chip"><DocIcon kind={docKind({ mime: a.mime, original_name: a.title, source_kind: '' })} size={14} /> {a.title}</span>
            {/if}
          {/each}
        </div>
      {/if}
      {#if editing}
        <textarea class="textarea" rows="3" bind:value={editText}></textarea>
        <div class="row" style="justify-content:flex-end;margin-top:6px">
          <button class="btn sm ghost" onclick={() => (editing = false)}>Cancel</button>
          <button class="btn sm primary" onclick={() => ((editing = false), onedit?.(message, editText))}>Send</button>
        </div>
      {:else if message.content}
        <div class="bubble selectable">{message.content}</div>
      {/if}
    </div>
    {#if !editing && message.id > 0}
      <div class="tools utools">
        <button class="icon-btn sm" title="Copy" onclick={copy}>{#if copied}<Check size={13} />{:else}<Copy size={13} />{/if}</button>
        {#if onedit}<button class="icon-btn sm" title="Edit & resend" onclick={() => ((editText = message.content), (editing = true))}><Pencil size={13} /></button>{/if}
      </div>
    {/if}
  </div>
{:else}
  <div class="msg assistant">
    <div class="avatar" class:live={streaming}><Sparkles size={15} /></div>
    <div class="abody">
      {#if streaming && !message.content}
        <div class="status shimmer-text">{status || 'Thinking…'}</div>
      {/if}
      {#if message.content}
        <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
        <div class="md selectable" class:caret={streaming} onclick={onClick}>{@html html}</div>
      {/if}
      {#if message.error}
        <div class="error"><CircleAlert size={15} /> <span class="selectable">{message.error}</span>
          {#if onregenerate}<button class="btn sm" onclick={onregenerate}><RotateCcw size={13} /> Retry</button>{/if}
        </div>
      {/if}
      {#if visibleCites.length && !streaming}
        <div class="sources">
          {#each visibleCites.slice(0, 6) as c}
            <button class="src" onclick={() => oncite?.(c, message.citations)} title={c.snippet}>
              <span class="n">{c.n}</span>
              <DocIcon kind={docKind({ mime: c.mime, original_name: c.title, source_kind: c.source_kind })} size={13} />
              <span class="truncate">{c.title}</span>
              {#if anchorLabel(c.anchor) && anchorLabel(c.anchor) !== c.title}<span class="where">{anchorLabel(c.anchor)}</span>{/if}
            </button>
          {/each}
          {#if visibleCites.length > 6}<span class="more">+{visibleCites.length - 6}</span>{/if}
        </div>
      {/if}
      {#if !streaming && message.content}
        <div class="tools">
          <button class="icon-btn sm" title="Copy" onclick={copy}>{#if copied}<Check size={13} />{:else}<Copy size={13} />{/if}</button>
          {#if isLast && onregenerate}<button class="icon-btn sm" title="Regenerate" onclick={onregenerate}><RotateCcw size={13} /></button>{/if}
          {#if message.model}<span class="meta">{store.modelLabel({ provider_id: 0, model: message.model })}{message.tokens_out ? ` · ${message.tokens_out} tokens` : ''}</span>{/if}
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .msg {
    display: flex;
    gap: 14px;
    max-width: 780px;
    margin: 0 auto;
    padding: 0 28px;
  }
  .user {
    flex-direction: column;
    align-items: flex-end;
    margin-top: 26px;
    margin-bottom: 6px;
  }
  .ubody {
    max-width: 82%;
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 8px;
  }
  .bubble {
    padding: 10px 15px;
    border-radius: 18px 18px 6px 18px;
    background: var(--user-bubble);
    border: 1px solid color-mix(in oklab, var(--accent) 14%, transparent);
    white-space: pre-wrap;
    font-size: 15px;
    line-height: 1.55;
    overflow-wrap: anywhere;
  }
  .atts {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    justify-content: flex-end;
  }
  .att-img {
    max-width: 220px;
    max-height: 160px;
    border-radius: 12px;
    border: 1px solid var(--border);
    object-fit: cover;
    background: var(--surface-2);
  }
  .user .utools {
    opacity: 0;
    transition: opacity var(--t);
  }
  .user:hover .utools {
    opacity: 1;
  }
  .assistant {
    margin-top: 18px;
  }
  .avatar {
    flex: none;
    width: 30px;
    height: 30px;
    border-radius: 10px;
    display: grid;
    place-items: center;
    color: white;
    background: linear-gradient(135deg, color-mix(in oklab, var(--accent) 75%, white), var(--accent));
    box-shadow: 0 2px 8px color-mix(in oklab, var(--accent) 35%, transparent);
    margin-top: 1px;
  }
  .avatar.live {
    animation: glow 1.6s ease-in-out infinite;
  }
  @keyframes glow {
    50% {
      box-shadow: 0 0 0 5px var(--accent-soft), 0 2px 12px color-mix(in oklab, var(--accent) 50%, transparent);
    }
  }
  .abody {
    flex: 1;
    min-width: 0;
    padding-top: 4px;
  }
  .status {
    font-size: 14px;
    font-weight: 500;
    padding: 2px 0 6px;
  }
  .md.caret > :global(:last-child)::after {
    content: '';
    display: inline-block;
    width: 7px;
    height: 1.05em;
    margin-left: 3px;
    vertical-align: -0.15em;
    border-radius: 2px;
    background: var(--accent);
    animation: blink 1s steps(2) infinite;
  }
  @keyframes blink {
    50% {
      opacity: 0;
    }
  }
  .error {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 8px;
    padding: 10px 12px;
    border-radius: var(--r);
    background: var(--danger-soft);
    color: var(--danger);
    font-size: 13px;
  }
  .error span {
    flex: 1;
  }
  .sources {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 12px;
  }
  .src {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 280px;
    height: 28px;
    padding: 0 10px 0 4px;
    border-radius: 9px;
    border: 1px solid var(--border);
    background: var(--surface-2);
    color: var(--text-2);
    font-size: 12px;
    transition: all var(--t-fast) var(--ease);
  }
  .src:hover {
    border-color: var(--accent-ring);
    background: var(--accent-softer);
    color: var(--text);
    transform: translateY(-1px);
  }
  .src .n {
    display: grid;
    place-items: center;
    min-width: 19px;
    height: 19px;
    border-radius: 6px;
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 700;
    font-size: 10.5px;
  }
  .src .where {
    color: var(--text-4);
    white-space: nowrap;
  }
  .more {
    align-self: center;
    font-size: 12px;
    color: var(--text-3);
  }
  .tools {
    display: flex;
    align-items: center;
    gap: 2px;
    margin-top: 8px;
    opacity: 0;
    transition: opacity var(--t);
  }
  .assistant:hover .tools,
  .user:hover .tools {
    opacity: 1;
  }
  .meta {
    margin-left: 6px;
    font-size: 11.5px;
    color: var(--text-4);
  }
</style>
