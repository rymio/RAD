// Games → Z80 Lab: build a Z80 computer on a canvas (chips, resistors, a power supply, wires
// pin to pin), write its program in assembler, and run it on the circuit you wired (see
// z80lab/sim.ts). The clock can be slowed right down to watch every bus cycle travel the wires.
//
// Projects are kept in the encrypted Games database. Gilgamesh sees the board, its problems and
// the running program (describe()), can build and run things on it ([circuit: …]), and talks
// about it in the chat under the canvas.

import { invoke } from '@tauri-apps/api/core';
import { store } from './state.svelte';
import * as G from './games';
import { PART, PARTS, pinPos, bodySize } from './z80lab/parts';
import { Sim, hex, type Circuit, type Placed, type Wire, type Problem, type Snapshot } from './z80lab/sim';
import { TEMPLATES, PROGRAMS } from './z80lab/templates';

export interface LabProject extends Circuit {
  id: string;
  title: string;
  created: number;
  updated: number;
  source: string;
}

/** The clock: 0 runs at the oscillator's speed; anything else is a slowed-down clock in Hz. */
export const CLOCKS = [
  { hz: 0, label: 'Full speed' },
  { hz: 1_000_000, label: '1 MHz' },
  { hz: 100_000, label: '100 kHz' },
  { hz: 10_000, label: '10 kHz' },
  { hz: 1000, label: '1 kHz (watch)' },
  { hz: 100, label: '100 Hz (watch)' },
  { hz: 20, label: '20 Hz (watch)' },
  { hz: 5, label: '5 Hz (watch)' },
  { hz: 1, label: '1 Hz (watch)' },
];
/** At or below this the wires are animated cycle by cycle. */
export const WATCH_HZ = 1000;

interface Assembled {
  ok: boolean;
  errors: { line: number | null; message: string; kind?: string }[];
  output: string;
  image?: string;
  code?: [number, number, number][];
  labels?: Record<string, number>;
}

/** What the canvas shows: net levels, who drives them, and the bus cycle. */
export interface View {
  val: Uint8Array;
  str: Uint8Array;
  drv: Int32Array;
  access: Snapshot['access'];
  lights: Map<string, number>;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

class Z80Lab {
  list = $state<LabProject[]>([]);
  loaded = $state(false);
  openId = $state<string | null>(null);
  power = $state(false);
  running = $state(false);
  clock = $state(this.loadClock());
  problems = $state.raw<Problem[]>([]);
  view = $state.raw<View | null>(null);
  /** Bumped when the circuit is rebuilt (new nets). */
  version = $state(0);
  terminal = $state('');
  asm = $state.raw<Assembled | null>(null);
  assembling = $state(false);
  /** The source as it was last burned into the ROM (so Run knows to burn again). */
  burnedSource = $state<string | null>(null);
  burnNote = $state('');
  line = $state<number | null>(null);
  regs = $state.raw<Record<string, number> | null>(null);
  /** The speed it really ran at (Hz), at full speed. */
  speed = $state(0);
  selected = $state<string[]>([]);
  selectedWire = $state<string | null>(null);
  /** The bus cycles of the last instructions (watching), newest last. */
  cycles = $state.raw<string[]>([]);

  sim: Sim | null = null;
  private lineOf = new Map<number, number>();
  private saveTimer: ReturnType<typeof setTimeout> | undefined;
  private rebuildTimer: ReturnType<typeof setTimeout> | undefined;
  private frame = 0;
  private queue: { snap: Snapshot; cost: number }[] = [];
  private loadingList: Promise<void> | null = null;
  private checkedAt = 0;

  get project(): LabProject | null {
    return this.find(this.openId);
  }

  find(id: string | null | undefined) {
    return id ? (this.list.find((p) => p.id === id) ?? null) : null;
  }

  private loadClock() {
    try {
      return Number(localStorage.getItem('rad-z80lab-clock') ?? 0) || 0;
    } catch {
      return 0;
    }
  }

  setClock(hz: number) {
    this.clock = hz;
    try {
      localStorage.setItem('rad-z80lab-clock', String(hz));
    } catch {
      // Not remembered.
    }
    this.queue = [];
  }

  /** The CPU's own clock (the oscillator), in Hz. */
  get oscHz() {
    return this.sim?.hz ?? 0;
  }

  /** The clock the CPU runs at now. */
  get effectiveHz() {
    return this.clock || this.oscHz;
  }

  get watching() {
    return this.effectiveHz > 0 && this.effectiveHz <= WATCH_HZ;
  }

  // ---------------------------------------------------------------- projects

  load(): Promise<void> {
    if (this.loaded) return Promise.resolve();
    return (this.loadingList ??= this.refresh().finally(() => (this.loadingList = null)));
  }

  private async table() {
    await G.ensure();
    await G.sql('CREATE TABLE IF NOT EXISTS z80lab (id TEXT PRIMARY KEY, data TEXT NOT NULL, updated INTEGER NOT NULL)');
  }

  async refresh() {
    await this.table();
    const [r] = await G.sql('SELECT data FROM z80lab ORDER BY updated DESC');
    this.list = r.rows.flatMap((x) => {
      try {
        return [JSON.parse(x[0] as string) as LabProject];
      } catch {
        return [];
      }
    });
    this.loaded = true;
  }

  private saveSoon(p = this.project) {
    if (!p) return;
    p.updated = Date.now();
    clearTimeout(this.saveTimer);
    this.saveTimer = setTimeout(() => {
      G.sql('INSERT OR REPLACE INTO z80lab (id, data, updated) VALUES (?1, ?2, ?3)', [p.id, JSON.stringify(p), p.updated]).catch((e) => store.error(e, 'Couldn’t save the circuit'));
    }, 600);
  }

  async create(template: keyof typeof TEMPLATES = 'easy', program: keyof typeof PROGRAMS = 'hello') {
    await this.load();
    const now = Date.now();
    const c = TEMPLATES[template].make();
    const p: LabProject = { id: `z${now}`, title: template === 'easy' ? 'Easy Z80' : 'My Z80 computer', created: now, updated: now, source: PROGRAMS[program].source, ...c };
    this.list = [p, ...this.list];
    this.saveSoon(this.find(p.id)!);
    this.open(p.id);
    store.navigate({ kind: 'z80lab' });
  }

  open(id: string) {
    this.stop();
    this.openId = id;
    this.power = false;
    this.terminal = '';
    this.asm = null;
    this.burnedSource = null;
    this.burnNote = '';
    this.line = null;
    this.regs = null;
    this.selected = [];
    this.selectedWire = null;
    this.cycles = [];
    this.rebuild(true);
  }

  close() {
    this.stop();
    this.openId = null;
    this.sim = null;
    this.view = null;
  }

  async remove(id: string) {
    if (this.openId === id) this.close();
    this.list = this.list.filter((p) => p.id !== id);
    await G.sql('DELETE FROM z80lab WHERE id = ?1', [id]).catch((e) => store.error(e, 'Couldn’t delete the circuit'));
  }

  rename(title: string) {
    const p = this.project;
    if (!p || !title.trim()) return;
    p.title = title.trim().slice(0, 80);
    this.saveSoon();
  }

  setSource(src: string) {
    const p = this.project;
    if (!p) return;
    p.source = src;
    this.saveSoon();
  }

  // ---------------------------------------------------------------- the circuit

  /** Makes the simulator from the circuit again (keeping what's in the memories). */
  private rebuild(now = false) {
    clearTimeout(this.rebuildTimer);
    const go = () => {
      const p = this.project;
      if (!p) return;
      const was = this.running;
      this.stop();
      const mem = this.sim?.memories;
      const sim = new Sim($state.snapshot(p) as Circuit, mem);
      sim.onSerial = (_t, b) => this.serialByte(b);
      sim.setPower(this.power);
      this.sim = sim;
      this.version++;
      this.updateView();
      if (was && this.power) this.start();
    };
    if (now) go();
    else this.rebuildTimer = setTimeout(go, 120);
  }

  /** A change to the circuit: saved, and the simulator rebuilt. */
  private changed() {
    this.saveSoon();
    this.rebuild();
  }

  private nextRef(prefix: string) {
    const used = new Set(this.project?.parts.map((p) => p.ref));
    let i = 1;
    while (used.has(`${prefix}${i}`)) i++;
    return `${prefix}${i}`;
  }

  addPart(type: string, x: number, y: number): Placed | null {
    const p = this.project;
    const def = PART[type];
    if (!p || !def) return null;
    const ref = this.nextRef(def.ref);
    const part: Placed = { id: `${ref}-${Date.now().toString(36)}`, type, ref, x: Math.round(x / 10) * 10, y: Math.round(y / 10) * 10, rot: 0, props: { ...(def.props ?? {}) } };
    p.parts.push(part);
    this.selected = [part.id];
    this.changed();
    return part;
  }

  /** A free spot right of everything (for parts added by name). */
  freeSpot() {
    const p = this.project;
    if (!p?.parts.length) return { x: 80, y: 80 };
    const right = Math.max(...p.parts.map((x) => x.x + bodySize(PART[x.type]).w));
    return { x: right + 80, y: 60 };
  }

  movePart(id: string, x: number, y: number) {
    const part = this.project?.parts.find((p) => p.id === id);
    if (!part) return;
    part.x = Math.round(x / 10) * 10;
    part.y = Math.round(y / 10) * 10;
    this.saveSoon();
  }

  rotate(id: string) {
    const part = this.project?.parts.find((p) => p.id === id);
    if (!part || PART[part.type]?.pkg.kind !== 'two') return;
    part.rot = part.rot === 90 ? 0 : 90;
    this.changed();
  }

  setProp(id: string, key: string, value: unknown) {
    const part = this.project?.parts.find((p) => p.id === id);
    if (!part) return;
    part.props[key] = value;
    this.changed();
  }

  setRef(id: string, ref: string) {
    const part = this.project?.parts.find((p) => p.id === id);
    ref = ref.trim().toUpperCase();
    if (!part || !ref || this.project!.parts.some((p) => p.ref === ref && p.id !== id)) return false;
    part.ref = ref;
    this.changed();
    return true;
  }

  removeParts(ids: string[]) {
    const p = this.project;
    if (!p) return;
    p.parts = p.parts.filter((x) => !ids.includes(x.id));
    p.wires = p.wires.filter((w) => !ids.includes(w.a.part) && !ids.includes(w.b.part));
    this.selected = [];
    this.changed();
  }

  connected(a: { part: string; pin: number }, b: { part: string; pin: number }) {
    return !!this.sim && this.sim.netOfPin(a.part, a.pin) === this.sim.netOfPin(b.part, b.pin) && this.sim.netOfPin(a.part, a.pin) >= 0;
  }

  addWire(a: { part: string; pin: number }, b: { part: string; pin: number }): Wire | null {
    const p = this.project;
    if (!p || (a.part === b.part && a.pin === b.pin)) return null;
    const dup = p.wires.some((w) => (w.a.part === a.part && w.a.pin === a.pin && w.b.part === b.part && w.b.pin === b.pin) || (w.a.part === b.part && w.a.pin === b.pin && w.b.part === a.part && w.b.pin === a.pin));
    if (dup) return null;
    const w: Wire = { id: `w${Date.now().toString(36)}${Math.random().toString(36).slice(2, 5)}`, a, b };
    p.wires.push(w);
    this.changed();
    return w;
  }

  removeWire(id: string) {
    const p = this.project;
    if (!p) return;
    p.wires = p.wires.filter((w) => w.id !== id);
    if (this.selectedWire === id) this.selectedWire = null;
    this.changed();
  }

  /** Wires the pins of two parts that carry the same signal (A0 to A0, /OE to /RD, VCC to +5V…). */
  connectMatching(aId: string, bId: string): number {
    const p = this.project;
    const a = p?.parts.find((x) => x.id === aId);
    const b = p?.parts.find((x) => x.id === bId);
    if (!a || !b) return 0;
    const da = PART[a.type];
    const db = PART[b.type];
    const sig = (pin: { name: string; bus?: string }) => pin.bus ?? (/^(\/?[A-Z]+\d*)$/.test(pin.name) ? pin.name : null);
    let n = 0;
    for (const pa of da.pins) {
      const s = sig(pa);
      if (!s) continue;
      const pb = db.pins.find((x) => sig(x) === s);
      if (!pb) continue;
      const ea = { part: a.id, pin: pa.n };
      const eb = { part: b.id, pin: pb.n };
      if (this.connected(ea, eb)) continue;
      const w: Wire = { id: `w${Date.now().toString(36)}${n}${Math.random().toString(36).slice(2, 4)}`, a: ea, b: eb };
      p!.wires.push(w);
      n++;
    }
    if (n) this.changed();
    return n;
  }

  // ---------------------------------------------------------------- power, program, running

  setPower(on: boolean) {
    this.power = on;
    if (!on) this.stop();
    this.sim?.setPower(on);
    if (!on) this.terminal += this.terminal && !this.terminal.endsWith('\n') ? '\n' : '';
    this.updateView();
  }

  /** Assembles the program and burns it into the ROM chip, through the circuit's own decoding. */
  async assemble(): Promise<boolean> {
    const p = this.project;
    if (!p || !this.sim) return false;
    this.assembling = true;
    try {
      const r = await invoke<Assembled>('z80lab_assemble', { source: p.source });
      this.asm = r;
      if (!r.ok || !r.image || !r.code) {
        this.burnNote = '';
        return false;
      }
      this.lineOf = new Map();
      for (const [a, n, line] of r.code) for (let k = 0; k < n; k++) this.lineOf.set((a + k) & 0xffff, line);
      this.burn(Uint8Array.from(atob(r.image), (c) => c.charCodeAt(0)), r.code);
      this.burnedSource = p.source;
      return true;
    } catch (e) {
      store.error(e, 'Couldn’t assemble the program');
      return false;
    } finally {
      this.assembling = false;
    }
  }

  private burn(image: Uint8Array, code: [number, number, number][]) {
    const sim = this.sim!;
    // A programmer powers the chip by itself: find which ROM answers each address with the power on.
    const was = sim.power;
    if (!was) sim.setPower(true);
    sim.compile();
    const roms = new Map<string, { n: number; lo: number; hi: number }>();
    const elsewhere: number[] = [];
    const nowhere: number[] = [];
    for (const [a, n] of code)
      for (let k = 0; k < n; k++) {
        const addr = (a + k) & 0xffff;
        const w = sim.whoAt(addr);
        const part = w && this.project!.parts.find((x) => x.ref === w.ref);
        if (w && part?.type === 'rom28c256') {
          sim.memories.get(part.id)![w.local] = image[addr];
          const s = roms.get(w.ref) ?? { n: 0, lo: w.local, hi: w.local };
          s.n++;
          s.lo = Math.min(s.lo, w.local);
          s.hi = Math.max(s.hi, w.local);
          roms.set(w.ref, s);
        } else if (w) elsewhere.push(addr);
        else nowhere.push(addr);
      }
    if (!was) sim.setPower(false);
    sim.uncompile();
    const parts = [...roms].map(([ref, s]) => `${s.n} bytes into ${ref} (its ${hex(s.lo, 4)}–${hex(s.hi, 4)})`);
    const warn = [
      elsewhere.length ? `${elsewhere.length} bytes from ${hex(elsewhere[0], 4)} are in RAM, not ROM: they won’t be there when it starts` : '',
      nowhere.length ? `no chip answers ${hex(nowhere[0], 4)}${nowhere.length > 1 ? ` (and ${nowhere.length - 1} more)` : ''}: check the address decoder and the ROM’s /CE, /OE` : '',
    ].filter(Boolean);
    this.burnNote = [parts.length ? `Burned ${parts.join(', ')}.` : 'Nothing was burned: no ROM answers where the program is.', ...warn].join(' ');
  }

  /** Assembles if need be, switches on, and runs. */
  async run() {
    const p = this.project;
    if (!p || !this.sim) return;
    if (this.burnedSource !== p.source && !(await this.assemble())) return;
    if (!this.power) this.setPower(true);
    this.start();
  }

  private start() {
    if (this.running || !this.sim) return;
    this.running = true;
    this.queue = [];
    let last = performance.now();
    let acc = 0;
    const tick = (now: number) => {
      if (!this.running || !this.sim) return;
      const sim = this.sim;
      const dt = Math.min(0.1, (now - last) / 1000);
      last = now;
      const hz = this.effectiveHz;
      const why = sim.blocked();
      if (why || !hz) {
        // Not running (no clock, held in reset…): keep looking, the wiring may change.
        if (sim.compiled) sim.uncompile();
        this.updateView();
      } else if (hz <= WATCH_HZ) {
        // Watching: each bus cycle is shown for as long as its T-states take at this clock.
        if (sim.compiled) sim.uncompile();
        sim.record = true;
        acc += hz * dt;
        let shown: Snapshot | null = null;
        for (let guard = 0; guard < 2000; guard++) {
          if (!this.queue.length) {
            sim.snapshots = [];
            const t = sim.stepOne();
            if (!t) break;
            const snaps = sim.snapshots.splice(0);
            if (!snaps.length) snaps.push({ access: null, val: sim.val.slice(), str: sim.str.slice(), drv: sim.drv.slice(), t: sim.tstates });
            for (const s of snaps) this.queue.push({ snap: s, cost: t / snaps.length });
            this.noteCycles(snaps);
          }
          if (!this.queue.length || acc < this.queue[0].cost) break;
          acc -= this.queue[0].cost;
          shown = this.queue.shift()!.snap;
        }
        if (shown) this.showSnap(shown);
      } else {
        sim.record = false;
        if (!sim.compiled) sim.compile();
        // As much as fits in a frame (the browser keeps the rest of RAD responsive).
        const started = performance.now();
        const want = hz * dt;
        let done = 0;
        while (done < want && performance.now() - started < 12) done += sim.run(Math.min(want - done, 50_000));
        this.speed = dt > 0 ? done / dt : 0;
        sim.refresh();
        this.updateView();
      }
      this.frame = requestAnimationFrame(tick);
    };
    this.frame = requestAnimationFrame(tick);
  }

  stop() {
    this.running = false;
    cancelAnimationFrame(this.frame);
    this.queue = [];
    if (this.sim) this.sim.record = false;
    this.speed = 0;
  }

  /** One instruction, with its bus cycles shown one after another. */
  async step() {
    const sim = this.sim;
    if (!sim) return;
    if (this.burnedSource !== this.project?.source && !(await this.assemble())) return;
    if (!this.power) this.setPower(true);
    this.stop();
    const why = sim.blocked();
    if (why) {
      store.toast('info', 'The CPU can’t run', why);
      return;
    }
    sim.uncompile();
    sim.record = true;
    sim.snapshots = [];
    sim.stepOne();
    const snaps = sim.snapshots.splice(0);
    sim.record = false;
    this.noteCycles(snaps);
    for (const s of snaps) {
      this.showSnap(s);
      await sleep(Math.min(400, 1200 / snaps.length));
    }
    this.updateView();
  }

  reset() {
    const sim = this.sim;
    if (!sim) return;
    this.queue = [];
    sim.inReset = true;
    this.terminal += this.terminal && !this.terminal.endsWith('\n') ? '\n' : '';
    this.updateView();
  }

  /** The reset button (or any button) held down on the canvas. */
  press(id: string, down: boolean) {
    const sim = this.sim;
    if (!sim) return;
    if (down) sim.pressed.add(id);
    else sim.pressed.delete(id);
    sim.uncompile();
    sim.refresh();
    this.updateView();
  }

  toggle(id: string) {
    const part = this.project?.parts.find((p) => p.id === id);
    if (!part) return;
    part.props.on = !part.props.on;
    this.saveSoon();
    if (this.sim) {
      const live = this.sim.circuit.parts.find((p) => p.id === id);
      if (live) live.props.on = part.props.on;
      this.sim.uncompile();
      this.sim.refresh();
    }
    this.updateView();
  }

  type(text: string) {
    const sim = this.sim;
    const term = this.project?.parts.find((p) => p.type === 'terminal');
    if (!sim || !term) return;
    for (const ch of text.replace(/\n/g, '\r')) sim.serialIn(term.id, ch.charCodeAt(0) & 0xff);
  }

  private serialByte(b: number) {
    if (b === 13) return;
    if (b === 8 || b === 127) this.terminal = this.terminal.slice(0, -1);
    else this.terminal = (this.terminal + String.fromCharCode(b)).slice(-20_000);
  }

  private showSnap(s: Snapshot) {
    const sim = this.sim!;
    this.view = { val: s.val, str: s.str, drv: s.drv, access: s.access, lights: sim.lights() };
    this.status();
  }

  private noteCycles(snaps: Snapshot[]) {
    const words: Record<string, string> = { fetch: 'fetch', read: 'read', write: 'write', in: 'in', out: 'out', int: 'interrupt' };
    const lines = snaps
      .filter((s) => s.access)
      .map((s) => {
        const a = s.access!;
        const io = a.kind === 'in' || a.kind === 'out';
        return `${words[a.kind].padEnd(9)} ${io ? `port ${hex(a.addr & 0xff)}` : hex(a.addr, 4)}  ${hex(a.data)}  ${a.who ?? '—'}`;
      });
    this.cycles = [...this.cycles, ...lines].slice(-40);
  }

  /** Registers and the line being run. */
  private status() {
    const z = this.sim?.core;
    if (!z) return;
    const r = z.regs;
    this.regs = { pc: r.pc, sp: r.sp, af: r.af, bc: r.bc, de: r.de, hl: r.hl, ix: r.ix, iy: r.iy, i: r.i, im: r.im, iff1: r.iff1, halted: r.halted, t: this.sim!.tstates };
    this.line = this.lineOf.get(r.pc) ?? null;
  }

  updateView() {
    const sim = this.sim;
    if (!sim) return;
    this.view = { val: sim.val.slice(), str: sim.str.slice(), drv: sim.drv.slice(), access: sim.lastAccess, lights: sim.lights() };
    // The checks look at every chip: twice a second is plenty while it runs.
    if (!this.running || performance.now() - this.checkedAt > 500) {
      this.problems = sim.problems();
      this.checkedAt = performance.now();
    }
    this.status();
  }

  // ---------------------------------------------------------------- for Gilgamesh

  /** "U1.6" or "U1 CLK" → a pin. */
  pinRef(s: string): { part: string; pin: number } | null {
    const m = s.trim().match(/^([A-Za-z]+\d+)[.\s:]+(.+)$/);
    const part = m && this.project?.parts.find((p) => p.ref.toUpperCase() === m[1].toUpperCase());
    if (!m || !part) return null;
    const def = PART[part.type];
    const want = m[2].trim().toUpperCase();
    const pin = def.pins.find((p) => String(p.n) === want) ?? def.pins.find((p) => p.name.toUpperCase() === want) ?? def.pins.find((p) => p.name.toUpperCase().replace(/^\//, '') === want.replace(/^\//, ''));
    return pin ? { part: part.id, pin: pin.n } : null;
  }

  partByRef(ref: string) {
    return this.project?.parts.find((p) => p.ref.toUpperCase() === ref.trim().toUpperCase()) ?? null;
  }

  /** A part type from words ("z80", "ram", "74hc138", "led"). */
  partType(words: string): string | null {
    const w = words.toLowerCase().replace(/[^a-z0-9 ]/g, ' ');
    const hit = PARTS.find((p) => p.type === w.trim()) ?? PARTS.find((p) => p.label.toLowerCase().replace(/[^a-z0-9 ]/g, ' ').includes(w.trim()));
    if (hit) return hit.type;
    const keys: [RegExp, string][] = [[/cpu|z80/, 'z80'], [/rom|eeprom/, 'rom28c256'], [/ram|sram/, 'ram62256'], [/sio|serial/, 'sio'], [/ctc|timer/, 'ctc'], [/gal|pld/, 'gal16v8'], [/138|decoder/, 'hc138'], [/273|latch/, 'hc273'], [/245|buffer/, 'hc245'], [/nand|00/, 'hc00'], [/ or|32/, 'hc32'], [/and|08/, 'hc08'], [/not|inverter|04/, 'hc04'], [/osc|crystal|clock/, 'osc'], [/led bar|bar/, 'ledbar'], [/led/, 'led'], [/network|array/, 'rnet'], [/resist/, 'resistor'], [/capac/, 'capacitor'], [/button/, 'button'], [/switch/, 'switch'], [/terminal|usb/, 'terminal'], [/power|psu|supply/, 'psu']];
    return keys.find(([re]) => re.test(w))?.[1] ?? null;
  }

  describe(): string {
    const p = this.project;
    if (!p || store.route.kind !== 'z80lab') return '';
    const sim = this.sim;
    const parts = p.parts.map((x) => `${x.ref} ${PART[x.type]?.label ?? x.type}${x.type === 'osc' ? ` ${(Number(x.props.hz) / 1e6).toFixed(4).replace(/\.?0+$/, '')} MHz` : x.type === 'resistor' ? ` ${x.props.ohms} Ω` : ''}`);
    // Nets with more than one pin, shortest first, so the interesting ones fit.
    const nets: string[] = [];
    if (sim) {
      const seen = new Set<number>();
      for (let n = 0; n < sim.netPins.length; n++) if (sim.netPins[n].length > 1 && !seen.has(n)) (seen.add(n), nets.push(sim.netName(n)));
    }
    nets.sort((a, b) => a.length - b.length);
    const gal = p.parts.filter((x) => x.type === 'gal16v8').map((x) => `${x.ref} pins (1–20): ${x.props.names}\n${x.props.equations}`);
    const sel = this.selected.map((id) => p.parts.find((x) => x.id === id)).filter(Boolean) as Placed[];
    const r = this.regs;
    const hz = this.effectiveHz;
    const run = !this.power
      ? 'The power is off.'
      : this.running
        ? `Running with the clock at ${hz >= 1e6 ? `${(hz / 1e6).toFixed(2)} MHz` : `${hz} Hz`}${this.watching ? ' (slowed down: the owner is watching every bus cycle travel the wires)' : ''}.`
        : 'The power is on; the program is stopped.';
    const lines = p.source.split('\n');
    return [
      `THE OWNER IS BUILDING A Z80 COMPUTER in RAD's Z80 Lab (“${p.title}”): parts on a canvas wired pin to pin, simulated pin by pin, with a program in Z80 assembler (sjasmplus) burned into its ROM. The ready-made design is the Easy Z80 by Sergey Kiselev, simplified: 10 MHz Z80, 32K ROM at 0000h–7FFFh, 32K RAM at 8000h–FFFFh, address decode in a GAL16V8, SIO/0 at 80h–83h (channel A data 80h, control 81h: RR0 bit 0 = received, bit 2 = ready to send) to a USB-serial terminal, CTC at 88h–8Bh, and an LED port (74HC273) at 40h.`,
      `PARTS: ${parts.join('; ')}.`,
      nets.length ? `WIRING (each line is one net, the pins joined together):\n${nets.slice(0, 140).join('\n')}${nets.length > 140 ? `\n(and ${nets.length - 140} more nets)` : ''}` : 'Nothing is wired yet.',
      gal.length ? `GAL EQUATIONS:\n${gal.join('\n')}` : '',
      sel.length ? `SELECTED ON THE CANVAS: ${sel.map((x) => `${x.ref} (${PART[x.type].label}: ${PART[x.type].pins.map((q) => `${q.n} ${q.name}`).join(', ')})`).join('; ')}` : '',
      this.problems.length ? `PROBLEMS THE CHECKER FOUND:\n${this.problems.map((x) => `- [${x.level}] ${x.text}`).join('\n')}` : 'The checker finds no problems.',
      sim?.notes.length ? `WHAT HAPPENED WHILE RUNNING (newest last):\n${sim.notes.slice(-8).join('\n')}` : '',
      run,
      r ? `CPU: PC=${hex(r.pc, 4)} SP=${hex(r.sp, 4)} AF=${hex(r.af, 4)} BC=${hex(r.bc, 4)} DE=${hex(r.de, 4)} HL=${hex(r.hl, 4)} IM ${r.im}, interrupts ${r.iff1 ? 'on' : 'off'}${r.halted ? ', halted' : ''}; ${r.t} T-states so far.${this.line ? ` It is at line ${this.line}: ${lines[this.line - 1]?.trim()}` : ''}` : '',
      this.view?.access ? `THE LAST BUS CYCLE: ${this.cycles.at(-1) ?? ''}` : '',
      this.asm && !this.asm.ok ? `THE ASSEMBLER'S ERRORS:\n${this.asm.errors.map((e) => `line ${e.line}: ${e.message}`).join('\n')}` : this.burnNote ? `THE LAST BURN: ${this.burnNote}` : '',
      this.terminal ? `THE TERMINAL SHOWS (last part):\n"""\n${this.terminal.slice(-800)}\n"""` : '',
      `THE PROGRAM (main.asm):\n"""\n${p.source.slice(0, 5000)}\n"""`,
    ]
      .filter(Boolean)
      .join('\n\n');
  }
}

export const lab = new Z80Lab();
export { PART, PARTS, pinPos, bodySize };
