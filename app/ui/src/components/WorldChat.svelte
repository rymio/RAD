<script lang="ts">
  // Beside the Fyrox view: Gilgamesh, in the same conversation as in his world. Ask about Fyrox
  // (he has the book), about the world and what to build, or about your tasks.
  import { tick } from 'svelte';
  import { store } from '../lib/state.svelte';
  import { gilgamesh } from '../lib/gilgamesh/gilgamesh.svelte';
  import { avatarImage } from '../lib/gilgamesh/face';
  import { Head } from '../lib/gilgamesh/head';
  import { renderMarkdown } from '../lib/markdown';
  import { world } from '../lib/world.svelte';
  import type { Citation } from '../lib/types';
  import { ArrowUp, Mic, MicOff, Square, Loader, SquarePen, ScanFace, Hammer, ListTodo, BookOpen } from '@lucide/svelte';

  const g = gilgamesh;
  let text = $state('');
  let scroller = $state<HTMLDivElement | null>(null);
  let canvas = $state<HTMLCanvasElement | null>(null);

  const turns = $derived(g.worldTalk ? g.turns.filter((t) => !t.quiet) : []);
  const on = $derived(g.phase !== 'off');
  const busy = $derived(g.phase === 'thinking' || g.phase === 'speaking');
  const caption = $derived(g.heard || (busy ? g.status || g.saying : '') || g.error || '');

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

  async function ask(q: string) {
    q = q.trim();
    if (!q) return;
    await g.discussWorld();
    void g.ask(q);
  }

  function send(e?: Event) {
    e?.preventDefault();
    const q = text;
    text = '';
    void ask(q);
  }

  async function toggleMic() {
    if (on) return g.stop();
    await g.discussWorld();
    await g.start();
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
      <button class="icon-btn sm" title="Give Gilgamesh a face (it’s on him in his world too)" onclick={() => store.navigate({ kind: 'gilgamesh' })}><ScanFace size={15} /></button>
    {/if}
    <div class="who">
      <strong>Gilgamesh</strong>
      <small>{world.running ? 'In his world now' : 'Ask about Fyrox, his world, your tasks'}</small>
    </div>
    <span class="spacer"></span>
    {#if turns.length}
      <button class="icon-btn sm" title="A new talk" onclick={async () => { await g.newConversation(); await g.discussWorld(); }}><SquarePen size={13} /></button>
    {/if}
    {#if busy}<button class="icon-btn sm" title="Stop him" onclick={() => g.interrupt()}><Square size={12} /></button>{/if}
    <button class="mic" class:on title={on ? 'Microphone off' : 'Talk with Gilgamesh'} disabled={g.phase === 'starting'} onclick={toggleMic}>
      {#if g.phase === 'starting'}<Loader size={14} class="spin" />{:else if on}<MicOff size={14} />{:else}<Mic size={14} />{/if}
    </button>
  </div>
  {#if caption}<div class="cap truncate" class:you={!!g.heard}>{caption}</div>{/if}
  <div class="msgs" bind:this={scroller}>
    {#each turns as t, i (i)}
      <div class="msg" class:you={t.who === 'you'}>
        {#if t.who === 'you'}
          <p>{t.text}</p>
        {:else}
          <div class="md selectable">{@html renderMarkdown(t.text)}{#if !t.text && busy}<Loader size={12} class="spin" />{/if}</div>
          {#if sourcesOf(t.sources).length}
            <div class="srcs">
              <span>From your vault:</span>
              {#each sourcesOf(t.sources).slice(0, 5) as s (s.document_id)}
                <button class="src" title={s.snippet} onclick={() => openSource(s)}>{s.title}</button>
              {/each}
            </div>
          {/if}
        {/if}
        {#if t.actions?.length}<small>{t.actions.map((a) => a.label).join(' · ')}</small>{/if}
      </div>
    {:else}
      <div class="hint">
        <p>This is the same talk as in his world: what you say there shows up here, and the other way round.</p>
        <p>Ask him to build something (“build a fountain by your house”), about Fyrox (“how do I add a script in the editor?”), or about your tasks; he can write them on his clay tablet.</p>
      </div>
    {/each}
  </div>
  <div class="chips">
    <button class="chip" onclick={() => ask('What would you like to build in your world next? Pick one thing and build it.')}><Hammer size={12} /> Build something</button>
    <button class="chip" onclick={() => ask('Let’s go over my tasks. Write them on your clay tablet.')}><ListTodo size={12} /> My tasks</button>
    <button class="chip" onclick={() => ask('How is your world made in Fyrox, and how could I change it in the Fyrox editor?')}><BookOpen size={12} /> Fyrox</button>
  </div>
  <form class="compose" onsubmit={send}>
    <textarea class="input" rows="1" bind:value={text} onkeydown={(e) => e.key === 'Enter' && !e.shiftKey && !e.isComposing && send(e)} placeholder="Say something to Gilgamesh…"></textarea>
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
