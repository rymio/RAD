// PhotoCraft, the image editor, in RAD (app/src-tauri/src/photocraft.rs launches and drives it).
// The owner draws in the app; Gilgamesh ([edit: …]) and bots (edit_image) can edit too, through
// an agent that works PhotoCraft's commands:
//
//   instruction → the model (POST /v1/assist) → one action as JSON → a PhotoCraft command, a
//   look at the document (its layers), a picture of it, or a brush stroke → the result → …
//
// "app" is the PhotoCraft window the owner sees (edits happen in front of them); "headless" is
// its engine with no window, for work in the background. Both only see one folder, ~/Pictures/RAD
// PhotoCraft; files are copied in and out of it.

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { rad } from './api';

export type Target = 'app' | 'headless';

export interface PcStatus {
  supported: boolean;
  installed: boolean;
  version: string | null;
  pinned: string;
  bytes: number;
  running: boolean;
  work_dir: string;
  downloading: { phase: string; done: number; total: number } | null;
}

export interface PcFile {
  name: string;
  size: number;
  modified: number;
}

export interface PcStep {
  at: number;
  kind: 'user' | 'say' | 'act' | 'result' | 'error' | 'done';
  text: string;
  ok?: boolean;
}

export interface PcJob {
  id: string;
  instruction: string;
  target: Target;
  from: 'owner' | 'gilgamesh' | 'bot';
  status: 'running' | 'done' | 'failed' | 'stopped';
  steps: PcStep[];
  result: string;
  /** What it saved (a file of the work folder). */
  output: string | null;
  started: number;
  ended: number | null;
}

const MAX_STEPS = 30;
const clip = (s: string, n: number) => (s.length > n ? `${s.slice(0, n)}…` : s);

function dataUrl(buf: ArrayBuffer, mime = 'image/png') {
  const bytes = new Uint8Array(buf);
  let bin = '';
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return `data:${mime};base64,${btoa(bin)}`;
}

/** The first JSON action in a reply, and the words before it. */
function parseAction(reply: string): { say: string; action: Record<string, any> | null } {
  const text = reply.replace(/<think>[\s\S]*?(<\/think>|$)/g, '');
  const fence = text.match(/```(?:json)?\s*\n?([\s\S]*?)```/);
  let raw = fence?.[1]?.trim() ?? null;
  let rest = fence ? text.slice(0, fence.index) + text.slice(fence.index! + fence[0].length) : text;
  if (!raw) {
    const i = text.indexOf('{"do"');
    if (i >= 0) {
      raw = text.slice(i, text.lastIndexOf('}') + 1);
      rest = text.slice(0, i);
    }
  }
  let action: Record<string, any> | null = null;
  try {
    const v = raw ? JSON.parse(raw) : null;
    if (v && typeof v.do === 'string') action = v;
  } catch {
    /* not JSON: a final answer */
  }
  return { say: rest.replace(/\s+/g, ' ').trim(), action };
}

function systemPrompt(target: Target, catalogue: string) {
  return `You are PhotoCraft's assistant in RAD. PhotoCraft is a Photoshop-like image editor (layers, masks, adjustment layers, layer styles, type, vector shapes, brushes, filters, selections). You carry out the owner's editing request by running PhotoCraft's commands, step by step.
${target === 'app' ? 'You work in the PhotoCraft window the owner is looking at: they see every change as you make it.' : 'You work in PhotoCraft\'s engine in the background (no window): the owner sees the result when you save it.'}

EVERY REPLY: one short sentence saying what you're doing, then exactly ONE action as JSON in a \`\`\`json block. You get its result back.

ACTIONS
{"do":"commands","filter":"gaussian"}  find commands (id, label and their params) by part of their id or label. Do this before running a command whose params you don't know.
{"do":"run","command":"filter.blur.gaussianBlur","params":{…}}  run a command (params as its doc says; omit for defaults). Commands never open dialogs.
{"do":"inspect"}  the document: size, layers (ids, names, kinds), selection, history
{"do":"look"}  a picture of the work (only if your model can see images)
${target === 'app' ? '{"do":"draw","tool":"brush","points":[[x,y],[x,y],…],"pressure":0.7}  a stroke with a painting tool (brush, pencil, eraser…) in document pixels, through the points in order. Set the colour and brush first with commands (tools.setColors, tools.setBrush).\n' : ''}{"do":"open","file":"name.jpg"}  open a file of the work folder
{"do":"save","file":"name.png"}  save the active document to the work folder (format from the extension: .png, .jpg, .psd, .pcraft…)
{"do":"done","summary":"…","file":"name.png"}  finish: what you did (and the file you saved, if any)

RULES
- Prefer adjustment layers, layer styles, shape and type layers to destroying pixels, so the owner can change them later.
- No document open and the request needs one: create it with file.new (its params give the size).
- Save under a NEW name (e.g. "photo-edited.png") unless told to overwrite. ${target === 'headless' ? 'Always save the result before done.' : 'Save only if the owner asked for a file.'}
- Read errors and fix the params; don't repeat a failing action. Stay on the request.

COMMAND IDS (grouped; use commands to see params):
${catalogue}`;
}

class Photocraft {
  status = $state<PcStatus | null>(null);
  files = $state<PcFile[]>([]);
  jobs = $state<PcJob[]>([]);
  /** What the running job is doing. */
  now = $state('');
  /** The latest picture of the work. */
  picture = $state<string | null>(null);
  error = $state<string | null>(null);

  private stopping = new Set<string>();
  private catalogues = new Map<Target, string>();

  async refresh() {
    try {
      this.status = await invoke<PcStatus>('photocraft_status');
      this.files = await invoke<PcFile[]>('photocraft_files');
    } catch (e: any) {
      this.error = e?.message ?? String(e);
    }
  }

  async install() {
    this.error = null;
    const un = await listen<any>('photocraft-progress', (e) => {
      if (this.status) this.status.downloading = e.payload?.done === true ? null : e.payload;
    });
    try {
      await invoke('photocraft_install');
    } finally {
      un();
      await this.refresh();
    }
  }

  async open(name?: string) {
    await invoke('photocraft_open', { name: name ?? null });
    await this.refresh();
  }

  reveal() {
    return invoke('photocraft_reveal');
  }

  /** Copies a file (an absolute path on this Mac) into the work folder; its name there. */
  importFile(path: string) {
    return invoke<string>('photocraft_import', { path });
  }

  call(target: Target, method: string, params: Record<string, unknown> = {}) {
    return invoke<any>('photocraft_call', { target, method, params });
  }

  async look(target: Target) {
    const png = await invoke<ArrayBuffer>('photocraft_picture', { target });
    this.picture = dataUrl(png);
    return this.picture;
  }

  busy() {
    return this.jobs.find((j) => j.status === 'running') ?? null;
  }

  stop() {
    const j = this.busy();
    if (j) this.stopping.add(j.id);
  }

  /** Command ids grouped by their first part ("filter: blur.gaussianBlur, …"), fetched once. */
  private async catalogue(target: Target) {
    const have = this.catalogues.get(target);
    if (have) return have;
    const list: { id: string }[] = await this.call(target, 'engine.commands', {});
    const groups = new Map<string, string[]>();
    for (const c of list) {
      const [head, ...rest] = c.id.split('.');
      groups.set(head, [...(groups.get(head) ?? []), rest.join('.') || '(itself)']);
    }
    const text = [...groups].map(([g, ids]) => `${g}: ${ids.join(', ')}`).join('\n');
    this.catalogues.set(target, text);
    return text;
  }

  private async commandsMatching(target: Target, filter: string) {
    const list: { id: string; label: string; params: string; enabled: boolean; disabledReason: string | null }[] = await this.call(target, 'engine.commands', { filter });
    if (!list.length) return `No commands match “${filter}”.`;
    return list
      .slice(0, 15)
      .map((c) => `${c.id} — ${c.label}${c.enabled ? '' : ` (can't run now: ${c.disabledReason})`}\n  params: ${clip(c.params || '(none)', 500)}`)
      .join('\n') + (list.length > 15 ? `\n… ${list.length - 15} more: narrow the filter.` : '');
  }

  private async inspect(target: Target) {
    try {
      const d = await this.call(target, 'engine.execute', { command: 'document.inspect', params: {} });
      return `THE DOCUMENT:\n${clip(JSON.stringify(d), 5000)}`;
    } catch (e: any) {
      return `No document is open (${e?.message ?? e}).`;
    }
  }

  /**
   * Carries out an editing request. `file`: a file of the work folder to open first; `out`: the
   * name to save the result under if the agent didn't.
   */
  async edit(instruction: string, opts: { target: Target; from: PcJob['from']; file?: string; out?: string }): Promise<PcJob> {
    await this.refresh();
    if (!this.status?.installed) throw new Error('PhotoCraft isn’t installed yet: install it in the PhotoCraft view.');
    if (this.busy()) throw new Error('PhotoCraft is already busy with another edit');
    const job: PcJob = { id: crypto.randomUUID().slice(0, 8), instruction, target: opts.target, from: opts.from, status: 'running', steps: [], result: '', output: null, started: Date.now(), ended: null };
    this.jobs.unshift(job);
    const j = this.jobs[0];
    const step = (kind: PcStep['kind'], text: string, ok?: boolean) => j.steps.push({ at: Date.now(), kind, text, ok });
    step('user', instruction);
    const t = opts.target;
    try {
      this.now = t === 'app' ? 'Opening PhotoCraft…' : 'Starting PhotoCraft’s engine…';
      if (opts.file) await this.call(t, t === 'app' ? 'app.open' : 'doc.open', { path: opts.file });
      const turns: { role: 'user' | 'assistant'; content: string; images?: string[] }[] = [
        { role: 'user', content: `REQUEST: ${instruction}${opts.file ? `\n(The file ${opts.file} is open.)` : ''}${opts.out ? `\nSave the result as ${opts.out}.` : ''}\n\n${await this.inspect(t)}` },
      ];
      const system = systemPrompt(t, await this.catalogue(t));
      for (let i = 0; i < MAX_STEPS; i++) {
        if (this.stopping.has(j.id)) {
          j.status = 'stopped';
          j.result = 'Stopped.';
          return j;
        }
        this.now = 'Thinking…';
        const msgs = turns.length > 30 ? [turns[0], ...turns.slice(-28)] : turns;
        const reply = await rad.assist(system, msgs.map((m, k) => ({ ...m, images: k === msgs.length - 1 ? m.images : undefined })), 1500);
        turns.push({ role: 'assistant', content: reply.content });
        const { say, action } = parseAction(reply.content);
        if (say) step('say', say);
        if (!action || action.do === 'done') {
          if (action?.file) j.output = String(action.file);
          j.result = String(action?.summary ?? say ?? 'Done.') || 'Done.';
          break;
        }
        const label = action.do === 'run' ? `${action.command} ${action.params ? clip(JSON.stringify(action.params), 160) : ''}` : action.do === 'draw' ? `draw with ${action.tool ?? 'brush'} (${(action.points ?? []).length} points)` : `${action.do}${action.filter ? ` “${action.filter}”` : ''}${action.file ? ` ${action.file}` : ''}`;
        step('act', label);
        this.now = label;
        let result: string;
        let images: string[] | undefined;
        try {
          ({ result, images } = await this.act(t, action, j));
          step('result', clip(result.split('\n')[0], 240), true);
        } catch (e: any) {
          result = `Error: ${e?.message ?? e}`;
          step('result', clip(result, 240), false);
        }
        turns.push({ role: 'user', content: result, images });
      }
      if (!j.result) j.result = `Stopped after ${MAX_STEPS} steps.`;
      // A background edit always ends in a file.
      if (opts.out && !j.output) {
        await this.call(t, t === 'app' ? 'app.save' : 'doc.save', { path: opts.out });
        j.output = opts.out;
      }
      j.status = 'done';
      step('done', j.result, true);
      if (t === 'app') void this.look('app').catch(() => {});
    } catch (e: any) {
      j.status = 'failed';
      j.result = e?.message ?? String(e);
      step('error', j.result, false);
    } finally {
      j.ended = Date.now();
      this.now = '';
      this.stopping.delete(j.id);
      void this.refresh();
    }
    return j;
  }

  private async act(t: Target, a: Record<string, any>, j: PcJob): Promise<{ result: string; images?: string[] }> {
    switch (a.do) {
      case 'commands':
        return { result: await this.commandsMatching(t, String(a.filter ?? '')) };
      case 'run': {
        const r = await this.call(t, 'engine.execute', { command: String(a.command ?? ''), params: a.params && typeof a.params === 'object' ? a.params : null });
        return { result: `OK.${r !== null && r !== undefined ? ` Result: ${clip(JSON.stringify(r), 1500)}` : ''}` };
      }
      case 'inspect':
        return { result: await this.inspect(t) };
      case 'look':
        return { result: 'Here is a picture of the work (attached).', images: [await this.look(t)] };
      case 'draw': {
        if (t !== 'app') return { result: 'Error: drawing strokes needs the PhotoCraft window; use commands (shapes, fills, gradients) instead.' };
        const pts: [number, number][] = (Array.isArray(a.points) ? a.points : []).filter((p: any) => Array.isArray(p) && p.length >= 2).slice(0, 400);
        if (pts.length < 2) return { result: 'Error: a stroke needs at least two points.' };
        const pressure = Math.min(1, Math.max(0.05, Number(a.pressure) || 0.7));
        const events = pts.map(([x, y], i) => ({ kind: i === 0 ? 'down' : i === pts.length - 1 ? 'up' : 'move', x: Number(x), y: Number(y), pressure }));
        await this.call('app', 'ui.pointer', { tool: String(a.tool ?? 'brush'), events });
        return { result: 'Drawn.' };
      }
      case 'open':
        await this.call(t, t === 'app' ? 'app.open' : 'doc.open', { path: String(a.file ?? '') });
        return { result: `Opened.\n${await this.inspect(t)}` };
      case 'save': {
        const file = String(a.file ?? '');
        const r = await this.call(t, t === 'app' ? 'app.save' : 'doc.save', { path: file });
        j.output = file;
        const warn = Array.isArray(r?.warnings) && r.warnings.length ? ` Notes: ${r.warnings.join('; ')}` : '';
        return { result: `Saved as ${file}.${warn}` };
      }
    }
    return { result: `Error: there's no action “${a.do}”.` };
  }

  // ---------------------------------------------------------------- for Gilgamesh

  describe(): string {
    if (!this.status?.installed) return '';
    const last = this.jobs.filter((x) => x.ended && Date.now() - x.ended < 3 * 3600_000).slice(0, 3);
    const files = this.files.slice(0, 6).map((f) => f.name);
    return `PHOTOCRAFT (the image editor; edit with [edit: …]): ${this.status.running ? 'open' : 'not open'}.${files.length ? ` Recent files: ${files.join(', ')}.` : ''}${last.length ? `\nRecent edits:\n${last.map((x) => `- “${clip(x.instruction, 80)}” → ${x.status}: ${clip(x.result, 160)}`).join('\n')}` : ''}`;
  }
}

export const photocraft = new Photocraft();
