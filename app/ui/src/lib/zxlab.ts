/** Spectrum lab projects: a game file, its notes (labels, comments), patches and the AI
 *  conversation, kept in the encrypted Games database. */
import { api } from './api';
import { ensure } from './games';
import type { DbResult, LabNotes } from './types';

export interface LabPatch { id: string; addr: number; bytes: string; note: string; source: string; enabled: boolean }
export interface LabProject { id: string; name: string; file: string; size: number; created_at: number; updated_at: number }

const sql = (q: string, params: unknown[] = []) => api.post<DbResult[]>('/v1/db/Games/sql', { sql: q, params });
let ready: Promise<void> | null = null;

function ensureLab() {
  ready ??= (async () => {
    await ensure();
    await sql(`CREATE TABLE IF NOT EXISTS zx_lab (id TEXT PRIMARY KEY, name TEXT NOT NULL, file TEXT NOT NULL, data BLOB NOT NULL, size INTEGER NOT NULL,
      notes TEXT NOT NULL DEFAULT '{}', patches TEXT NOT NULL DEFAULT '[]', chat TEXT NOT NULL DEFAULT '[]', rag_note TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL)`);
  })().catch((e) => {
    ready = null;
    throw e;
  });
  return ready;
}

export async function projects(): Promise<LabProject[]> {
  await ensureLab();
  const [r] = await sql('SELECT id, name, file, size, created_at, updated_at FROM zx_lab ORDER BY updated_at DESC');
  return r.rows.map((x) => ({ id: x[0] as string, name: x[1] as string, file: x[2] as string, size: x[3] as number, created_at: x[4] as number, updated_at: x[5] as number }));
}

export async function create(file: string, b64: string, size: number): Promise<string> {
  await ensureLab();
  const id = `lab${Date.now()}`;
  const name = file.replace(/\.(tzx|tap|z80|sna)$/i, '').replace(/_+/g, ' ').trim() || file;
  const now = Date.now();
  await sql('INSERT INTO zx_lab (id, name, file, data, size, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)', [id, name, file, { $blob: b64 }, size, now]);
  return id;
}

export interface LabState { file: string; data: string; notes: LabNotes; patches: LabPatch[]; chat: unknown[]; ragNote: string | null }

export async function load(id: string): Promise<LabState> {
  const [r] = await sql('SELECT file, data, notes, patches, chat, rag_note FROM zx_lab WHERE id = ?1', [id]);
  const x = r.rows[0];
  if (!x) throw new Error('That lab project is gone');
  const parse = <T>(s: unknown, d: T): T => {
    try {
      return JSON.parse(s as string) ?? d;
    } catch {
      return d;
    }
  };
  const notes = parse<LabNotes>(x[2], { labels: {}, comments: {} });
  return { file: x[0] as string, data: (x[1] as { $blob: string }).$blob, notes: { labels: notes.labels ?? {}, comments: notes.comments ?? {} }, patches: parse(x[3], []), chat: parse(x[4], []), ragNote: (x[5] as string) ?? null };
}

export async function saveNotes(id: string, notes: LabNotes) {
  await sql('UPDATE zx_lab SET notes = ?1, updated_at = ?2 WHERE id = ?3', [JSON.stringify(notes), Date.now(), id]);
}
export async function savePatches(id: string, patches: LabPatch[]) {
  await sql('UPDATE zx_lab SET patches = ?1, updated_at = ?2 WHERE id = ?3', [JSON.stringify(patches), Date.now(), id]);
}
export async function saveChat(id: string, chat: unknown[]) {
  await sql('UPDATE zx_lab SET chat = ?1 WHERE id = ?2', [JSON.stringify(chat.slice(-80)), id]);
}
export async function setRagNote(id: string, noteId: string) {
  await sql('UPDATE zx_lab SET rag_note = ?1 WHERE id = ?2', [noteId, id]);
}
export async function rename(id: string, name: string) {
  await sql('UPDATE zx_lab SET name = ?1, updated_at = ?2 WHERE id = ?3', [name, Date.now(), id]);
}
export async function remove(id: string) {
  await sql('DELETE FROM zx_lab WHERE id = ?1', [id]);
}

export const hex4 = (n: number) => '$' + n.toString(16).padStart(4, '0');
export const hex2 = (n: number) => n.toString(16).padStart(2, '0');

/** Parses "$8000", "0x8000", "8000h" or decimal "32768". */
export function parseAddr(s: string): number | null {
  const t = s.trim().toLowerCase();
  let n: number;
  if (/^(\$|0x)[0-9a-f]+$/.test(t)) n = parseInt(t.replace(/^(\$|0x)/, ''), 16);
  else if (/^[0-9a-f]+h$/.test(t)) n = parseInt(t.slice(0, -1), 16);
  else if (/^[0-9]+$/.test(t)) n = parseInt(t, 10);
  else if (/^[0-9a-f]{4}$/.test(t)) n = parseInt(t, 16);
  else return null;
  return n >= 0 && n <= 0xffff ? n : null;
}

/** Spectrum screen ($4000, 6912 bytes) → RGBA pixels (256 × 192). */
export function screenToImage(bytes: Uint8Array, flashOn = false): ImageData {
  const img = new ImageData(256, 192);
  const pal = [
    [0, 0, 0], [0, 0, 0xd7], [0xd7, 0, 0], [0xd7, 0, 0xd7], [0, 0xd7, 0], [0, 0xd7, 0xd7], [0xd7, 0xd7, 0], [0xd7, 0xd7, 0xd7],
    [0, 0, 0], [0, 0, 0xff], [0xff, 0, 0], [0xff, 0, 0xff], [0, 0xff, 0], [0, 0xff, 0xff], [0xff, 0xff, 0], [0xff, 0xff, 0xff],
  ];
  for (let y = 0; y < 192; y++) {
    for (let cx = 0; cx < 32; cx++) {
      const addr = ((y & 0xc0) << 5) | ((y & 7) << 8) | ((y & 0x38) << 2) | cx;
      const byte = bytes[addr] ?? 0;
      const attr = bytes[6144 + (y >> 3) * 32 + cx] ?? 0x38;
      const bright = attr & 0x40 ? 8 : 0;
      let ink = (attr & 7) + bright;
      let paper = ((attr >> 3) & 7) + bright;
      if (flashOn && attr & 0x80) [ink, paper] = [paper, ink];
      for (let b = 0; b < 8; b++) {
        const c = pal[byte & (0x80 >> b) ? ink : paper];
        const i = (y * 256 + cx * 8 + b) * 4;
        img.data[i] = c[0];
        img.data[i + 1] = c[1];
        img.data[i + 2] = c[2];
        img.data[i + 3] = 255;
      }
    }
  }
  return img;
}
