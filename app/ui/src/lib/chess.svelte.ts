// Games → Chess: the owner plays chess with Gilgamesh. The rules are chess.js's; his moves come
// from RAD's chess engine (RustChess, app/src-tauri/src/chess.rs), which also says what it saw: the
// opening book, the score, the line it expected and how the other moves scored. That is kept with
// each of his moves, so he can explain any of them later ("why did you play that?").
//
// Games are kept in the encrypted Games database (next to the vault), move by move, and each has
// its own conversation with Gilgamesh (in his view too). He sees the game (describe(), in every
// answer) and, unless told to keep quiet, says a word at the moments that matter.

import { Chess, type Square } from 'chess.js';
import { invoke } from '@tauri-apps/api/core';
import { store } from './state.svelte';
import * as G from './games';
import { gilgamesh } from './gilgamesh/gilgamesh.svelte';

export type Side = 'w' | 'b';
export type Level = 'beginner' | 'easy' | 'medium' | 'hard';
/** Even depths only: the engine's scores then lean the same way all game (see chess/rad-chess). */
export const LEVELS: Record<Level, { label: string; note: string; depth: number; margin: number }> = {
  beginner: { label: 'Beginner', note: 'Plays a good move, not always the best one', depth: 2, margin: 150 },
  easy: { label: 'Easy', note: 'Looks one move ahead (his and yours)', depth: 2, margin: 0 },
  medium: { label: 'Medium', note: 'Looks two moves ahead', depth: 4, margin: 0 },
  hard: { label: 'Hard', note: 'Looks three moves ahead; takes a second or two', depth: 6, margin: 0 },
};
/** How much Gilgamesh says while playing. */
export type Talk = 'often' | 'sometimes' | 'quiet';

/** What his engine saw when it chose a move. */
export interface Think {
  book: boolean;
  opening: string | null;
  /** Centipawns from White's side (null: a book move with no look at the rest). */
  score: number | null;
  depth: number | null;
  ms: number;
  /** The moves he expected next (SAN). */
  line: string[];
  /** The best moves, each searched as deep as his (SAN). */
  candidates: { san: string; score: number }[];
}

export interface PlayedMove {
  uci: string;
  san: string;
  by: 'owner' | 'gilgamesh';
  at: number;
  think?: Think;
  /** The owner's moves: centipawns it gained the owner against what he expected (negative: it cost them). */
  delta?: number;
  /** The owner's moves: the move he expected them to play. */
  expected?: string;
}

export interface ChessGame {
  id: string;
  title: string;
  created: number;
  updated: number;
  owner: Side;
  level: Level;
  moves: PlayedMove[];
  result: '1-0' | '0-1' | '1/2-1/2' | null;
  /** How it ended, in words ("Checkmate: you won"). */
  ending: string | null;
  takebacks: number;
}

export interface Arrow {
  from: Square;
  to: Square;
  kind: 'hint' | 'show';
}

interface Answer {
  best: string;
  book: boolean;
  opening: string | null;
  score: number | null;
  depth: number | null;
  ms: number;
  line: string[];
  candidates: { uci: string; score: number }[];
}

interface ChessSettings {
  level: Level;
  side: Side | 'random';
  talk: Talk;
}

const SETTINGS_KEY = 'rad-chess';
const DEFAULTS: ChessSettings = { level: 'medium', side: 'w', talk: 'sometimes' };
/** RustChess's mate score; anything beyond MATE_NEAR is a forced mate. */
const MATE_NEAR = 15000;
const VALUE: Record<string, number> = { p: 1, n: 3, b: 3, r: 5, q: 9, k: 0 };
const NAME: Record<string, string> = { p: 'pawn', n: 'knight', b: 'bishop', r: 'rook', q: 'queen', k: 'king' };
const SIDE = { w: 'White', b: 'Black' } as const;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const other = (s: Side): Side => (s === 'w' ? 'b' : 'w');
const toMove = (uci: string) => ({ from: uci.slice(0, 2), to: uci.slice(2, 4), promotion: uci[4] });

/** "12.Nf3" or "12...Nf3" for the move at a ply (0 = White's first). */
export function moveLabel(ply: number, san: string) {
  return `${Math.floor(ply / 2) + 1}${ply % 2 ? '...' : '.'}${san}`;
}

/** A score (centipawns from White's side) as pawns for one side: "+1.4", "a forced mate". */
function pawns(cp: number, side: Side) {
  const v = side === 'w' ? cp : -cp;
  if (Math.abs(v) > MATE_NEAR) return v > 0 ? 'a forced mate for you' : 'a forced mate against you';
  return `${v >= 0 ? '+' : '−'}${Math.abs(v / 100).toFixed(1)}`;
}

/** UCI moves as SAN, played on from the position `c` is in (left as it was). */
function sanLine(c: Chess, ucis: string[]): string[] {
  const out: string[] = [];
  for (const u of ucis) {
    try {
      out.push(c.move(toMove(u)).san);
    } catch {
      break;
    }
  }
  for (let i = 0; i < out.length; i++) c.undo();
  return out;
}

function sanOf(c: Chess, uci: string) {
  return sanLine(c, [uci])[0] ?? uci;
}

class ChessState {
  list = $state<ChessGame[]>([]);
  loaded = $state(false);
  /** The game on the board (kept when the owner looks at another view). */
  openId = $state<string | null>(null);
  /** The position on the board, as FEN (the live one, or an earlier one being looked at). */
  fen = $state(new Chess().fen());
  /** The game now, as FEN (what `fen` shows unless looking back). */
  liveFen = $state(new Chess().fen());
  /** Looking back: the number of moves played in the position shown (null: the game now). */
  viewAt = $state<number | null>(null);
  /** He is choosing a move. */
  thinking = $state(false);
  arrows = $state<Arrow[]>([]);
  /** The hint the owner asked for (shown as an arrow, and told to Gilgamesh). */
  hint = $state<{ san: string; think: Think } | null>(null);
  hinting = $state(false);
  /** The engine is built (null: not known yet). */
  installed = $state<boolean | null>(null);
  settings = $state<ChessSettings>(this.loadSettings());

  /** The live game. */
  private c = new Chess();
  private run = 0;
  private loadingList: Promise<void> | null = null;
  private lastComment = 0;
  /** Openings he has named already in this game. */
  private named = new Set<string>();

  get game(): ChessGame | null {
    return this.find(this.openId);
  }

  find(id: string | null | undefined) {
    return id ? (this.list.find((g) => g.id === id) ?? null) : null;
  }

  get over() {
    return !!this.game?.result;
  }

  get turn(): Side {
    return this.liveFen.split(' ')[1] as Side;
  }

  /** The owner may move now (on the live board). */
  get ownersTurn() {
    const g = this.game;
    return !!g && !g.result && !this.thinking && this.viewAt === null && this.turn === g.owner;
  }

  get canTakeBack() {
    const g = this.game;
    return !!g && g.moves.some((m) => m.by === 'owner');
  }

  private loadSettings(): ChessSettings {
    try {
      return { ...DEFAULTS, ...JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? '{}') };
    } catch {
      return { ...DEFAULTS };
    }
  }

  saveSettings(patch: Partial<ChessSettings>) {
    this.settings = { ...this.settings, ...patch };
    try {
      localStorage.setItem(SETTINGS_KEY, JSON.stringify(this.settings));
    } catch {
      // Not remembered this time.
    }
  }

  // ---------------------------------------------------------------- the saved games

  load(): Promise<void> {
    if (this.loaded) return Promise.resolve();
    return (this.loadingList ??= this.refresh().finally(() => (this.loadingList = null)));
  }

  private async table() {
    await G.ensure();
    await G.sql('CREATE TABLE IF NOT EXISTS chess (id TEXT PRIMARY KEY, data TEXT NOT NULL, updated INTEGER NOT NULL)');
  }

  async refresh() {
    await this.table();
    const [r] = await G.sql('SELECT data FROM chess ORDER BY updated DESC');
    this.list = r.rows.flatMap((x) => {
      try {
        return [JSON.parse(x[0] as string) as ChessGame];
      } catch {
        return [];
      }
    });
    this.loaded = true;
  }

  private async save(g: ChessGame) {
    g.updated = Date.now();
    try {
      await G.sql('INSERT OR REPLACE INTO chess (id, data, updated) VALUES (?1, ?2, ?3)', [g.id, JSON.stringify(g), g.updated]);
    } catch (e) {
      store.error(e, 'Couldn’t save the game');
    }
  }

  async checkEngine() {
    this.installed = await invoke<{ installed: boolean }>('chess_status')
      .then((s) => s.installed)
      .catch(() => false);
  }

  async newGame(side: Side | 'random' = this.settings.side, level: Level = this.settings.level) {
    await this.load();
    const owner: Side = side === 'random' ? (Math.random() < 0.5 ? 'w' : 'b') : side;
    const now = Date.now();
    const day = new Date(now).toLocaleDateString('en-GB', { day: 'numeric', month: 'short' });
    const g: ChessGame = {
      id: `c${now}`,
      title: `${owner === 'w' ? 'You vs Gilgamesh' : 'Gilgamesh vs you'}, ${day}`,
      created: now,
      updated: now,
      owner,
      level,
      moves: [],
      result: null,
      ending: null,
      takebacks: 0,
    };
    this.list = [g, ...this.list];
    await this.save(g);
    this.open(g.id);
    store.navigate({ kind: 'chess' });
  }

  open(id: string) {
    const g = this.find(id);
    if (!g) return;
    this.stop();
    this.openId = id;
    this.c = new Chess();
    for (const m of g.moves) this.c.move(toMove(m.uci));
    this.named = new Set(g.moves.map((m) => m.think?.opening).filter((x): x is string => !!x));
    this.viewAt = null;
    this.arrows = [];
    this.hint = null;
    this.sync();
    if (!g.result && this.c.turn() !== g.owner) void this.reply();
  }

  close() {
    this.stop();
    this.openId = null;
    this.viewAt = null;
    this.arrows = [];
    this.hint = null;
  }

  async remove(id: string) {
    if (this.openId === id) this.close();
    this.list = this.list.filter((g) => g.id !== id);
    await G.sql('DELETE FROM chess WHERE id = ?1', [id]).catch((e) => store.error(e, 'Couldn’t delete the game'));
  }

  async rename(id: string, title: string) {
    const g = this.find(id);
    if (!g || !title.trim()) return;
    g.title = title.trim().slice(0, 80);
    await this.save(g);
  }

  /** Stops his thinking (the move he was choosing is dropped). */
  private stop() {
    this.run++;
    if (this.thinking) void invoke('chess_stop').catch(() => {});
    this.thinking = false;
    this.hinting = false;
  }

  private sync() {
    this.liveFen = this.c.fen();
    this.fen = this.viewAt === null ? this.liveFen : this.positionAt(this.viewAt).fen();
  }

  /** The game after `n` moves. */
  private positionAt(n: number) {
    const c = new Chess();
    for (const m of this.game?.moves.slice(0, n) ?? []) c.move(toMove(m.uci));
    return c;
  }

  /** Shows the position after `n` moves (null: the game now). */
  view(n: number | null) {
    const len = this.game?.moves.length ?? 0;
    this.viewAt = n === null || n >= len ? null : Math.max(0, n);
    this.arrows = [];
    this.sync();
  }

  /** The squares a piece on `from` can go to (on the live board, on the owner's turn). */
  targets(from: Square): Square[] {
    if (!this.ownersTurn) return [];
    return this.c.moves({ square: from, verbose: true }).map((m) => m.to);
  }

  /** A pawn going to the last rank (the board asks which piece). */
  isPromotion(from: Square, to: Square) {
    return this.c.moves({ square: from, verbose: true }).some((m) => m.to === to && !!m.promotion);
  }

  /** The last move played in the position shown, for highlighting. */
  get lastMove(): { from: Square; to: Square } | null {
    const g = this.game;
    const n = this.viewAt ?? g?.moves.length ?? 0;
    const m = g?.moves[n - 1];
    return m ? { from: m.uci.slice(0, 2) as Square, to: m.uci.slice(2, 4) as Square } : null;
  }

  // ---------------------------------------------------------------- playing

  /** The owner's move; false if it isn't legal (or not their turn). */
  ownerMove(from: Square, to: Square, promotion?: string): boolean {
    const g = this.game;
    if (!g || !this.ownersTurn) return false;
    let san: string;
    try {
      san = this.c.move({ from, to, promotion }).san;
    } catch {
      return false;
    }
    g.moves.push({ uci: `${from}${to}${promotion ?? ''}`, san, by: 'owner', at: Date.now() });
    this.arrows = [];
    this.hint = null;
    this.sync();
    if (!this.finishIfOver(g)) void this.reply();
    void this.save(g);
    return true;
  }

  /** Gilgamesh chooses his move and plays it. */
  private async reply() {
    const g = this.game;
    if (!g || g.result) return;
    const run = ++this.run;
    this.thinking = true;
    const lv = LEVELS[g.level];
    const started = Date.now();
    try {
      const a = await invoke<Answer>('chess_think', { request: { moves: g.moves.map((m) => m.uci), depth: lv.depth, margin: lv.margin, explain: true } });
      // He takes a moment over it, even when the book answers at once.
      await sleep(Math.max(0, 700 - (Date.now() - started)));
      if (run !== this.run || this.game !== g) return;
      const think = this.toThink(a);
      // The owner's last move, against what he expected of them.
      const theirs = g.moves.at(-1);
      const before = [...g.moves].reverse().find((m) => m.by === 'gilgamesh' && m.think?.candidates.length)?.think;
      if (theirs?.by === 'owner' && before && think.candidates.length) {
        const was = before.candidates[0].score;
        const now = think.candidates[0].score;
        const clamp = (v: number) => Math.max(-2000, Math.min(2000, v));
        theirs.delta = (clamp(now) - clamp(was)) * (g.owner === 'w' ? 1 : -1);
        theirs.expected = before.line[0];
      }
      const san = this.c.move(toMove(a.best)).san;
      g.moves.push({ uci: a.best, san, by: 'gilgamesh', at: Date.now(), think });
      this.thinking = false;
      if (this.viewAt === null) this.sync();
      const ended = this.finishIfOver(g);
      await this.save(g);
      void this.comment(g, ended);
    } catch (e: any) {
      if (run !== this.run) return;
      this.thinking = false;
      if (e?.code === 'stopped') return;
      if (e?.code === 'missing') this.installed = false;
      // The engine counts repetitions a little differently from chess.js: it's a draw all the same.
      if (/game is over/i.test(e?.message ?? '')) {
        this.end(g, '1/2-1/2', 'A draw by repetition');
        await this.save(g);
        void this.comment(g, true);
        return;
      }
      store.error(e, 'Gilgamesh couldn’t choose a move');
    }
  }

  private toThink(a: Answer): Think {
    return {
      book: a.book,
      opening: a.opening,
      score: a.score,
      depth: a.depth,
      ms: a.ms,
      line: (() => {
        this.c.move(toMove(a.best));
        try {
          return sanLine(this.c, a.line);
        } finally {
          this.c.undo();
        }
      })(),
      candidates: a.candidates.map((x) => ({ san: sanOf(this.c, x.uci), score: x.score })),
    };
  }

  /** Ends the game if the position on the board ends it. True if it did. */
  private finishIfOver(g: ChessGame): boolean {
    const c = this.c;
    const owner = (s: Side) => (s === g.owner ? 'you' : 'Gilgamesh');
    if (c.isCheckmate()) {
      const winner = other(c.turn());
      return this.end(g, winner === 'w' ? '1-0' : '0-1', winner === g.owner ? 'Checkmate: you won' : 'Checkmate: Gilgamesh won');
    }
    if (c.isStalemate()) return this.end(g, '1/2-1/2', `Stalemate (${owner(c.turn())} had no move): a draw`);
    if (c.isInsufficientMaterial()) return this.end(g, '1/2-1/2', 'A draw: neither side can mate');
    if (c.isThreefoldRepetition()) return this.end(g, '1/2-1/2', 'A draw by repetition');
    if (c.isDrawByFiftyMoves()) return this.end(g, '1/2-1/2', 'A draw by the fifty-move rule');
    return false;
  }

  private end(g: ChessGame, result: ChessGame['result'], ending: string) {
    g.result = result;
    g.ending = ending;
    return true;
  }

  /** Takes back the owner's last move and his answer to it. */
  takeBack() {
    const g = this.game;
    if (!g || !this.canTakeBack) return;
    this.stop();
    while (g.moves.length && g.moves.at(-1)!.by === 'gilgamesh') g.moves.pop();
    if (g.moves.at(-1)?.by === 'owner') g.moves.pop();
    g.result = null;
    g.ending = null;
    g.takebacks++;
    this.c = this.positionAt(g.moves.length);
    this.viewAt = null;
    this.arrows = [];
    this.hint = null;
    this.sync();
    void this.save(g);
    if (this.c.turn() !== g.owner) void this.reply();
  }

  resign() {
    const g = this.game;
    if (!g || g.result) return;
    this.stop();
    this.end(g, g.owner === 'w' ? '0-1' : '1-0', 'You resigned: Gilgamesh won');
    void this.save(g);
    void this.comment(g, true);
  }

  /** The owner offers a draw: he takes it unless he thinks he's better. */
  offerDraw(): boolean {
    const g = this.game;
    if (!g || g.result) return false;
    const last = [...g.moves].reverse().find((m) => m.by === 'gilgamesh' && m.think?.candidates.length)?.think;
    const mine = last ? last.candidates[0].score * (g.owner === 'w' ? -1 : 1) : 0;
    const yes = g.moves.length >= 20 && mine <= 30;
    if (yes) {
      this.stop();
      this.end(g, '1/2-1/2', 'A draw, agreed');
      void this.save(g);
    }
    void this.comment(g, yes, yes ? 'The owner offered a draw and you accepted.' : `The owner offered a draw and you declined${g.moves.length < 20 ? ' (it’s early days)' : ' (you think you’re better)'}.`);
    return yes;
  }

  /** What his engine would play for the owner, shown as an arrow. */
  async requestHint() {
    const g = this.game;
    if (!g || !this.ownersTurn) return;
    if (this.hint) {
      this.showHint();
      return;
    }
    this.hinting = true;
    const lv = LEVELS.medium;
    try {
      const a = await invoke<Answer>('chess_think', { request: { moves: g.moves.map((m) => m.uci), depth: Math.max(lv.depth, LEVELS[g.level].depth), explain: true } });
      if (this.game !== g || !this.ownersTurn) return;
      this.hint = { san: sanOf(this.c, a.best), think: this.toThink(a) };
      this.showHint();
    } catch (e: any) {
      if (e?.code !== 'stopped') store.error(e, 'Couldn’t find a hint');
    } finally {
      this.hinting = false;
    }
  }

  private showHint() {
    const m = this.hint && this.c.moves({ verbose: true }).find((x) => x.san === this.hint!.san);
    if (m) this.arrows = [{ from: m.from, to: m.to, kind: 'hint' }];
  }

  /** Points at a move ("Nf3", "e2 e4") on the board shown. */
  show(what: string): boolean {
    const shown = this.viewAt === null ? this.c : this.positionAt(this.viewAt);
    const sq = what.toLowerCase().match(/[a-h][1-8]/g);
    let arrow: Arrow | null = null;
    try {
      const m = shown.moves({ verbose: true }).find((x) => x.san.replace(/[+#]/g, '') === what.trim().replace(/[+#!?]/g, ''));
      if (m) arrow = { from: m.from, to: m.to, kind: 'show' };
    } catch {
      // Not a move in this position.
    }
    if (!arrow && sq && sq.length >= 2) arrow = { from: sq[0] as Square, to: sq[1] as Square, kind: 'show' };
    if (!arrow) return false;
    this.arrows = [...this.arrows.filter((a) => a.kind !== 'show'), arrow];
    return true;
  }

  // ---------------------------------------------------------------- talking about it

  /** Says a word about what just happened, when it matters (or always, if the owner likes chat). */
  private async comment(g: ChessGame, ended: boolean, happened?: string) {
    const talk = this.settings.talk;
    if (talk === 'quiet' || store.route.kind !== 'chess' || this.game !== g) return;
    if (['starting', 'hearing', 'thinking', 'speaking'].includes(gilgamesh.phase)) return;
    const mine = g.moves.at(-1);
    const theirs = mine?.by === 'gilgamesh' ? g.moves.at(-2) : mine;
    const ply = (m: PlayedMove | undefined) => (m ? g.moves.indexOf(m) : -1);
    const notes: string[] = [];
    if (happened) notes.push(happened);
    let notable = !!happened || ended;
    if (theirs?.by === 'owner') {
      const d = theirs.delta;
      let how = '';
      if (d !== undefined && d <= -250) how = ' (a blunder, by your search)';
      else if (d !== undefined && d <= -120) how = ' (a mistake, by your search)';
      else if (d !== undefined && d >= 120) how = ' (better than the move you expected)';
      if (how || /x/.test(theirs.san)) notable = true;
      notes.push(`The owner played ${moveLabel(ply(theirs), theirs.san)}${how}${theirs.expected && theirs.expected !== theirs.san && how ? `; you’d expected ${theirs.expected}` : ''}.`);
    }
    if (mine?.by === 'gilgamesh') {
      const t = mine.think;
      const fresh = t?.opening && !this.named.has(t.opening) ? t.opening : null;
      if (fresh) {
        this.named.add(fresh);
        notable = true;
      }
      if (/[x+#]/.test(mine.san)) notable = true;
      notes.push(`You answered ${moveLabel(ply(mine), mine.san)}${fresh ? ` (the opening is the ${fresh})` : ''}.`);
    }
    if (ended && g.ending) notes.push(`The game is over: ${g.ending}.`);
    if (!notable && talk === 'sometimes') return;
    if (!ended && Date.now() - this.lastComment < 15_000 && !happened) return;
    this.lastComment = Date.now();
    await gilgamesh.discussChess(g.id);
    void gilgamesh.ask(`(The chess game, not the owner speaking: ${notes.join(' ')} Say one short line to the owner about it, as their opponent across the board.${ended ? ' The game is over: be a good sport.' : ''})`, { quiet: true });
  }

  /** The game as PGN. */
  pgn(g: ChessGame) {
    const c = new Chess();
    for (const m of g.moves) c.move(toMove(m.uci));
    const d = new Date(g.created);
    c.setHeader('Event', g.title);
    c.setHeader('Site', 'RAD');
    c.setHeader('Date', `${d.getFullYear()}.${String(d.getMonth() + 1).padStart(2, '0')}.${String(d.getDate()).padStart(2, '0')}`);
    c.setHeader('White', g.owner === 'w' ? 'You' : 'Gilgamesh');
    c.setHeader('Black', g.owner === 'b' ? 'You' : 'Gilgamesh');
    c.setHeader('Result', g.result ?? '*');
    return c.pgn();
  }

  /** The material on the board: each side's points, and what each has lost. */
  material(fen = this.fen) {
    const start: Record<string, number> = { p: 8, n: 2, b: 2, r: 2, q: 1 };
    const have = { w: { p: 0, n: 0, b: 0, r: 0, q: 0 } as Record<string, number>, b: { p: 0, n: 0, b: 0, r: 0, q: 0 } as Record<string, number> };
    for (const ch of fen.split(' ')[0]) {
      const k = ch.toLowerCase();
      if (k in start) have[ch === k ? 'b' : 'w'][k]++;
    }
    const points = (s: Side) => Object.entries(have[s]).reduce((n, [k, v]) => n + VALUE[k] * v, 0);
    // Lost pieces (a promoted pawn makes a count go over the start: nothing lost there).
    const lost = (s: Side) => Object.keys(start).flatMap((k) => Array(Math.max(0, start[k] - have[s][k])).fill(k) as string[]);
    return { w: points('w'), b: points('b'), lost: { w: lost('w'), b: lost('b') } };
  }

  /** The game on the board, and what his engine saw, for his answers. */
  describe(): string {
    const g = this.game;
    if (g && store.route.kind === 'chess') {
      const you = other(g.owner);
      const mat = this.material(this.c.fen());
      const diff = mat[g.owner] - mat[you];
      const lostNames = (s: Side) => mat.lost[s].map((k) => NAME[k]).join(', ') || 'nothing';
      const moves = new Chess();
      for (const m of g.moves) moves.move(toMove(m.uci));
      const opening = [...g.moves].reverse().find((m) => m.think?.opening)?.think?.opening;
      const lv = LEVELS[g.level];
      const state = g.result
        ? `The game is over: ${g.ending} (${g.result}).`
        : this.thinking
          ? 'You are choosing your move now.'
          : this.c.turn() === g.owner
            ? `It's the owner's move (${SIDE[g.owner]}).${this.c.inCheck() ? ' They are in check.' : ''}`
            : 'It is your move.';
      const mineLines = g.moves
        .map((m, i) => ({ m, i }))
        .filter(({ m, i }) => m.by === 'gilgamesh' && m.think && (i >= g.moves.length - 16 || (this.viewAt !== null && Math.abs(i - (this.viewAt - 1)) <= 1)))
        .map(({ m, i }) => {
          const t = m.think!;
          const parts = [`- ${moveLabel(i, m.san)}:`];
          if (t.book) parts.push(`from your opening book${t.opening ? ` (${t.opening})` : ''};`);
          if (t.score !== null) parts.push(`${t.book ? 'scored' : `your search ${t.depth ?? ''} half-moves deep scored it`} ${pawns(t.score, you)};`);
          if (t.line.length) parts.push(`you expected ${t.line.map((s, k) => moveLabel(i + 1 + k, s)).join(' ')};`);
          const others = t.candidates.filter((c) => c.san !== m.san).slice(0, 4);
          if (others.length) parts.push(`the other moves you weighed: ${others.map((c) => `${c.san} (${pawns(c.score, you)})`).join(', ')}.`);
          return parts.join(' ');
        });
      const ownerLines = g.moves
        .map((m, i) => ({ m, i }))
        .filter(({ m }) => m.by === 'owner' && m.delta !== undefined && (m.delta <= -120 || m.delta >= 120))
        .slice(-8)
        .map(({ m, i }) => `- ${moveLabel(i, m.san)}: ${m.delta! < 0 ? `cost them about ${(-m.delta! / 100).toFixed(1)} pawns` : `about ${(m.delta! / 100).toFixed(1)} pawns better than you expected`}${m.expected && m.expected !== m.san ? ` (you'd expected ${m.expected})` : ''}.`);
      const looking = this.viewAt !== null ? `THE OWNER IS LOOKING BACK at the position after ${this.viewAt ? moveLabel(this.viewAt - 1, g.moves[this.viewAt - 1].san) : 'no moves'}, not the game now; "this move" means ${this.viewAt ? moveLabel(this.viewAt - 1, g.moves[this.viewAt - 1].san) : 'the start'}.` : '';
      return [
        `YOU ARE PLAYING CHESS WITH THE OWNER in RAD (Games → Chess, “${g.title}”). They play ${SIDE[g.owner]}, you play ${SIDE[you]}. Your level: ${lv.label} (your engine looks ${lv.depth} half-moves ahead${lv.margin ? ' and plays any move close to the best' : ''}).${g.takebacks ? ` They have taken back ${g.takebacks} move${g.takebacks > 1 ? 's' : ''}.` : ''}`,
        `THE GAME SO FAR: ${moves.pgn({ maxWidth: 0 }).replace(/^\[.*\]\s*/gm, '').trim() || '(no moves yet)'}`,
        state,
        `Material: ${diff === 0 ? 'even' : diff > 0 ? `the owner is ${diff} point${diff > 1 ? 's' : ''} ahead` : `you are ${-diff} point${diff < -1 ? 's' : ''} ahead`}. The owner has lost: ${lostNames(g.owner)}. You have lost: ${lostNames(you)}.${opening ? ` Opening: ${opening}.` : ''}`,
        `Position (FEN): ${this.c.fen()}`,
        looking,
        mineLines.length ? `YOUR MOVES, AS YOUR ENGINE SAW THEM (scores in pawns from your side, + is good for you; differences under about half a pawn mean little):\n${mineLines.join('\n')}` : '',
        ownerLines.length ? `THE OWNER'S MOVES THAT STOOD OUT, by your search:\n${ownerLines.join('\n')}` : '',
        this.hint ? `THE HINT THE OWNER ASKED FOR (an arrow on the board shows it): your engine would play ${this.hint.san} for them${this.hint.think.line.length ? `, expecting ${this.hint.think.line.slice(0, 3).join(' ')}` : ''}.` : '',
      ]
        .filter(Boolean)
        .join('\n');
    }
    if (!this.list.length) return '';
    const recent = this.list.slice(0, 5).map((x) => `- “${x.title}”: ${x.result ? x.ending : `unfinished, ${x.moves.length} moves`}`);
    return `YOUR CHESS GAMES WITH THE OWNER (Games → Chess; start a new one with [chess: new | white]):\n${recent.join('\n')}`;
  }
}

export const chess = new ChessState();
