<script lang="ts">
  import { tick } from 'svelte';
  import { api, rad } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import { hostOf, pageContext, wantsScreenshot, type ShotMode } from '../lib/web';
  import type { BrowserSnapshot, BrowserTab, Chat, Citation, Message } from '../lib/types';
  import MessageItem from './MessageItem.svelte';
  import { ArrowUp, Square, FileText, Eye, EyeOff, BookOpen, MessagesSquare, SquarePen, ExternalLink, ChevronDown, ChevronUp } from '@lucide/svelte';

  let { page, onlink }: { page: BrowserTab | null; onlink: (href: string) => void } = $props();

  let text = $state('');
  let input: HTMLTextAreaElement | undefined = $state();
  let scroller: HTMLDivElement | undefined = $state();
  let sendText = $state(true);
  let shot = $state<ShotMode>('auto');
  let useVault = $state(false);
  let expanded = $state(true);
  let height = $state(260);
  let looking = $state('');

  const chatId = $derived(store.browserChatId);
  const chat = $derived(chatId ? store.chats.find((c) => c.id === chatId) : null);
  const messages = $derived(chatId ? (store.messages[chatId] ?? []) : []);
  const draft = $derived(chatId ? store.drafts[chatId] : undefined);
  const streaming = $derived(chatId ? !!store.streamingChats[chatId] : false);
  const hasVision = $derived(!!store.settings?.vision_model);
  const hasThread = $derived(messages.length > 0 || !!draft);

  $effect(() => {
    if (chat) useVault = chat.mode === 'rag';
  });

  $effect(() => {
    if (chatId && !store.messages[chatId] && !store.drafts[chatId]) store.loadChat(chatId).catch(() => (store.browserChatId = null));
  });

  const draftAssistant = $derived<Message | null>(
    draft
      ? { id: -1, chat_id: chatId!, role: 'assistant', content: draft.content, attachments: [], citations: draft.citations, model: null, error: draft.error, tokens_in: null, tokens_out: null, created_at: Date.now() }
      : null,
  );

  $effect(() => {
    messages.length;
    draft?.content;
    tick().then(() => scroller?.scrollTo({ top: scroller.scrollHeight }));
  });

  const shotLabel = $derived(shot === 'auto' ? 'Screenshot: auto' : shot === 'always' ? 'Screenshot: always' : 'No screenshot');
  const shotTitle = $derived(
    shot === 'auto'
      ? hasVision
        ? 'The AI gets a screenshot when your question is about how the page looks, or the page has little text. Click to change.'
        : 'No vision model is set (Settings → Models), so no screenshots are sent. Click to change.'
      : shot === 'always'
        ? `Every question includes a screenshot of the page${hasVision ? '' : ' — set a vision model in Settings → Models, or use a chat model that can see images'}. Click to change.`
        : 'The AI never gets a screenshot. Click to change.',
  );

  function cycleShot() {
    shot = shot === 'auto' ? 'always' : shot === 'always' ? 'off' : 'auto';
  }

  async function toggleVault() {
    useVault = !useVault;
    if (chat) await api.patch(`/v1/chats/${chat.id}`, { mode: useVault ? 'rag' : 'chat_only' }).catch((e) => store.error(e));
    store.reloadChats();
  }

  async function ensureChat(): Promise<number> {
    if (chatId && store.chats.some((c) => c.id === chatId)) return chatId;
    const host = hostOf(page?.url ?? '');
    const c = await api.post<Chat>('/v1/chats', { title: host ? `Web · ${host}` : 'Web browsing', mode: useVault ? 'rag' : 'chat_only' });
    store.chats = [c, ...store.chats];
    store.messages[c.id] = [];
    store.browserChatId = c.id;
    return c.id;
  }

  async function send() {
    const q = text.trim();
    if (!q || streaming || looking) return;
    text = '';
    autosize();
    expanded = true;
    try {
      const id = await ensureChat();
      let snap: BrowserSnapshot = {};
      if (page && (sendText || shot !== 'off')) {
        looking = 'Reading the page…';
        snap = await rad.browserSnapshot(sendText || shot === 'auto', false).catch(() => ({}));
        if (wantsScreenshot(shot, q, snap.page, hasVision)) {
          looking = 'Looking at the page…';
          const img = await rad.browserSnapshot(false, true).catch((e) => (store.error(e, 'Couldn’t take a screenshot'), {} as BrowserSnapshot));
          snap.image = img.image;
        }
      }
      looking = '';
      const withPage = !!page && (sendText || !!snap.image);
      await store.sendMessage(id, {
        content: q,
        context: withPage ? pageContext(sendText ? snap.page : undefined, !!snap.image) : undefined,
        images: snap.image ? [snap.image] : [],
      });
    } catch (e) {
      store.error(e, 'Couldn’t send');
    } finally {
      looking = '';
    }
  }

  function newChat() {
    store.browserChatId = null;
    text = '';
    input?.focus();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      send();
    }
  }

  function autosize() {
    if (!input) return;
    input.style.height = 'auto';
    input.style.height = Math.min(input.scrollHeight, 140) + 'px';
  }

  function openCite(c: Citation) {
    if (c.chat_id) store.navigate({ kind: 'chat', id: c.chat_id });
    else store.navigate({ kind: 'project', id: c.project_id, doc: c.document_id });
  }

  function startResize(e: PointerEvent) {
    const y0 = e.clientY;
    const h0 = height;
    const move = (m: PointerEvent) => (height = Math.max(120, Math.min(window.innerHeight * 0.7, h0 + (y0 - m.clientY))));
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }
</script>

<section class="bchat">
  {#if hasThread && expanded}
    <div class="grip" role="separator" aria-orientation="horizontal" onpointerdown={startResize}></div>
    <div class="thread" bind:this={scroller} style="height:{height}px">
      {#each messages as m (m.id)}
        <MessageItem message={m} oncite={openCite} {onlink} />
      {/each}
      {#if draft}
        {#if draft.user && !messages.some((m) => m.id === draft.user?.id)}<MessageItem message={draft.user} {onlink} />{/if}
        {#if draftAssistant}<MessageItem message={draftAssistant} {streaming} status={draft.status} oncite={openCite} {onlink} />{/if}
      {/if}
    </div>
  {/if}

  <div class="bar">
    <div class="opts">
      <button class="chip" class:accent={sendText} title={sendText ? 'The AI reads the page’s text, links and your selection.' : 'The page’s text is not sent.'} onclick={() => (sendText = !sendText)}>
        <FileText size={13} /> Page text
      </button>
      <button class="chip" class:accent={shot !== 'off'} title={shotTitle} onclick={cycleShot}>
        {#if shot === 'off'}<EyeOff size={13} />{:else}<Eye size={13} />{/if} {shotLabel}
      </button>
      <button class="chip" class:accent={useVault} title={useVault ? 'Answers also search your vault.' : 'Your vault is not searched. Click to include it.'} onclick={toggleVault}>
        {#if useVault}<BookOpen size={13} /> Vault{:else}<MessagesSquare size={13} /> Page only{/if}
      </button>
      <span class="spacer"></span>
      {#if looking}<span class="looking">{looking}</span>{/if}
      {#if chat}
        <button class="icon-btn sm" title="Open this conversation in Chats" onclick={() => store.navigate({ kind: 'chat', id: chat.id })}><ExternalLink size={14} /></button>
      {/if}
      <button class="icon-btn sm" title="New conversation" onclick={newChat} disabled={!chatId}><SquarePen size={14} /></button>
      {#if hasThread}
        <button class="icon-btn sm" title={expanded ? 'Hide conversation' : 'Show conversation'} onclick={() => (expanded = !expanded)}>
          {#if expanded}<ChevronDown size={15} />{:else}<ChevronUp size={15} />{/if}
        </button>
      {/if}
    </div>
    <div class="input-row">
      <textarea
        bind:this={input}
        bind:value={text}
        rows="1"
        placeholder={page ? `Ask about ${hostOf(page.url) || 'this page'}…` : 'Ask anything…'}
        onkeydown={onKey}
        oninput={autosize}
      ></textarea>
      {#if streaming}
        <button class="send stop" title="Stop" onclick={() => chatId && store.stopChat(chatId)}><Square size={14} /></button>
      {:else}
        <button class="send" title="Send (Enter)" disabled={!text.trim() || !!looking} onclick={send}><ArrowUp size={17} /></button>
      {/if}
    </div>
  </div>
</section>

<style>
  .bchat {
    flex: none;
    display: flex;
    flex-direction: column;
    border-top: 1px solid var(--border);
    background: var(--surface);
  }
  .grip {
    height: 6px;
    margin-top: -3px;
    cursor: ns-resize;
    flex: none;
  }
  .thread {
    overflow-y: auto;
    padding: 4px 22px 8px;
    border-bottom: 1px solid var(--border);
  }
  .bar {
    padding: 8px 14px 12px;
  }
  .opts {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 8px;
  }
  .looking {
    font-size: 12px;
    color: var(--text-3);
    animation: pulse 1.2s var(--ease) infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.45;
    }
  }
  .input-row {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    padding: 6px 6px 6px 14px;
    border-radius: var(--r-lg);
    border: 1px solid var(--border);
    background: var(--surface-2);
    transition: border-color var(--t-fast), box-shadow var(--t-fast);
  }
  .input-row:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-ring);
  }
  textarea {
    flex: 1;
    min-height: 22px;
    max-height: 140px;
    padding: 5px 0;
    border: none;
    outline: none;
    resize: none;
    background: transparent;
    color: var(--text);
    font: inherit;
    font-size: 14px;
    line-height: 1.45;
  }
  .send {
    display: grid;
    place-items: center;
    flex: none;
    width: 32px;
    height: 32px;
    border: none;
    border-radius: 50%;
    background: var(--accent);
    color: var(--on-accent);
    transition: opacity var(--t-fast), transform var(--t-fast);
  }
  .send:disabled {
    opacity: 0.35;
  }
  .send:not(:disabled):active {
    transform: scale(0.94);
  }
  .send.stop {
    background: var(--text);
    color: var(--surface);
  }
</style>
