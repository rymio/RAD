<script lang="ts">
  // Beside the board: talking with Gilgamesh about the game. The conversation is one of his (kept,
  // recapped, in his own view too), about this game; he sees the board, the moves and what his
  // engine saw for each of his, so "why did you play that?" gets a real answer.
  import { tick } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { store } from '../lib/state.svelte';
  import { gilgamesh } from '../lib/gilgamesh/gilgamesh.svelte';
  import { avatarImage } from '../lib/gilgamesh/face';
  import { Head } from '../lib/gilgamesh/head';
  import { chess, moveLabel, type ChessGame, type Talk } from '../lib/chess.svelte';
  import { ArrowUp, Mic, MicOff, Square, Loader, SquarePen, CircleHelp, Lightbulb, TrendingUp, ScanFace } from '@lucide/svelte';

  let { game }: { game: ChessGame } = $props();
  const g = gilgamesh;

  let text = $state('');
  let input = $state<HTMLTextAreaElement | null>(null);
  let scroller = $state<HTMLDivElement | null>(null);
  let canvas = $state<HTMLCanvasElement | null>(null);
  let ready = $state<boolean | null>(null);

  /** This conversation is about the game (else his other talk isn't shown here). */
  const about = $derived(g.game?.id === game.id);
  const turns = $derived(about ? g.turns.filter((t) => !t.quiet) : []);
  const on = $derived(g.phase !== 'off');
  const busy = $derived(g.phase === 'thinking' || g.phase === 'speaking');
  const caption = $derived(g.heard || (busy ? g.status || g.saying : '') || g.error || '');

  /** The move "why?" is about: the one being looked at if it's his, else his last. */
  const whyMove = $derived.by(() => {
    const n = chess.viewAt ?? game.moves.length;
    const at = game.moves[n - 1]?.by === 'gilgamesh' ? n - 1 : game.moves.findLastIndex((m) => m.by === 'gilgamesh');
    return at >= 0 ? moveLabel(at, game.moves[at].san) : null;
  });

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

  async function ask(q: string) {
    q = q.trim();
    if (!q) return;
    await g.discussChess(game.id);
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
    await g.discussChess(game.id);
    await g.start();
  }

  async function hint() {
    await chess.requestHint();
    if (chess.hint) void ask('Give me a hint.');
  }
</script>

<div class="chat">
  <div class="bar">
    {#if g.avatar}
      <span class="pic" class:talking={busy} title="Gilgamesh"><canvas bind:this={canvas}></canvas></span>
    {:else}
      <button class="icon-btn sm" title="Give Gilgamesh a face" onclick={() => store.navigate({ kind: 'gilgamesh' })}><ScanFace size={15} /></button>
    {/if}
    <strong>Gilgamesh</strong>
    <span class="spacer"></span>
    <select class="input sm talk" title="How much he says while you play" value={chess.settings.talk} onchange={(e) => chess.saveSettings({ talk: e.currentTarget.value as Talk })}>
      <option value="often">Chatty</option>
      <option value="sometimes">Key moments</option>
      <option value="quiet">Only when asked</option>
    </select>
    {#if about && turns.length}
      <button class="icon-btn sm" title="New talk about this game" onclick={async () => { await g.newConversation(); await g.discussChess(game.id); }}><SquarePen size={13} /></button>
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
        <p>{t.text}{#if t.who === 'gilgamesh' && !t.text && busy}<Loader size={12} class="spin" />{/if}</p>
        {#if t.actions?.length}<small>{t.actions.map((a) => a.label).join(' · ')}</small>{/if}
      </div>
    {:else}
      <p class="hint">Play, and talk as you go. Ask him why he made a move (click one of his moves to go back to it), how you're doing, or for a hint. He says a word at the key moments; change that above.</p>
    {/each}
  </div>
  <div class="chips">
    <button class="chip" disabled={!whyMove} onclick={() => ask(`Why did you play ${whyMove}?`)}><CircleHelp size={12} /> Why {whyMove ?? 'that move'}?</button>
    <button class="chip" disabled={!game.moves.length} onclick={() => ask('How am I doing?')}><TrendingUp size={12} /> How am I doing?</button>
    <button class="chip" disabled={!chess.ownersTurn || chess.hinting} onclick={hint}>{#if chess.hinting}<Loader size={12} class="spin" />{:else}<Lightbulb size={12} />{/if} Hint</button>
  </div>
  <form class="compose" onsubmit={send}>
    <textarea class="input" rows="1" bind:this={input} bind:value={text} onkeydown={onKey} placeholder="Talk to Gilgamesh about the game…"></textarea>
    <button class="send" disabled={!text.trim()} title="Send"><ArrowUp size={15} /></button>
  </form>
</div>

<style>
  .chat {
    display: flex;
    flex-direction: column;
    min-height: 0;
    flex: 1;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--surface);
    overflow: hidden;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 8px 6px 10px;
    min-height: 40px;
    font-size: 12.5px;
    border-bottom: 1px solid var(--border);
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
  .talk {
    width: auto;
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
    gap: 8px;
  }
  .msg {
    max-width: 88%;
    align-self: flex-start;
  }
  .msg p {
    margin: 0;
    padding: 7px 11px;
    border-radius: 14px;
    background: var(--surface-2);
    font-size: 13.5px;
    line-height: 1.45;
    white-space: pre-wrap;
  }
  .msg.you {
    align-self: flex-end;
  }
  .msg.you p {
    background: var(--accent-soft);
  }
  .msg small {
    display: block;
    margin: 3px 11px 0;
    color: var(--text-3);
    font-size: 11px;
  }
  .hint {
    margin: 0;
    color: var(--text-3);
    font-size: 12.5px;
    line-height: 1.5;
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
  .chip:hover:not(:disabled) {
    color: var(--text);
    border-color: var(--border-strong);
  }
  .chip:disabled {
    opacity: 0.45;
    cursor: default;
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
