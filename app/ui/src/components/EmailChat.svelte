<script lang="ts" module>
  /** One conversation per email, for as long as RAD is open (key: folder + uid). */
  const threads = new Map<string, number>();
</script>

<script lang="ts">
  import { tick } from 'svelte';
  import { api, rad } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import type { Chat, Citation, MailMessage, Message, Note } from '../lib/types';
  import MessageItem from './MessageItem.svelte';
  import { ArrowUp, Square, Sparkles, StickyNote, ExternalLink, ChevronDown, ChevronUp, Loader, NotebookPen, SquarePen } from '@lucide/svelte';

  let { message, folder }: { message: MailMessage; folder: string } = $props();

  const key = $derived(`${folder}\u0000${message.uid}`);
  let chatId = $state<number | null>(null);
  $effect(() => {
    chatId = threads.get(key) ?? null;
  });

  let text = $state('');
  let input: HTMLTextAreaElement | undefined = $state();
  let scroller: HTMLDivElement | undefined = $state();
  let busy = $state<'summary' | 'note' | 'answer' | null>(null);
  let saved = $state<{ title: string; projectId: number; docId: number } | null>(null);

  const prefs = (() => {
    try {
      return JSON.parse(localStorage.getItem('rad-email-chat') ?? '{}');
    } catch {
      return {};
    }
  })();
  let collapsed = $state<boolean>(prefs.collapsed ?? false);
  let target = $state<number | null>(prefs.target ?? null);
  $effect(() => {
    try {
      localStorage.setItem('rad-email-chat', JSON.stringify({ collapsed, target }));
    } catch {
      /* private mode */
    }
  });

  const projects = $derived(store.projects.filter((p) => !p.archived && p.name !== 'Inbox'));
  const messages = $derived(chatId ? (store.messages[chatId] ?? []) : []);
  const draft = $derived(chatId ? store.drafts[chatId] : undefined);
  const streaming = $derived(chatId ? !!store.streamingChats[chatId] : false);
  const lastAnswer = $derived([...messages].reverse().find((m) => m.role === 'assistant' && !m.error && m.content.trim()));

  $effect(() => {
    if (chatId && !store.messages[chatId] && !store.drafts[chatId]) store.loadChat(chatId).catch(() => threads.delete(key));
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

  function when(ms: number) {
    return ms ? new Date(ms).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }) : '';
  }

  /** What the AI is told about the email (sent with each question, not stored). */
  function mailContext() {
    const m = message;
    const lines = ["The user is reading this email in RAD's email client. Answer about it; say so when the email doesn't contain the answer."];
    lines.push(`SUBJECT: ${m.subject}`, `FROM: ${m.from}`);
    if (m.to) lines.push(`TO: ${m.to}`);
    if (m.cc) lines.push(`CC: ${m.cc}`);
    if (m.date) lines.push(`DATE: ${when(m.date)}`);
    if (m.attachments.length) lines.push(`ATTACHMENTS (not opened): ${m.attachments.map((a) => a.name).join(', ')}`);
    lines.push(`BODY:\n${m.text.trim().slice(0, 30_000) || '(no text)'}`);
    return lines.join('\n');
  }

  function header() {
    const m = message;
    return [`**From:** ${m.from}`, m.to && `**To:** ${m.to}`, m.date && `**Date:** ${when(m.date)}`, `**Subject:** ${m.subject}`].filter(Boolean).join('  \n');
  }

  async function projectId() {
    if (target && store.projects.some((p) => p.id === target && !p.archived)) return target;
    return (await store.inboxProject()).id;
  }

  async function saveNote(title: string, content: string) {
    const pid = await projectId();
    const note = await api.post<Note>(`/v1/projects/${pid}/notes`, { title: title.slice(0, 200), content, tags: ['email'] });
    saved = { title, projectId: pid, docId: note.document.id };
    store.reloadProjects();
    const where = store.projectMap.get(pid)?.name ?? 'Inbox';
    store.toast('success', `Note saved to ${where}`, title);
  }

  async function summarize() {
    if (busy) return;
    busy = 'summary';
    try {
      const r = await rad.assist(
        'You turn emails into short notes in Markdown. Start with a one-sentence summary. Then the key points as bullets. ' +
          'Then "Action items" (with any dates or deadlines) only if there are some. Write in the language of the email. No preamble.',
        [{ role: 'user', content: mailContext() }],
      );
      await saveNote(`Summary — ${message.subject}`, `${header()}\n\n${r.content}`);
    } catch (e) {
      store.error(e, 'Couldn’t summarize');
    } finally {
      busy = null;
    }
  }

  async function saveAsNote() {
    if (busy) return;
    busy = 'note';
    try {
      await saveNote(message.subject, `${header()}\n\n---\n\n${message.text.trim()}`);
    } catch (e) {
      store.error(e, 'Couldn’t save the note');
    } finally {
      busy = null;
    }
  }

  async function saveAnswer() {
    if (!lastAnswer || busy) return;
    busy = 'answer';
    const q = [...messages].reverse().find((m) => m.role === 'user' && m.id < lastAnswer.id);
    try {
      await saveNote(`${q ? q.content.slice(0, 80) : 'Answer'} — ${message.subject}`, `${header()}\n\n${q ? `> ${q.content.replace(/\n/g, '\n> ')}\n\n` : ''}${lastAnswer.content}`);
    } catch (e) {
      store.error(e, 'Couldn’t save the note');
    } finally {
      busy = null;
    }
  }

  async function ensureChat(): Promise<number> {
    if (chatId && store.chats.some((c) => c.id === chatId)) return chatId;
    const c = await api.post<Chat>('/v1/chats', { title: `Email · ${message.subject}`.slice(0, 120), mode: 'chat_only' });
    store.chats = [c, ...store.chats];
    store.messages[c.id] = [];
    threads.set(key, c.id);
    chatId = c.id;
    return c.id;
  }

  async function send() {
    const q = text.trim();
    if (!q || streaming) return;
    text = '';
    autosize();
    try {
      const id = await ensureChat();
      await store.sendMessage(id, { content: q, context: mailContext() });
    } catch (e) {
      store.error(e, 'Couldn’t send');
    }
  }

  function newThread() {
    threads.delete(key);
    chatId = null;
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

  async function expand() {
    collapsed = false;
    await tick();
    input?.focus();
  }

  function openCite(c: Citation) {
    if (c.chat_id) store.navigate({ kind: 'chat', id: c.chat_id });
    else store.navigate({ kind: 'project', id: c.project_id, doc: c.document_id });
  }
</script>

{#if collapsed}
  <section class="echat slim">
    <button class="ask" onclick={expand}>
      <Sparkles size={14} />
      <span class="truncate">{streaming ? 'Answering…' : 'Ask AI about this email…'}</span>
    </button>
    <button class="icon-btn sm" title="Show assistant" onclick={expand}><ChevronUp size={15} /></button>
  </section>
{:else}
  <section class="echat">
    {#if messages.length || draft}
      <div class="thread" bind:this={scroller}>
        {#each messages as m (m.id)}
          <MessageItem message={m} oncite={openCite} />
        {/each}
        {#if draft}
          {#if draft.user && !messages.some((m) => m.id === draft.user?.id)}<MessageItem message={draft.user} />{/if}
          {#if draftAssistant}<MessageItem message={draftAssistant} {streaming} status={draft.status} oncite={openCite} />{/if}
        {/if}
      </div>
    {/if}

    <div class="bar">
      <div class="opts">
        <button class="chip accent" title="The AI summarizes this email and saves it as a note" disabled={!!busy} onclick={summarize}>
          {#if busy === 'summary'}<Loader size={13} class="spin" />{:else}<Sparkles size={13} />{/if} Summarize to note
        </button>
        <button class="chip" title="Save the email itself as a note" disabled={!!busy} onclick={saveAsNote}>
          {#if busy === 'note'}<Loader size={13} class="spin" />{:else}<StickyNote size={13} />{/if} Save as note
        </button>
        {#if lastAnswer && !streaming}
          <button class="chip" title="Save the AI’s last answer as a note" disabled={!!busy} onclick={saveAnswer}>
            {#if busy === 'answer'}<Loader size={13} class="spin" />{:else}<NotebookPen size={13} />{/if} Answer to note
          </button>
        {/if}
        <select class="select sm" bind:value={target} title="Where notes are saved">
          <option value={null}>Notes go to Inbox</option>
          {#each projects as p}<option value={p.id}>Notes go to {p.name}</option>{/each}
        </select>
        <span class="spacer"></span>
        {#if saved}
          <button class="saved truncate" title="Open the note" onclick={() => store.navigate({ kind: 'project', id: saved!.projectId, doc: saved!.docId })}>
            <ExternalLink size={12} /> {saved.title}
          </button>
        {/if}
        {#if chatId}
          <button class="icon-btn sm" title="Open this conversation in Chats" onclick={() => store.navigate({ kind: 'chat', id: chatId! })}><ExternalLink size={14} /></button>
          <button class="icon-btn sm" title="New conversation" onclick={newThread}><SquarePen size={14} /></button>
        {/if}
        <button class="icon-btn sm" title="Collapse" onclick={() => (collapsed = true)}><ChevronDown size={15} /></button>
      </div>
      <div class="input-row">
        <textarea bind:this={input} bind:value={text} rows="1" placeholder="Ask about this email — “what do they need from me, and by when?”" onkeydown={onKey} oninput={autosize}></textarea>
        {#if streaming}
          <button class="send stop" title="Stop" onclick={() => chatId && store.stopChat(chatId)}><Square size={14} /></button>
        {:else}
          <button class="send" title="Send (Enter)" disabled={!text.trim()} onclick={send}><ArrowUp size={17} /></button>
        {/if}
      </div>
    </div>
  </section>
{/if}

<style>
  .echat {
    flex: none;
    display: flex;
    flex-direction: column;
    border-top: 1px solid var(--border);
    background: var(--surface);
  }
  .echat.slim {
    flex-direction: row;
    align-items: center;
    gap: 6px;
    height: 40px;
    padding: 0 10px 0 14px;
  }
  .ask {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 28px;
    padding: 0 12px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--text-3);
    font-size: 13px;
    text-align: left;
  }
  .ask:hover {
    border-color: var(--border-strong);
    color: var(--text-2);
  }
  .ask :global(svg) {
    color: var(--accent);
    flex: none;
  }
  .thread {
    max-height: 38vh;
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
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: 8px;
  }
  .select.sm {
    height: 26px;
    width: auto;
    max-width: 200px;
    padding: 0 26px 0 8px;
    font-size: 12px;
  }
  .saved {
    display: flex;
    align-items: center;
    gap: 5px;
    max-width: 240px;
    border: none;
    background: transparent;
    color: var(--accent);
    font-size: 12px;
  }
  .saved:hover {
    text-decoration: underline;
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
