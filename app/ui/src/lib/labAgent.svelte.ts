/** The lab's AI (Spectrum and C64): explains how a game works and helps change it. It reads code
 *  through RAD (view ranges, references, memory), writes notes (labels, comments) and patches that
 *  RAD assembles, applies and test-runs on a hidden machine. */
import { rad } from './api';
import type { LabMachine } from './labMachine';
import type { LabNotes, LabSummary } from './types';
import { hex4, parseAddr, type LabPatch } from './zxlab';

export type LabItem =
  | { kind: 'user'; text: string }
  | { kind: 'say'; text: string }
  | { kind: 'tool'; title: string; detail?: string; ok?: boolean }
  | { kind: 'error'; text: string };

type Turn = { role: 'user' | 'assistant'; content: string };

export interface LabIO {
  id: string;
  machine: LabMachine;
  name(): string;
  summary(): LabSummary | null;
  notes(): LabNotes;
  patches(): LabPatch[];
  /** What the user is looking at in the Code tab (listing text), if anything. */
  focus(): string;
  setNotes(n: LabNotes): Promise<void>;
  addPatch(p: LabPatch): Promise<void>;
  persist(turns: Turn[]): void;
}

const MAX_ROUNDS = 10;

function systemPrompt(io: LabIO): string {
  const s = io.summary();
  const n = io.notes();
  const labels = Object.entries(n.labels).map(([a, l]) => `${hex4(+a)} ${l}`).join(', ');
  const comments = Object.entries(n.comments).slice(0, 80).map(([a, c]) => `${hex4(+a)}: ${c}`).join('\n');
  const patches = io.patches().map((p) => `${hex4(p.addr)} ${p.enabled ? 'on' : 'off'} — ${p.note || p.bytes}`).join('\n');
  const m = io.machine;
  const c64 = m.id === 'c64';
  const hardware = c64
    ? `Chip registers its own code used: ${(s?.hardware ?? []).map((h) => `${hex4(h.addr)} ${h.name ?? h.chip ?? ''}${h.reads ? ` read ×${h.reads}` : ''}${h.writes ? ` written ×${h.writes}` : ''}`).join(', ') || 'none'}.
${s?.files?.length ? `Files: ${s.files.join(', ')}.` : ''}${s?.sprites?.length ? `\nSprites on at the end: ${s.sprites.map((x) => `#${x.n} data at ${hex4(x.addr)}${x.multicolor ? ' (multicolour)' : ''}`).join(', ')}.` : ''}
Screen at ${s?.screen_addr != null ? hex4(s.screen_addr) : '?'}, character set at ${s?.charset ? `${hex4(s.charset.addr)}${s.charset.rom ? ' (the ROM font)' : ''}` : '?'}${s?.bitmap != null ? `, bitmap at ${hex4(s.bitmap)}` : ''}.`
    : `Ports used: ${Object.entries(s?.ports ?? {}).map(([p, c]) => `${p} ×${c}`).join(', ') || 'none'}.
Tape: ${(s?.tape ?? []).map((b) => (b.header ? `header "${b.header.name}" (${['program', 'number array', 'char array', 'bytes'][b.header.kind] ?? b.header.kind}, ${b.header.length} bytes, ${b.header.kind === 3 ? 'to ' + hex4(b.header.param1) : 'line ' + b.header.param1})` : `${b.length} bytes${b.speed !== 'standard' ? ' (custom loader)' : ''}`)).join('; ') || 'a snapshot'}.`;
  const overview = s
    ? `${c64 ? 'Commodore 64.' : `Model ${s.model}K.`} Code entry: ${s.entry != null ? hex4(s.entry) : 'unknown'}. Program range ${hex4(s.range[0])}–${hex4(s.range[1])}. ${s.functions} functions found, ${s.code_bytes} bytes of code seen running, ${s.reachable_bytes} more reachable, ${s.variables} bytes that change (variables).
${hardware}
${c64 ? 'KERNAL' : 'ROM'} routines called: ${s.rom_calls.map((r) => `${hex4(r.addr)} ${r.name ?? ''} ×${r.count}`).join(', ') || 'none'}.
Text found: ${s.strings.slice(0, 25).map((t) => `${hex4(t.addr)} "${t.text}"`).join('; ')}.
${s.basic ? `BASIC program:\n${s.basic.slice(0, 3000)}` : 'No BASIC program.'}`
    : '(not analysed yet)';
  return `You are a friendly expert in ${c64 ? 'Commodore 64' : 'ZX Spectrum'} reverse engineering, helping the user understand and modify "${io.name()}", a ${m.name} game, in RAD's ${m.title}. RAD ran the game in an emulator and traced which addresses really execute, so the disassembly separates code (seen running), reachable code (not run yet) and data. Explain clearly, like a good teacher: what each part does, how the hardware is used, and why the programmers did it that way in the 1980s.

What you can do — reply with fenced blocks like these (you get the results back, then continue):
\`\`\`view
8000-8100
\`\`\`  (disassembly of a range; up to 300 lines)
\`\`\`xrefs
8123
\`\`\`  (instructions that jump to, call or point at an address)
\`\`\`memory
8123 32
\`\`\`  (hex dump: address and length)
\`\`\`notes
label 8000 main_loop
comment 8003 Waits for the next frame
\`\`\`  (name addresses and explain lines; the user sees them in the code view — add them as you understand the code)
\`\`\`patch 8123 Infinite lives
${c64 ? '        nop\n        nop' : '    nop\n    nop'}
\`\`\`  (${m.assembler} — the program's labels can be used; it's added to the user's patches, applied to the game and test-run for 3 seconds; you get the result)
\`\`\`poke 8123 Start with 9 lives
09
\`\`\`  (raw hex bytes written at an address, added as a patch)
- Addresses are hex. Look before you guess: use view/xrefs/memory to check code before explaining or patching it.
- Patches must not cut an instruction in half: replace whole instructions (fill leftovers with nop). Keep the same length when changing code inside a routine.${c64 ? "\n- Patches go into the snapshot RAD took after the game started, so changes to start-up code only show when the game restarts (e.g. after game over)." : ''}
- The game belongs to the user; help them learn and have fun with their own copy.

${m.hardware}

The game:
${overview}

Names so far: ${labels || 'none (sub_XXXX = a function, l_XXXX = a jump target, d_XXXX = data)'}
${comments ? `Comments so far:\n${comments}` : ''}
${patches ? `Patches:\n${patches}` : ''}
${io.focus() ? `\nThe user is looking at:\n${io.focus()}` : ''}`;
}

const BLOCK = /```(view|xrefs|memory|notes|patch|poke)([^\n]*)\n([\s\S]*?)```/g;

export class LabAgent {
  items = $state<LabItem[]>([]);
  busy = $state(false);
  status = $state('');
  private turns: Turn[] = [];
  private stopped = false;

  constructor(private io: LabIO) {}

  restore(turns: Turn[]) {
    this.turns = turns.filter((t) => t && typeof t.content === 'string');
    this.items = [];
    for (const t of this.turns) {
      if (t.role === 'user' && !t.content.startsWith('[rad]')) this.items.push({ kind: 'user', text: t.content });
      if (t.role === 'assistant') {
        const say = t.content.replace(BLOCK, '').trim();
        if (say) this.items.push({ kind: 'say', text: say });
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
    const last = this.turns[this.turns.length - 1];
    if (last && last.role === role && role === 'user') last.content += `\n\n${content}`;
    else this.turns.push({ role, content });
  }

  /** The assistant's explanations (for saving to the vault). */
  explanations(): string[] {
    return this.turns.filter((t) => t.role === 'assistant').map((t) => t.content.replace(BLOCK, '').trim()).filter((t) => t.length > 40);
  }

  private async listing(from: number, to: number): Promise<string> {
    const lines = await this.io.machine.listing(this.io.id, from, Math.min(to, from + 0x800), this.io.notes());
    return lines
      .slice(0, 300)
      .map((l) => `${l.label ? l.label + ':\n' : ''}${hex4(l.addr).slice(1)}  ${l.text.padEnd(28)}${l.kind === 'reachable' ? ' [not seen running]' : l.kind !== 'code' ? ` [${l.kind}${l.variable ? ', changes' : ''}]` : ''}${l.comment ? ' ; ' + l.comment : ''}`)
      .join('\n');
  }

  /** Carries out one action block; returns what to tell the model. */
  private async act(kind: string, head: string, body: string): Promise<string> {
    const io = this.io;
    switch (kind) {
      case 'view': {
        const [a, b] = body.trim().split(/[-\s]+/);
        const from = parseAddr('$' + a.replace(/^\$|^0x/, ''));
        const to = parseAddr('$' + (b ?? '').replace(/^\$|^0x/, '')) ?? (from ?? 0) + 0x100;
        if (from == null) return `view: "${body.trim()}" isn't an address range`;
        this.items.push({ kind: 'tool', title: `Read the code at ${hex4(from)}–${hex4(to)}` });
        return `Disassembly ${hex4(from)}–${hex4(to)}:\n${await this.listing(from, to + 1)}`;
      }
      case 'xrefs': {
        const a = parseAddr('$' + body.trim().replace(/^\$|^0x/, ''));
        if (a == null) return 'xrefs: not an address';
        const r = await io.machine.xrefs(io.id, a, io.notes());
        this.items.push({ kind: 'tool', title: `Looked up who uses ${hex4(a)}`, detail: `${r.length} references` });
        return `References to ${hex4(a)}:\n${r.map((x) => `${hex4(x.addr)}  ${x.text}`).join('\n') || '(none in the program)'}`;
      }
      case 'memory': {
        const [a, n] = body.trim().split(/\s+/);
        const from = parseAddr('$' + a.replace(/^\$|^0x/, ''));
        const len = Math.min(parseInt(n ?? '64', 10) || 64, 512);
        if (from == null) return 'memory: not an address';
        const b = Uint8Array.from(atob(await io.machine.memory(io.id, from, len)), (c) => c.charCodeAt(0));
        const rows: string[] = [];
        for (let i = 0; i < b.length; i += 16) {
          const chunk = [...b.slice(i, i + 16)];
          rows.push(`${hex4(from + i).slice(1)}  ${chunk.map((x) => x.toString(16).padStart(2, '0')).join(' ')}  ${chunk.map((x) => (x >= 32 && x < 127 ? String.fromCharCode(x) : '.')).join('')}`);
        }
        this.items.push({ kind: 'tool', title: `Read ${len} bytes at ${hex4(from)}` });
        return `Memory at ${hex4(from)}:\n${rows.join('\n')}`;
      }
      case 'notes': {
        const notes = structuredClone($state.snapshot(io.notes())) as LabNotes;
        let n = 0;
        for (const line of body.split('\n')) {
          const m = line.trim().match(/^(label|comment)\s+\$?([0-9a-fA-F]{1,4})h?\s+(.+)$/);
          if (!m) continue;
          const addr = parseInt(m[2], 16);
          if (m[1] === 'label') {
            const name = m[3].trim().replace(/[^A-Za-z0-9_]/g, '_').replace(/^([0-9])/, '_$1').slice(0, 40);
            if (name) notes.labels[addr] = name;
          } else notes.comments[addr] = m[3].trim().slice(0, 300);
          n++;
        }
        await io.setNotes(notes);
        this.items.push({ kind: 'tool', title: `Added ${n} note${n === 1 ? '' : 's'} to the code`, ok: true });
        return `Saved ${n} notes.`;
      }
      case 'patch':
      case 'poke': {
        const m = head.trim().match(/^\$?([0-9a-fA-F]{1,4})h?\s*(.*)$/);
        if (!m) return `${kind}: the first line needs an address, like "${kind} 8123 What it does"`;
        const addr = parseInt(m[1], 16);
        const note = m[2].trim() || (kind === 'poke' ? 'Poke' : 'Patch');
        let bytes = '';
        let source = '';
        if (kind === 'poke') {
          bytes = body.replace(/[^0-9a-fA-F]/g, '').toLowerCase();
          if (!bytes || bytes.length % 2) return 'poke: give the bytes as hex pairs';
        } else {
          source = body.replace(/\s+$/, '');
          const r = await io.machine.assemblePatch(io.id, addr, source, io.notes());
          if (!r.ok || !r.bytes) {
            this.items.push({ kind: 'tool', title: `Patch at ${hex4(addr)} didn't assemble`, detail: r.messages, ok: false });
            return `The patch didn't assemble:\n${r.messages}`;
          }
          if (r.splits != null) {
            this.items.push({ kind: 'tool', title: `Patch at ${hex4(addr)} would cut the instruction at ${hex4(r.splits)} in half`, ok: false });
            return `The patch (${r.len} bytes, ${hex4(addr)}–${hex4(r.end! - 1)}) ends inside the instruction at ${hex4(r.splits)}: pad it with nops to replace whole instructions.`;
          }
          bytes = r.bytes;
        }
        const patch: LabPatch = { id: `p${Date.now()}`, addr, bytes, note, source, enabled: true };
        await io.addPatch(patch);
        const all = io.patches().filter((p) => p.enabled).map((p) => ({ addr: p.addr, bytes: p.bytes }));
        const b = await io.machine.build(io.id, all, true);
        const c = b.check;
        const result = !c ? 'built' : c.crashed ? `the patched game crashed (${io.machine.id === 'c64' ? 'the CPU stopped on a JAM instruction' : 'it reset to the start-up screen'})` : c.problem ? `the patched game stopped with "${c.problem}"` : `the patched game ran 3 s without crashing`;
        this.items.push({ kind: 'tool', title: `${note}: ${bytes.length / 2} bytes at ${hex4(addr)} — ${result}`, detail: c?.screen.join('\n'), ok: !c?.crashed });
        return `Patch "${note}" added (${bytes.length / 2} bytes at ${hex4(addr)}: ${bytes}); ${result}.${c?.screen.length ? `\nScreen after 3 s:\n${c.screen.join('\n')}` : ''}`;
      }
    }
    return '';
  }

  async ask(text: string) {
    if (this.busy || !text.trim()) return;
    this.busy = true;
    this.stopped = false;
    this.items.push({ kind: 'user', text });
    this.push('user', text);
    try {
      for (let round = 0; round < MAX_ROUNDS && !this.stopped; round++) {
        this.status = round ? 'Reading the code…' : 'Thinking…';
        const r = await rad.assist(systemPrompt(this.io), this.turns.slice(-30));
        if (this.stopped) break;
        this.push('assistant', r.content);
        const say = r.content.replace(BLOCK, '').trim();
        if (say) this.items.push({ kind: 'say', text: say });
        const blocks = [...r.content.matchAll(BLOCK)];
        if (!blocks.length) break;
        const results: string[] = [];
        for (const m of blocks.slice(0, 6)) {
          this.status = `Working: ${m[1]}…`;
          try {
            results.push(await this.act(m[1], m[2], m[3]));
          } catch (e: any) {
            results.push(`${m[1]} failed: ${e?.message ?? e}`);
          }
        }
        this.push('user', `[rad]\n${results.join('\n\n')}`);
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
