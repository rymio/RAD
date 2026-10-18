/** What the lab needs to know about each machine it can take games apart for: the ZX Spectrum
 *  (crates/rad-zx, zxlab.rs) and the Commodore 64 (crates/rad-c64, c64lab.rs). */
import { rad } from './api';
import { store } from './state.svelte';
import type { GameKind } from './games';
import type { LabLine, LabNotes, LabSummary } from './types';
import { labDb, type LabDb } from './zxlab';

export type LabMachineId = 'zx' | 'c64';
export type LabTab = 'overview' | 'code' | 'basic' | 'graphics' | 'text' | 'tape' | 'patches' | 'cheats' | 'board';

/** A patched game tried for a few seconds: did it crash, the screen, a problem in words. */
export interface LabCheck { crashed: boolean; screen: string[]; problem: string | null }

export interface LabMachine {
  id: LabMachineId;
  /** "Spectrum Lab", and how the machine is called in sentences. */
  title: string;
  name: string;
  exts: string[];
  fileKinds: string;
  intro: string;
  db: LabDb;
  tabs: LabTab[];
  open(id: string, name: string, data: string, seconds: number): Promise<LabSummary>;
  listing(id: string, from: number, to: number, notes: LabNotes): Promise<LabLine[]>;
  xrefs(id: string, addr: number, notes: LabNotes): Promise<{ addr: number; text: string }[]>;
  memory(id: string, from: number, len: number): Promise<string>;
  source(id: string, from: number, to: number, notes: LabNotes, title: string): Promise<{ source: string; verified: boolean; note?: string }>;
  assemblePatch(id: string, addr: number, code: string, notes: LabNotes): Promise<{ ok: boolean; bytes?: string; end?: number; len?: number; splits?: number | null; messages: string }>;
  build(id: string, patches: { addr: number; bytes: string }[], check: boolean): Promise<{ file: { name: string; data: string }; check: LabCheck | null }>;
  close(id: string): Promise<void>;
  /** Plays a file in the machine's own view. */
  play(name: string, data: string, title: string): void;
  /** The live machine's memory (base64) and held bytes, for the cheat finder. */
  liveMemory(): Promise<string>;
  lock(addr: number, value: number | null): Promise<void>;
  /** Addresses worth searching for game values. */
  cheatable(addr: number): boolean;
  asmKind: GameKind;
  basicKind: GameKind;
  /** Where the code runs from in the ROM, for the "where is the CPU" line. */
  inRom(pc: number): boolean;
  /** Patch syntax, for the form's placeholder and the AI. */
  patchExample: string;
  assembler: string;
  /** The AI's notes on the hardware. */
  hardware: string;
  /** For "Save to RAD": the project and tags the notes go in. */
  rag: { project: string; color: string; tags: string[] };
  /** The assembly source as a Games project (where it starts, if that needs saying). */
  exportSource(src: string, entry: number | null): string;
  crashText: string;
}

const ZX_HARDWARE = `ZX Spectrum 48K essentials for reading its code:
- Memory: $0000–$3FFF ROM (BASIC, tape routines, font at $3D00); $4000–$57FF screen pixels; $5800–$5AFF colour attributes (32×24 cells: bit 7 flash, 6 bright, 5–3 paper, 2–0 ink); $5B00–$5BFF printer buffer; $5C00–$5CB5 system variables (LAST_K $5C08 last key, FRAMES $5C78 frame counter, UDG $5C7B, CHARS $5C36); BASIC program from PROG ($5CCB usually); machine code usually from $6000–$8000 up. $4000–$7FFF is shared with the ULA (slower, "contended"); $8000–$FFFF is the CPU's alone.
- Screen pixel address of row y, byte column x: $4000 | ((y&$C0)<<5) | ((y&7)<<8) | ((y&$38)<<2) | x — games often keep a lookup table of row addresses or use "inc h" to step down a character cell.
- Ports: OUT ($FE),A border (bits 0–2) and speaker (bit 4, toggled in timed loops for sound); IN A,($FE) with A/B = a keyboard half-row ($FE shift–V, $FD A–G, $FB Q–T, $F7 1–5, $EF 0–6, $DF P–Y, $BF enter–H, $7F space–B; bits 0–4, 0 = pressed) and bit 6 = the tape (EAR). IN A,($1F) Kempston joystick (bits: right, left, down, up, fire). OUT (C) with BC=$FFFD/$BFFD: the 128K AY sound chip.
- Interrupts: IM 1 calls the ROM at $0038 50 times a second; IM 2 games point I at a vector table to run their own interrupt routine (music, timing). HALT waits for the next interrupt (frame sync).
- Common patterns: lives/score counters as bytes or BCD; "ld a,(lives) / dec a / ld (lives),a" for losing a life; sprite drawing with XOR or OR/AND masks; tables of 8-byte character graphics; LDIR to copy blocks; RST $10 to print a character via the ROM; LD_BYTES ($0556) in custom loaders.`;

const C64_HARDWARE = `Commodore 64 essentials for reading its code (6502):
- Memory: $0000–$00FF zero page (fast variables and pointers; $00/$01 is the CPU port: bits 0–2 choose whether BASIC ROM $A000–$BFFF, KERNAL ROM $E000–$FFFF and I/O $D000–$DFFF or RAM are seen — games often switch the ROMs out to use all 64K); $0100–$01FF stack; $0314/$0315 the IRQ vector games point at their own routine; $0400–$07E7 the default screen (screen codes; sprite pointers at $07F8–$07FF); $0801 BASIC programs and "10 SYS 2061" starters; $D800–$DBE7 colour RAM.
- VIC-II ($D000–$D02E): sprite X/Y $D000–$D00F (bit 8 of X in $D010), $D011 control (bit 5 bitmap mode, bit 3 25 rows, bit 4 screen on, bit 7 raster bit 8, bits 0–2 vertical scroll), $D012 raster line (read to wait for a line; write to set the raster interrupt), $D015 sprites on, $D016 horizontal scroll and multicolour, $D018 where the screen ((value>>4)*$400) and character set (((value>>1)&7)*$800) are inside the VIC bank, $D019/$D01A interrupt flags/mask (games acknowledge with inc $d019 or lda #$ff / sta $d019), $D01E/$D01F collisions, $D020 border, $D021 background, $D027–$D02E sprite colours. The VIC bank (16K) is chosen with $DD00 bits 0–1 (inverted: 3 = $0000, 2 = $4000, 1 = $8000, 0 = $C000).
- SID ($D400–$D418): three voices of 7 registers (frequency, pulse width, control: waveform + gate bit 0, attack/decay, sustain/release), filter $D415–$D417, volume $D418. Music players are usually called once per frame from the raster interrupt.
- CIA 1 ($DC00–$DC0F): $DC00 write keyboard row / read joystick 2 (bits 0–4: up, down, left, right, fire; 0 = pushed), $DC01 keyboard columns / joystick 1, timers $DC04–$DC07, $DC0D interrupt control (lda #$7f / sta $dc0d turns the KERNAL's timer interrupt off). CIA 2 ($DD00–$DD0F): VIC bank, serial bus, NMI.
- KERNAL jump table: $FFD2 CHROUT (print A), $FFE4 GETIN (a key), $FFD5 LOAD, $FFBA SETLFS, $FFBD SETNAM, $FF9F SCNKEY; $EA31 the default interrupt routine, $EA81 its exit.
- Common patterns: sei / set $0314 / cli for a raster interrupt; "wait for raster": lda $d012 / cmp #n / bne *-3; self-modifying code (sta into an instruction's operand); lives and scores in BCD with sed/adc/cld; tables of sprite positions; the screen drawn by copying from tables with indexed addressing; dec lives / bne still_alive.
- Many programs are packed (crunched): a small decruncher runs first and unpacks the game. RAD lets the program start and unpack for two seconds before taking its snapshot, so the listing shows the real game.`;

function b64Bytes(b64: string) {
  return Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
}

export const ZX_LAB: LabMachine = {
  id: 'zx',
  title: 'Spectrum Lab',
  name: 'Spectrum',
  exts: ['tzx', 'tap', 'z80', 'sna'],
  fileKinds: 'ZX Spectrum tapes and snapshots',
  intro: 'Open a TZX, TAP, Z80 or SNA file. RAD runs it and watches the CPU to tell code from graphics, lists the BASIC and the machine code with names, finds the text and pictures, and shows the data flowing on the board while you play. The AI explains it all and helps you change it — then you play your version.',
  db: labDb('zx_lab', ['tzx', 'tap', 'z80', 'sna']),
  tabs: ['overview', 'code', 'basic', 'graphics', 'text', 'tape', 'patches', 'cheats', 'board'],
  open: (id, name, data, seconds) => rad.zxlabOpen(id, name, data, seconds),
  listing: (id, from, to, notes) => rad.zxlabListing(id, from, to, notes),
  xrefs: (id, addr, notes) => rad.zxlabXrefs(id, addr, notes),
  memory: (id, from, len) => rad.zxlabMemory(id, from, len),
  source: (id, from, to, notes, title) => rad.zxlabSource(id, from, to, notes, title),
  assemblePatch: (id, addr, code, notes) => rad.zxlabAssemblePatch(id, addr, code, notes),
  async build(id, patches, check) {
    const r = await rad.zxlabBuild(id, patches, check);
    const c = r.check;
    return { file: r.file, check: c ? { crashed: c.crashed, screen: c.screen, problem: c.report && !['0', '9', 'D', 'L'].includes(c.report.code) ? `${c.report.code} ${c.report.message}, ${c.report.line}:${c.report.statement}` : null } : null };
  },
  close: (id) => rad.zxlabClose(id),
  play(name, data, title) {
    store.pendingZx = { name, data, title };
    store.navigate({ kind: 'zx' });
  },
  liveMemory: () => rad.zxMemory(),
  lock: (addr, value) => rad.zxLock(addr, value),
  cheatable: (a) => a >= 0x5b00,
  asmKind: 'zx-asm',
  basicKind: 'zx-basic',
  inRom: (pc) => pc < 0x4000,
  patchExample: '    nop\n    nop        ; the program’s labels can be used',
  assembler: 'Z80 code assembled at that address with sjasmplus',
  hardware: ZX_HARDWARE,
  rag: { project: 'ZX Spectrum Lab', color: '#d7a700', tags: ['zx-spectrum', 'reverse-engineering'] },
  exportSource: (src, entry) => (!/^start:/m.test(src) && entry != null ? `${src}\n; Where the game's code starts (RAD's lab found it running here).\nstart EQU $${entry.toString(16).padStart(4, '0')}\n` : src),
  crashText: 'It crashed (reset to the start-up screen) within 3 seconds.',
};

export const C64_LAB: LabMachine = {
  id: 'c64',
  title: 'C64 Lab',
  name: 'C64',
  exts: ['prg', 'p00', 't64', 'tap', 'd64', 'crt'],
  fileKinds: 'C64 programs, tapes, disks and cartridges',
  intro: 'Open a PRG, T64, TAP, D64 or CRT file. RAD lets it load, start and unpack itself, then watches the 6502 to tell code from graphics, lists the BASIC and the machine code with names (and the VIC-II, SID and CIA registers it uses), finds the text, sprites and character sets. The AI explains it all and helps you change it — then you play your version.',
  db: labDb('c64_lab', ['prg', 'p00', 't64', 'tap', 'd64', 'crt']),
  tabs: ['overview', 'code', 'basic', 'graphics', 'text', 'patches', 'cheats'],
  open: (id, name, data, seconds) => rad.c64labOpen(id, name, data, seconds),
  listing: (id, from, to, notes) => rad.c64labListing(id, from, to, notes),
  xrefs: (id, addr, notes) => rad.c64labXrefs(id, addr, notes),
  memory: (id, from, len) => rad.c64labMemory(id, from, len),
  source: (id, from, to, notes, title) => rad.c64labSource(id, from, to, notes, title),
  assemblePatch: (id, addr, code, notes) => rad.c64labAssemblePatch(id, addr, code, notes),
  async build(id, patches, check) {
    const r = await rad.c64labBuild(id, patches, check);
    const c = r.check;
    return { file: r.file, check: c ? { crashed: c.crashed, screen: c.screen, problem: c.error && c.error.message !== 'BREAK' ? `?${c.error.message}${c.error.line != null ? ` IN ${c.error.line}` : ''}` : null } : null };
  },
  close: (id) => rad.c64labClose(id),
  play(name, data, title) {
    store.pendingC64 = { name, data, title };
    store.navigate({ kind: 'c64' });
  },
  liveMemory: () => rad.c64Memory(),
  lock: (addr, value) => rad.c64Lock(addr, value),
  cheatable: (a) => a >= 0x0002 && a < 0x0100 ? true : a >= 0x0200 && (a < 0xd000 || a >= 0xe000),
  asmKind: 'c64-asm',
  basicKind: 'c64-basic',
  inRom: (pc) => pc >= 0xe000 || (pc >= 0xa000 && pc < 0xc000),
  patchExample: '        nop\n        nop        ; the program’s labels can be used',
  assembler: '6502 code assembled at that address with 64tass (instructions indented; labels in the first column)',
  hardware: C64_HARDWARE,
  rag: { project: 'C64 Lab', color: '#6c5eb5', tags: ['c64', 'reverse-engineering'] },
  exportSource: (src, entry) => (entry != null ? `${src}\n; RAD's lab found the game starting at $${entry.toString(16).padStart(4, '0')} (SYS ${entry}).\n` : src),
  crashText: 'It crashed (the CPU stopped on a JAM instruction) within 3 seconds.',
};

export const LAB_MACHINES: Record<LabMachineId, LabMachine> = { zx: ZX_LAB, c64: C64_LAB };

/** A C64 picture (392 × 272 colour indices, base64) → RGBA pixels. */
export function c64Picture(b64: string, palette: number[][]): ImageData {
  const px = b64Bytes(b64);
  const img = new ImageData(392, 272);
  for (let i = 0; i < 392 * 272; i++) {
    const c = palette[px[i] & 15];
    img.data[i * 4] = c[0];
    img.data[i * 4 + 1] = c[1];
    img.data[i * 4 + 2] = c[2];
    img.data[i * 4 + 3] = 255;
  }
  return img;
}

/** The C64's 16 colours (Pepto's palette, as rad_c64::PALETTE). */
export const C64_PALETTE = [
  [0x00, 0x00, 0x00], [0xff, 0xff, 0xff], [0x81, 0x33, 0x38], [0x75, 0xce, 0xc8], [0x8e, 0x3c, 0x97], [0x56, 0xac, 0x4d], [0x2e, 0x2c, 0x9b], [0xed, 0xf1, 0x71],
  [0x8e, 0x50, 0x29], [0x55, 0x38, 0x00], [0xc4, 0x6c, 0x71], [0x4a, 0x4a, 0x4a], [0x7b, 0x7b, 0x7b], [0xa9, 0xff, 0x9f], [0x70, 0x6d, 0xeb], [0xb2, 0xb2, 0xb2],
];
