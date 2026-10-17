/** Games: kept in the encrypted "Games" database (Turso, next to the vault). */
import { api } from './api';
import type { DbInfo, DbResult } from './types';
import STAR_CATCH from './zx-templates/star-catch.bas?raw';
import BAT_AND_BALL from './zx-templates/bat-and-ball.asm?raw';

/** What a game is made with: ggez (Rust, runs on this Mac), or for the ZX Spectrum in BASIC or Z80 assembler. */
export type GameKind = 'ggez' | 'zx-basic' | 'zx-asm';
export interface Game { id: string; name: string; kind: GameKind; created_at: number; updated_at: number }
export const isZx = (k: GameKind | undefined) => k === 'zx-basic' || k === 'zx-asm';
export interface GameFile { path: string; content: string }
/** A file in the game's `resources/` folder (images, sounds, fonts…). A path ending in `/` is a folder. */
export interface GameAsset { path: string; size: number; info: string }

const DB = 'Games';
let ready: Promise<void> | null = null;

async function sql(q: string, params: unknown[] = []) {
  return api.post<DbResult[]>(`/v1/db/${DB}/sql`, { sql: q, params });
}

/** Creates the database and its tables the first time. */
export function ensure(): Promise<void> {
  ready ??= (async () => {
    const dbs = await api.get<DbInfo[]>('/v1/db');
    if (!dbs.some((d) => d.name === DB)) await api.post('/v1/db', { name: DB });
    await sql(`CREATE TABLE IF NOT EXISTS games (id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS files (game_id TEXT NOT NULL, path TEXT NOT NULL, content TEXT NOT NULL, PRIMARY KEY (game_id, path));
CREATE TABLE IF NOT EXISTS chats (game_id TEXT PRIMARY KEY, turns TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS assets (game_id TEXT NOT NULL, path TEXT NOT NULL, data BLOB NOT NULL, size INTEGER NOT NULL, info TEXT NOT NULL DEFAULT '', PRIMARY KEY (game_id, path))`);
    // Added later: the project kind (older games are ggez).
    const [cols] = await sql('PRAGMA table_info(games)');
    if (!cols.rows.some((r) => r[1] === 'kind')) await sql("ALTER TABLE games ADD COLUMN kind TEXT NOT NULL DEFAULT 'ggez'");
  })().catch((e) => {
    ready = null;
    throw e;
  });
  return ready;
}

export async function list(): Promise<Game[]> {
  await ensure();
  const [r] = await sql('SELECT id, name, created_at, updated_at, kind FROM games ORDER BY updated_at DESC');
  return r.rows.map((x) => ({ id: x[0] as string, name: x[1] as string, created_at: x[2] as number, updated_at: x[3] as number, kind: ((x[4] as string) || 'ggez') as GameKind }));
}

export async function files(id: string): Promise<GameFile[]> {
  const [r] = await sql('SELECT path, content FROM files WHERE game_id = ?1 ORDER BY path', [id]);
  return r.rows.map((x) => ({ path: x[0] as string, content: x[1] as string }));
}

export async function saveFile(id: string, path: string, content: string) {
  await sql('INSERT OR REPLACE INTO files (game_id, path, content) VALUES (?1, ?2, ?3)', [id, path, content]);
  await sql('UPDATE games SET updated_at = ?1 WHERE id = ?2', [Date.now(), id]);
}

export async function removeFile(id: string, path: string) {
  await sql('DELETE FROM files WHERE game_id = ?1 AND path = ?2', [id, path]);
}

export async function create(name: string, template: keyof typeof TEMPLATES): Promise<Game> {
  await ensure();
  const id = `g${Date.now()}`;
  const now = Date.now();
  const kind = TEMPLATES[template].kind;
  await sql('INSERT INTO games (id, name, created_at, updated_at, kind) VALUES (?1, ?2, ?3, ?3, ?4)', [id, name, now, kind]);
  for (const f of TEMPLATES[template].files(crateName(name), name)) await saveFile(id, f.path, f.content);
  return { id, name, kind, created_at: now, updated_at: now };
}

export async function rename(id: string, name: string) {
  await sql('UPDATE games SET name = ?1, updated_at = ?2 WHERE id = ?3', [name, Date.now(), id]);
}

export async function remove(id: string) {
  await sql('DELETE FROM files WHERE game_id = ?1', [id]);
  await sql('DELETE FROM chats WHERE game_id = ?1', [id]);
  await sql('DELETE FROM assets WHERE game_id = ?1', [id]);
  await sql('DELETE FROM games WHERE id = ?1', [id]);
}

// ------------------------------------------------------------------ assets (resources/)

export const IMAGE_EXT = ['png', 'jpg', 'jpeg', 'webp', 'bmp'];
export const SOUND_EXT = ['ogg', 'wav', 'mp3', 'flac'];
export const FONT_EXT = ['ttf', 'otf'];
export const ext = (p: string) => p.split('.').pop()?.toLowerCase() ?? '';
export const assetKind = (p: string) =>
  p.endsWith('/') ? 'folder' : IMAGE_EXT.includes(ext(p)) ? 'image' : SOUND_EXT.includes(ext(p)) ? 'sound' : FONT_EXT.includes(ext(p)) ? 'font' : 'file';

export async function listAssets(id: string): Promise<GameAsset[]> {
  const [r] = await sql('SELECT path, size, info FROM assets WHERE game_id = ?1 ORDER BY path', [id]);
  return r.rows.map((x) => ({ path: x[0] as string, size: x[1] as number, info: x[2] as string }));
}

/** An asset's bytes, base64. */
export async function assetData(id: string, path: string): Promise<string> {
  const [r] = await sql('SELECT data FROM assets WHERE game_id = ?1 AND path = ?2', [id, path]);
  const v = r.rows[0]?.[0];
  return v && typeof v === 'object' ? v.$blob : '';
}

/** Every asset with its bytes, for a run. */
export async function allAssets(id: string): Promise<{ path: string; data: string }[]> {
  const [r] = await sql('SELECT path, data FROM assets WHERE game_id = ?1', [id]);
  return r.rows.map((x) => ({ path: x[0] as string, data: x[1] && typeof x[1] === 'object' ? (x[1] as { $blob: string }).$blob : '' }));
}

export async function putAsset(id: string, path: string, b64: string, size: number, info: string) {
  await sql('INSERT OR REPLACE INTO assets (game_id, path, data, size, info) VALUES (?1, ?2, ?3, ?4, ?5)', [id, path, { $blob: b64 }, size, info]);
  await sql('UPDATE games SET updated_at = ?1 WHERE id = ?2', [Date.now(), id]);
}

export async function addFolder(id: string, folder: string) {
  await putAsset(id, folder.replace(/\/*$/, '/'), '', 0, '');
}

/** Moves or renames an asset, or a folder with everything in it. */
export async function moveAsset(id: string, from: string, to: string) {
  if (from === to) return;
  if (from.endsWith('/')) {
    if (to.startsWith(from)) throw new Error('A folder can’t go inside itself');
    await sql('UPDATE assets SET path = ?3 || substr(path, ?4) WHERE game_id = ?1 AND substr(path, 1, ?5) = ?2', [id, from, to, from.length + 1, from.length]);
  } else {
    await sql('UPDATE assets SET path = ?3 WHERE game_id = ?1 AND path = ?2', [id, from, to]);
  }
}

export async function deleteAsset(id: string, path: string) {
  if (path.endsWith('/')) await sql('DELETE FROM assets WHERE game_id = ?1 AND substr(path, 1, ?3) = ?2', [id, path, path.length]);
  else await sql('DELETE FROM assets WHERE game_id = ?1 AND path = ?2', [id, path]);
}

/** Width × height of a PNG, JPEG, WebP or BMP, from its first bytes. */
export function imageSize(b64: string): string {
  try {
    const bin = atob(b64.slice(0, 200_000));
    const b = (i: number) => bin.charCodeAt(i);
    const be16 = (i: number) => (b(i) << 8) | b(i + 1);
    const le16 = (i: number) => b(i) | (b(i + 1) << 8);
    const be32 = (i: number) => ((b(i) << 24) >>> 0) + (b(i + 1) << 16) + (b(i + 2) << 8) + b(i + 3);
    const le32 = (i: number) => (b(i) | (b(i + 1) << 8) | (b(i + 2) << 16) | (b(i + 3) << 24)) >>> 0;
    if (bin.startsWith('\x89PNG')) return `${be32(16)}×${be32(20)}`;
    if (bin.startsWith('BM')) return `${le32(18)}×${Math.abs(le32(22) | 0)}`;
    if (bin.startsWith('RIFF') && bin.slice(8, 12) === 'WEBP') {
      const kind = bin.slice(12, 16);
      if (kind === 'VP8X') return `${1 + (b(24) | (b(25) << 8) | (b(26) << 16))}×${1 + (b(27) | (b(28) << 8) | (b(29) << 16))}`;
      if (kind === 'VP8 ') return `${le16(26) & 0x3fff}×${le16(28) & 0x3fff}`;
      if (kind === 'VP8L') return `${1 + (((b(22) & 0x3f) << 8) | b(21))}×${1 + (((b(24) & 0xf) << 10) | (b(23) << 2) | ((b(22) & 0xc0) >> 6))}`;
    }
    if (b(0) === 0xff && b(1) === 0xd8) {
      let i = 2;
      while (i < bin.length - 9) {
        if (b(i) !== 0xff) return '';
        const m = b(i + 1);
        if (m >= 0xc0 && m <= 0xcf && m !== 0xc4 && m !== 0xc8 && m !== 0xcc) return `${be16(i + 7)}×${be16(i + 5)}`;
        i += 2 + be16(i + 2);
      }
    }
  } catch {
    /* not an image we can read */
  }
  return '';
}

/** How to load an asset in ggez 0.10. */
export function loadSnippet(path: string): string {
  const p = '/' + path;
  switch (assetKind(path)) {
    case 'image':
      return `let image = graphics::Image::from_path(ctx, "${p}")?;\n// draw: canvas.draw(&image, DrawParam::new().dest(Vec2::new(x, y)));`;
    case 'sound':
      return `let mut sound = audio::Source::new(ctx, "${p}")?; // use ggez::audio::{self, SoundSource};\n// play: sound.play();  (play_detached() for overlapping copies; set_volume(0.5), set_repeat(true) for music)`;
    case 'font':
      return `let font = graphics::FontData::from_path(ctx, "${p}")?;\nctx.gfx.add_font("MyFont", font);\n// use: let mut t = Text::new("Hi"); t.set_font("MyFont");`;
    default:
      return `let bytes = ctx.fs.open("${p}")?; // std::io::Read`;
  }
}

export async function loadChat(id: string): Promise<unknown[]> {
  const [r] = await sql('SELECT turns FROM chats WHERE game_id = ?1', [id]);
  try {
    return r.rows.length ? JSON.parse(r.rows[0][0] as string) : [];
  } catch {
    return [];
  }
}

export async function saveChat(id: string, turns: unknown[]) {
  await sql('INSERT OR REPLACE INTO chats (game_id, turns) VALUES (?1, ?2)', [id, JSON.stringify(turns.slice(-60))]);
}

/** A Cargo package name from a game's name ("Space Dodge!" → "space_dodge"). */
export function crateName(name: string) {
  const n = name.toLowerCase().replace(/[^a-z0-9]+/g, '_').replace(/^_+|_+$/g, '');
  return /^[a-z]/.test(n) ? n.slice(0, 40) : `game_${n}`.slice(0, 40);
}

const cargo = (crate: string) => `[package]
name = "${crate}"
version = "0.1.0"
edition = "2021"

[dependencies]
ggez = "0.10"
`;

const DODGE = (title: string) => `//! ${title}: move with ← → (or A/D) and dodge the falling blocks. Space restarts, Esc quits.
use ggez::glam::Vec2;
use ggez::graphics::{self, Color, DrawParam, Rect, Text};
use ggez::input::keyboard::KeyInput;
use ggez::winit::keyboard::{Key, NamedKey};
use ggez::{conf, event, Context, GameResult};

const W: f32 = 800.0;
const H: f32 = 600.0;

struct Block {
    rect: Rect,
    speed: f32,
}

struct Game {
    player: Rect,
    blocks: Vec<Block>,
    spawn_in: f32,
    score: f32,
    over: bool,
    seed: u32,
}

impl Game {
    fn new() -> Self {
        Game { player: Rect::new(W / 2.0 - 20.0, H - 60.0, 40.0, 40.0), blocks: Vec::new(), spawn_in: 0.0, score: 0.0, over: false, seed: 12345 }
    }

    /// A small random number generator (0.0 ..= 1.0).
    fn rand(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (self.seed % 10_000) as f32 / 10_000.0
    }
}

impl event::EventHandler for Game {
    fn update(&mut self, ctx: &mut Context) -> GameResult {
        if self.over {
            return Ok(());
        }
        let dt = ctx.time.delta().as_secs_f32();
        let kb = &ctx.keyboard;
        let left = kb.is_logical_key_pressed(&Key::Named(NamedKey::ArrowLeft)) || kb.is_logical_key_pressed(&Key::Character("a".into()));
        let right = kb.is_logical_key_pressed(&Key::Named(NamedKey::ArrowRight)) || kb.is_logical_key_pressed(&Key::Character("d".into()));
        if left {
            self.player.x -= 420.0 * dt;
        }
        if right {
            self.player.x += 420.0 * dt;
        }
        self.player.x = self.player.x.clamp(0.0, W - self.player.w);

        self.spawn_in -= dt;
        if self.spawn_in <= 0.0 {
            let size = 20.0 + self.rand() * 40.0;
            let x = self.rand() * (W - size);
            let speed = 150.0 + self.rand() * 150.0 + self.score * 4.0;
            self.blocks.push(Block { rect: Rect::new(x, -size, size, size), speed });
            self.spawn_in = (0.6 - self.score * 0.005).max(0.15);
        }
        for b in &mut self.blocks {
            b.rect.y += b.speed * dt;
        }
        self.blocks.retain(|b| b.rect.y < H);
        if self.blocks.iter().any(|b| b.rect.overlaps(&self.player)) {
            self.over = true;
        }
        self.score += dt;
        Ok(())
    }

    fn draw(&mut self, ctx: &mut Context) -> GameResult {
        let mut canvas = graphics::Canvas::from_frame(ctx, Color::from_rgb(20, 20, 32));
        canvas.draw(&graphics::Quad, DrawParam::new().dest_rect(self.player).color(Color::from_rgb(120, 200, 255)));
        for b in &self.blocks {
            canvas.draw(&graphics::Quad, DrawParam::new().dest_rect(b.rect).color(Color::from_rgb(255, 110, 130)));
        }
        let mut hud = Text::new(format!("Score: {:.0}", self.score));
        hud.set_scale(28.0);
        canvas.draw(&hud, DrawParam::new().dest(Vec2::new(16.0, 12.0)).color(Color::WHITE));
        if self.over {
            let mut t = Text::new("Game over — press Space to play again");
            t.set_scale(32.0);
            canvas.draw(&t, DrawParam::new().dest(Vec2::new(150.0, H / 2.0 - 16.0)).color(Color::YELLOW));
        }
        canvas.finish(ctx)
    }

    fn key_down_event(&mut self, ctx: &mut Context, input: KeyInput, _repeated: bool) -> GameResult {
        match input.event.logical_key {
            Key::Named(NamedKey::Space) if self.over => *self = Game::new(),
            Key::Named(NamedKey::Escape) => ctx.request_quit(),
            _ => {}
        }
        Ok(())
    }
}

fn main() -> GameResult {
    let (ctx, event_loop) = ggez::ContextBuilder::new("${title.replace(/["\\]/g, '')}", "rad")
        .window_setup(conf::WindowSetup::default().title("${title.replace(/["\\]/g, '')}"))
        .window_mode(conf::WindowMode::default().dimensions(W, H))
        .build()?;
    event::run(ctx, event_loop, Game::new())
}
`;

const BLANK = (title: string) => `//! ${title}
use ggez::glam::Vec2;
use ggez::graphics::{self, Color, DrawParam, Text};
use ggez::input::keyboard::KeyInput;
use ggez::winit::keyboard::{Key, NamedKey};
use ggez::{conf, event, Context, GameResult};

const W: f32 = 800.0;
const H: f32 = 600.0;

struct Game {
    t: f32,
}

impl event::EventHandler for Game {
    fn update(&mut self, ctx: &mut Context) -> GameResult {
        self.t += ctx.time.delta().as_secs_f32();
        Ok(())
    }

    fn draw(&mut self, ctx: &mut Context) -> GameResult {
        let mut canvas = graphics::Canvas::from_frame(ctx, Color::from_rgb(24, 24, 36));
        let mut t = Text::new("${title.replace(/["\\`$]/g, '')}");
        t.set_scale(48.0);
        let y = H / 2.0 - 24.0 + (self.t * 2.0).sin() * 20.0;
        canvas.draw(&t, DrawParam::new().dest(Vec2::new(60.0, y)).color(Color::WHITE));
        canvas.finish(ctx)
    }

    fn key_down_event(&mut self, ctx: &mut Context, input: KeyInput, _repeated: bool) -> GameResult {
        if input.event.logical_key == Key::Named(NamedKey::Escape) {
            ctx.request_quit();
        }
        Ok(())
    }
}

fn main() -> GameResult {
    let (ctx, event_loop) = ggez::ContextBuilder::new("game", "rad")
        .window_setup(conf::WindowSetup::default().title("${title.replace(/["\\]/g, '')}"))
        .window_mode(conf::WindowMode::default().dimensions(W, H))
        .build()?;
    event::run(ctx, event_loop, Game { t: 0.0 })
}
`;

const ZX_BASIC_BLANK = (title: string) => `10 REM ${title.replace(/[^ -~]/g, '')}
20 BORDER 1: PAPER 0: INK 7: CLS
30 PRINT AT 10,8; INK 6;"Hello, Spectrum!"
40 PRINT AT 12,4;"Press a key to change colour"
50 PAUSE 0: BORDER INT (RND*8): GO TO 50
`;

const ZX_ASM_BLANK = (title: string) => `; ${title.replace(/[^ -~]/g, '')}: Z80 assembly for the ZX Spectrum (sjasmplus).
; RAD adds DEVICE ZXSPECTRUM48 and makes a snapshot that starts at \`start\`.

        ORG $8000

start:
        di
        ld sp,$ff00
        im 1
        ei
loop:
        halt                ; wait for the next frame (50 per second)
        ld a,(colour)
        inc a
        and 7
        ld (colour),a
        out ($fe),a         ; change the border colour
        jr loop

colour: db 0
`;

export const TEMPLATES = {
  dodge: { label: 'Dodge (playable example)', kind: 'ggez' as GameKind, files: (crate: string, title: string): GameFile[] => [{ path: 'Cargo.toml', content: cargo(crate) }, { path: 'src/main.rs', content: DODGE(title) }] },
  blank: { label: 'Blank window', kind: 'ggez' as GameKind, files: (crate: string, title: string): GameFile[] => [{ path: 'Cargo.toml', content: cargo(crate) }, { path: 'src/main.rs', content: BLANK(title) }] },
  zxBasicStars: { label: 'Star Catch (playable example)', kind: 'zx-basic' as GameKind, files: (_c: string, _t: string): GameFile[] => [{ path: 'main.bas', content: STAR_CATCH }] },
  zxBasic: { label: 'Blank BASIC program', kind: 'zx-basic' as GameKind, files: (_c: string, t: string): GameFile[] => [{ path: 'main.bas', content: ZX_BASIC_BLANK(t) }] },
  zxAsmBat: { label: 'Bat & Ball (playable example)', kind: 'zx-asm' as GameKind, files: (_c: string, _t: string): GameFile[] => [{ path: 'main.asm', content: BAT_AND_BALL }] },
  zxAsm: { label: 'Blank assembler program', kind: 'zx-asm' as GameKind, files: (_c: string, t: string): GameFile[] => [{ path: 'main.asm', content: ZX_ASM_BLANK(t) }] },
};

/** How each kind is described in the New game dialog. */
export const KINDS: { kind: GameKind; label: string; note: string; templates: (keyof typeof TEMPLATES)[] }[] = [
  { kind: 'ggez', label: 'ggez (Rust)', note: 'Modern 2D games in a window on this Mac.', templates: ['dodge', 'blank'] },
  { kind: 'zx-basic', label: 'ZX Spectrum — BASIC', note: 'Sinclair BASIC, like the type-in games of 1983. Simple, a bit slow, lots of charm.', templates: ['zxBasicStars', 'zxBasic'] },
  { kind: 'zx-asm', label: 'ZX Spectrum — Z80 assembler', note: 'How the classic games were made: full speed, smooth movement.', templates: ['zxAsmBat', 'zxAsm'] },
];
