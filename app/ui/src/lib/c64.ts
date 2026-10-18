/** Commodore 64: the keyboard mapping, the game library and the user's ROMs (all in the
 *  encrypted Games database). */
import { api, rad } from './api';
import { ensure } from './games';
import type { DbResult } from './types';

// ------------------------------------------------------------------ keyboard

/** The C64's keys: their places in the 8×8 matrix (row × 8 + column), as rad_c64::keys. */
export const K = {
  DEL: 0, RETURN: 1, CRSR_RIGHT: 2, F7: 3, F1: 4, F3: 5, F5: 6, CRSR_DOWN: 7,
  N3: 8, W: 9, A: 10, N4: 11, Z: 12, S: 13, E: 14, LSHIFT: 15,
  N5: 16, R: 17, D: 18, N6: 19, C: 20, F: 21, T: 22, X: 23,
  N7: 24, Y: 25, G: 26, N8: 27, B: 28, H: 29, U: 30, V: 31,
  N9: 32, I: 33, J: 34, N0: 35, M: 36, K: 37, O: 38, N: 39,
  PLUS: 40, P: 41, L: 42, MINUS: 43, PERIOD: 44, COLON: 45, AT: 46, COMMA: 47,
  POUND: 48, ASTERISK: 49, SEMICOLON: 50, HOME: 51, RSHIFT: 52, EQUALS: 53, UP_ARROW: 54, SLASH: 55,
  N1: 56, LEFT_ARROW: 57, CTRL: 58, N2: 59, SPACE: 60, CBM: 61, Q: 62, RUN_STOP: 63,
  RESTORE: 64,
} as const;

/** What the arrow keys (and Tab, to fire) do: the joystick, or the C64's cursor keys. */
export type ArrowMode = 'joystick' | 'keys';

export const JOY = { UP: 1, DOWN: 2, LEFT: 4, RIGHT: 8, FIRE: 16 };

/** A held PC key's C64 keys. `exact`: they're the whole story for shift (a typed symbol like
 *  `"` is SHIFT+2 on the C64 whatever the PC's shift key says). */
type Press = { keys: number[]; exact: boolean };

const SH = K.LSHIFT;
/** Typed characters → C64 keys (symbolic mapping, so `"`, `(`, `=`, `:`… land where they
 *  should). */
const SYMBOLS: Record<string, number[]> = {
  '!': [SH, K.N1], '"': [SH, K.N2], '#': [SH, K.N3], $: [SH, K.N4], '%': [SH, K.N5], '&': [SH, K.N6], "'": [SH, K.N7], '(': [SH, K.N8], ')': [SH, K.N9],
  '+': [K.PLUS], '-': [K.MINUS], '*': [K.ASTERISK], '/': [K.SLASH], '=': [K.EQUALS], ':': [K.COLON], ';': [K.SEMICOLON], ',': [K.COMMA], '.': [K.PERIOD],
  '@': [K.AT], '^': [K.UP_ARROW], '£': [K.POUND], '\\': [K.POUND], '<': [SH, K.COMMA], '>': [SH, K.PERIOD], '?': [SH, K.SLASH], '[': [SH, K.COLON],
  ']': [SH, K.SEMICOLON], '`': [K.LEFT_ARROW], '_': [K.LEFT_ARROW],
};

function press(e: KeyboardEvent, arrows: ArrowMode): Press | null {
  const code = e.code;
  if (/^Key[A-Z]$/.test(code)) return { keys: [K[code[3] as keyof typeof K]], exact: false };
  if (/^Digit[0-9]$/.test(code) && /^[0-9]$/.test(e.key)) return { keys: [K[('N' + code[5]) as keyof typeof K]], exact: false };
  if (e.key.length === 1 && SYMBOLS[e.key]) return { keys: SYMBOLS[e.key], exact: true };
  if (arrows === 'keys') {
    const a: Record<string, number[]> = { ArrowRight: [K.CRSR_RIGHT], ArrowLeft: [SH, K.CRSR_RIGHT], ArrowDown: [K.CRSR_DOWN], ArrowUp: [SH, K.CRSR_DOWN] };
    if (a[code]) return { keys: a[code], exact: true };
  }
  const f: Record<string, number[]> = { F1: [K.F1], F2: [SH, K.F1], F3: [K.F3], F4: [SH, K.F3], F5: [K.F5], F6: [SH, K.F5], F7: [K.F7], F8: [SH, K.F7] };
  if (f[code]) return { keys: f[code], exact: true };
  switch (code) {
    case 'Enter':
    case 'NumpadEnter':
      return { keys: [K.RETURN], exact: false };
    case 'Space':
      return { keys: [K.SPACE], exact: false };
    case 'Backspace':
      return { keys: [K.DEL], exact: false };
    case 'ShiftLeft':
      return { keys: [K.LSHIFT], exact: false };
    case 'ShiftRight':
      return { keys: [K.RSHIFT], exact: false };
    case 'ControlLeft':
    case 'ControlRight':
      return { keys: [K.CTRL], exact: false };
    case 'AltLeft':
    case 'AltRight':
      return { keys: [K.CBM], exact: false };
    case 'Escape':
      return { keys: [K.RUN_STOP], exact: false };
    case 'Home':
      return { keys: [K.HOME], exact: false };
    case 'PageUp':
      return { keys: [K.RESTORE], exact: false };
    case 'Insert':
      return { keys: [SH, K.DEL], exact: true };
  }
  return null;
}

function joyFor(code: string, arrows: ArrowMode): number {
  if (arrows !== 'joystick') return 0;
  return ({ ArrowUp: JOY.UP, ArrowDown: JOY.DOWN, ArrowLeft: JOY.LEFT, ArrowRight: JOY.RIGHT, Tab: JOY.FIRE } as Record<string, number>)[code] ?? 0;
}

/** The PC keys held down (and what each meant when pressed) → C64 keys and joystick bits. */
export class Keyboard {
  held = new Map<string, Press | null>();

  /** A key went down: true when the C64 uses it. */
  down(e: KeyboardEvent, arrows: ArrowMode): boolean {
    const p = press(e, arrows);
    if (!p && !joyFor(e.code, arrows)) return false;
    this.held.set(e.code, p);
    return true;
  }

  up(code: string): boolean {
    return this.held.delete(code);
  }

  clear() {
    this.held.clear();
  }

  state(arrows: ArrowMode): { keys: number[]; joy: number } {
    const keys = new Set<number>();
    let exact: number[] | null = null;
    let joy = 0;
    for (const [code, p] of this.held) {
      joy |= joyFor(code, arrows);
      if (!p) continue;
      if (p.exact) exact = p.keys;
      else for (const k of p.keys) keys.add(k);
    }
    // The last symbol typed decides shift: drop the PC's shift keys and use its own.
    if (exact) {
      keys.delete(K.LSHIFT);
      keys.delete(K.RSHIFT);
      for (const k of exact) keys.add(k);
    }
    return { keys: [...keys].sort((a, b) => a - b), joy };
  }
}

// ------------------------------------------------------------------ library

export type C64Kind = 'program' | 'tape' | 'disk' | 'cartridge';
export interface C64Game { id: string; name: string; file: string; size: number; added_at: number; played_at: number | null; states: number }
export interface C64State { id: string; game_id: string; name: string; saved_at: number }

export const C64_EXTS = ['prg', 'p00', 't64', 'tap', 'd64', 'crt'];
export function kindOf(file: string): C64Kind {
  const e = file.split('.').pop()?.toLowerCase() ?? '';
  return e === 'tap' ? 'tape' : e === 'd64' ? 'disk' : e === 'crt' ? 'cartridge' : 'program';
}

const sql = (q: string, params: unknown[] = []) => api.post<DbResult[]>('/v1/db/Games/sql', { sql: q, params });
let ready: Promise<void> | null = null;

function ensureC64() {
  ready ??= (async () => {
    await ensure();
    await sql(`CREATE TABLE IF NOT EXISTS c64_games (id TEXT PRIMARY KEY, name TEXT NOT NULL, file TEXT NOT NULL, data BLOB NOT NULL, size INTEGER NOT NULL, added_at INTEGER NOT NULL, played_at INTEGER);
CREATE TABLE IF NOT EXISTS c64_states (id TEXT PRIMARY KEY, game_id TEXT NOT NULL, name TEXT NOT NULL, data BLOB NOT NULL, saved_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS c64_roms (id TEXT PRIMARY KEY, chars BLOB NOT NULL, basic BLOB NOT NULL, kernal BLOB NOT NULL, added_at INTEGER NOT NULL)`);
  })().catch((e) => {
    ready = null;
    throw e;
  });
  return ready;
}

export async function games(): Promise<C64Game[]> {
  await ensureC64();
  const [r] = await sql(`SELECT g.id, g.name, g.file, g.size, g.added_at, g.played_at, (SELECT count(*) FROM c64_states s WHERE s.game_id = g.id)
    FROM c64_games g ORDER BY COALESCE(g.played_at, g.added_at) DESC`);
  return r.rows.map((x) => ({ id: x[0] as string, name: x[1] as string, file: x[2] as string, size: x[3] as number, added_at: x[4] as number, played_at: x[5] as number | null, states: x[6] as number }));
}

export async function addGame(file: string, b64: string, size: number): Promise<string> {
  await ensureC64();
  const id = `c64${Date.now()}${Math.floor(Math.random() * 1000)}`;
  const name = file.replace(/\.(prg|p00|t64|tap|d64|crt)$/i, '').replace(/[_]+/g, ' ').trim() || file;
  await sql('INSERT INTO c64_games (id, name, file, data, size, added_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)', [id, name, file, { $blob: b64 }, size, Date.now()]);
  return id;
}

export async function gameData(id: string, touch = true): Promise<{ file: string; data: string }> {
  const [r] = await sql('SELECT file, data FROM c64_games WHERE id = ?1', [id]);
  const x = r.rows[0];
  if (!x) throw new Error('That game is gone');
  if (touch) await sql('UPDATE c64_games SET played_at = ?1 WHERE id = ?2', [Date.now(), id]);
  return { file: x[0] as string, data: (x[1] as { $blob: string }).$blob };
}

export async function renameGame(id: string, name: string) {
  await sql('UPDATE c64_games SET name = ?1 WHERE id = ?2', [name, id]);
}

export async function removeGame(id: string) {
  await sql('DELETE FROM c64_states WHERE game_id = ?1', [id]);
  await sql('DELETE FROM c64_games WHERE id = ?1', [id]);
}

export async function states(gameId: string): Promise<C64State[]> {
  await ensureC64();
  const [r] = await sql('SELECT id, game_id, name, saved_at FROM c64_states WHERE game_id = ?1 ORDER BY saved_at DESC', [gameId]);
  return r.rows.map((x) => ({ id: x[0] as string, game_id: x[1] as string, name: x[2] as string, saved_at: x[3] as number }));
}

export async function saveState(gameId: string, b64: string, name: string) {
  await ensureC64();
  await sql('INSERT INTO c64_states (id, game_id, name, data, saved_at) VALUES (?1, ?2, ?3, ?4, ?5)', [`st${Date.now()}`, gameId, name, { $blob: b64 }, Date.now()]);
}

export async function stateData(id: string): Promise<string> {
  const [r] = await sql('SELECT data FROM c64_states WHERE id = ?1', [id]);
  const v = r.rows[0]?.[0];
  if (!v || typeof v !== 'object') throw new Error('That save is gone');
  return v.$blob;
}

export async function removeState(id: string) {
  await sql('DELETE FROM c64_states WHERE id = ?1', [id]);
}

// ------------------------------------------------------------------ ROMs

/** Hands RAD the user's C64 ROMs (if they added them) — once per session, or after a change.
 *  Returns which set is in use. */
let romsLoaded: Promise<'open' | 'commodore'> | null = null;
export function loadRoms(force = false): Promise<'open' | 'commodore'> {
  if (force) romsLoaded = null;
  romsLoaded ??= (async () => {
    await ensureC64();
    const [r] = await sql("SELECT chars, basic, kernal FROM c64_roms WHERE id = 'roms'");
    const x = r.rows[0];
    const b = (v: unknown) => (v as { $blob: string }).$blob;
    return x ? await rad.c64SetRoms(b(x[0]), b(x[1]), b(x[2])) : await rad.c64SetRoms(null, null, null);
  })().catch((e) => {
    romsLoaded = null;
    throw e;
  });
  return romsLoaded;
}

/** Keeps the user's ROMs (checked by RAD first) and starts using them. */
export async function saveRoms(chars: string, basic: string, kernal: string) {
  await ensureC64();
  await rad.c64SetRoms(chars, basic, kernal);
  await sql('INSERT OR REPLACE INTO c64_roms (id, chars, basic, kernal, added_at) VALUES (?1, ?2, ?3, ?4, ?5)', ['roms', { $blob: chars }, { $blob: basic }, { $blob: kernal }, Date.now()]);
  romsLoaded = Promise.resolve('commodore');
}

/** Forgets the user's ROMs (back to the free Open ROMs). */
export async function removeRoms() {
  await ensureC64();
  await sql("DELETE FROM c64_roms WHERE id = 'roms'");
  await rad.c64SetRoms(null, null, null);
  romsLoaded = Promise.resolve('open');
}
