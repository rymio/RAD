<script lang="ts">
  // Beside the game: Gilgamesh as an Oort and Rust tutor. The conversation is one of his (kept,
  // recapped), one per scenario; he sees the code in the editor and the compiler's errors, and
  // the Oort docs (downloaded here, and in the vault for RAG). A code block in his answer goes
  // into the game's editor with one click, and can be run there.
  import { tick } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { store } from '../lib/state.svelte';
  import { gilgamesh } from '../lib/gilgamesh/gilgamesh.svelte';
  import { avatarImage } from '../lib/gilgamesh/face';
  import { Head } from '../lib/gilgamesh/head';
  import { renderMath } from '../lib/colab/notebook';
  import { oort, scenarioTitle } from '../lib/oort.svelte';
  import { timeAgo } from '../lib/util';
  import type { Citation } from '../lib/types';
  import { ArrowUp, Mic, MicOff, Square, Loader, SquarePen, ScanFace, Bug, Lightbulb, BookOpen, Download, Check, DatabaseZap, RefreshCw, Rocket, Crosshair } from '@lucide/svelte';

  const g = gilgamesh;

  let text = $state('');
  let scroller = $state<HTMLDivElement | null>(null);
  let canvas = $state<HTMLCanvasElement | null>(null);
  let ready = $state<boolean | null>(null);

  const scenario = $derived(oort.game?.scenario ?? 'welcome');
  const about = $derived(g.oortGame?.id === scenario);
  const turns = $derived(about ? g.turns.filter((t) => !t.quiet) : []);
  const on = $derived(g.phase !== 'off');
  const busy = $derived(g.phase === 'thinking' || g.phase === 'speaking');
  const caption = $derived(g.heard || (busy ? g.status || g.saying : '') || g.error || '');
  const broken = $derived(!!oort.game?.compiler);
  const tutorial = $derived(scenario.startsWith('tutorial_'));

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
    if (!q) return;
    await oort.refresh();
    await g.discussOort(oort.game?.scenario ?? 'welcome');
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
    await g.discussOort(oort.game?.scenario ?? 'welcome');
    await g.start();
  }

  /** His answer, with buttons on each code block to put it in the game's editor. */
  function answer(md: string) {
    return renderMath(md).replace(/<pre>/g, '<pre><span class="ins"><button data-put="1" title="Replace the code in Oort’s editor with this">Put in editor</button><button data-put="run" title="Put it in the editor and run it">▶ Run</button></span>');
  }

  function onClick(e: MouseEvent) {
    const t = e.target as HTMLElement;
    const put = t.closest('[data-put]') as HTMLElement | null;
    if (put) {
      const code = put.closest('pre')?.querySelector('code')?.textContent ?? '';
      void oort.setCode(code, put.dataset.put === 'run');
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
      <small>Your Oort tutor · sees your code and the docs</small>
    </div>
    <span class="spacer"></span>
    {#if about && turns.length}
      <button class="icon-btn sm" title="New talk about this scenario" onclick={async () => { await g.newConversation(); await g.discussOort(scenario); }}><SquarePen size={13} /></button>
    {/if}
    {#if busy}<button class="icon-btn sm" title="Stop him" onclick={() => g.interrupt()}><Square size={12} /></button>{/if}
    <button class="mic" class:on title={on ? 'Microphone off' : 'Talk with Gilgamesh'} disabled={g.phase === 'starting'} onclick={toggleMic}>
      {#if g.phase === 'starting'}<Loader size={14} class="spin" />{:else if on}<MicOff size={14} />{:else}<Mic size={14} />{/if}
    </button>
  </div>

  <div class="docs" class:warn={oort.docsLoaded && !oort.docs}>
    {#if oort.downloading}
      <Loader size={13} class="spin" />
      <span class="truncate">{oort.downloading.step}{oort.downloading.total ? ` · ${oort.downloading.done + 1} of ${oort.downloading.total}` : '…'}</span>
    {:else if oort.docs}
      <BookOpen size={13} />
      <span class="truncate" title="Downloaded {new Date(oort.docs.fetched).toLocaleString()}, kept on this computer">Docs: oort_api {oort.docs.version}, {oort.docs.tutorials} tutorials, wiki · {timeAgo(oort.docs.fetched)}</span>
      <span class="spacer"></span>
      {#if oort.inRag}
        <span class="rag" title="In your vault’s Oort project: every chat can search them"><Check size={11} /> RAG</span>
      {:else}
        <button class="btn sm ghost" disabled={oort.adding} title="Add the docs to your vault (project Oort), so every chat can use them" onclick={() => oort.addToRag().catch((e) => store.error(e, 'Couldn’t add the docs to the vault'))}>
          {#if oort.adding}<Loader size={12} class="spin" />{:else}<DatabaseZap size={12} />{/if} Add to RAG
        </button>
      {/if}
      <button class="icon-btn sm" title="Download them again (a new version of the game)" onclick={() => oort.download()}><RefreshCw size={12} /></button>
    {:else if oort.docsLoaded}
      <BookOpen size={13} />
      <span class="grow">Download the Oort docs so Gilgamesh knows the API, the tutorials and the wiki (offline too).</span>
      <button class="btn sm primary" onclick={() => oort.download()}><Download size={12} /> Download</button>
    {/if}
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
        <p><Rocket size={12} /> <strong>{scenarioTitle(scenario)}</strong></p>
        <p>Ask about the API, the physics or your code. Gilgamesh sees the code in the editor and the compiler’s errors. His code goes into the editor with <em>Put in editor</em>, or straight into a run with <em>▶ Run</em>.</p>
        <p>Pick a scenario from the list at the top right of the game. If you’re new, start with “Tutorial 1: Guns”.</p>
      </div>
    {/each}
  </div>
  <div class="chips">
    {#if broken}
      <button class="chip" onclick={() => ask('My code doesn’t compile. What does the error mean, and how do I fix it? Give me the fixed file.')}><Bug size={12} /> Why doesn’t it compile?</button>
    {/if}
    {#if tutorial}
      <button class="chip" onclick={() => ask('What does this tutorial want me to do? Explain the idea behind it.')}><Crosshair size={12} /> What’s the goal?</button>
      <button class="chip" onclick={() => ask('Give me a hint for this tutorial, without the full solution.')}><Lightbulb size={12} /> A hint</button>
    {/if}
    <button class="chip" onclick={() => ask('Explain my code: what does my ship do each tick, and what would make it better?')}><BookOpen size={12} /> Explain my code</button>
  </div>
  <form class="compose" onsubmit={send}>
    <textarea class="input" rows="1" bind:value={text} onkeydown={onKey} placeholder="Ask Gilgamesh about Oort…"></textarea>
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
  .docs {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 10px;
    min-height: 36px;
    font-size: 11.5px;
    color: var(--text-2);
    border-bottom: 1px solid var(--border);
    background: var(--surface-2);
  }
  .docs.warn {
    background: var(--accent-softer);
  }
  .docs .grow {
    flex: 1;
    line-height: 1.35;
  }
  .rag {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    font-size: 10px;
    font-weight: 600;
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--success-soft);
    color: var(--success);
    flex: none;
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
  .hint strong {
    color: var(--text);
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
