/** The Games view's AI agent: writes ggez games with the user. It answers with whole files;
 *  RAD saves them (encrypted), builds the game and hands compiler errors back until it builds. */
import { rad } from './api';
import { assetKind, isC64, type GameAsset, type GameFile, type GameKind } from './games';

export type GameItem =
  | { kind: 'user'; text: string }
  | { kind: 'say'; text: string }
  | { kind: 'files'; paths: string[] }
  | { kind: 'build'; ok: boolean | null; output?: string }
  | { kind: 'docs'; query: string; files: string[] }
  | { kind: 'error'; text: string };

type Turn = { role: 'user' | 'assistant'; content: string };

const MAX_BUILDS = 6;

const ZX_BASIC = `Sinclair BASIC (ZX Spectrum 48K) essentials — RAD turns main.bas into a tape and runs it:
- Every line starts with a line number (1–9999); statements are separated by ":". IF … THEN runs the rest of the line (there is no ELSE, WHILE or REPEAT: use GO TO / GO SUB and RETURN). FOR n=1 TO 10 STEP 2 … NEXT n (loop variables are single letters).
- Variables: numbers have names of letters and digits (score, x1); strings and arrays have one letter (a$, DIM b(10), DIM c$(5,8)). Keywords may be typed in lowercase; GOTO/GOSUB work without the space.
- Screen: 32 columns × 22 rows for PRINT (rows 22–23 are the lower screen). PRINT AT row,col;"text"; PRINT AT 5,10; INK 2; PAPER 0; BRIGHT 1; FLASH 1;"X". Colours: 0 black 1 blue 2 red 3 magenta 4 green 5 cyan 6 yellow 7 white. BORDER n, CLS, PLOT x,y / DRAW dx,dy / CIRCLE x,y,r (256×176 pixels, y=0 at the bottom).
- Reading what's on screen (collisions): SCREEN$ (row,col) gives the character, ATTR (row,col) the colour byte.
- Keys: LET k$=INKEY$ (lowercase letters, "" if none) — check every loop, don't wait. PAUSE 0 waits for any key; PAUSE n waits n/50 s. Kempston joystick: IN 31 (1 right, 2 left, 4 down, 8 up, 16 fire).
- Sound: BEEP seconds,pitch (e.g. BEEP .05,12) — it stops the program while it plays, so keep beeps short.
- Random: RND (0–1), INT (RND*n), RANDOMIZE. Maths: ABS, INT, SGN, SQR, SIN, COS (radians), PI.
- Graphics characters: user-defined graphics A–U, in strings written \\a … \\u; define one with FOR i=0 TO 7: READ b: POKE USR "a"+i,b: NEXT i and DATA BIN 00111100, BIN 01111110, … (8 rows).
- Speed: BASIC is slow (a few hundred statements a second). Keep the main loop short, only redraw what moved (print a space over the old position), avoid long PRINTs and maths in the loop.`;

const ZX_ASM = `Z80 assembly for the ZX Spectrum 48K, assembled with sjasmplus. RAD adds "DEVICE ZXSPECTRUM48" and saves a snapshot that starts at the label \`start\`, so main.asm must have a \`start:\` label and must not use SAVESNA/SAVETAP itself.
- Layout: ORG $8000 for the code. At start: di, ld sp,$ff00, im 1, ei — then HALT waits for the next frame (50 a second): the main loop is "halt / read input / update / draw".
- Screen pixels: $4000–$57FF, 32 bytes per pixel row, rows in an odd order: address of pixel row y (0–191), byte column c (0–31) = $4000 | ((y & $C0) << 5) | ((y & 7) << 8) | ((y & $38) << 2) | c. For character cell row r: high byte = $40 | (r & $18) | line(0–7), low byte = ((r & 7) << 5) | column; the 8 lines of a cell are 256 bytes apart (inc h).
- Colours: $5800–$5AFF, one byte per 8×8 cell (32 × 24): bit 7 flash, bit 6 bright, bits 5–3 paper, bits 2–0 ink (0 black 1 blue 2 red 3 magenta 4 green 5 cyan 6 yellow 7 white). Cell address = $5800 + row*32 + column. Colour-block graphics (writing attributes only) are the easiest way to draw.
- Border and sound: out ($fe),a — bits 0–2 border colour, bit 4 the speaker (toggle it in a timed loop for a beep; keep the border bits).
- Keyboard: ld a,ROW : in a,($fe) — bits 0–4 are the keys, 0 = pressed. Rows: $FE shift Z X C V · $FD A S D F G · $FB Q W E R T · $F7 1 2 3 4 5 · $EF 0 9 8 7 6 · $DF P O I U Y · $BF enter L K J H · $7F space symbol M N B (bit 0 is the first key listed, e.g. $DF bit 0 = P, bit 1 = O). Kempston joystick: in a,($1f) — bit 0 right, 1 left, 2 down, 3 up, 4 fire (1 = pressed).
- The ROM font is at $3D00 (8 bytes per character from code 32): copy its 8 bytes into the 8 lines of a cell to draw text — RAD reads the screen as text this way, so print a title and a score. Don't rely on ROM printing routines (system variables aren't set up).
- sjasmplus syntax: one instruction per line; labels "name:" (local ones ".name" belong to the label above); EQU for constants; db / dw / ds for data; "$" for hex, "%" for binary; the screen and code are 48K: keep code and data between $8000 and $FEFF.
- Random numbers: a small LFSR or xorshift on a 16-bit value (ld a,r is not very random).`;

function zxPrompt(kind: GameKind, name: string, files: GameFile[]) {
  const listing = files.map((f) => `=== ${f.path} ===\n${f.content}`).join('\n\n');
  const basic = kind === 'zx-basic';
  return `You are a friendly expert ZX Spectrum game programmer from the 1980s, helping the user make "${name}", a game for the ZX Spectrum 48K in ${basic ? 'Sinclair BASIC' : 'Z80 assembly'}. When the user presses Run, it plays in RAD's Spectrum emulator.

How to change the game:
- Reply with a short sentence on what you're doing, then the COMPLETE new content of each file you change, in a fenced block whose info string names the file, exactly like:
\`\`\`${basic ? 'basic file=main.bas' : 'asm file=main.asm'}
…the whole file…
\`\`\`
- Never send partial files or "…rest unchanged".
- RAD then builds it and runs it for a few seconds on a Spectrum with no keys pressed, and tells you the errors${basic ? ' (and the BASIC error report, e.g. "2 Variable not found, 40:1")' : ''} or what the screen showed (as text). Fix problems by sending corrected files. When it works, the user plays it.
- To only talk, reply without file blocks.
- Make it fun and true to the Spectrum: a title, clear controls shown on screen (O/P/Q/A/space or a Kempston joystick), a score, game over and play again; bright colours on black.

${basic ? ZX_BASIC : ZX_ASM}

The game's current files:

${listing || '(none yet)'}`;
}
const C64_BASIC = `Commodore BASIC V2 (C64) essentials — RAD turns main.bas into a program (PRG) and runs it:
- Every line starts with a line number (0–63999) and is at most 80 characters as typed (a {CLR}-style code counts as one); statements are separated by ":". IF … THEN runs the rest of the line (no ELSE, WHILE or REPEAT: use GOTO / GOSUB and RETURN). FOR I=1 TO 10 STEP 2 … NEXT I.
- Variable names: only the FIRST TWO letters count (SCORE and SC are the same variable!), and a name must not contain a keyword anywhere (SCORE holds OR, TOTAL holds TO, FORT holds FOR) — use short names like S, X, Y, SX, LV. ST, TI and TI$ are the system's. Strings A$, whole numbers A%, arrays DIM A(10).
- Screen: 40 × 25. In strings, write control characters as {CLR}, {HOME}, {DOWN}, {UP}, {LEFT}, {RIGHT}, {RVS ON}, {RVS OFF} and colours {BLACK} {WHITE} {RED} {CYAN} {PURPLE} {GREEN} {BLUE} {YELLOW} {ORANGE} {BROWN} {LIGHT RED} {DARK GRAY} {GRAY} {LIGHT GREEN} {LIGHT BLUE} {LIGHT GRAY}. There's no PRINT AT: use {HOME}, {DOWN}s and TAB(col), or POKE 781,ROW:POKE 782,COL:POKE 783,0:SYS 65520 then PRINT.
- Fast drawing: POKE straight into screen memory 1024+ROW*40+COL with a screen code (A–Z = 1–26, 0–9 = 48–57, space 32, * 42, < 60, = 61, > 62, @ 0, ball 81, solid block 160) and its colour into 55296+ROW*40+COL (0 black 1 white 2 red 3 cyan 4 purple 5 green 6 blue 7 yellow 8 orange 9 brown 10 light red 11 dark grey 12 grey 13 light green 14 light blue 15 light grey). PEEK the screen for collisions. Border POKE 53280,C; background POKE 53281,C.
- Keys: GET K$ (doesn't wait; "" if no key). PEEK(197) is the key held now (64 = none; A 10, S 13, W 9, Z 12, X 23, space 60, RETURN 1). Joystick in port 2: J=PEEK(56320) — bit 0 up, 1 down, 2 left, 3 right, 4 fire, and 0 means pushed: IF (J AND 16)=0 THEN fire.
- Sound (the SID, 54272–54296): POKE 54296,15 for volume; voice 1 frequency 54272 (low) / 54273 (high), attack/decay 54277, sustain/release 54278, then the control register 54276: 17 triangle, 33 sawtooth, 65 pulse (width at 54274/54275), 129 noise — each with the gate on; write 16, 32, 64 or 128 to let the note go.
- Sprites: 63 bytes of data in a 64-byte block (e.g. block 13 = addresses 832–894), POKE 2040,13 points sprite 0 at it; enable POKE 53269,1; position POKE 53248,X (X over 255: POKE 53264,1) and POKE 53249,Y (visible X 24–343, Y 50–229); colour POKE 53287,C.
- Random: INT(RND(1)*N). Speed: BASIC is slow (a few hundred statements a second): keep the main loop short and only redraw what moved.
- Running it needs the C64's own BASIC ROM: with only RAD's free ROMs, RAD can check that it builds but not run it.`;

const C64_ASM = `6502 assembly for the Commodore 64, assembled by 64tass into a PRG (--cbm-prg). RAD loads it and types RUN (or SYS to its load address).
- Start main.asm with a BASIC line so RUN starts it:
        * = $0801
        .word (+), 10
        .null $9e, format("%d", start)
+       .word 0
  then the code from a \`start\` label.
- 64tass syntax: labels in the first column (no colon needed), instructions indented; NAME = value for constants; "-" and "+" are anonymous labels (bne - jumps back to the nearest "-"); .byte / .word / .text / .null (text plus a 0); .enc "screen" makes .text produce screen codes (upper-case letters), .enc "none" switches back; .for i := 0, i < 25, i += 1 … .endfor repeats lines; asl a / lsr a / rol a for the accumulator; $ hex, % binary, #<label / #>label for an address's low/high byte.
- Take over the machine at start: sei, then lda #$7f / sta $dc0d (no CIA interrupts). Don't call KERNAL or BASIC routines (RAD may be running free replacement ROMs). Frame timing: wait for the raster: lda $d012 / cmp #251 / bne - (once per frame, 50 a second), then read input, update, draw.
- Screen: $0400–$07E7, 40 × 25 screen codes (A–Z 1–26, 0–9 $30–$39, space $20); colour RAM $D800 + the same offset (0 black 1 white 2 red 3 cyan 4 purple 5 green 6 blue 7 yellow 8 orange 9 brown 10 light red 11 dark grey 12 grey 13 light green 14 light blue 15 light grey). Border $D020, background $D021; lda #$15 / sta $d018 for the upper-case character set. RAD reads the screen as text from $0400, so keep a title and the score in text.
- Joystick in port 2: lda #$ff / sta $dc00, then lda $dc00 — bit 0 up, 1 down, 2 left, 3 right, 4 fire; 0 means pushed. Keyboard: write a row mask (one bit 0) to $dc00 and read the columns from $dc01 (bit n = column n, 0 = pressed). Rows (bit 0 of $dc00 = row 0) and their columns 0–7: row 0 DEL RETURN →  F7 F1 F3 F5 ↓ · row 1: 3 W A 4 Z S E LSHIFT · row 2: 5 R D 6 C F T X · row 3: 7 Y G 8 B H U V · row 4: 9 I J 0 M K O N · row 5: + P L - . : @ , · row 6: £ * ; HOME RSHIFT = ↑ / · row 7: 1 ← CTRL 2 SPACE C= Q RUN/STOP. Write $ff to $dc00 afterwards.
- Sprites (8, 24 × 21 pixels): 63 bytes of data per 64-byte block; pointers at $07F8–$07FF (block number = address / 64, e.g. data at $2000 → 128); enable bits $D015; positions $D000/$D001 (sprite 0 X/Y), $D002/$D003 (sprite 1)…, bit 8 of each X in $D010; colours $D027–$D02E; collisions $D01E (sprite–sprite) and $D01F (sprite–background), reading clears them. Visible: X 24–343, Y 50–229.
- Sound (SID $D400–$D418): $D418 volume (15); voice 1: $D400/$D401 frequency, $D402/$D403 pulse width, $D405 attack/decay, $D406 sustain/release, $D404 control ($11 triangle, $21 sawtooth, $41 pulse, $81 noise with the gate on; clear bit 0 to release). Voices 2 and 3 at $D407 and $D40E.
- Zero page $FB–$FE is free for pointers; once interrupts are off, $02–$8F can be used too. Random numbers: an 8-bit LFSR (asl a / bcc + / eor #$1d). Decimal scores: sed / adc / cld.`;

function c64Prompt(kind: GameKind, name: string, files: GameFile[]) {
  const listing = files.map((f) => `=== ${f.path} ===\n${f.content}`).join('\n\n');
  const basic = kind === 'c64-basic';
  return `You are a friendly expert Commodore 64 game programmer from the 1980s, helping the user make "${name}", a game for the C64 in ${basic ? 'Commodore BASIC V2' : '6502 assembly (64tass)'}. When the user presses Run, it plays in RAD's C64 emulator.

How to change the game:
- Reply with a short sentence on what you're doing, then the COMPLETE new content of each file you change, in a fenced block whose info string names the file, exactly like:
\`\`\`${basic ? 'basic file=main.bas' : 'asm file=main.asm'}
…the whole file…
\`\`\`
- Never send partial files or "…rest unchanged".
- RAD then builds it and runs it for a few seconds on a C64 with no keys pressed, and tells you the errors${basic ? ' (and the BASIC error, e.g. "?SYNTAX ERROR IN 40")' : ''} or what the screen showed (as text). Fix problems by sending corrected files. When it works, the user plays it.
- To only talk, reply without file blocks.
- Make it fun and true to the C64: a title, clear controls shown on screen (a joystick in port 2, and keys too), a score, game over and play again; colourful, with SID sound effects.

${basic ? C64_BASIC : C64_ASM}

The game's current files:

${listing || '(none yet)'}`;
}
const MAX_LOOKUPS = 4;

type Doc = { file: string; line: number; text: string };

function docsText(docs: Doc[]) {
  return docs.map((d) => `--- ${d.file} (line ${d.line}) ---\n${d.text}`).join('\n\n');
}

/** What to look up for a build error: the names the compiler complains about, and the error text. */
function errorQuery(output: string) {
  const errs = output.split('\n').filter((l) => /^error/.test(l.trim())).slice(0, 6).join(' ');
  const names = [...errs.matchAll(/`([^`]+)`/g)].map((m) => m[1]).join(' ');
  return `${names} ${errs}`.slice(0, 600);
}

const CHEATSHEET = `ggez 0.10 essentials (this exact version — older ggez code doesn't compile):
- Cargo.toml: [dependencies] ggez = "0.10" (add other crates from crates.io if needed, e.g. rand = "0.8").
- Imports: use ggez::{conf, event, Context, GameResult}; use ggez::graphics::{self, Color, DrawParam, Rect, Text, Mesh, DrawMode}; use ggez::glam::Vec2; use ggez::input::keyboard::KeyInput; use ggez::winit::keyboard::{Key, NamedKey}; use ggez::input::mouse::MouseButton;
- main: let (ctx, event_loop) = ggez::ContextBuilder::new("id", "rad").window_setup(conf::WindowSetup::default().title("Title")).window_mode(conf::WindowMode::default().dimensions(800.0, 600.0)).build()?; event::run(ctx, event_loop, state)  — state is created after build (pass &mut ctx to constructors that make meshes/images).
- impl event::EventHandler for State { fn update(&mut self, ctx: &mut Context) -> GameResult; fn draw(&mut self, ctx: &mut Context) -> GameResult; optional: fn key_down_event(&mut self, ctx: &mut Context, input: KeyInput, repeated: bool) -> GameResult, key_up_event(&mut self, ctx, input: KeyInput), mouse_button_down_event(&mut self, ctx, button: MouseButton, x: f32, y: f32), mouse_motion_event(&mut self, ctx, x, y, dx, dy) }
- Time: let dt = ctx.time.delta().as_secs_f32(); fixed steps: while ctx.time.check_update_time(60) { … }
- Keys held: ctx.keyboard.is_logical_key_pressed(&Key::Named(NamedKey::ArrowLeft)), letters: &Key::Character("a".into()); in key_down_event match input.event.logical_key { Key::Named(NamedKey::Space) => …, Key::Named(NamedKey::Escape) => ctx.request_quit(), _ => {} }
- Mouse: ctx.mouse.position() (a Point2<f32> with .x/.y), ctx.mouse.button_pressed(MouseButton::Left)
- Drawing: let mut canvas = graphics::Canvas::from_frame(ctx, Color::from_rgb(20, 20, 32)); rectangles: canvas.draw(&graphics::Quad, DrawParam::new().dest_rect(Rect::new(x, y, w, h)).color(Color::RED)); circles/shapes: let m = Mesh::new_circle(ctx, DrawMode::fill(), Vec2::new(0.0, 0.0), radius, 0.5, Color::WHITE)?; canvas.draw(&m, Vec2::new(x, y)); also Mesh::new_rectangle(ctx, DrawMode::stroke(2.0), rect, color)?, Mesh::new_line(ctx, &[p1, p2], width, color)?, Mesh::new_polygon(ctx, DrawMode::fill(), &points, color)?; text: let mut t = Text::new("Score"); t.set_scale(32.0); canvas.draw(&t, DrawParam::new().dest(Vec2::new(10.0, 10.0)).color(Color::WHITE)); finish with canvas.finish(ctx)  (returns GameResult).
- DrawParam: .dest(Vec2) .rotation(radians) .offset(Vec2::new(0.5, 0.5)) .scale(Vec2) .color(Color); Color::new(r, g, b, a) with 0.0–1.0 floats, Color::from_rgb(u8, u8, u8), consts WHITE BLACK RED GREEN BLUE YELLOW CYAN MAGENTA.
- Rect: Rect::new(x, y, w, h), .overlaps(&other), .contains(point), fields x y w h.
- Random numbers: no rand in ggez — write a tiny xorshift, or add rand = "0.8" (use rand::Rng; rand::thread_rng().gen_range(0.0..1.0)).
- Assets (the game's resources/ folder; paths start with "/"): images: let img = graphics::Image::from_path(ctx, "/sprites/player.png")?; (load once in the constructor, keep in the state) canvas.draw(&img, DrawParam::new().dest(Vec2::new(x, y)).scale(Vec2::splat(2.0)).rotation(a).offset(Vec2::new(0.5, 0.5))); img.width() / img.height() are u32. For crisp pixel art: canvas.set_sampler(graphics::Sampler::nearest_clamp()); before drawing.
- Sounds: use ggez::audio::{self, SoundSource}; let mut s = audio::Source::new(ctx, "/sfx/jump.ogg")?; s.play(); (returns nothing; restarts it) — for sounds that overlap: s.play_detached() consumes the source, so keep audio::SoundData::new(ctx, "/sfx/hit.wav")? and make audio::Source::from_data(ctx, data.clone())? each time. Music: s.set_repeat(true); s.set_volume(0.5); s.play(). Formats: ogg, wav, mp3, flac.
- Fonts (two steps — one line borrows ctx twice and doesn't compile): let font = graphics::FontData::from_path(ctx, "/fonts/pixel.ttf")?; ctx.gfx.add_font("Pixel", font); then let mut t = Text::new("Hi"); t.set_font("Pixel");`;

function assetList(assets: GameAsset[]) {
  const files = assets.filter((a) => !a.path.endsWith('/'));
  if (!files.length) return '(none — the user can add images, sounds and fonts in the Assets panel)';
  const kb = (n: number) => (n < 1024 ? `${n} B` : `${Math.round(n / 1024)} KB`);
  return files.map((a) => `- "/${a.path}" — ${assetKind(a.path)}${a.info ? ` ${a.info}` : ''}, ${kb(a.size)}`).join('\n');
}

function systemPrompt(name: string, files: GameFile[], docs: Doc[], assets: GameAsset[]) {
  const listing = files.map((f) => `=== ${f.path} ===\n${f.content}`).join('\n\n');
  return `You are a friendly, expert Rust game developer helping the user build "${name}", a 2D game with the ggez game engine. The game is built with cargo and opens in its own window on the user's Mac when they press Run.

How to change the game:
- Reply with a short sentence on what you're doing, then the COMPLETE new content of every file you change, each in its own fenced block whose info string names the file, exactly like:
\`\`\`rust file=src/main.rs
…the whole file…
\`\`\`
\`\`\`toml file=Cargo.toml
…the whole file…
\`\`\`
- Never send partial files, diffs or "…rest unchanged". Files you don't send stay as they are.
- RAD builds the game right after your reply and sends you any compiler errors; then fix them by sending the corrected files. When it builds, the user can play it.
- To only talk (answer a question, suggest ideas), reply without file blocks.
- You have the ggez 0.10 documentation on this computer (guides, the official examples and the library's documented source). Relevant parts are included below; to look something else up, reply with only a block like
\`\`\`docs
how to draw an image rotated around its centre
\`\`\`
and you get the matching documentation back. Check the docs instead of guessing when you're unsure of an API.
- Assets: the game can use the images, sounds and fonts listed below (the user adds them in the Assets panel; they're put in resources/ when the game runs). Use only paths from that list — a missing file stops the game at start. If a game would need art or sound that isn't there, draw with shapes and text instead, and tell the user which files to add (name, folder, format, e.g. a 32×32 PNG at /sprites/player.png).
- Keep Esc to quit. Don't read or write files outside the game's own folder and don't use the network.
- Make games fun and polished: a clear goal, a score, game over and restart, smooth movement (use delta time), on-screen hints for the controls.

${CHEATSHEET}

The game's assets (resources/):
${assetList(assets)}

${docs.length ? `Relevant ggez 0.10 documentation (local):\n\n${docsText(docs)}\n\n` : ''}The game's current files:

${listing || '(none yet)'}`;
}

/** File blocks in a reply: ```lang file=path … ```. */
/** Documentation lookups the model asked for: ```docs … ```. */
export function parseDocsRequests(reply: string): string[] {
  return [...reply.matchAll(/```docs[^\n]*\n([\s\S]*?)```/g)].map((m) => m[1].trim()).filter(Boolean);
}

export function parseFiles(reply: string): { text: string; files: GameFile[] } {
  const files: GameFile[] = [];
  const re = /```[\w+-]*[ \t]+file=([^\s`]+)[^\n]*\n([\s\S]*?)```/g;
  const text = reply.replace(re, (_m, path: string, body: string) => {
    files.push({ path: path.trim().replace(/^\.?\//, ''), content: body.endsWith('\n') ? body : body + '\n' });
    return '';
  });
  return { text: text.replace(/\n{3,}/g, '\n\n').trim(), files };
}

/** A build's result: for Spectrum and C64 projects `output` also says what the program did when it ran. */
export interface BuildResult {
  ok: boolean;
  output: string;
}

export interface GameIO {
  kind: GameKind;
  files(): GameFile[];
  assets(): GameAsset[];
  /** Builds (and for Spectrum and C64 projects, test-runs) the game. */
  build(): Promise<BuildResult>;
  write(files: GameFile[]): Promise<void>;
  /** Saves the conversation (in the encrypted database). */
  persist(turns: Turn[]): void;
}

export class GameAgent {
  items = $state<GameItem[]>([]);
  busy = $state(false);
  status = $state('');
  /** After a successful build, run the game. */
  autoRun = $state(true);
  onBuilt: (() => void) | null = null;
  private turns: Turn[] = [];
  private stopped = false;

  constructor(
    public gameId: string,
    public name: string,
    private io: GameIO,
  ) {}

  /** Restores a saved conversation. */
  restore(turns: Turn[]) {
    this.turns = turns.filter((t) => t && (t.role === 'user' || t.role === 'assistant') && typeof t.content === 'string');
    this.items = [];
    for (const t of this.turns) {
      if (t.role === 'user') {
        if (t.content.startsWith('[build]') || t.content.startsWith('[docs]')) continue;
        this.items.push({ kind: 'user', text: t.content });
      } else {
        const { text, files } = parseFiles(t.content);
        if (text) this.items.push({ kind: 'say', text });
        if (files.length) this.items.push({ kind: 'files', paths: files.map((f) => f.path) });
      }
    }
  }

  stop() {
    this.stopped = true;
  }

  clear() {
    if (this.busy) return;
    this.items = [];
    this.turns = [];
    this.io.persist(this.turns);
  }

  private push(role: Turn['role'], content: string) {
    // The model wants alternating turns: fold consecutive user notes together.
    const last = this.turns[this.turns.length - 1];
    if (last && last.role === role && role === 'user') last.content += `\n\n${content}`;
    else this.turns.push({ role, content });
  }

  async ask(text: string) {
    if (this.busy || !text.trim()) return;
    this.busy = true;
    this.stopped = false;
    this.items.push({ kind: 'user', text });
    this.push('user', text);
    try {
      let builds = 0;
      let lookups = 0;
      // Docs for what the user asked; after a failed build, for the compiler's complaints.
      let query = text;
      while (!this.stopped) {
        this.status = builds ? 'Fixing the build…' : 'Thinking…';
        const ggez = this.io.kind === 'ggez';
        const docs = ggez ? ((await rad.gameDocs(query, 4).catch(() => null))?.results ?? []) : [];
        const prompt = ggez ? systemPrompt(this.name, this.io.files(), docs, this.io.assets()) : isC64(this.io.kind) ? c64Prompt(this.io.kind, this.name, this.io.files()) : zxPrompt(this.io.kind, this.name, this.io.files());
        const r = await rad.assist(prompt, this.turns.slice(-30));
        if (this.stopped) break;
        this.push('assistant', r.content);
        const { text: rawSay, files } = parseFiles(r.content);
        const asks = parseDocsRequests(r.content);
        const say = rawSay.replace(/```docs[^\n]*\n[\s\S]*?```/g, '').trim();
        if (say) this.items.push({ kind: 'say', text: say });
        if (!files.length && asks.length && lookups < MAX_LOOKUPS && ggez) {
          // The model wants to read documentation first.
          lookups++;
          const found: Doc[] = [];
          for (const q of asks.slice(0, 3)) {
            this.status = `Reading the ggez docs: ${q}`;
            const res = (await rad.gameDocs(q, 3).catch(() => null))?.results ?? [];
            found.push(...res);
            this.items.push({ kind: 'docs', query: q, files: res.map((d) => d.file) });
          }
          this.push('user', `[docs] ${found.length ? `Documentation for your lookup:\n\n${docsText(found)}` : 'Nothing in the docs matched that lookup.'}`);
          continue;
        }
        if (!files.length) break;
        await this.io.write(files);
        this.items.push({ kind: 'files', paths: files.map((f) => f.path) });

        this.status = !ggez ? (builds ? 'Building and trying it again…' : `Building and trying it on a ${isC64(this.io.kind) ? 'C64' : 'Spectrum'}…`) : builds ? 'Building again…' : 'Building the game (the first build takes a minute)…';
        const item: GameItem = { kind: 'build', ok: null };
        this.items.push(item);
        const b = await this.io.build();
        builds++;
        const last = this.items[this.items.length - 1] as Extract<GameItem, { kind: 'build' }>;
        last.ok = b.ok;
        last.output = b.output;
        if (b.ok) {
          this.push('user', ggez ? '[build] The game built successfully.' : `[build] ${b.output}`);
          if (this.autoRun) this.onBuilt?.();
          break;
        }
        if (builds >= MAX_BUILDS) {
          this.push('user', '[build] The build still fails; stopped trying for now.');
          this.items.push({ kind: 'error', text: `Still doesn’t build after ${MAX_BUILDS} tries — tell the agent what to try, or fix it in the editor.` });
          break;
        }
        this.push('user', `[build] The build failed. Fix it and send the corrected complete files.\n\`\`\`\n${b.output.slice(-6000)}\n\`\`\``);
        query = errorQuery(b.output) || text;
      }
      if (this.stopped) this.items.push({ kind: 'error', text: 'Stopped.' });
    } catch (e: any) {
      this.items.push({ kind: 'error', text: e?.message ?? String(e) });
    } finally {
      this.busy = false;
      this.status = '';
      this.io.persist(this.turns);
    }
  }
}
