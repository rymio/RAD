<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { store } from '../lib/state.svelte';
  import { rad } from '../lib/api';
  import { renderMarkdown } from '../lib/markdown';
  import { anchorLabel, docKind } from '../lib/util';
  import type { Id } from '../lib/types';
  import Unlock from './Unlock.svelte';
  import Logo from './Logo.svelte';
  import DocIcon from './DocIcon.svelte';
  import { ArrowUp, Square, Maximize2, RotateCcw, Loader, Camera } from '@lucide/svelte';

  let q = $state('');
  let chatId = $state<Id | null>(null);
  let input: HTMLInputElement | undefined = $state();
  let scroller: HTMLDivElement | undefined = $state();
  let unlocked = $state(false);
  let ready = $state(false);

  const draft = $derived(chatId ? store.drafts[chatId] : undefined);
  const msgs = $derived(chatId ? (store.messages[chatId] ?? []) : []);
  const last = $derived(msgs.findLast((m) => m.role === 'assistant'));
  const streaming = $derived(chatId ? !!store.streamingChats[chatId] : false);
  const answer = $derived(draft ? draft.content : (last?.content ?? ''));
  const cites = $derived(draft ? draft.citations : (last?.citations ?? []));
  const question = $derived(draft?.user?.content ?? msgs.findLast((m) => m.role === 'user')?.content ?? '');

  async function refresh() {
    store.config = await rad.config();
    store.applyTheme();
    try {
      store.status = await rad.status();
      unlocked = !!store.status.session_valid;
      if (unlocked && !store.settings) await store.loadAll();
    } catch {
      unlocked = false;
    }
    ready = true;
  }

  onMount(() => {
    refresh();
    const un: Array<() => void> = [];
    listen('rad-quick-open', async () => {
      await refresh();
      await tick();
      input?.focus();
      input?.select();
    }).then((u) => un.push(u));
    listen('rad-unlocked', () => refresh()).then((u) => un.push(u));
    listen('rad-locked', () => {
      unlocked = false;
      chatId = null;
    }).then((u) => un.push(u));
    return () => un.forEach((u) => u());
  });

  $effect(() => {
    answer;
    tick().then(() => scroller?.scrollTo({ top: scroller.scrollHeight }));
  });

  async function ask(e?: Event) {
    e?.preventDefault();
    const text = q.trim();
    if (!text || streaming) return;
    q = '';
    try {
      if (!chatId) {
        const c = await store.newChat();
        chatId = c.id;
        store.messages[c.id] = [];
      }
      await store.sendMessage(chatId, { content: text });
    } catch (err) {
      store.error(err);
    }
  }

  function reset() {
    chatId = null;
    q = '';
    input?.focus();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') rad.hideQuick();
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter' && chatId) rad.openMain({ kind: 'chat', id: chatId });
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="quick">
  {#if !ready}
    <div class="center"><Loader size={18} class="spin" /></div>
  {:else if !unlocked}
    <div class="lockwrap"><Unlock compact /></div>
  {:else}
    <form class="bar drag" onsubmit={ask}>
      <span class="no-drag"><Logo size={26} /></span>
      <input class="no-drag" bind:this={input} bind:value={q} placeholder={chatId ? 'Ask a follow-up…' : 'Ask your vault anything…'} autocomplete="off" />
      {#if streaming}
        <button type="button" class="send no-drag stop" onclick={() => chatId && store.stopChat(chatId)}><Square size={12} fill="currentColor" /></button>
      {:else}
        <button class="send no-drag" disabled={!q.trim()}><ArrowUp size={17} strokeWidth={2.4} /></button>
      {/if}
    </form>

    {#if chatId}
      <div class="answer" bind:this={scroller}>
        {#if question}<div class="q">{question}</div>{/if}
        {#if streaming && !answer}<div class="shimmer-text status">{draft?.status || 'Thinking…'}</div>{/if}
        {#if answer}<div class="md selectable">{@html renderMarkdown(answer, cites.length)}</div>{/if}
        {#if draft?.error}<div class="err">{draft.error}</div>{/if}
        {#if cites.length && !streaming}
          <div class="srcs">
            {#each cites.slice(0, 5) as c}
              <span class="src"><span class="n">{c.n}</span><DocIcon kind={docKind({ mime: c.mime, original_name: c.title, source_kind: c.source_kind })} size={12} /><span class="truncate">{c.title}</span>{#if anchorLabel(c.anchor)}<span class="muted">{anchorLabel(c.anchor)}</span>{/if}</span>
            {/each}
          </div>
        {/if}
      </div>
      <div class="foot">
        <button class="btn sm ghost" onclick={reset}><RotateCcw size={13} /> New question</button>
        <span class="spacer"></span>
        <button class="btn sm" onclick={() => chatId && rad.openMain({ kind: 'chat', id: chatId })}><Maximize2 size={13} /> Continue in RAD <span class="kbd">⌘⏎</span></button>
      </div>
    {:else}
      <div class="tips">
        <button class="tip" onclick={() => rad.captureScreenshot().then((r) => r && store.toast('success', 'Screenshot saved'))}><Camera size={14} /> Capture a screenshot into your vault</button>
        <div class="muted small">Searches all projects · <span class="kbd">esc</span> to close</div>
      </div>
    {/if}
  {/if}
</div>

<style>
  :global(html.quick),
  :global(html.quick body) {
    background: transparent !important;
  }
  .quick {
    height: 100%;
    display: flex;
    flex-direction: column;
    border-radius: 18px;
    overflow: hidden;
    background: color-mix(in oklab, var(--surface) 72%, transparent);
    border: 1px solid var(--border-strong);
  }
  :global(html:not(.translucent)) .quick {
    background: var(--surface);
  }
  .center,
  .lockwrap {
    flex: 1;
    display: grid;
    place-items: center;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 14px 14px 14px 18px;
    border-bottom: 1px solid var(--border);
    flex: none;
  }
  .bar input {
    flex: 1;
    border: none;
    outline: none;
    background: transparent;
    font-size: 19px;
    font-weight: 450;
    letter-spacing: -0.01em;
    user-select: text;
    -webkit-user-select: text;
  }
  .bar input::placeholder {
    color: var(--text-4);
  }
  .send {
    width: 34px;
    height: 34px;
    border-radius: 11px;
    border: none;
    display: grid;
    place-items: center;
    background: var(--accent);
    color: var(--on-accent);
  }
  .send:disabled {
    opacity: 0.3;
  }
  .send.stop {
    background: var(--text);
    color: var(--surface);
  }
  .answer {
    flex: 1;
    overflow: auto;
    padding: 18px 22px;
  }
  .q {
    font-size: 13px;
    color: var(--text-3);
    margin-bottom: 10px;
    font-weight: 500;
  }
  .status {
    font-weight: 500;
  }
  .md {
    font-size: 14.5px;
  }
  .err {
    color: var(--danger);
    font-size: 13px;
    margin-top: 8px;
  }
  .srcs {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 12px;
  }
  .src {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    max-width: 260px;
    height: 24px;
    padding: 0 8px 0 3px;
    border-radius: 8px;
    background: var(--surface-2);
    border: 1px solid var(--border);
    font-size: 11.5px;
    color: var(--text-2);
  }
  .src .n {
    display: grid;
    place-items: center;
    min-width: 17px;
    height: 17px;
    border-radius: 5px;
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 700;
    font-size: 10px;
  }
  .foot {
    display: flex;
    align-items: center;
    padding: 8px 12px;
    border-top: 1px solid var(--border);
    flex: none;
  }
  .tips {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
  }
  .tip {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 14px;
    border-radius: var(--r);
    border: 1px solid var(--border);
    background: var(--surface);
    color: var(--text-2);
    font-size: 13px;
  }
  .tip:hover {
    color: var(--text);
    border-color: var(--border-strong);
  }
  .small {
    font-size: 12px;
  }
</style>
