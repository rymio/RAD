<script lang="ts">
  import { tick } from 'svelte';
  import { store } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import { greeting } from '../lib/util';
  import type { Citation, Document, Id, Message, ModelRef } from '../lib/types';
  import MessageItem from './MessageItem.svelte';
  import Composer from './Composer.svelte';
  import ScopePicker from './ScopePicker.svelte';
  import ModelPicker from './ModelPicker.svelte';
  import SourcesPanel from './SourcesPanel.svelte';
  import Menu from './Menu.svelte';
  import Logo from './Logo.svelte';
  import { MoreHorizontal, Pin, PinOff, Trash2, Download, BookOpen, MessagesSquare, PanelRight, Sparkles, FileSearch, CalendarDays, StickyNote, ImageIcon } from '@lucide/svelte';

  let { chatId }: { chatId: Id | null } = $props();

  let scroller: HTMLDivElement | undefined = $state();
  let composer: Composer | undefined = $state();
  let stick = $state(true);
  let landingScope = $state<Id[]>([]);
  let landingModel = $state<ModelRef | null>(null);
  let panel = $state<{ citations: Citation[]; selected: Citation | null } | null>(null);
  let loading = $state(false);

  const chat = $derived(chatId ? store.chats.find((c) => c.id === chatId) : null);
  const messages = $derived(chatId ? (store.messages[chatId] ?? []) : []);
  const draft = $derived(chatId ? store.drafts[chatId] : undefined);
  const streaming = $derived(chatId ? !!store.streamingChats[chatId] : false);
  const scope = $derived(chat ? chat.scope.projects : landingScope);
  const model = $derived(chat ? chat.model : landingModel);
  const ragMode = $derived(chat ? chat.mode === 'rag' : true);

  $effect(() => {
    if (chatId && !store.messages[chatId] && !store.drafts[chatId]) {
      loading = true;
      store.loadChat(chatId).catch((e) => store.error(e)).finally(() => (loading = false));
    }
  });

  const draftAssistant = $derived<Message | null>(
    draft
      ? {
          id: -1, chat_id: chatId!, role: 'assistant', content: draft.content, attachments: [], citations: draft.citations,
          model: null, error: draft.error, tokens_in: null, tokens_out: null, created_at: Date.now(),
        }
      : null,
  );

  // Keep scrolled to the bottom while streaming, unless the user scrolled up.
  $effect(() => {
    messages.length;
    draft?.content;
    draft?.status;
    if (stick) tick().then(() => scroller?.scrollTo({ top: scroller.scrollHeight }));
  });

  function onScroll() {
    if (!scroller) return;
    stick = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 80;
  }

  // Auto-open sources for the latest answer.
  $effect(() => {
    if (draft?.citations.length && !panel) panel = { citations: draft.citations, selected: null };
  });

  async function send(text: string, attachments: Document[]) {
    let id = chatId;
    if (!id) {
      const c = await api.post<any>('/v1/chats', { scope: { projects: landingScope }, model: landingModel });
      store.chats = [c, ...store.chats];
      store.messages[c.id] = [];
      id = c.id;
      // Start streaming, then switch the route (the stream lives in the store).
      const p = store.sendMessage(c.id, { content: text, attachments: attachments.map((a) => a.id) });
      store.navigate({ kind: 'chat', id: c.id });
      await p;
      return;
    }
    stick = true;
    await store.sendMessage(id, { content: text, attachments: attachments.map((a) => a.id) });
  }

  async function regenerate() {
    if (!chatId) return;
    await store.sendMessage(chatId, { content: '', regenerate: true });
  }

  async function edit(m: Message, text: string) {
    if (!chatId || !text.trim()) return;
    await store.sendMessage(chatId, { content: text.trim(), truncate_from: m.id });
  }

  async function patch(body: Record<string, unknown>) {
    if (!chatId) return;
    try {
      const c = await api.patch<any>(`/v1/chats/${chatId}`, body);
      const i = store.chats.findIndex((x) => x.id === chatId);
      if (i >= 0) store.chats[i] = c;
    } catch (e) {
      store.error(e);
    }
  }

  function setScope(s: Id[]) {
    if (chat) patch({ scope: { projects: s } });
    else landingScope = s;
  }
  function setModel(m: ModelRef | null) {
    if (chat) patch({ model: m });
    else landingModel = m;
  }

  async function exportMarkdown() {
    if (!chat) return;
    const { save } = await import('@tauri-apps/plugin-dialog');
    const { invoke } = await import('@tauri-apps/api/core');
    const dest = await save({ defaultPath: `${chat.title.replace(/[\\/:*?"<>|]/g, '-')}.md`, filters: [{ name: 'Markdown', extensions: ['md'] }] });
    if (!dest) return;
    const body = messages
      .map((m) => {
        const cites = m.citations.length ? '\n\n' + m.citations.map((c) => `[${c.n}] ${c.title} (${c.project_name})`).join('\n') : '';
        return `### ${m.role === 'user' ? 'You' : 'RAD'}\n\n${m.content}${cites}`;
      })
      .join('\n\n---\n\n');
    await invoke('save_text_file', { dest, content: `# ${chat.title}\n\n${body}\n` }).catch((e) => store.error(e));
    store.toast('success', 'Chat exported', dest);
  }

  async function remove() {
    if (!chatId) return;
    await api.del(`/v1/chats/${chatId}`).catch((e) => store.error(e));
    store.chats = store.chats.filter((c) => c.id !== chatId);
    store.navigate({ kind: 'chat', id: null });
  }

  function openCite(c: Citation, all: Citation[]) {
    panel = { citations: all, selected: c };
  }

  const suggestions = [
    { icon: FileSearch, text: 'What did I add to my vault this week?' },
    { icon: CalendarDays, text: 'Summarize upcoming dates and deadlines in my notes' },
    { icon: ImageIcon, text: 'Find the screenshot where I saved a receipt' },
    { icon: StickyNote, text: 'Remember that ' },
  ];
  const noModel = $derived(!store.settings?.chat_model && !model);
</script>

<div class="chat">
  <div class="col">
    <header class="drag">
      <div class="title no-drag">
        {#if chat}
          <span class="truncate t">{chat.title}</span>
        {:else}
          <span class="t muted">New chat</span>
        {/if}
      </div>
      <div class="controls no-drag">
        <ScopePicker {scope} onchange={setScope} />
        <button
          class="chip"
          class:accent={!ragMode}
          title={ragMode ? 'Answers use your vault. Click for plain chat.' : 'Plain chat — your vault is not searched.'}
          onclick={() => chat && patch({ mode: ragMode ? 'chat_only' : 'rag' })}
          disabled={!chat}
        >
          {#if ragMode}<BookOpen size={13} /> Vault{:else}<MessagesSquare size={13} /> Chat only{/if}
        </button>
        <ModelPicker value={model} onchange={setModel} filter="chat" />
        {#if chat}
          <Menu
            items={[
              { label: chat.pinned ? 'Unpin' : 'Pin to top', icon: chat.pinned ? PinOff : Pin, onclick: () => patch({ pinned: !chat!.pinned }) },
              { label: 'Export as Markdown…', icon: Download, onclick: exportMarkdown },
              null,
              { label: 'Delete chat', icon: Trash2, danger: true, onclick: remove },
            ]}
          >
            {#snippet trigger(toggle)}<button class="icon-btn" onclick={toggle} aria-label="Chat menu"><MoreHorizontal size={17} /></button>{/snippet}
          </Menu>
        {/if}
        <button class="icon-btn" class:active={!!panel} title="Sources" onclick={() => (panel = panel ? null : { citations: messages.findLast((m) => m.citations.length)?.citations ?? [], selected: null })}><PanelRight size={17} /></button>
      </div>
    </header>

    <div class="scroller" bind:this={scroller} onscroll={onScroll}>
      {#if !chatId}
        <div class="landing">
          <div class="hero fade-up">
            <Logo size={52} />
            <h1>{greeting()}</h1>
            <p>Ask anything across your projects, notes, documents, screenshots and recordings.</p>
          </div>
          {#if noModel}
            <button class="setup card" onclick={() => store.navigate({ kind: 'settings', tab: 'models' })}>
              <Sparkles size={18} />
              <div><strong>Choose a model to start chatting</strong><span>Download a built-in model that runs offline, or add Ollama, LM Studio, OpenAI, Anthropic or any OpenAI-compatible provider.</span></div>
            </button>
          {/if}
          <div class="composer-wrap">
            <Composer bind:this={composer} large onsend={send} targetProject={scope.length === 1 ? scope[0] : null} />
          </div>
          <div class="suggest">
            {#each suggestions as s}
              <button class="sg" onclick={() => composer?.setText(s.text)}><s.icon size={15} />{s.text}</button>
            {/each}
          </div>
        </div>
      {:else}
        <div class="thread">
          {#if loading && !messages.length}
            <div class="msg-skel"><div class="shimmer b1"></div><div class="shimmer b2"></div></div>
          {/if}
          {#each messages as m, i (m.id)}
            <MessageItem
              message={m}
              isLast={i === messages.length - 1 && !draft}
              oncite={openCite}
              onregenerate={i === messages.length - 1 && !streaming ? regenerate : undefined}
              onedit={!streaming ? edit : undefined}
            />
          {/each}
          {#if draft}
            {#if draft.user && !messages.some((m) => m.id === draft.user?.id)}<MessageItem message={draft.user} />{/if}
            {#if draftAssistant}<MessageItem message={draftAssistant} streaming={streaming} status={draft.status} oncite={openCite} />{/if}
          {/if}
          <div style="height:24px"></div>
        </div>
      {/if}
    </div>

    {#if chatId}
      <div class="dock">
        <Composer bind:this={composer} {streaming} onsend={send} onstop={() => chatId && store.stopChat(chatId)} targetProject={scope.length === 1 ? scope[0] : null} placeholder="Reply…" />
      </div>
    {/if}
  </div>

  {#if panel}
    <SourcesPanel citations={panel.citations} selected={panel.selected} onclose={() => (panel = null)} onselect={(c) => panel && (panel.selected = c)} />
  {/if}
</div>

<style>
  .chat {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .col {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    container-type: inline-size;
  }
  header {
    flex: none;
    display: flex;
    align-items: center;
    gap: 12px;
    height: var(--titlebar-h);
    padding: 0 14px 0 calc(22px + var(--lead));
    border-bottom: 1px solid var(--border);
  }
  .title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
  }
  .t {
    display: block;
    font-weight: 600;
    font-size: 14px;
  }
  .controls {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  /* Narrow (sources panel open): keep the controls compact. */
  @container (max-width: 640px) {
    .controls :global(.chip) {
      max-width: 150px;
      overflow: hidden;
      text-overflow: ellipsis;
    }
  }
  .scroller {
    flex: 1;
    overflow-y: auto;
    overflow-x: hidden;
  }
  .thread {
    padding: 8px 0 0;
  }
  .dock {
    flex: none;
    padding: 0 28px 18px;
    max-width: 836px;
    width: 100%;
    margin: 0 auto;
  }
  .landing {
    max-width: 760px;
    margin: 0 auto;
    padding: 9vh 28px 40px;
  }
  .hero {
    text-align: center;
    margin-bottom: 28px;
  }
  .hero h1 {
    margin: 16px 0 6px;
    font-size: 30px;
    font-weight: 700;
    letter-spacing: -0.03em;
  }
  .hero p {
    margin: 0;
    color: var(--text-2);
    font-size: 15px;
  }
  .setup {
    display: flex;
    gap: 14px;
    align-items: center;
    width: 100%;
    padding: 14px 16px;
    margin-bottom: 14px;
    text-align: left;
    color: var(--accent);
    border-color: var(--accent-ring);
    background: var(--accent-softer);
  }
  .setup strong {
    display: block;
    color: var(--text);
    font-size: 14px;
  }
  .setup span {
    color: var(--text-2);
    font-size: 13px;
  }
  .suggest {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
    margin-top: 14px;
  }
  .sg {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 12px 14px;
    border-radius: var(--r);
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-2);
    font-size: 13px;
    text-align: left;
    transition: all var(--t-fast) var(--ease);
  }
  .sg:hover {
    background: var(--surface-2);
    border-color: var(--border-strong);
    color: var(--text);
    transform: translateY(-1px);
  }
  .sg :global(svg) {
    color: var(--accent);
    flex: none;
  }
  .msg-skel {
    max-width: 780px;
    margin: 30px auto;
    padding: 0 28px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .msg-skel div {
    height: 18px;
    border-radius: 8px;
  }
  .b1 {
    width: 40%;
    align-self: flex-end;
  }
  .b2 {
    width: 85%;
    height: 70px !important;
  }
</style>
