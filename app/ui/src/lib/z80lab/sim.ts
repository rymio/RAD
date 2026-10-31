// Z80 Lab's simulator: the circuit on the canvas, pin by pin. Wires join pins into nets; every
// chip drives its output pins from the nets on its input pins (event-driven: when a net changes,
// only the chips on it look again). The CPU core is z80-emulator; each of its memory and I/O
// accesses becomes a real bus cycle on the wires: the address goes out on A0–A15, /MREQ or /IORQ
// and /RD or /WR go low, the address decoder (your gates or GAL) selects a chip, and the byte
// comes back over your data wires. Miswire something and the computer misbehaves the same way.
//
// For speed it also compiles the wiring into a memory map (which chip answers each address, at
// which of its own addresses, through which data wires) and uses it only where it gives exactly
// what the wires would; everything else (a buffer on the data bus, a bus fight) runs pin by pin.

import { Z80 } from 'z80-emulator';
import { PART, type PartDef } from './parts';

export interface Placed {
  id: string;
  type: string;
  ref: string;
  x: number;
  y: number;
  rot?: number;
  props: Record<string, any>;
}
export interface End {
  part: string;
  pin: number;
}
export interface Wire {
  id: string;
  a: End;
  b: End;
}
export interface Circuit {
  parts: Placed[];
  wires: Wire[];
}

/** Net levels: low, high, floating (nothing drives it), unknown (two drivers disagree). */
export const LO = 0,
  HI = 1,
  Z = 2,
  X = 3;
const NONE = 0,
  WEAK = 1,
  STRONG = 2;

export interface Problem {
  level: 'error' | 'warning' | 'info';
  text: string;
  parts?: string[];
}
export type AccessKind = 'fetch' | 'read' | 'write' | 'in' | 'out' | 'int';
export interface Access {
  kind: AccessKind;
  addr: number;
  data: number;
  /** The chip that answered (its ref), if one did. */
  who: string | null;
}
/** The nets after a bus cycle, for the canvas. */
export interface Snapshot {
  access: Access | null;
  val: Uint8Array;
  str: Uint8Array;
  drv: Int32Array;
  t: number;
}

type L = 0 | 1 | null;
const and3 = (a: L, b: L): L => (a === 0 || b === 0 ? 0 : a === 1 && b === 1 ? 1 : null);
const or3 = (a: L, b: L): L => (a === 1 || b === 1 ? 1 : a === 0 && b === 0 ? 0 : null);
const not3 = (a: L): L => (a === null ? null : a === 1 ? 0 : 1);
const lvl = (l: L) => (l === null ? X : l);

// ------------------------------------------------------------------ devices

abstract class Dev {
  /** Global pin index by pin number. */
  g: Record<number, number> = {};
  /** Answers on the bus (memory or I/O) and can be mapped for speed. */
  bus: 'mem' | 'io' | null = null;
  /** D0–D7 of the chip (pin numbers), for bus chips. */
  dataPins: number[] = [];
  addrPins: number[] = [];
  readonly hasPower: boolean;
  constructor(
    public sim: Sim,
    public part: Placed,
    public def: PartDef,
  ) {
    this.hasPower = def.pins.some((p) => p.kind === 'pwr') && def.pins.some((p) => p.kind === 'gnd');
  }
  pinNo(name: string) {
    return this.def.pins.find((p) => p.name === name)!.n;
  }
  net(n: number) {
    return this.sim.pinNet[this.g[n]];
  }
  v(n: number) {
    return this.sim.val[this.net(n)];
  }
  /** A pin's logic level (null: floating or unknown). */
  l(n: number): L {
    const v = this.v(n);
    return v === LO ? 0 : v === HI ? 1 : null;
  }
  /** Inputs that float read as unknown, except where the chip pulls them itself. */
  out(n: number, v: number, s = STRONG) {
    this.sim.setDrive(this.g[n], v, s);
  }
  release(n: number) {
    this.sim.setDrive(this.g[n], Z, NONE);
  }
  powered() {
    if (!this.hasPower) return true;
    const vcc = this.def.pins.find((p) => p.kind === 'pwr')!.n;
    const gnd = this.def.pins.find((p) => p.kind === 'gnd')!.n;
    return this.v(vcc) === HI && this.v(gnd) === LO;
  }
  releaseAll() {
    for (const p of this.def.pins) if (p.kind !== 'pas') this.release(p.n);
  }
  /** Sets the drives of its pins from the nets now. */
  abstract eval(): void;
  /** The byte on its D0–D7 (floating bits read as 1). */
  dataIn(): number {
    let b = 0;
    this.dataPins.forEach((n, i) => {
      if (this.v(n) !== LO) b |= 1 << i;
    });
    return b;
  }
  driveData(byte: number) {
    this.dataPins.forEach((n, i) => this.out(n, (byte >> i) & 1));
  }
  local(): number {
    let a = 0;
    this.addrPins.forEach((n, i) => {
      if (this.v(n) === HI) a |= 1 << i;
    });
    return a;
  }
  // Bus chips (fast path).
  readSel(): boolean {
    return false;
  }
  writeSel(): boolean {
    return false;
  }
  fastRead(_local: number): number {
    return 0xff;
  }
  fastWrite(_local: number, _v: number) {}
  tick(_t: number) {}
  reset() {}
  intPending(): boolean {
    return false;
  }
  intAck(): number {
    return 0xff;
  }
}

class Psu extends Dev {
  eval() {
    if (this.sim.power) {
      this.out(1, HI);
      this.out(2, LO);
    } else this.releaseAll();
  }
}

class Osc extends Dev {
  get hz() {
    return Number(this.part.props.hz) || 4_000_000;
  }
  running() {
    return this.powered() && this.v(1) !== LO;
  }
  eval() {
    if (this.running()) this.out(5, HI);
    else this.release(5);
  }
}

/** Passive parts copy what drives one side to the other: weakly through a resistor, fully
 *  through a closed switch. */
class Passive extends Dev {
  pairs: [number, number][] = [];
  /** Only from the first pin of each pair to the second (a resistor network's common pin). */
  oneWay = false;
  conducts(): 'weak' | 'strong' | null {
    const t = this.part.type;
    if (t === 'resistor' || t === 'rnet') return 'weak';
    if (t === 'switch') return this.part.props.on ? 'strong' : null;
    if (t === 'button') return this.sim.pressed.has(this.part.id) ? 'strong' : null;
    return null;
  }
  eval() {
    const c = this.conducts();
    for (const [a, b] of this.pairs) {
      if (!c) {
        this.release(a);
        this.release(b);
        continue;
      }
      const s = c === 'weak' ? WEAK : STRONG;
      // Each side gets what the others drive on the far side (never its own drive back).
      const fa = this.sim.strongExcept(this.net(b), this.g[b]);
      const fb = this.sim.strongExcept(this.net(a), this.g[a]);
      if (fa !== null && !this.oneWay) this.out(a, fa, s);
      else this.release(a);
      if (fb !== null) this.out(b, fb, s);
      else this.release(b);
    }
  }
}

class Inert extends Dev {
  eval() {}
}

class Terminal extends Dev {
  eval() {
    this.out(2, HI); // the idle "mark" level of a serial line
  }
}

class Gates extends Dev {
  constructor(
    sim: Sim,
    part: Placed,
    def: PartDef,
    private fn: (a: L, b: L) => L,
    private units: [number, number, number][],
  ) {
    super(sim, part, def);
  }
  eval() {
    if (!this.powered()) return this.releaseAll();
    for (const [a, b, y] of this.units) this.out(y, lvl(this.fn(this.l(a), this.l(b))));
  }
}

class Inverters extends Dev {
  eval() {
    if (!this.powered()) return this.releaseAll();
    for (const [a, y] of [[1, 2], [3, 4], [5, 6], [9, 8], [11, 10], [13, 12]]) this.out(y, lvl(not3(this.l(a))));
  }
}

class Decoder138 extends Dev {
  eval() {
    if (!this.powered()) return this.releaseAll();
    const outs = [15, 14, 13, 12, 11, 10, 9, 7];
    const en = and3(and3(this.l(6), not3(this.l(4))), not3(this.l(5)));
    const sel = [this.l(1), this.l(2), this.l(3)];
    for (let i = 0; i < 8; i++) {
      let hit: L = en;
      for (let b = 0; b < 3; b++) hit = and3(hit, (i >> b) & 1 ? sel[b] : not3(sel[b]));
      this.out(outs[i], lvl(not3(hit)));
    }
  }
}

class Latch273 extends Dev {
  q = 0;
  private cp: number = Z;
  constructor(sim: Sim, part: Placed, def: PartDef) {
    super(sim, part, def);
    this.dataPins = [3, 4, 7, 8, 13, 14, 17, 18];
    this.bus = 'io';
  }
  private qPins = [2, 5, 6, 9, 12, 15, 16, 19];
  eval() {
    if (!this.powered()) return this.releaseAll();
    const cp = this.v(11);
    if (!this.sim.probing) {
      if (this.v(1) === LO) this.q = 0;
      else if (this.cp === LO && cp === HI) this.q = this.dataIn();
      this.cp = cp;
    }
    this.qPins.forEach((n, i) => this.out(n, (this.q >> i) & 1));
  }
  writeSel() {
    return this.powered() && this.v(11) === LO && this.v(1) !== LO;
  }
  fastWrite(_l: number, v: number) {
    this.q = v;
    this.sim.dirtyDev(this);
  }
  reset() {
    this.q = 0;
  }
}

class Buffer245 extends Dev {
  private a = [2, 3, 4, 5, 6, 7, 8, 9];
  private b = [18, 17, 16, 15, 14, 13, 12, 11];
  eval() {
    if (!this.powered() || this.v(19) !== LO) return this.releaseAll();
    const [from, to] = this.v(1) === HI ? [this.a, this.b] : [this.b, this.a];
    for (const n of from) this.release(n);
    from.forEach((n, i) => {
      const v = this.sim.valueExcept(this.net(n), this.g[n]);
      if (v === null) this.release(to[i]);
      else this.out(to[i], v);
    });
  }
}

class Memory extends Dev {
  data: Uint8Array;
  readonly rom: boolean;
  private wasWrite = false;
  constructor(sim: Sim, part: Placed, def: PartDef) {
    super(sim, part, def);
    this.rom = def.type.startsWith('rom');
    this.bus = 'mem';
    this.dataPins = [0, 1, 2, 3, 4, 5, 6, 7].map((i) => this.pinNo(`D${i}`));
    this.addrPins = [...Array(15)].map((_, i) => this.pinNo(`A${i}`));
    this.data = sim.memories.get(part.id) ?? new Uint8Array(1 << 15);
    sim.memories.set(part.id, this.data);
  }
  readSel() {
    return this.powered() && this.v(20) === LO && this.v(22) === LO && this.v(27) !== LO;
  }
  writeSel() {
    return this.powered() && this.v(20) === LO && this.v(27) === LO;
  }
  eval() {
    if (!this.powered()) return this.releaseAll();
    if (this.readSel()) this.driveData(this.data[this.local()]);
    else for (const n of this.dataPins) this.release(n);
    const w = this.writeSel();
    // An SRAM stores while /CE and /WE are low; the last byte before /WE rises is what stays.
    if (this.sim.probing) return;
    if (w && !this.rom) this.data[this.local()] = this.dataIn();
    if (w && this.rom && !this.wasWrite) this.sim.note(`${this.part.ref} is a ROM: the write to its address ${hex(this.local(), 4)} was ignored`);
    this.wasWrite = w;
  }
  fastRead(l: number) {
    return this.data[l];
  }
  fastWrite(l: number, v: number) {
    if (this.rom) this.sim.note(`${this.part.ref} is a ROM: the write to its address ${hex(l, 4)} was ignored`);
    else this.data[l] = v;
  }
}

/** Z80 peripherals: selected by /CE with /IORQ low and /M1 high; /RD low reads, else writes.
 *  The write happens when the cycle ends (/IORQ goes high), with the byte held on the bus. */
abstract class ZPeripheral extends Dev {
  private phase: 'idle' | 'read' | 'write' = 'idle';
  private held = 0;
  constructor(sim: Sim, part: Placed, def: PartDef) {
    super(sim, part, def);
    this.bus = 'io';
    this.dataPins = [0, 1, 2, 3, 4, 5, 6, 7].map((i) => this.pinNo(`D${i}`));
  }
  abstract ceN: number;
  abstract iorqN: number;
  abstract rdN: number;
  abstract m1N: number;
  abstract intN: number;
  abstract readReg(local: number): number;
  abstract afterRead(local: number): void;
  abstract writeReg(local: number, v: number): void;
  private sel() {
    return this.powered() && this.v(this.ceN) === LO && this.v(this.iorqN) === LO && this.v(this.m1N) !== LO;
  }
  readSel() {
    return this.sel() && this.v(this.rdN) === LO;
  }
  writeSel() {
    return this.sel() && this.v(this.rdN) !== LO;
  }
  eval() {
    if (!this.powered()) return this.releaseAll();
    const r = this.readSel();
    const w = this.writeSel();
    const local = this.local();
    if (r) this.driveData(this.readReg(local));
    else for (const n of this.dataPins) this.release(n);
    if (this.sim.probing) this.phase = 'idle';
    else {
      if (w) this.held = this.dataIn();
      if (this.phase === 'write' && !w) this.writeReg(this.lastLocal, this.held);
      if (this.phase === 'read' && !r) this.afterRead(this.lastLocal);
      this.phase = r ? 'read' : w ? 'write' : 'idle';
      if (r || w) this.lastLocal = local;
    }
    if (this.intPending()) this.out(this.intN, LO);
    else this.release(this.intN);
  }
  private lastLocal = 0;
  fastRead(l: number) {
    const v = this.readReg(l);
    this.afterRead(l);
    return v;
  }
  fastWrite(l: number, v: number) {
    this.writeReg(l, v);
  }
}

class Sio extends ZPeripheral {
  ceN = 35;
  iorqN = 36;
  rdN = 32;
  m1N = 8;
  intN = 5;
  wr = [new Uint8Array(8), new Uint8Array(8)];
  ptr = [0, 0];
  rx: number[][] = [[], []];
  constructor(sim: Sim, part: Placed, def: PartDef) {
    super(sim, part, def);
    this.addrPins = [33, 34]; // C/D, B/A
  }
  reset() {
    this.wr = [new Uint8Array(8), new Uint8Array(8)];
    this.ptr = [0, 0];
    this.rx = [[], []];
  }
  readReg(l: number) {
    const ch = (l >> 1) & 1;
    if (!(l & 1)) return this.rx[ch][0] ?? 0;
    const p = this.ptr[ch];
    if (p === 1) return 0x01; // all sent
    if (p === 2) return ch ? this.wr[1][2] : 0;
    // RR0: received character available, transmit buffer empty, DCD, CTS.
    return (this.rx[ch].length ? 0x01 : 0) | 0x04 | 0x08 | 0x20;
  }
  afterRead(l: number) {
    const ch = (l >> 1) & 1;
    if (l & 1) this.ptr[ch] = 0;
    else this.rx[ch].shift();
  }
  writeReg(l: number, v: number) {
    const ch = (l >> 1) & 1;
    if (l & 1) {
      const p = this.ptr[ch];
      if (p === 0) {
        this.ptr[ch] = v & 7;
        if (((v >> 3) & 7) === 3) {
          // Channel reset.
          this.wr[ch] = new Uint8Array(8);
          this.rx[ch] = [];
        }
      } else {
        this.wr[ch][p] = v;
        this.ptr[ch] = 0;
      }
      return;
    }
    this.sim.serialOut(this, ch, v);
  }
  /** Bytes typed in the terminal arrive here. */
  receive(ch: number, v: number) {
    if (this.rx[ch].length < 64) this.rx[ch].push(v);
  }
  txEnabled(ch: number) {
    return !!(this.wr[ch][5] & 0x08);
  }
  rxEnabled(ch: number) {
    return !!(this.wr[ch][3] & 0x01);
  }
}

class Ctc extends ZPeripheral {
  ceN = 16;
  iorqN = 10;
  rdN = 6;
  m1N = 14;
  intN = 12;
  ctl = [0, 0, 0, 0];
  tc = [0, 0, 0, 0];
  count = [0, 0, 0, 0];
  acc = [0, 0, 0, 0];
  wantTc = [false, false, false, false];
  running = [false, false, false, false];
  pending = [false, false, false, false];
  vector = 0;
  constructor(sim: Sim, part: Placed, def: PartDef) {
    super(sim, part, def);
    this.addrPins = [18, 19];
  }
  reset() {
    this.ctl = [0, 0, 0, 0];
    this.running = [false, false, false, false];
    this.pending = [false, false, false, false];
    this.wantTc = [false, false, false, false];
  }
  readReg(l: number) {
    return this.count[l & 3] & 0xff;
  }
  afterRead() {}
  writeReg(l: number, v: number) {
    const ch = l & 3;
    if (this.wantTc[ch]) {
      this.tc[ch] = v || 256;
      this.count[ch] = this.tc[ch];
      this.wantTc[ch] = false;
      this.running[ch] = true;
      return;
    }
    if (v & 1) {
      this.ctl[ch] = v;
      if (v & 2) {
        this.running[ch] = false;
        this.pending[ch] = false;
      }
      if (v & 4) this.wantTc[ch] = true;
    } else if (ch === 0) this.vector = v & 0xf8;
  }
  tick(t: number) {
    if (!this.powered() || !this.sim.clockedBy(this, 15)) return;
    for (let ch = 0; ch < 4; ch++) {
      if (!this.running[ch] || this.ctl[ch] & 0x40) continue; // counter mode needs a trigger input
      const pre = this.ctl[ch] & 0x20 ? 256 : 16;
      this.acc[ch] += t;
      while (this.acc[ch] >= pre) {
        this.acc[ch] -= pre;
        if (--this.count[ch] <= 0) {
          this.count[ch] = this.tc[ch];
          if (this.ctl[ch] & 0x80) this.pending[ch] = true;
        }
      }
    }
  }
  intPending() {
    return this.pending.some(Boolean) && this.powered();
  }
  intAck() {
    const ch = this.pending.findIndex(Boolean);
    this.pending[ch] = false;
    this.sim.dirtyDev(this);
    return this.vector | (ch << 1);
  }
}

/** GAL16V8 in simple (combinational) mode: you name the pins and write the equations. */
class Gal extends Dev {
  names: string[] = [];
  eqs: { pin: number; invert: boolean; terms: { pin: number; neg: boolean }[][] }[] = [];
  errors: string[] = [];
  constructor(sim: Sim, part: Placed, def: PartDef) {
    super(sim, part, def);
    this.compile();
  }
  compile() {
    const raw = String(this.part.props.names ?? '').trim().split(/\s+/);
    this.names = [...Array(20)].map((_, i) => (raw[i] ?? '').toUpperCase());
    const pinOf = (name: string) => this.names.indexOf(name.toUpperCase()) + 1;
    this.eqs = [];
    this.errors = [];
    const text = String(this.part.props.equations ?? '')
      .split('\n')
      .map((l) => l.replace(/;.*$/, '').trim())
      .filter(Boolean)
      .reduce<string[]>((acc, l) => (l.startsWith('+') && acc.length ? (acc[acc.length - 1] += ' ' + l, acc) : (acc.push(l), acc)), []);
    for (const line of text) {
      const m = line.match(/^(\/?)([A-Za-z_][\w]*)\s*=\s*(.+)$/);
      if (!m) {
        this.errors.push(`can't read “${line}”`);
        continue;
      }
      const pin = pinOf(m[2]);
      if (pin < 12 || pin > 19) {
        this.errors.push(`${m[2]} must be one of pins 12–19 to be an output`);
        continue;
      }
      const terms = m[3].split('+').map((t) =>
        t.split(/[*&]/).map((f) => {
          f = f.trim();
          const neg = f.startsWith('/') || f.startsWith('!');
          const name = f.replace(/^[/!]/, '');
          const p = pinOf(name);
          if (!p) this.errors.push(`no pin is called ${name}`);
          return { pin: p, neg };
        }),
      );
      this.eqs.push({ pin, invert: m[1] === '/', terms });
    }
  }
  eval() {
    if (!this.powered()) return this.releaseAll();
    for (const e of this.eqs) {
      let sum: L = 0;
      for (const t of e.terms) {
        let prod: L = 1;
        for (const f of t) prod = and3(prod, f.pin ? (f.neg ? not3(this.l(f.pin)) : this.l(f.pin)) : null);
        sum = or3(sum, prod);
      }
      this.out(e.pin, lvl(e.invert ? not3(sum) : sum));
    }
  }
}

/** The CPU's pins. The core (z80-emulator) runs in Sim; this is what it puts on the wires. */
class Cpu extends Dev {
  addr = 0;
  data: number | null = null;
  mreq = false;
  iorq = false;
  rd = false;
  wr = false;
  m1 = false;
  halted = false;
  readonly A = [30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 1, 2, 3, 4, 5];
  constructor(sim: Sim, part: Placed, def: PartDef) {
    super(sim, part, def);
    this.dataPins = [14, 15, 12, 8, 7, 9, 10, 13];
  }
  eval() {
    if (!this.powered()) {
      this.sim.inReset = true;
      return this.releaseAll();
    }
    // However short the press, a low /RESET starts the program again when it goes high.
    if (this.v(26) !== HI) this.sim.inReset = true;
    this.A.forEach((n, i) => this.out(n, (this.addr >> i) & 1));
    if (this.data !== null) this.driveData(this.data);
    else for (const n of this.dataPins) this.release(n);
    this.out(19, this.mreq ? LO : HI);
    this.out(20, this.iorq ? LO : HI);
    this.out(21, this.rd ? LO : HI);
    this.out(22, this.wr ? LO : HI);
    this.out(27, this.m1 ? LO : HI);
    this.out(28, HI);
    this.out(18, this.halted ? LO : HI);
    this.out(23, HI);
  }
}

export const hex = (v: number, digits = 2) => v.toString(16).toUpperCase().padStart(digits, '0') + 'h';

// ------------------------------------------------------------------ the simulator

interface MapEntry {
  dev: number;
  local: number;
}
const OPEN = -1,
  LIVE = -2;

export class Sim {
  devs: Dev[] = [];
  byId = new Map<string, Dev>();
  pinNet = new Int32Array(0);
  pinDev = new Int32Array(0);
  pinNum = new Int32Array(0);
  netPins: number[][] = [];
  /** Pins by "partId:pin". */
  pinIndex = new Map<string, number>();
  // drives by pin, levels by net
  dval = new Uint8Array(0);
  dstr = new Uint8Array(0);
  val = new Uint8Array(0);
  str = new Uint8Array(0);
  drv = new Int32Array(0);
  private devsOnNet: number[][] = [];
  private dirtyNets = new Set<number>();
  private devQueue = new Set<number>();
  power = false;
  /** Working out the memory map: chips answer but nothing is stored or sent. */
  probing = false;
  pressed = new Set<string>();
  memories = new Map<string, Uint8Array>();
  cpu: Cpu | null = null;
  core: Z80 | null = null;
  tstates = 0;
  /** What the canvas plays back in slow motion. */
  snapshots: Snapshot[] = [];
  record = false;
  lastAccess: Access | null = null;
  /** Things that happened while running ("U3 is a ROM: …"), newest last. */
  notes: string[] = [];
  /** Serial bytes sent to a terminal: [terminal part id, byte]. */
  onSerial: (terminal: string, byte: number) => void = () => {};
  private interrupters: Dev[] = [];
  private tickers: Dev[] = [];
  /** /RESET has been low (or the power off): the CPU starts at 0000h when it next runs. */
  inReset = true;
  private maps: { mem: { r: Int32Array; rl: Int32Array; w: Int32Array; wl: Int32Array }; io: { r: Int32Array; rl: Int32Array; w: Int32Array; wl: Int32Array } } | null = null;
  /** Data wires of mapped chips: chip D bit → CPU D bit. */
  private dataMap = new Map<number, number[]>();
  private openByte = 0xff;
  private m1Pc = -1;
  private hal = {
    tStateCount: 0,
    readMemory: (a: number) => this.access(a === this.m1Pc ? 'fetch' : 'read', a, 0),
    writeMemory: (a: number, v: number) => void this.access('write', a, v),
    contendMemory: () => {},
    readPort: (a: number) => this.access('in', a & 0xffff, 0),
    writePort: (a: number, v: number) => void this.access('out', a & 0xffff, v),
    contendPort: () => {},
  };

  constructor(public circuit: Circuit, memories?: Map<string, Uint8Array>) {
    if (memories) this.memories = memories;
    this.build();
  }

  private build() {
    const { parts, wires } = this.circuit;
    let g = 0;
    const owners: number[] = [];
    const nums: number[] = [];
    parts.forEach((p, di) => {
      const def = PART[p.type];
      if (!def) return;
      for (const pin of def.pins) {
        this.pinIndex.set(`${p.id}:${pin.n}`, g++);
        owners.push(di);
        nums.push(pin.n);
      }
    });
    // Nets: pins joined by wires (union-find).
    const up = [...Array(g).keys()];
    const find = (i: number): number => (up[i] === i ? i : (up[i] = find(up[i])));
    for (const w of wires) {
      const a = this.pinIndex.get(`${w.a.part}:${w.a.pin}`);
      const b = this.pinIndex.get(`${w.b.part}:${w.b.pin}`);
      if (a !== undefined && b !== undefined) up[find(a)] = find(b);
    }
    const ids = new Map<number, number>();
    this.pinNet = new Int32Array(g);
    for (let i = 0; i < g; i++) {
      const r = find(i);
      if (!ids.has(r)) ids.set(r, ids.size);
      this.pinNet[i] = ids.get(r)!;
    }
    const nets = ids.size;
    this.netPins = [...Array(nets)].map(() => []);
    for (let i = 0; i < g; i++) this.netPins[this.pinNet[i]].push(i);
    this.pinNum = Int32Array.from(nums);
    this.dval = new Uint8Array(g).fill(Z);
    this.dstr = new Uint8Array(g);
    this.val = new Uint8Array(nets).fill(Z);
    this.str = new Uint8Array(nets);
    this.drv = new Int32Array(nets).fill(-1);
    // Devices.
    const devIndex: number[] = [];
    parts.forEach((p) => {
      const def = PART[p.type];
      if (!def) return;
      const d = this.makeDev(p, def);
      for (const pin of def.pins) d.g[pin.n] = this.pinIndex.get(`${p.id}:${pin.n}`)!;
      devIndex.push(this.devs.length);
      this.byId.set(p.id, d);
      this.devs.push(d);
    });
    this.pinDev = new Int32Array(g);
    for (let i = 0; i < g; i++) this.pinDev[i] = devIndex[owners[i]];
    this.devsOnNet = [...Array(nets)].map(() => []);
    this.devs.forEach((d, di) => {
      for (const gp of Object.values(d.g)) {
        const list = this.devsOnNet[this.pinNet[gp]];
        if (!list.includes(di)) list.push(di);
      }
    });
    this.cpu = (this.devs.find((d) => d instanceof Cpu) as Cpu) ?? null;
    if (this.cpu) this.core = new Z80(this.hal);
    // Who can interrupt the CPU (their /INT wired to its /INT) and who counts time.
    const c = this.cpu;
    this.interrupters = c ? this.devs.filter((d) => (d instanceof Sio && this.sameNet(d, 5, c, 16)) || (d instanceof Ctc && this.sameNet(d, 12, c, 16))) : [];
    this.tickers = this.devs.filter((d) => d instanceof Ctc);
    this.devs.forEach((_, i) => this.devQueue.add(i));
    this.propagate();
  }

  private makeDev(p: Placed, def: PartDef): Dev {
    switch (p.type) {
      case 'z80':
        return new Cpu(this, p, def);
      case 'rom28c256':
      case 'ram62256':
        return new Memory(this, p, def);
      case 'sio':
        return new Sio(this, p, def);
      case 'ctc':
        return new Ctc(this, p, def);
      case 'gal16v8':
        return new Gal(this, p, def);
      case 'hc00':
        return new Gates(this, p, def, (a, b) => not3(and3(a, b)), [[1, 2, 3], [4, 5, 6], [9, 10, 8], [12, 13, 11]]);
      case 'hc08':
        return new Gates(this, p, def, and3, [[1, 2, 3], [4, 5, 6], [9, 10, 8], [12, 13, 11]]);
      case 'hc32':
        return new Gates(this, p, def, or3, [[1, 2, 3], [4, 5, 6], [9, 10, 8], [12, 13, 11]]);
      case 'hc04':
        return new Inverters(this, p, def);
      case 'hc138':
        return new Decoder138(this, p, def);
      case 'hc273':
        return new Latch273(this, p, def);
      case 'hc245':
        return new Buffer245(this, p, def);
      case 'osc':
        return new Osc(this, p, def);
      case 'psu':
        return new Psu(this, p, def);
      case 'terminal':
        return new Terminal(this, p, def);
      case 'resistor':
      case 'button':
      case 'switch': {
        const d = new Passive(this, p, def);
        d.pairs = [[1, 2]];
        return d;
      }
      case 'rnet': {
        const d = new Passive(this, p, def);
        d.pairs = [2, 3, 4, 5, 6, 7, 8, 9].map((n) => [1, n] as [number, number]);
        d.oneWay = true;
        return d;
      }
      default:
        return new Inert(this, p, def);
    }
  }

  // ---------------------------------------------------------------- nets

  setDrive(gp: number, v: number, s: number) {
    if (this.dval[gp] === v && this.dstr[gp] === s) return;
    this.dval[gp] = v;
    this.dstr[gp] = s;
    this.dirtyNets.add(this.pinNet[gp]);
  }

  dirtyDev(d: Dev) {
    this.devQueue.add(this.devs.indexOf(d));
  }

  /** The strong level others drive on a net, leaving one pin out (null: none). */
  strongExcept(net: number, skip: number): number | null {
    let v: number | null = null;
    for (const p of this.netPins[net]) {
      if (p === skip || this.dstr[p] !== STRONG) continue;
      if (v === null) v = this.dval[p];
      else if (v !== this.dval[p]) return X;
    }
    return v;
  }

  /** The level a net has without one pin's drive (null: floating). */
  valueExcept(net: number, skip: number): number | null {
    let best = NONE;
    let v: number | null = null;
    for (const p of this.netPins[net]) {
      if (p === skip || this.dstr[p] === NONE) continue;
      if (this.dstr[p] > best) {
        best = this.dstr[p];
        v = this.dval[p];
      } else if (this.dstr[p] === best && v !== this.dval[p]) v = X;
    }
    return v;
  }

  private resolve(net: number) {
    let s = NONE;
    let v = Z;
    let d = -1;
    for (const p of this.netPins[net]) {
      const ps = this.dstr[p];
      if (ps === NONE) continue;
      if (ps > s) {
        s = ps;
        v = this.dval[p];
        d = p;
      } else if (ps === s && v !== this.dval[p]) v = X;
    }
    if (v === this.val[net] && s === this.str[net]) {
      this.drv[net] = d;
      return false;
    }
    this.val[net] = v;
    this.str[net] = s;
    this.drv[net] = d;
    return true;
  }

  /** Lets every change ripple through the chips until nothing changes. */
  propagate() {
    for (let guard = 0; guard < 400; guard++) {
      for (const n of this.dirtyNets) if (this.resolve(n)) for (const d of this.devsOnNet[n]) this.devQueue.add(d);
      this.dirtyNets.clear();
      if (!this.devQueue.size) return;
      const q = [...this.devQueue];
      this.devQueue.clear();
      for (const d of q) this.devs[d].eval();
    }
    this.note('The circuit oscillates (a loop of gates that never settles)');
  }

  /** Re-evaluates everything (after a switch flips, power, a reset…). */
  refresh() {
    this.devs.forEach((_, i) => this.devQueue.add(i));
    this.propagate();
  }

  note(text: string) {
    if (this.notes.at(-1) === text) return;
    this.notes.push(text);
    if (this.notes.length > 50) this.notes.shift();
  }

  sameNet(a: Dev, an: number, b: Dev, bn: number) {
    return this.pinNet[a.g[an]] === this.pinNet[b.g[bn]];
  }

  /** The oscillator driving a pin's net, if one does. */
  clockOf(d: Dev, n: number): Osc | null {
    const drv = this.drv[d.net(n)];
    const o = drv >= 0 ? this.devs[this.pinDev[drv]] : null;
    return o instanceof Osc && o.running() ? o : null;
  }

  clockedBy(d: Dev, n: number) {
    return !!this.clockOf(d, n);
  }

  /** The CPU's clock in Hz (0: none). */
  get hz() {
    return this.cpu ? (this.clockOf(this.cpu, 6)?.hz ?? 0) : 0;
  }

  serialOut(sio: Sio, ch: number, byte: number) {
    const txPin = ch ? 26 : 15;
    const clk = ch ? [27] : [13, 14];
    if (!sio.txEnabled(ch)) return this.note(`${sio.part.ref} channel ${ch ? 'B' : 'A'}: a byte was written but the transmitter is off (set WR5 bit 3)`);
    if (!clk.some((n) => this.clockedBy(sio, n) || this.ctcClock(sio, n))) return this.note(`${sio.part.ref} channel ${ch ? 'B' : 'A'} has no baud clock on ${ch ? '/RxTxCB' : '/TxCA'}: nothing was sent`);
    const net = sio.net(txPin);
    const g = this.netPins[net].find((p) => this.devs[this.pinDev[p]] instanceof Terminal && this.pinNum[p] === 1);
    const term = g === undefined ? null : this.devs[this.pinDev[g]];
    if (!term) return this.note(`${sio.part.ref} sent ${hex(byte)} on ${ch ? 'TxDB' : 'TxDA'}, but no terminal’s RXD is wired to it`);
    this.onSerial(term.part.id, byte);
  }

  private ctcClock(d: Dev, n: number) {
    const drv = this.drv[d.net(n)];
    return drv >= 0 && this.devs[this.pinDev[drv]] instanceof Ctc;
  }

  /** A key typed in a terminal: to every SIO channel whose RxD is wired to its TXD. */
  serialIn(terminal: string, byte: number) {
    const t = this.byId.get(terminal);
    if (!t) return;
    for (const d of this.devs) {
      if (!(d instanceof Sio)) continue;
      for (const ch of [0, 1]) {
        if (!this.sameNet(t, 2, d, ch ? 28 : 12)) continue;
        if (!d.rxEnabled(ch)) this.note(`${d.part.ref} channel ${ch ? 'B' : 'A'}: a byte arrived but the receiver is off (set WR3 bit 0)`);
        else d.receive(ch, byte);
      }
    }
  }

  // ---------------------------------------------------------------- bus cycles

  private live(kind: AccessKind, addr: number, v: number): number {
    const c = this.cpu!;
    const io = kind === 'in' || kind === 'out';
    const write = kind === 'write' || kind === 'out';
    c.addr = addr;
    c.mreq = !io;
    c.iorq = io;
    c.rd = !write;
    c.wr = write;
    c.m1 = kind === 'fetch';
    c.data = write ? v : null;
    this.devQueue.add(this.devs.indexOf(c));
    this.propagate();
    let byte = v;
    if (!write) {
      byte = 0;
      c.dataPins.forEach((n, i) => {
        const x = c.v(n);
        if (x === X) this.note(`Bus fight on the data bus (D${i}) reading ${hex(addr, 4)}: two chips answer at once`);
        if (x !== LO) byte |= 1 << i;
      });
    }
    const who = this.answering(write);
    if (!write && !who && c.dataPins.every((n) => this.str[c.net(n)] === NONE)) this.note(`Nothing answered the ${kind === 'in' ? 'IN from port' : 'read of'} ${hex(kind === 'in' ? addr & 0xff : addr, kind === 'in' ? 2 : 4)}: the data bus floats`);
    if (this.record) this.snap({ kind, addr, data: byte, who });
    this.lastAccess = { kind, addr, data: byte, who };
    // End of the cycle: controls go high (the data is still held), then the CPU lets go.
    c.mreq = c.iorq = c.rd = c.wr = c.m1 = false;
    this.devQueue.add(this.devs.indexOf(c));
    this.propagate();
    if (write) {
      c.data = null;
      this.devQueue.add(this.devs.indexOf(c));
      this.propagate();
    }
    return byte;
  }

  private answering(write: boolean): string | null {
    const d = this.devs.find((x) => x.bus && (write ? x.writeSel() : x.readSel()));
    return d?.part.ref ?? null;
  }

  private access(kind: AccessKind, addr: number, v: number): number {
    if (this.maps) {
      const io = kind === 'in' || kind === 'out';
      const m = io ? this.maps.io : this.maps.mem;
      const a = io ? addr & 0xff : addr;
      const write = kind === 'write' || kind === 'out';
      const e = write ? m.w[a] : m.r[a];
      if (e !== LIVE && !(io && addr >> 8 && this.ioHigh)) {
        let byte = v;
        let who: string | null = null;
        if (e === OPEN) byte = write ? v : this.openByte;
        else {
          const d = this.devs[e];
          who = d.part.ref;
          const local = write ? m.wl[a] : m.rl[a];
          const map = this.dataMap.get(e)!;
          if (write) {
            let b = 0;
            for (let i = 0; i < 8; i++) if ((v >> map[i]) & 1) b |= 1 << i;
            d.fastWrite(local, b);
          } else {
            const raw = d.fastRead(local);
            byte = 0;
            for (let i = 0; i < 8; i++) if ((raw >> i) & 1) byte |= 1 << map[i];
          }
        }
        this.lastAccess = { kind, addr, data: byte, who };
        return byte;
      }
    }
    return this.live(kind, addr, v);
  }
  /** The I/O decode looks at A8–A15 too: ports aren't mapped. */
  private ioHigh = false;

  private snap(access: Access | null) {
    this.snapshots.push({ access, val: this.val.slice(), str: this.str.slice(), drv: this.drv.slice(), t: this.tstates });
    if (this.snapshots.length > 4000) this.snapshots.splice(0, 1000);
  }

  /** Works out which chip answers every address, if the wiring allows it (see the top). */
  compile() {
    const c = this.cpu;
    this.maps = null;
    this.dataMap.clear();
    if (!c || !this.power) return;
    // Data wires: each bus chip's D pins straight onto CPU D pins (in any order).
    const cpuBit = new Map<number, number>();
    c.dataPins.forEach((n, i) => cpuBit.set(c.net(n), i));
    this.devs.forEach((d, i) => {
      if (!d.bus) return;
      const m = d.dataPins.map((n) => cpuBit.get(d.net(n)) ?? -1);
      if (!m.includes(-1) && new Set(m).size === 8) this.dataMap.set(i, m);
    });
    // The level the data bus floats to with nothing on it (pull-ups make it FFh).
    c.addr = 0;
    c.data = null;
    c.mreq = c.iorq = c.rd = c.wr = c.m1 = false;
    this.refresh();
    let open = 0;
    c.dataPins.forEach((n, i) => {
      if (c.v(n) !== LO) open |= 1 << i;
    });
    this.openByte = open;
    const cpuIdx = this.devs.indexOf(c);
    const scan = (io: boolean, write: boolean, size: number) => {
      const ent = new Int32Array(size);
      const loc = new Int32Array(size);
      c.mreq = !io;
      c.iorq = io;
      c.rd = !write;
      c.wr = write;
      c.data = write ? 0 : null;
      for (let k = 0; k < size; k++) {
        // Gray code: one address line changes per step, so little has to be looked at again.
        c.addr = k ^ (k >> 1);
        this.devQueue.add(cpuIdx);
        this.propagate();
        const a = c.addr;
        const sel: number[] = [];
        this.devs.forEach((d, i) => {
          if (d.bus && (write ? d.writeSel() : d.readSel())) sel.push(i);
        });
        // Anything else driving the data bus (a buffer, a gate) means: do it pin by pin.
        let other = false;
        if (!write)
          for (const n of c.dataPins) {
            const dr = this.drv[c.net(n)];
            if (dr >= 0 && this.str[c.net(n)] === STRONG && !sel.includes(this.pinDev[dr])) other = true;
          }
        if (other || sel.length > 1 || (sel.length === 1 && !this.dataMap.has(sel[0]))) ent[a] = LIVE;
        else if (!sel.length) ent[a] = OPEN;
        else {
          ent[a] = sel[0];
          loc[a] = this.devs[sel[0]].local();
        }
      }
      return { ent, loc };
    };
    this.probing = true;
    const mr = scan(false, false, 1 << 16);
    const mw = scan(false, true, 1 << 16);
    const ir = scan(true, false, 256);
    const iw = scan(true, true, 256);
    // Does the I/O decode look at A8–A15? Try a few ports with them set.
    this.ioHigh = false;
    c.mreq = false;
    c.iorq = true;
    c.rd = true;
    c.wr = false;
    c.data = null;
    for (const p of [0x00, 0x40, 0x80, 0x81, 0x88, 0xff]) {
      if (ir.ent[p] === LIVE) continue;
      c.addr = 0xff00 | p;
      this.devQueue.add(cpuIdx);
      this.propagate();
      const sel = this.devs.findIndex((d) => d.bus !== null && d.readSel());
      if ((sel < 0 ? OPEN : sel) !== ir.ent[p]) this.ioHigh = true;
    }
    c.addr = 0;
    c.mreq = c.iorq = c.rd = c.wr = c.m1 = false;
    c.data = null;
    this.devQueue.add(cpuIdx);
    this.propagate();
    this.probing = false;
    this.refresh();
    this.maps = { mem: { r: mr.ent, rl: mr.loc, w: mw.ent, wl: mw.loc }, io: { r: ir.ent, rl: ir.loc, w: iw.ent, wl: iw.loc } };
  }

  /** Forgets the map: every access goes over the wires (for watching, or when wiring changes). */
  uncompile() {
    this.maps = null;
  }

  get compiled() {
    return !!this.maps;
  }

  /** Which chip answers a memory address for a read (and at which of its addresses). */
  whoAt(addr: number, write = false): { ref: string; local: number } | null {
    if (!this.maps) this.compile();
    if (!this.maps) return null;
    const e = write ? this.maps.mem.w[addr] : this.maps.mem.r[addr];
    if (e < 0) return null;
    return { ref: this.devs[e].part.ref, local: write ? this.maps.mem.wl[addr] : this.maps.mem.rl[addr] };
  }

  // ---------------------------------------------------------------- running

  /** Why the CPU can't run now (null: it can). */
  blocked(): string | null {
    const c = this.cpu;
    if (!c) return 'There is no Z80 CPU on the board';
    if (!this.power) return 'The power is off';
    if (!c.powered()) return `${c.part.ref} has no power: wire VCC (pin 11) to +5V and GND (pin 29) to GND`;
    if (!this.hz) return `${c.part.ref} has no clock: wire an oscillator’s OUT to CLK (pin 6)`;
    const r = c.v(26);
    if (r === Z) return `${c.part.ref}’s /RESET (pin 26) is floating: add a pull-up resistor to +5V (and a reset button to GND)`;
    if (r === LO) return `${c.part.ref} is held in reset (/RESET is low)`;
    if (r === X) return `${c.part.ref}’s /RESET (pin 26) is driven high and low at once`;
    if (c.v(24) === LO) return `${c.part.ref} is waiting: /WAIT (pin 24) is low`;
    if (c.v(25) === LO) return `${c.part.ref} has given the bus away: /BUSREQ (pin 25) is low`;
    return null;
  }

  /** Power on, a reset, or the reset button let go: the CPU starts again at 0000h. */
  resetAll() {
    this.core?.reset();
    for (const d of this.devs) d.reset();
    this.tstates = 0;
    this.hal.tStateCount = 0;
    this.refresh();
  }

  setPower(on: boolean) {
    this.power = on;
    this.inReset = true;
    if (!on) for (const [id, m] of this.memories) if (this.byId.get(id) instanceof Memory && !(this.byId.get(id) as Memory).rom) m.fill(0);
    this.refresh();
  }

  /** Runs whole instructions for about `t` T-states. Returns the T-states run. */
  run(t: number): number {
    const z = this.core;
    if (!z) return 0;
    const start = this.hal.tStateCount;
    // The wires that could stop it (power, clock, /RESET, /WAIT…) only change between runs, or
    // when a chip's state changes the nets: looked at once a run and after every reset.
    while (this.hal.tStateCount - start < t) {
      if (this.blocked()) {
        this.inReset = true;
        break;
      }
      if (this.inReset) {
        this.inReset = false;
        this.resetAll();
      }
      const chunk = this.hal.tStateCount;
      while (this.hal.tStateCount - start < t && this.hal.tStateCount - chunk < 2000 && !this.inReset) this.step1();
    }
    return this.hal.tStateCount - start;
  }

  /** One instruction for watching or stepping: starts from 0000h after a reset; 0 if it can't run. */
  stepOne(): number {
    if (!this.core || this.blocked()) return 0;
    if (this.inReset) {
      this.inReset = false;
      this.resetAll();
    }
    return this.step1();
  }

  /** One instruction (or an interrupt being taken). */
  step1() {
    const z = this.core!;
    const c = this.cpu!;
    const before = this.hal.tStateCount;
    let intLine: Dev | null = null;
    for (const d of this.interrupters) if (d.intPending()) (intLine ??= d);
    if (intLine && z.regs.iff1) {
      // Interrupt acknowledge: /M1 and /IORQ together; mode 2 takes the vector from the chip.
      const vec = intLine.intAck();
      if (this.record) this.snap({ kind: 'int', addr: z.regs.pc, data: vec, who: intLine.part.ref });
      if (z.regs.im === 2) {
        if (z.regs.halted) {
          z.regs.pc = (z.regs.pc + 1) & 0xffff;
          z.regs.halted = 0;
        }
        z.regs.iff1 = z.regs.iff2 = 0;
        z.incTStateCount(19);
        z.pushWord(z.regs.pc);
        z.regs.pc = z.readWord(((z.regs.i << 8) | vec) & 0xffff);
      } else z.maskableInterrupt();
    } else {
      this.m1Pc = z.regs.pc;
      z.step();
      this.m1Pc = -1;
    }
    c.halted = !!z.regs.halted;
    const dt = this.hal.tStateCount - before;
    this.tstates += dt;
    for (const d of this.tickers) d.tick(dt);
    return dt;
  }

  // ---------------------------------------------------------------- what's wrong

  /** Wiring checks, for the Problems list (and Gilgamesh). */
  problems(): Problem[] {
    const out: Problem[] = [];
    const parts = this.circuit.parts;
    if (!parts.some((p) => p.type === 'psu')) out.push({ level: 'error', text: 'There is no power supply. Add one and wire +5V and GND to every chip.' });
    else if (!this.power) out.push({ level: 'info', text: 'The power is off (the power switch is in the toolbar).' });
    if (!this.cpu) out.push({ level: 'info', text: 'There is no Z80 CPU yet.' });
    for (const d of this.devs) {
      const p = d.part;
      if (d.hasPower && this.power && !d.powered()) {
        const vcc = d.def.pins.find((x) => x.kind === 'pwr')!.n;
        const gnd = d.def.pins.find((x) => x.kind === 'gnd')!.n;
        const what = [d.v(vcc) !== HI ? `VCC (pin ${vcc}) to +5V` : '', d.v(gnd) !== LO ? `GND (pin ${gnd}) to GND` : ''].filter(Boolean).join(' and ');
        out.push({ level: 'error', text: `${p.ref} (${d.def.label}) has no power: wire ${what}.`, parts: [p.id] });
        continue;
      }
      if (!this.power || !d.hasPower) continue;
      const floating = d.def.pins.filter((x) => (x.kind === 'in' || (x.kind === 'io' && d instanceof Gal && !d.eqs.some((e) => e.pin === x.n))) && d.v(x.n) === Z);
      // A GAL's unused pins, an oscillator's EN and SIO/CTC extras don't matter.
      const matters = floating.filter((x) => !(d instanceof Gal && !d.names[x.n - 1]?.match(/^[A-Z]/)) && !(d instanceof Gal && d.names[x.n - 1] === 'NC') && !(d instanceof Osc) && !/^(\/SYNC|\/DCD|\/CTS|IEI|CLK\/TRG|RxD|\/RxC|\/TxC|\/RxTxC)/.test(x.name));
      if (matters.length) {
        const cpuIn = d instanceof Cpu && matters.every((x) => ['/INT', '/NMI', '/WAIT', '/BUSREQ'].includes(x.name));
        const wired = (x: { n: number }) => this.netPins[d.net(x.n)].length > 1;
        const list = (xs: typeof matters) => xs.map((x) => `${x.name} (pin ${x.n})`).join(', ');
        const loose = matters.filter((x) => !wired(x));
        const undriven = matters.filter(wired);
        const said = [loose.length ? `${list(loose)} ${loose.length > 1 ? 'are' : 'is'} not connected` : '', undriven.length ? `${list(undriven)} ${undriven.length > 1 ? 'are' : 'is'} wired, but nothing drives ${undriven.length > 1 ? 'them' : 'it'} (is the chip at the other end powered?)` : ''].filter(Boolean).join('; ');
        out.push({ level: cpuIn ? 'warning' : 'error', text: `${p.ref} (${d.def.label}): ${said}${cpuIn ? '. Inputs like these need a pull-up resistor to +5V' : ''}.`, parts: [p.id] });
      }
      if (d instanceof Gal) for (const e of d.errors) out.push({ level: 'error', text: `${p.ref} (GAL): ${e}.`, parts: [p.id] });
    }
    // Shorts: a net driven high and low at once while idle.
    this.val.forEach((v, n) => {
      if (v !== X || !this.power) return;
      const strong = this.netPins[n].filter((g) => this.dstr[g] === STRONG && this.dval[g] <= HI);
      if (!strong.some((g) => this.dval[g] === HI) || !strong.some((g) => this.dval[g] === LO)) return;
      const who = strong.map((g) => `${this.devs[this.pinDev[g]].part.ref}.${this.pinNum[g]}`);
      out.push({ level: 'error', text: `Short circuit: ${who.join(' and ')} drive the same wire high and low.` });
    });
    // LEDs: a resistor in series, the right way round.
    for (const d of this.devs) {
      if (d.part.type !== 'led' || !this.power) continue;
      const a = d.net(1);
      const k = d.net(2);
      if (this.str[a] === STRONG && this.str[k] === STRONG && this.val[a] === HI && this.val[k] === LO) out.push({ level: 'warning', text: `${d.part.ref}: no resistor in series. A real LED would burn out; add about 470 Ω.`, parts: [d.part.id] });
      if (this.val[a] === LO && this.val[k] === HI) out.push({ level: 'info', text: `${d.part.ref} is the wrong way round (anode low, cathode high): it stays dark.`, parts: [d.part.id] });
    }
    const chips = this.devs.filter((d) => d.hasPower && d.def.pkg.kind === 'dip' && d.part.type !== 'osc');
    const caps = parts.filter((p) => p.type === 'capacitor').length;
    if (chips.length && caps < chips.length) out.push({ level: 'info', text: `${chips.length} chips and ${caps} capacitors: a real board has a 100 nF capacitor between VCC and GND by every chip (it runs here without them).` });
    if (this.power && this.cpu) {
      const why = this.blocked();
      if (why) out.push({ level: 'error', text: `The CPU can’t run: ${why}.`, parts: [this.cpu.part.id] });
    }
    return out;
  }

  /** LEDs that are lit now (part id → lit, or for an LED bar the byte shown). */
  lights(): Map<string, number> {
    const m = new Map<string, number>();
    for (const d of this.devs) {
      if (d.part.type === 'led') m.set(d.part.id, this.val[d.net(1)] === HI && this.val[d.net(2)] === LO ? 1 : 0);
      if (d.part.type === 'ledbar') {
        let b = 0;
        if (this.val[d.net(9)] === LO) for (let i = 0; i < 8; i++) if (this.val[d.net(i + 1)] === HI) b |= 1 << i;
        m.set(d.part.id, b);
      }
    }
    return m;
  }

  /** The net a pin is on, and every pin on it ("U1.6 CLK, X1.5 OUT"). */
  netOfPin(part: string, pin: number) {
    const g = this.pinIndex.get(`${part}:${pin}`);
    return g === undefined ? -1 : this.pinNet[g];
  }

  netName(net: number) {
    return this.netPins[net]
      .map((g) => {
        const d = this.devs[this.pinDev[g]];
        const pin = d.def.pins.find((p) => p.n === this.pinNum[g]);
        return `${d.part.ref}.${this.pinNum[g]}${pin && pin.name !== String(pin.n) ? ` ${pin.name}` : ''}`;
      })
      .join(', ');
  }

  galOf(id: string) {
    const d = this.byId.get(id);
    return d instanceof Gal ? d : null;
  }

  sioOf(id: string) {
    const d = this.byId.get(id);
    return d instanceof Sio ? d : null;
  }
}
