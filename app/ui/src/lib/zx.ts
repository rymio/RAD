/** ZX Spectrum: the keyboard mapping and the game library (in the encrypted Games database). */
import { api } from './api';
import { ensure } from './games';
import type { DbResult } from './types';

// ------------------------------------------------------------------ keyboard

/** The Spectrum's 40 keys as bits (rad_zx::Keys, SPECTRUSTY's ZXKeyboardMap). */
export const ZX: Record<string, number> = {
  V: 0x1, G: 0x2, T: 0x4, N5: 0x8, N6: 0x10, Y: 0x20, H: 0x40, B: 0x80, C: 0x100, F: 0x200, R: 0x400, N4: 0x800,
  N7: 0x1000, U: 0x2000, J: 0x4000, N: 0x8000, X: 0x10000, D: 0x20000, E: 0x40000, N3: 0x80000, N8: 0x100000,
  I: 0x200000, K: 0x400000, M: 0x800000, Z: 0x1000000, S: 0x2000000, W: 0x4000000, N2: 0x8000000, N9: 0x10000000,
  O: 0x20000000, L: 0x40000000, SS: 0x80000000, CS: 0x100000000, A: 0x200000000, Q: 0x400000000, N1: 0x800000000,
  N0: 0x1000000000, P: 0x2000000000, EN: 0x4000000000, BR: 0x8000000000,
};

/** What the arrow keys do: a Kempston joystick, a Cursor (Protek) joystick, QAOP, or Spectrum cursor keys. */
export type ArrowMode = 'kempston' | 'cursor' | 'qaop' | 'keys';

export const JOY = { UP: 1, DOWN: 2, LEFT: 4, RIGHT: 8, FIRE: 16 };

/** Spectrum keys for a PC key (KeyboardEvent.code). */
function keysFor(code: string, arrows: ArrowMode): string[] | null {
  if (/^Key[A-Z]$/.test(code)) return [code[3]];
  if (/^Digit[0-9]$/.test(code)) return ['N' + code[5]];
  const arrowsTo: Record<ArrowMode, Record<string, string[]>> = {
    kempston: {},
    cursor: { ArrowLeft: ['N5'], ArrowDown: ['N6'], ArrowUp: ['N7'], ArrowRight: ['N8'], Tab: ['N0'] },
    qaop: { ArrowUp: ['Q'], ArrowDown: ['A'], ArrowLeft: ['O'], ArrowRight: ['P'], Tab: ['M'] },
    keys: { ArrowLeft: ['CS', 'N5'], ArrowDown: ['CS', 'N6'], ArrowUp: ['CS', 'N7'], ArrowRight: ['CS', 'N8'] },
  };
  const a = arrowsTo[arrows][code];
  if (a) return a;
  switch (code) {
    case 'Enter':
    case 'NumpadEnter':
      return ['EN'];
    case 'Space':
      return ['BR'];
    case 'ShiftLeft':
    case 'ShiftRight':
      return ['CS'];
    case 'ControlLeft':
    case 'ControlRight':
    case 'AltLeft':
    case 'AltRight':
      return ['SS'];
    case 'Backspace':
      return ['CS', 'N0'];
    case 'Escape':
      return ['CS', 'BR']; // BREAK
    case 'Comma':
      return ['SS', 'N'];
    case 'Period':
      return ['SS', 'M'];
    case 'Slash':
      return ['SS', 'V'];
    case 'Semicolon':
      return ['SS', 'O'];
    case 'Quote':
      return ['SS', 'P'];
    case 'Minus':
      return ['SS', 'J'];
    case 'Equal':
      return ['SS', 'L'];
  }
  return null;
}

/** Joystick bits for a PC key, in Kempston mode (arrows + Tab to fire). */
function joyFor(code: string, arrows: ArrowMode): number {
  if (arrows !== 'kempston') return 0;
  return ({ ArrowUp: JOY.UP, ArrowDown: JOY.DOWN, ArrowLeft: JOY.LEFT, ArrowRight: JOY.RIGHT, Tab: JOY.FIRE } as Record<string, number>)[code] ?? 0;
}

/** The PC keys held down → the Spectrum keyboard and joystick. Returns null for keys it doesn't use. */
export function mapKeys(held: Set<string>, arrows: ArrowMode): { keys: number; joy: number } {
  const zx = new Set<string>();
  let joy = 0;
  for (const code of held) {
    for (const k of keysFor(code, arrows) ?? []) zx.add(k);
    joy |= joyFor(code, arrows);
  }
  // Distinct powers of two below 2^40 add up exactly (JS bit operators stop at 32 bits).
  let keys = 0;
  for (const k of zx) keys += ZX[k];
  return { keys, joy };
}

export function isMapped(code: string, arrows: ArrowMode) {
  return keysFor(code, arrows) !== null || joyFor(code, arrows) !== 0;
}

// ------------------------------------------------------------------ library

export interface ZxGame { id: string; name: string; file: string; size: number; model: '48' | '128'; added_at: number; played_at: number | null; states: number }
export interface ZxState { id: string; game_id: string; name: string; saved_at: number }

const sql = (q: string, params: unknown[] = []) => api.post<DbResult[]>('/v1/db/Games/sql', { sql: q, params });
let ready: Promise<void> | null = null;

function ensureZx() {
  ready ??= (async () => {
    await ensure();
    await sql(`CREATE TABLE IF NOT EXISTS zx_games (id TEXT PRIMARY KEY, name TEXT NOT NULL, file TEXT NOT NULL, data BLOB NOT NULL, size INTEGER NOT NULL, model TEXT NOT NULL DEFAULT '48', added_at INTEGER NOT NULL, played_at INTEGER);
CREATE TABLE IF NOT EXISTS zx_states (id TEXT PRIMARY KEY, game_id TEXT NOT NULL, name TEXT NOT NULL, data BLOB NOT NULL, saved_at INTEGER NOT NULL)`);
  })().catch((e) => {
    ready = null;
    throw e;
  });
  return ready;
}

export async function games(): Promise<ZxGame[]> {
  await ensureZx();
  const [r] = await sql(`SELECT g.id, g.name, g.file, g.size, g.model, g.added_at, g.played_at, (SELECT count(*) FROM zx_states s WHERE s.game_id = g.id)
    FROM zx_games g ORDER BY COALESCE(g.played_at, g.added_at) DESC`);
  return r.rows.map((x) => ({ id: x[0] as string, name: x[1] as string, file: x[2] as string, size: x[3] as number, model: (x[4] as string) === '128' ? '128' : '48', added_at: x[5] as number, played_at: x[6] as number | null, states: x[7] as number }));
}

export async function addGame(file: string, b64: string, size: number, model: '48' | '128'): Promise<string> {
  await ensureZx();
  const id = `zx${Date.now()}${Math.floor(Math.random() * 1000)}`;
  const name = file.replace(/\.(tzx|tap|z80|sna)$/i, '').replace(/[_]+/g, ' ').trim() || file;
  await sql('INSERT INTO zx_games (id, name, file, data, size, model, added_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)', [id, name, file, { $blob: b64 }, size, model, Date.now()]);
  return id;
}

export async function gameData(id: string): Promise<{ file: string; data: string; model: '48' | '128' }> {
  const [r] = await sql('SELECT file, data, model FROM zx_games WHERE id = ?1', [id]);
  const x = r.rows[0];
  if (!x) throw new Error('That game is gone');
  await sql('UPDATE zx_games SET played_at = ?1 WHERE id = ?2', [Date.now(), id]);
  return { file: x[0] as string, data: (x[1] as { $blob: string }).$blob, model: (x[2] as string) === '128' ? '128' : '48' };
}

export async function setGame(id: string, fields: { name?: string; model?: '48' | '128' }) {
  if (fields.name !== undefined) await sql('UPDATE zx_games SET name = ?1 WHERE id = ?2', [fields.name, id]);
  if (fields.model !== undefined) await sql('UPDATE zx_games SET model = ?1 WHERE id = ?2', [fields.model, id]);
}

export async function removeGame(id: string) {
  await sql('DELETE FROM zx_states WHERE game_id = ?1', [id]);
  await sql('DELETE FROM zx_games WHERE id = ?1', [id]);
}

export async function states(gameId: string): Promise<ZxState[]> {
  await ensureZx();
  const [r] = await sql('SELECT id, game_id, name, saved_at FROM zx_states WHERE game_id = ?1 ORDER BY saved_at DESC', [gameId]);
  return r.rows.map((x) => ({ id: x[0] as string, game_id: x[1] as string, name: x[2] as string, saved_at: x[3] as number }));
}

export async function saveState(gameId: string, b64: string, name: string) {
  await ensureZx();
  await sql('INSERT INTO zx_states (id, game_id, name, data, saved_at) VALUES (?1, ?2, ?3, ?4, ?5)', [`st${Date.now()}`, gameId, name, { $blob: b64 }, Date.now()]);
}

export async function stateData(id: string): Promise<string> {
  const [r] = await sql('SELECT data FROM zx_states WHERE id = ?1', [id]);
  const v = r.rows[0]?.[0];
  if (!v || typeof v !== 'object') throw new Error('That save is gone');
  return v.$blob;
}

export async function removeState(id: string) {
  await sql('DELETE FROM zx_states WHERE id = ?1', [id]);
}
