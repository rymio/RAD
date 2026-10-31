<script lang="ts">
  // A chess board: shows a position (FEN) and lets the owner move by clicking or dragging. The
  // rules stay with the caller: `targets` says where a piece may go, `onmove` makes the move.
  import type { Square } from 'chess.js';

  type Arrow = { from: Square; to: Square; kind: 'hint' | 'show' };
  let {
    fen,
    orientation = 'w',
    targets = () => [],
    isPromotion = () => false,
    onmove = () => false,
    lastMove = null,
    check = null,
    arrows = [],
  }: {
    fen: string;
    orientation?: 'w' | 'b';
    targets?: (from: Square) => Square[];
    isPromotion?: (from: Square, to: Square) => boolean;
    onmove?: (from: Square, to: Square, promotion?: string) => boolean;
    lastMove?: { from: Square; to: Square } | null;
    check?: Square | null;
    arrows?: Arrow[];
  } = $props();

  const FILES = 'abcdefgh';
  // One glyph for both colours (the filled one), coloured by CSS; FE0E keeps it from turning into emoji.
  const GLYPH: Record<string, string> = { k: '♚', q: '♛', r: '♜', b: '♝', n: '♞', p: '♟' };
  const glyph = (p: string) => GLYPH[p.toLowerCase()] + '︎';

  /** Piece letters by square ("e1" → "K"). */
  const pieces = $derived.by(() => {
    const out: Record<string, string> = {};
    fen
      .split(' ')[0]
      .split('/')
      .forEach((row, r) => {
        let f = 0;
        for (const ch of row) {
          if (/\d/.test(ch)) f += Number(ch);
          else out[`${FILES[f++]}${8 - r}`] = ch;
        }
      });
    return out;
  });

  /** The squares from the top left, as the board is turned. */
  const squares = $derived.by(() => {
    const list: Square[] = [];
    for (let row = 0; row < 8; row++)
      for (let col = 0; col < 8; col++) {
        const f = orientation === 'w' ? col : 7 - col;
        const r = orientation === 'w' ? 8 - row : row + 1;
        list.push(`${FILES[f]}${r}` as Square);
      }
    return list;
  });

  let selected = $state<Square | null>(null);
  let moves = $state<Square[]>([]);
  let drag = $state<{ from: Square; x: number; y: number } | null>(null);
  let promo = $state<{ from: Square; to: Square } | null>(null);
  let boardEl = $state<HTMLDivElement | null>(null);

  // A new position clears the selection.
  $effect(() => {
    void fen;
    selected = null;
    moves = [];
    promo = null;
  });

  const isLight = (sq: string) => (FILES.indexOf(sq[0]) + Number(sq[1])) % 2 === 1;
  const white = (p: string | undefined) => !!p && p === p.toUpperCase();

  /** Centre of a square in board units (0–8, from the top left as shown). */
  function centre(sq: Square) {
    const i = squares.indexOf(sq);
    return { x: (i % 8) + 0.5, y: Math.floor(i / 8) + 0.5 };
  }

  function select(sq: Square | null) {
    selected = sq;
    moves = sq ? targets(sq) : [];
  }

  function attempt(from: Square, to: Square) {
    if (!targets(from).includes(to)) return false;
    if (isPromotion(from, to)) {
      promo = { from, to };
      return true;
    }
    const ok = onmove(from, to);
    select(null);
    return ok;
  }

  function choose(piece: string) {
    if (!promo) return;
    onmove(promo.from, promo.to, piece);
    promo = null;
    select(null);
  }

  function squareAt(x: number, y: number): Square | null {
    if (!boardEl) return null;
    const r = boardEl.getBoundingClientRect();
    const col = Math.floor(((x - r.left) / r.width) * 8);
    const row = Math.floor(((y - r.top) / r.height) * 8);
    if (col < 0 || col > 7 || row < 0 || row > 7) return null;
    return squares[row * 8 + col];
  }

  function down(e: PointerEvent, sq: Square) {
    if (promo || e.button !== 0) return;
    // A second click on a target makes the move.
    if (selected && moves.includes(sq)) {
      attempt(selected, sq);
      return;
    }
    const t = targets(sq);
    if (!t.length) {
      select(null);
      return;
    }
    select(sq);
    drag = { from: sq, x: e.clientX, y: e.clientY };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }

  function move(e: PointerEvent) {
    if (drag) drag = { ...drag, x: e.clientX, y: e.clientY };
  }

  function up(e: PointerEvent) {
    if (!drag) return;
    const from = drag.from;
    drag = null;
    const to = squareAt(e.clientX, e.clientY);
    if (to && to !== from) attempt(from, to);
  }
</script>

<div class="board" bind:this={boardEl}>
  {#each squares as sq, i (sq)}
    {@const p = pieces[sq]}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div
      class="sq"
      class:light={isLight(sq)}
      class:last={lastMove && (lastMove.from === sq || lastMove.to === sq)}
      class:sel={selected === sq}
      class:check={check === sq}
      onpointerdown={(e) => down(e, sq)}
      onpointermove={move}
      onpointerup={up}
    >
      {#if p}
        <span class="pc" class:w={white(p)} class:can={targets(sq).length > 0} class:lifted={drag?.from === sq}>{glyph(p)}</span>
      {/if}
      {#if moves.includes(sq)}<i class="dot" class:take={!!p}></i>{/if}
      {#if i % 8 === 0}<small class="rank">{sq[1]}</small>{/if}
      {#if i >= 56}<small class="file">{sq[0]}</small>{/if}
    </div>
  {/each}

  {#if arrows.length}
    <svg class="arrows" viewBox="0 0 8 8" aria-hidden="true">
      <defs>
        <marker id="chess-head-hint" markerWidth="3" markerHeight="3" refX="1.6" refY="1.5" orient="auto"><path d="M0,0 L3,1.5 L0,3 z" fill="var(--arrow-hint)" /></marker>
        <marker id="chess-head-show" markerWidth="3" markerHeight="3" refX="1.6" refY="1.5" orient="auto"><path d="M0,0 L3,1.5 L0,3 z" fill="var(--arrow-show)" /></marker>
      </defs>
      {#each arrows as a (`${a.from}${a.to}${a.kind}`)}
        {@const s = centre(a.from)}
        {@const t = centre(a.to)}
        {@const len = Math.hypot(t.x - s.x, t.y - s.y)}
        {@const k = len ? (len - 0.35) / len : 1}
        <line x1={s.x} y1={s.y} x2={s.x + (t.x - s.x) * k} y2={s.y + (t.y - s.y) * k} class={a.kind} stroke-width="0.16" stroke-linecap="round" marker-end="url(#chess-head-{a.kind})" />
      {/each}
    </svg>
  {/if}

  {#if drag && pieces[drag.from] && boardEl}
    {@const r = boardEl.getBoundingClientRect()}
    <span class="pc ghost" class:w={white(pieces[drag.from])} style="left:{drag.x - r.left}px; top:{drag.y - r.top}px; font-size:{r.width / 10}px">{glyph(pieces[drag.from])}</span>
  {/if}

  {#if promo}
    {@const w = white(pieces[promo.from])}
    <div class="promo">
      <div class="pick">
        {#each ['q', 'r', 'b', 'n'] as k (k)}
          <button class="pc" class:w title={{ q: 'Queen', r: 'Rook', b: 'Bishop', n: 'Knight' }[k]} onclick={() => choose(k)}>{glyph(k)}</button>
        {/each}
      </div>
      <button class="btn sm ghost" onclick={() => (promo = null)}>Cancel</button>
    </div>
  {/if}
</div>

<style>
  .board {
    --light: #eedbb8;
    --dark: #b58863;
    --arrow-hint: rgba(34, 160, 90, 0.85);
    --arrow-show: rgba(230, 120, 20, 0.85);
    position: relative;
    display: grid;
    grid-template-columns: repeat(8, 1fr);
    grid-template-rows: repeat(8, 1fr);
    aspect-ratio: 1;
    width: 100%;
    border-radius: 6px;
    overflow: hidden;
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.22);
    user-select: none;
    touch-action: none;
    container-type: inline-size;
  }
  .sq {
    position: relative;
    background: var(--dark);
    display: grid;
    place-items: center;
  }
  .sq.light {
    background: var(--light);
  }
  .sq.last::before,
  .sq.sel::before {
    content: '';
    position: absolute;
    inset: 0;
    background: rgba(255, 214, 10, 0.42);
  }
  .sq.sel::before {
    background: rgba(255, 214, 10, 0.6);
  }
  .sq.check {
    background: radial-gradient(circle, rgba(255, 40, 40, 0.95) 0%, rgba(220, 30, 30, 0.6) 45%, transparent 75%), var(--dark);
  }
  .sq.light.check {
    background: radial-gradient(circle, rgba(255, 40, 40, 0.95) 0%, rgba(220, 30, 30, 0.6) 45%, transparent 75%), var(--light);
  }
  .pc {
    position: relative;
    font-family: 'Apple Symbols', 'Segoe UI Symbol', 'Noto Sans Symbols 2', 'DejaVu Sans', serif;
    font-size: 10cqw;
    line-height: 1;
    color: #1d1d1f;
    text-shadow: 0 1px 0 rgba(255, 255, 255, 0.25);
    pointer-events: none;
  }
  .pc.w {
    color: #fbfbf8;
    -webkit-text-stroke: 0.6px #2b2b2b;
    text-shadow: 0 1px 2px rgba(0, 0, 0, 0.45);
  }
  .pc.can {
    cursor: grab;
  }
  .pc.lifted {
    opacity: 0.3;
  }
  .ghost {
    position: absolute;
    transform: translate(-50%, -55%);
    z-index: 5;
    pointer-events: none;
  }
  .dot {
    position: absolute;
    width: 30%;
    height: 30%;
    border-radius: 50%;
    background: rgba(20, 20, 20, 0.22);
    pointer-events: none;
  }
  .dot.take {
    width: 86%;
    height: 86%;
    background: none;
    border: 5px solid rgba(20, 20, 20, 0.22);
  }
  .rank,
  .file {
    position: absolute;
    font-size: 10px;
    font-weight: 600;
    pointer-events: none;
    color: var(--light);
  }
  .sq.light .rank,
  .sq.light .file {
    color: var(--dark);
  }
  .rank {
    top: 2px;
    left: 3px;
  }
  .file {
    bottom: 1px;
    right: 3px;
  }
  .arrows {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    pointer-events: none;
  }
  .arrows line.hint {
    stroke: var(--arrow-hint);
  }
  .arrows line.show {
    stroke: var(--arrow-show);
  }
  .promo {
    position: absolute;
    inset: 0;
    z-index: 6;
    background: rgba(0, 0, 0, 0.45);
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
  }
  .pick {
    display: flex;
    gap: 6px;
    background: var(--surface);
    padding: 8px;
    border-radius: var(--r);
    box-shadow: var(--shadow);
  }
  .pick .pc {
    pointer-events: auto;
    font-size: 8cqw;
    width: 11cqw;
    height: 11cqw;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    background: var(--light);
    cursor: pointer;
  }
  .promo .btn {
    color: #fff;
  }
</style>
