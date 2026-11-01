<script lang="ts">
  // Beside the notebook: Gilgamesh as a tutor. The conversation is one of his (kept, recapped),
  // about this notebook; he sees its cells, outputs and errors, and searches the vault (the
  // books and papers added to RAG), whose passages are listed under his answer. A code block in
  // his answer goes into the notebook as a new cell with one click.
  import { tick } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { store } from '../lib/state.svelte';
  import { gilgamesh } from '../lib/gilgamesh/gilgamesh.svelte';
  import { avatarImage } from '../lib/gilgamesh/face';
  import { Head } from '../lib/gilgamesh/head';
  import { colab, type Notebook } from '../lib/colab.svelte';
  import { renderMath } from '../lib/colab/notebook';
  import type { Citation } from '../lib/types';
  import { ArrowUp, Mic, MicOff, Square, Loader, SquarePen, ScanFace, BookPlus, Sigma, GraduationCap, Lightbulb, Bug } from '@lucide/svelte';

  let { nb }: { nb: Notebook } = $props();
  const g = gilgamesh;

  let text = $state('');
  let scroller = $state<HTMLDivElement | null>(null);
  let canvas = $state<HTMLCanvasElement | null>(null);
  let ready = $state<boolean | null>(null);

  const about = $derived(!!nb.id && g.notebook?.id === nb.id);
  const turns = $derived(about ? g.turns.filter((t) => !t.quiet) : []);
  const on = $derived(g.phase !== 'off');
  const busy = $derived(g.phase === 'thinking' || g.phase === 'speaking');
  const caption = $derived(g.heard || (busy ? g.status || g.saying : '') || g.error || '');
  const failing = $derived(nb.cells.find((c) => c.state === 'error') ?? null);

  $effect(() => {
    invoke<any>('tts_status')
      .then((t) => (ready = !!t?.installed && !!t?.engine))
      .catch(() => (ready = false));
  });

  // His face, small, beside the chat; it moves with every other picture of him.
  $effect(() => {
    const a = g.avatar;
    const c = canvas;
    if (!a || !c) return;
    let head: Head | null = null;
    let off = () => {};
    let dead = false;
    avatarImage(a)
      .then((img) => {
        if (dead) return;
        head = new Head(c, a, img);
        off = g.head.add(head);
      })
      .catch(() => {});
    return () => {
      dead = true;
      off();
      head?.destroy();
    };
  });

  $effect(() => {
    void turns.length;
    void turns.at(-1)?.text;
    tick().then(() => scroller?.scrollTo({ top: scroller.scrollHeight }));
  });

  export async function ask(q: string) {
    q = q.trim();
    if (!q || !nb.id) return;
    await colab.flush();
    await g.discussNotebook(nb.id);
    void g.ask(q);
  }

  function send(e?: Event) {
    e?.preventDefault();
    const q = text;
    text = '';
    void ask(q);
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) send(e);
  }

  async function toggleMic() {
    if (on) return g.stop();
    if (!ready) return store.navigate({ kind: 'gilgamesh' });
    if (nb.id) await g.discussNotebook(nb.id);
    await g.start();
  }

  /** His answer, with buttons on each code block to put it in the notebook. */
  function answer(md: string) {
    return renderMath(md).replace(/<pre>/g, '<pre><span class="ins"><button data-insert="1" title="Add as a new cell">＋ Cell</button><button data-insert="run" title="Add as a new cell and run it">▶ Add &amp; run</button></span>');
  }

  function onClick(e: MouseEvent) {
    const t = e.target as HTMLElement;
    const ins = t.closest('[data-insert]') as HTMLElement | null;
    if (ins) {
      const code = ins.closest('pre')?.querySelector('code')?.textContent ?? '';
      const c = colab.insertCode(code, ins.dataset.insert === 'run');
      if (c) queueMicrotask(() => document.querySelector(`[data-cell="${c.id}"]`)?.scrollIntoView({ behavior: 'smooth', block: 'center' }));
      return;
    }
    const copy = t.closest('[data-copy]') as HTMLElement | null;
    if (copy) {
      void navigator.clipboard.writeText(copy.parentElement?.querySelector('code')?.textContent ?? '');
      store.toast('success', 'Copied');
    }
  }

  function openSource(c: Citation) {
    if (c.chat_id) store.navigate({ kind: 'chat', id: c.chat_id });
    else store.navigate({ kind: 'project', id: c.project_id, doc: c.document_id });
  }

  const sourcesOf = (list: Citation[] | undefined) => {
    const seen = new Set<number>();
    return (list ?? []).filter((c) => !c.chat_id && !seen.has(c.document_id) && seen.add(c.document_id));
  };
</script>

<div class="chat">
  <div class="bar">
    {#if g.avatar}
      <span class="pic" class:talking={busy} title="Gilgamesh"><canvas bind:this={canvas}></canvas></span>
    {:else}
      <button class="icon-btn sm" title="Give Gilgamesh a face" onclick={() => store.navigate({ kind: 'gilgamesh' })}><ScanFace size={15} /></button>
    {/if}
    <div class="who">
      <strong>Gilgamesh</strong>
      <small>Your tutor · sees the notebook, searches your vault</small>
    </div>
    <span class="spacer"></span>
    <button class="icon-btn sm" title="Add PDFs or books to RAG (the Colab Library), so he can use them" disabled={colab.adding} onclick={() => colab.addToRag()}>{#if colab.adding}<Loader size={13} class="spin" />{:else}<BookPlus size={14} />{/if}</button>
    {#if about && turns.length}
      <button class="icon-btn sm" title="New talk about this notebook" onclick={async () => { await g.newConversation(); if (nb.id) await g.discussNotebook(nb.id); }}><SquarePen size={13} /></button>
    {/if}
    {#if busy}<button class="icon-btn sm" title="Stop him" onclick={() => g.interrupt()}><Square size={12} /></button>{/if}
    <button class="mic" class:on title={on ? 'Microphone off' : 'Talk with Gilgamesh'} disabled={g.phase === 'starting'} onclick={toggleMic}>
      {#if g.phase === 'starting'}<Loader size={14} class="spin" />{:else if on}<MicOff size={14} />{:else}<Mic size={14} />{/if}
    </button>
  </div>
  {#if caption}<div class="cap truncate" class:you={!!g.heard}>{caption}</div>{/if}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="msgs" bind:this={scroller} onclick={onClick}>
    {#each turns as t, i (i)}
      <div class="msg" class:you={t.who === 'you'}>
        {#if t.who === 'you'}
          <p>{t.text}</p>
        {:else}
          <div class="md selectable">{@html answer(t.text)}{#if !t.text && busy}<Loader size={12} class="spin" />{/if}</div>
          {#if sourcesOf(t.sources).length}
            <div class="srcs">
              <span>From your vault:</span>
              {#each sourcesOf(t.sources).slice(0, 5) as s (s.document_id)}
                <button class="src" title={s.snippet} onclick={() => openSource(s)}>{s.title}{s.project_name ? ` · ${s.project_name}` : ''}</button>
              {/each}
            </div>
          {/if}
        {/if}
        {#if t.actions?.length}<small>{t.actions.map((a) => a.label).join(' · ')}</small>{/if}
      </div>
    {:else}
      <div class="hint">
        <p>Ask about the code, the maths behind it, or an error. He sees every cell and what it printed, and his code goes into the notebook with a click.</p>
        <p>He also searches your vault: add the books, papers and docs you learn from with <BookPlus size={12} /> (or drop a PDF here), and he'll use them, offline too.</p>
      </div>
    {/each}
  </div>
  <div class="chips">
    {#if failing}
      <button class="chip" onclick={() => ask(`Cell ${colab.index(failing.id) + 1} failed. What does the error mean, and how do I fix it?`)}><Bug size={12} /> Why does cell {colab.index(failing.id) + 1} fail?</button>
    {/if}
    <button class="chip" onclick={() => ask('Explain the maths behind what this notebook does, step by step.')}><Sigma size={12} /> Explain the maths</button>
    <button class="chip" onclick={() => ask('Give me a small exercise to try next in this notebook, building on what is here. Don’t give the answer yet.')}><GraduationCap size={12} /> An exercise</button>
    <button class="chip" onclick={() => ask('What should I try next to understand this better?')}><Lightbulb size={12} /> What next?</button>
  </div>
  <form class="compose" onsubmit={send}>
    <textarea class="input" rows="1" bind:value={text} onkeydown={onKey} placeholder="Ask Gilgamesh about this notebook…"></textarea>
    <button class="send" disabled={!text.trim()} title="Send"><ArrowUp size={15} /></button>
  </form>
</div>

<style>
  .chat {
    display: flex;
    flex-direction: column;
    min-height: 0;
    flex: 1;
    background: var(--surface);
    overflow: hidden;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 8px 6px 10px;
    min-height: 46px;
    font-size: 12.5px;
    border-bottom: 1px solid var(--border);
  }
  .who {
    display: flex;
    flex-direction: column;
    line-height: 1.2;
    min-width: 0;
  }
  .who small {
    color: var(--text-3);
    font-size: 11px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pic {
    width: 30px;
    height: 30px;
    border-radius: 50%;
    overflow: hidden;
    border: 1px solid var(--border);
    background: var(--surface-2);
    flex: none;
  }
  .pic.talking {
    box-shadow: 0 0 0 2px var(--accent-ring);
  }
  .pic canvas {
    width: 100%;
    height: 100%;
    display: block;
  }
  .spacer {
    flex: 1;
  }
  .cap {
    padding: 4px 12px;
    font-size: 12px;
    color: var(--text-3);
    border-bottom: 1px solid var(--border);
  }
  .cap.you {
    color: var(--text);
    font-style: italic;
  }
  .mic {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    border: 1px solid var(--border);
    background: var(--surface-2);
    color: var(--text-2);
    cursor: pointer;
    flex: none;
  }
  .mic.on {
    background: var(--accent);
    color: var(--on-accent);
    border-color: transparent;
  }
  .msgs {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .msg {
    align-self: stretch;
  }
  .msg.you {
    align-self: flex-end;
    max-width: 88%;
  }
  .msg.you p {
    margin: 0;
    padding: 7px 11px;
    border-radius: 14px;
    background: var(--accent-soft);
    font-size: 13.5px;
    line-height: 1.45;
    white-space: pre-wrap;
  }
  .msg .md {
    font-size: 13.5px;
  }
  .msg .md :global(pre) {
    position: relative;
    padding-top: 30px;
  }
  .msg .md :global(.run-code) {
    display: none !important;
  }
  .msg .md :global(.ins) {
    /* Top left: the language label and the copy button are on the right. */
    position: absolute;
    top: 5px;
    left: 8px;
    z-index: 1;
    display: flex;
    gap: 4px;
  }
  .msg .md :global(.ins button) {
    font: 11px var(--font);
    padding: 2px 7px;
    border-radius: 6px;
    border: 1px solid var(--border-strong);
    background: var(--surface);
    color: var(--text-2);
    cursor: pointer;
  }
  .msg .md :global(.ins button:hover) {
    background: var(--accent);
    color: var(--on-accent);
    border-color: transparent;
  }
  .msg small {
    display: block;
    margin: 3px 2px 0;
    color: var(--text-3);
    font-size: 11px;
  }
  .srcs {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
    margin-top: 4px;
    font-size: 11px;
    color: var(--text-3);
  }
  .src {
    font-size: 11px;
    padding: 1px 7px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--surface-2);
    color: var(--text-2);
    cursor: pointer;
    max-width: 220px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hint {
    color: var(--text-3);
    font-size: 12.5px;
    line-height: 1.5;
  }
  .hint p {
    margin: 0 0 8px;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 0 10px 8px;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    padding: 3px 9px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--surface-2);
    color: var(--text-2);
    cursor: pointer;
    white-space: nowrap;
  }
  .chip:hover {
    color: var(--text);
    border-color: var(--border-strong);
  }
  .compose {
    display: flex;
    gap: 6px;
    padding: 0 10px 10px;
  }
  .compose textarea {
    flex: 1;
    resize: none;
    min-height: 36px;
    max-height: 120px;
  }
  .send {
    width: 36px;
    border-radius: var(--r-sm);
    border: 0;
    background: var(--accent);
    color: var(--on-accent);
    display: grid;
    place-items: center;
    cursor: pointer;
  }
  .send:disabled {
    opacity: 0.4;
    cursor: default;
  }
</style>
