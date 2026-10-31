<script lang="ts">
  // Games → Chess: play chess with Gilgamesh, and talk about the game while you play. The saved
  // games are listed first; a game opens on the board with its moves and the chat beside it.
  // See lib/chess.svelte.ts.
  import { onMount, tick } from 'svelte';
  import { ask as confirmDialog } from '@tauri-apps/plugin-dialog';
  import { Chess, type Square } from 'chess.js';
  import { store } from '../lib/state.svelte';
  import { chess, LEVELS, moveLabel, type ChessGame, type Level, type Side } from '../lib/chess.svelte';
  import { timeAgo } from '../lib/util';
  import ChessBoard from './ChessBoard.svelte';
  import ChessChat from './ChessChat.svelte';
  import Menu from './Menu.svelte';
  import { Crown, Plus, Loader, ArrowLeft, Undo2, Flag, Handshake, Copy, Trash2, MoreHorizontal, Repeat2, ChevronLeft, ChevronRight, ChevronsLeft, ChevronsRight, CircleAlert, Pencil } from '@lucide/svelte';

  const GLYPH: Record<string, string> = { q: '♛', r: '♜', b: '♝', n: '♞', p: '♟' };

  const game = $derived(chess.game);
  let flipped = $state(false);
  let movesEl = $state<HTMLDivElement | null>(null);
  const orientation = $derived<Side>(game ? (flipped ? (game.owner === 'w' ? 'b' : 'w') : game.owner) : 'w');

  /** The king in check on the board shown. */
  const check = $derived.by(() => {
    try {
      const c = new Chess(chess.fen);
      return c.inCheck() ? ((c.findPiece({ type: 'k', color: c.turn() })[0] ?? null) as Square | null) : null;
    } catch {
      return null;
    }
  });
  const material = $derived(chess.material(chess.fen));
  const opening = $derived(game ? [...game.moves].reverse().find((m) => m.think?.opening)?.think?.opening ?? null : null);
  const shownPly = $derived(chess.viewAt ?? game?.moves.length ?? 0);
  /** The moves in pairs, for the list. */
  const rows = $derived.by(() => {
    const out: { n: number; w?: { i: number; san: string; by: string; tag: string }; b?: { i: number; san: string; by: string; tag: string } }[] = [];
    game?.moves.forEach((m, i) => {
      const tag = m.by === 'owner' && m.delta !== undefined ? (m.delta <= -250 ? '??' : m.delta <= -120 ? '?' : m.delta >= 120 ? '!' : '') : '';
      const cell = { i, san: m.san, by: m.by, tag };
      if (i % 2 === 0) out.push({ n: i / 2 + 1, w: cell });
      else if (out.length) out[out.length - 1].b = cell;
      else out.push({ n: 1, b: cell });
    });
    return out;
  });

  onMount(() => {
    chess.load().catch((e) => store.error(e, 'Couldn’t load your chess games'));
    void chess.checkEngine();
  });

  // The list follows the game.
  $effect(() => {
    void game?.moves.length;
    void chess.viewAt;
    tick().then(() => movesEl?.querySelector('.cur')?.scrollIntoView({ block: 'nearest' }));
  });

  function onKey(e: KeyboardEvent) {
    if (!game || (e.target as HTMLElement)?.closest('input, textarea, select')) return;
    const len = game.moves.length;
    if (e.key === 'ArrowLeft') chess.view(shownPly - 1);
    else if (e.key === 'ArrowRight') chess.view(shownPly + 1 >= len ? null : shownPly + 1);
    else if (e.key === 'Home') chess.view(0);
    else if (e.key === 'End') chess.view(null);
    else return;
    e.preventDefault();
  }

  async function resign() {
    if (await confirmDialog('Resign this game? Gilgamesh wins it.', { title: 'Resign', kind: 'warning' })) chess.resign();
  }

  function draw() {
    if (!chess.offerDraw()) store.toast('info', 'Gilgamesh plays on', 'He’d rather keep going.');
  }

  async function remove(g: ChessGame) {
    if (await confirmDialog(`Delete “${g.title}”? Your conversation with Gilgamesh about it stays in his view.`, { title: 'Delete game', kind: 'warning' })) await chess.remove(g.id);
  }

  function rename(g: ChessGame) {
    const t = window.prompt('Name this game', g.title);
    if (t) void chess.rename(g.id, t);
  }

  async function copyPgn(g: ChessGame) {
    await navigator.clipboard.writeText(chess.pgn(g));
    store.toast('success', 'Copied the game (PGN)', 'Paste it into any chess program.');
  }

  const status = $derived.by(() => {
    if (!game) return '';
    if (game.result) return game.ending ?? game.result;
    if (chess.viewAt !== null) return `Looking back: after ${shownPly ? moveLabel(shownPly - 1, game.moves[shownPly - 1].san) : 'no moves'}`;
    if (chess.thinking) return 'Gilgamesh is thinking…';
    if (chess.turn === game.owner) return check ? 'Your move: you’re in check' : 'Your move';
    return 'Gilgamesh to move';
  });

  const lost = (s: Side) => material.lost[s].map((k) => GLYPH[k] + '︎').join('');
  const lead = (s: Side) => {
    const d = material[s] - material[s === 'w' ? 'b' : 'w'];
    return d > 0 ? `+${d}` : '';
  };
  const result = (g: ChessGame) => (g.result ? (g.result === '1/2-1/2' ? 'Draw' : (g.result === '1-0') === (g.owner === 'w') ? 'You won' : 'Gilgamesh won') : `${g.moves.length ? `Move ${Math.floor(g.moves.length / 2) + 1}` : 'Not started'}`);
</script>

<svelte:window onkeydown={onKey} />

{#snippet player(side: Side)}
  {@const mine = game && side === game.owner}
  <div class="player">
    <span class="dotside" class:white={side === 'w'}></span>
    <strong>{mine ? 'You' : 'Gilgamesh'}</strong>
    {#if !mine && chess.thinking}<span class="muted small think"><Loader size={12} class="spin" /> thinking</span>{/if}
    <span class="spacer"></span>
    <span class="caps" title="Pieces taken from the other side">{lost(side === 'w' ? 'b' : 'w')}</span>
    {#if lead(side)}<span class="lead">{lead(side)}</span>{/if}
  </div>
{/snippet}

<div class="chess">
  <header class="drag">
    {#if game}
      <button class="icon-btn no-drag" title="Your games" onclick={() => chess.close()}><ArrowLeft size={16} /></button>
      <div class="ttl">
        <h1 class="truncate">{game.title}</h1>
        <span class="muted small">{LEVELS[game.level].label}</span>
      </div>
      <span class="spacer"></span>
      <button class="icon-btn no-drag" title="Turn the board round" onclick={() => (flipped = !flipped)}><Repeat2 size={16} /></button>
      <button class="btn sm no-drag" disabled={!chess.canTakeBack} title="Take back your last move" onclick={() => chess.takeBack()}><Undo2 size={13} /> Take back</button>
      {#if !game.result}
        <button class="btn sm no-drag" title="Offer a draw" onclick={draw}><Handshake size={13} /> Draw?</button>
        <button class="btn sm no-drag" title="Resign" onclick={resign}><Flag size={13} /> Resign</button>
      {/if}
      <Menu
        items={[
          { label: 'Copy the game (PGN)', icon: Copy, onclick: () => copyPgn(game) },
          { label: 'Rename', icon: Pencil, onclick: () => rename(game) },
          null,
          { label: 'Delete game', icon: Trash2, danger: true, onclick: () => remove(game) },
        ]}
      >
        {#snippet trigger(toggle: () => void)}<button class="icon-btn no-drag" title="More" onclick={toggle}><MoreHorizontal size={16} /></button>{/snippet}
      </Menu>
      <button class="btn sm primary no-drag" onclick={() => chess.newGame()}><Plus size={13} /> New game</button>
    {:else}
      <Crown size={17} />
      <h1>Chess</h1>
      <span class="muted small">with Gilgamesh</span>
      <span class="spacer"></span>
    {/if}
  </header>

  {#if chess.installed === false}
    <div class="warn"><CircleAlert size={14} /> The chess engine isn’t built yet. Run <code>./scripts/build-chess.sh</code> and restart RAD.</div>
  {/if}

  {#if game}
    <div class="play">
      <div class="left">
        {@render player(orientation === 'w' ? 'b' : 'w')}
        <div class="boardbox">
          <ChessBoard
            fen={chess.fen}
            {orientation}
            targets={(sq) => chess.targets(sq)}
            isPromotion={(a, b) => chess.isPromotion(a, b)}
            onmove={(a, b, p) => chess.ownerMove(a, b, p)}
            lastMove={chess.lastMove}
            {check}
            arrows={chess.arrows}
          />
        </div>
        {@render player(orientation)}
      </div>
      <div class="right">
        <div class="status" class:done={!!game.result} class:back={chess.viewAt !== null}>
          <span>{status}</span>
          {#if chess.viewAt !== null}<button class="btn sm" onclick={() => chess.view(null)}>Back to the game</button>{/if}
        </div>
        {#if opening}<div class="opening muted small truncate" title={opening}>{opening}</div>{/if}
        <div class="moves" bind:this={movesEl}>
          {#each rows as r (r.n)}
            <span class="no">{r.n}.</span>
            {#each [r.w, r.b] as m}
              {#if m}
                <button class="mv" class:his={m.by === 'gilgamesh'} class:cur={shownPly === m.i + 1} title={m.by === 'gilgamesh' ? 'Gilgamesh’s move: look at it, and ask him why' : 'Your move'} onclick={() => chess.view(m.i + 1)}>{m.san}{#if m.tag}<i class="tag" class:bad={m.tag.startsWith('?')}>{m.tag}</i>{/if}</button>
              {:else}
                <span></span>
              {/if}
            {/each}
          {:else}
            <span class="muted small none">{game.owner === 'w' ? 'You play White: make the first move.' : 'Gilgamesh plays White and moves first.'}</span>
          {/each}
        </div>
        <div class="nav">
          <button class="icon-btn sm" title="Start (Home)" disabled={shownPly === 0} onclick={() => chess.view(0)}><ChevronsLeft size={15} /></button>
          <button class="icon-btn sm" title="Back (←)" disabled={shownPly === 0} onclick={() => chess.view(shownPly - 1)}><ChevronLeft size={15} /></button>
          <button class="icon-btn sm" title="Forward (→)" disabled={chess.viewAt === null} onclick={() => chess.view(shownPly + 1)}><ChevronRight size={15} /></button>
          <button class="icon-btn sm" title="The game now (End)" disabled={chess.viewAt === null} onclick={() => chess.view(null)}><ChevronsRight size={15} /></button>
        </div>
        {#key game.id}<ChessChat {game} />{/key}
      </div>
    </div>
  {:else if !chess.loaded}
    <div class="center"><Loader size={20} class="spin" /></div>
  {:else}
    <div class="lib">
      <section class="newgame">
        <h2>New game</h2>
        <div class="row">
          <span class="lbl">You play</span>
          <div class="seg">
            {#each [['w', 'White'], ['b', 'Black'], ['random', 'Either']] as [k, l] (k)}
              <button class:on={chess.settings.side === k} onclick={() => chess.saveSettings({ side: k as Side | 'random' })}>{l}</button>
            {/each}
          </div>
        </div>
        <div class="row">
          <span class="lbl">Gilgamesh</span>
          <div class="seg">
            {#each Object.entries(LEVELS) as [k, l] (k)}
              <button class:on={chess.settings.level === k} title={l.note} onclick={() => chess.saveSettings({ level: k as Level })}>{l.label}</button>
            {/each}
          </div>
        </div>
        <p class="muted small">{LEVELS[chess.settings.level].note}. Talk to him while you play: ask why he made a move, how you're doing, or for a hint.</p>
        <button class="btn primary" disabled={chess.installed === false} onclick={() => chess.newGame()}><Crown size={14} /> Play</button>
      </section>

      {#if chess.list.length}
        <h3 class="muted small">Your games</h3>
        <div class="games">
          {#each chess.list as g (g.id)}
            <div class="game">
              <button class="open" onclick={() => chess.open(g.id)}>
                <span class="name truncate">{g.title}</span>
                <span class="sub muted small">{result(g)} · {LEVELS[g.level].label} · {timeAgo(g.updated)}</span>
              </button>
              <Menu
                items={[
                  { label: 'Copy the game (PGN)', icon: Copy, onclick: () => copyPgn(g) },
                  { label: 'Rename', icon: Pencil, onclick: () => rename(g) },
                  null,
                  { label: 'Delete', icon: Trash2, danger: true, onclick: () => remove(g) },
                ]}
              >
                {#snippet trigger(toggle: () => void)}<button class="icon-btn sm" title="More" onclick={toggle}><MoreHorizontal size={14} /></button>{/snippet}
              </Menu>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .chess {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  header {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    height: var(--titlebar-h);
    padding: 0 12px 0 calc(14px + var(--lead));
    border-bottom: 1px solid var(--border);
  }
  h1 {
    margin: 0;
    font-size: 15px;
    font-weight: 650;
  }
  .ttl {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-width: 0;
    flex: 0 1 auto;
  }
  .small {
    font-size: 12px;
  }
  .spacer {
    flex: 1;
  }
  .warn {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 12px 16px 0;
    padding: 8px 12px;
    border-radius: var(--r-sm);
    background: var(--surface-2);
    border: 1px solid var(--border);
    font-size: 12.5px;
  }
  .center {
    flex: 1;
    display: grid;
    place-items: center;
  }

  /* playing */
  .play {
    flex: 1;
    min-height: 0;
    display: flex;
    gap: 18px;
    padding: 14px 18px 16px;
  }
  .left {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    min-height: 0;
  }
  .boardbox {
    width: min(100%, calc(100vh - var(--titlebar-h) - 110px));
    min-width: 240px;
  }
  .player {
    width: min(100%, calc(100vh - var(--titlebar-h) - 110px));
    min-width: 240px;
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    min-height: 24px;
  }
  .dotside {
    width: 12px;
    height: 12px;
    border-radius: 3px;
    background: #222;
    border: 1px solid var(--border-strong);
  }
  .dotside.white {
    background: #f5f5f0;
  }
  .think {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .caps {
    font-family: 'Apple Symbols', 'Segoe UI Symbol', 'Noto Sans Symbols 2', 'DejaVu Sans', serif;
    font-size: 17px;
    letter-spacing: -2px;
    color: var(--text-2);
  }
  .lead {
    font-size: 12px;
    color: var(--text-3);
  }
  .right {
    width: 340px;
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 8px;
    min-height: 0;
  }
  .status {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 12px;
    border-radius: var(--r-sm);
    background: var(--surface-2);
    font-size: 13px;
    font-weight: 600;
    min-height: 38px;
  }
  .status.done {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .status.back {
    font-weight: 500;
  }
  .opening {
    padding: 0 4px;
  }
  .moves {
    flex: 0 1 auto;
    max-height: 30%;
    min-height: 64px;
    overflow: auto;
    display: grid;
    grid-template-columns: 34px 1fr 1fr;
    align-content: start;
    gap: 1px 4px;
    padding: 6px;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    font-size: 13px;
    font-variant-numeric: tabular-nums;
  }
  .no {
    color: var(--text-3);
    padding: 3px 0 3px 4px;
  }
  .none {
    grid-column: 1 / -1;
    padding: 6px;
  }
  .mv {
    text-align: left;
    padding: 3px 6px;
    border: 0;
    border-radius: 4px;
    background: none;
    color: var(--text);
    cursor: pointer;
    font: inherit;
  }
  .mv.his {
    color: var(--text-2);
  }
  .mv:hover {
    background: var(--surface-2);
  }
  .mv.cur {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .tag {
    font-style: normal;
    margin-left: 2px;
    color: #2f9e55;
    font-weight: 700;
  }
  .tag.bad {
    color: #d9534f;
  }
  .nav {
    display: flex;
    justify-content: center;
    gap: 4px;
  }

  /* the list of games */
  .lib {
    flex: 1;
    overflow: auto;
    padding: 24px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    max-width: 640px;
    width: 100%;
    margin: 0 auto;
  }
  .newgame {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 18px;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--surface);
  }
  .newgame h2 {
    margin: 0;
    font-size: 17px;
  }
  .newgame p {
    margin: 0;
    line-height: 1.5;
  }
  .newgame .btn {
    align-self: flex-start;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  .lbl {
    width: 76px;
    font-size: 12.5px;
    color: var(--text-3);
  }
  .seg {
    display: inline-flex;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    overflow: hidden;
  }
  .seg button {
    background: none;
    border: 0;
    padding: 5px 11px;
    font-size: 12.5px;
    color: var(--text-2);
    cursor: pointer;
  }
  .seg button + button {
    border-left: 1px solid var(--border);
  }
  .seg button.on {
    background: var(--accent-soft);
    color: var(--accent);
  }
  h3 {
    margin: 12px 0 0;
    font-weight: 600;
  }
  .games {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--r);
    overflow: hidden;
  }
  .game {
    display: flex;
    align-items: center;
    gap: 6px;
    padding-right: 8px;
  }
  .game + .game {
    border-top: 1px solid var(--border);
  }
  .open {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    padding: 10px 14px;
    background: none;
    border: 0;
    color: var(--text);
    cursor: pointer;
    text-align: left;
  }
  .open:hover {
    background: var(--surface-2);
  }
  .name {
    font-size: 13.5px;
    font-weight: 550;
    max-width: 100%;
  }
  @media (max-width: 900px) {
    .play {
      flex-direction: column;
      overflow: auto;
    }
    .right {
      width: 100%;
      min-height: 420px;
    }
  }
</style>
