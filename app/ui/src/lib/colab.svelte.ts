// Colab (Development → Rust Colab and Python Colab): notebooks of code cells, run on this
// computer by a kernel per notebook (colab.rs), for experimenting with and learning maths,
// machine learning and LLMs. Gilgamesh is in the chat beside the notebook: he sees it
// (describe(): its cells, outputs and errors) and helps; his code blocks go into cells with a
// click; and he searches the vault as in any chat, so the PDFs and notes the owner adds to RAG
// (here: "Add to RAG", into the "Colab Library" project) are part of what he knows.
//
// It works offline: the notebooks, the kernels, Colab's Python packages and Rust crates are all on
// this computer. "Packages & offline" downloads packs ahead of time; offline (auto-detected, or
// set by hand) Rust builds from the crates already there and `%pip install` from local wheels.

import { invoke, Channel } from '@tauri-apps/api/core';
import { api, rad } from './api';
import { store } from './state.svelte';
import type * as T from './types';
import { fromIpynb, toIpynb, newCell, errorText, type Cell, type Lang, type Notebook, type Output } from './colab/notebook';
import { TEMPLATES, type Template } from './colab/templates';

export type { Lang, Cell, Notebook, Output } from './colab/notebook';

export interface Listed {
  id: string;
  title: string;
  lang: Lang;
  updated: number;
  cells: number;
}

export interface Pack {
  id: string;
  name: string;
  about: string;
  items: string[];
  size: string;
  downloaded: number | null;
}

export interface Status {
  offline: boolean;
  online: boolean;
  network: 'auto' | 'offline' | 'online';
  folder: string;
  python: { system: string | null; version: string | null; ready: boolean; wheels_mb: number; packs: Pack[] };
  rust: { cargo: boolean; rustc: string | null; kernel: boolean; ready: boolean; crates_mb: number; packs: Pack[] };
}

export interface Variable {
  name: string;
  type: string;
  preview?: string;
  shape?: string;
  dtype?: string;
  len?: number;
}

export type KernelState = 'off' | 'starting' | 'idle' | 'busy' | 'dead';

const LIBRARY = 'Colab Library';
const LAST_KEY = 'rad-colab-last';

class Colab {
  status = $state<Status | null>(null);
  lists = $state<Record<Lang, Listed[]>>({ python: [], rust: [] });
  nb = $state<Notebook | null>(null);
  selected = $state<string | null>(null);
  /** Markdown cells being edited (the rest show rendered). */
  editing = $state<Set<string>>(new Set());
  kernel = $state<KernelState>('off');
  kernelInfo = $state<string>('');
  vars = $state<Variable[]>([]);
  /** Cells waiting to run, in order; the first is running. */
  queue = $state<string[]>([]);
  dirty = $state(false);
  /** A download of packs going on, with its log. */
  downloading = $state<Lang | null>(null);
  log = $state<string[]>([]);
  adding = $state(false);

  private saveTimer: ReturnType<typeof setTimeout> | undefined;
  private count = 0;
  private draining = false;

  get lang(): Lang {
    return store.route.kind === 'colab' ? store.route.lang : (this.nb?.lang ?? 'python');
  }

  get ready(): boolean {
    const s = this.status;
    return !!s && (this.lang === 'python' ? s.python.ready : s.rust.ready);
  }

  // ---------------------------------------------------------------- status & packs

  async refresh() {
    try {
      this.status = await invoke<Status>('colab_status');
    } catch (e) {
      store.error(e, 'Couldn’t check Colab');
    }
  }

  async setNetwork(network: Status['network']) {
    const r = await invoke<{ offline: boolean; online: boolean }>('colab_set_network', { network });
    if (this.status) this.status = { ...this.status, network, ...r };
  }

  /** Downloads packs (and extra packages / crates) for offline use. */
  async download(lang: Lang, packs: string[], extra: string[] = []) {
    if (this.downloading) return;
    this.downloading = lang;
    this.log = [];
    const ch = new Channel<{ type: string; text: string }>();
    ch.onmessage = (m) => {
      if (m.type === 'log') this.log = [...this.log.slice(-400), m.text];
    };
    try {
      await invoke('colab_download', { lang, packsWanted: packs, extra, onEvent: ch });
      store.toast('success', 'Downloaded', lang === 'python' ? 'Installed, and kept in Colab’s local packages for offline use.' : 'The crates are in Colab’s store; notebooks build from it offline.');
    } catch (e: any) {
      if (e?.message !== 'Stopped') store.error(e, 'The download didn’t finish');
    } finally {
      this.downloading = null;
      await this.refresh();
    }
  }

  stopDownload() {
    if (this.downloading) void invoke('colab_download_stop', { lang: this.downloading });
  }

  // ---------------------------------------------------------------- notebooks

  async loadList(lang: Lang = this.lang) {
    try {
      this.lists = { ...this.lists, [lang]: await invoke<Listed[]>('colab_list', { lang }) };
    } catch (e) {
      store.error(e, 'Couldn’t list the notebooks');
    }
  }

  /** Opens the last notebook of this language, if there is one. */
  async restore(lang: Lang) {
    if (this.nb?.lang === lang) return;
    await this.loadList(lang);
    let last: string | null = null;
    try {
      last = JSON.parse(localStorage.getItem(LAST_KEY) ?? '{}')[lang] ?? null;
    } catch {
      // Not remembered.
    }
    const id = this.lists[lang].find((n) => n.id === last)?.id ?? this.lists[lang][0]?.id;
    if (id) await this.open(id);
    else this.nb = null;
  }

  async open(id: string) {
    if (this.nb?.id === id) return;
    await this.flush();
    try {
      const raw = await invoke<any>('colab_open', { id });
      this.nb = fromIpynb(id, raw);
      this.selected = this.nb.cells[0]?.id ?? null;
      this.editing = new Set(this.nb.cells.filter((c) => c.type === 'markdown' && !c.source.trim()).map((c) => c.id));
      this.queue = [];
      this.vars = [];
      this.count = Math.max(0, ...this.nb.cells.map((c) => c.count ?? 0));
      this.kernel = 'off';
      this.kernelInfo = '';
      try {
        const last = JSON.parse(localStorage.getItem(LAST_KEY) ?? '{}');
        localStorage.setItem(LAST_KEY, JSON.stringify({ ...last, [this.nb.lang]: id }));
      } catch {
        // Not remembered this time.
      }
      if (store.route.kind !== 'colab' || store.route.lang !== this.nb.lang) store.navigate({ kind: 'colab', lang: this.nb.lang });
    } catch (e) {
      store.error(e, 'Couldn’t open the notebook');
    }
  }

  /** A new notebook, from a template (or blank). */
  async create(lang: Lang, template?: Template, title?: string) {
    await this.flush();
    const cells = template ? template.cells.map((c) => (c.md !== undefined ? newCell('markdown', c.md) : newCell('code', c.code ?? ''))) : [newCell('code')];
    const nb: Notebook = { id: null, lang, title: title ?? (template && !template.id.endsWith('blank') ? template.title : 'Untitled'), cells, metadata: {} };
    const id = await invoke<string>('colab_save', { id: null, notebook: toIpynb(nb) });
    await this.loadList(lang);
    this.nb = null;
    await this.open(id);
    return id;
  }

  /** Saves a moment after the last change. */
  touch() {
    this.dirty = true;
    clearTimeout(this.saveTimer);
    this.saveTimer = setTimeout(() => void this.flush(), 700);
  }

  async flush() {
    clearTimeout(this.saveTimer);
    const nb = this.nb;
    if (!nb?.id || !this.dirty) return;
    this.dirty = false;
    try {
      await invoke('colab_save', { id: nb.id, notebook: toIpynb(nb) });
      const l = this.lists[nb.lang].find((x) => x.id === nb.id);
      if (l) {
        l.title = nb.title;
        l.updated = Date.now();
        l.cells = nb.cells.length;
      }
    } catch (e) {
      this.dirty = true;
      store.error(e, 'Couldn’t save the notebook');
    }
  }

  rename(title: string) {
    if (!this.nb || !title.trim()) return;
    this.nb.title = title.trim();
    this.touch();
  }

  async remove(id: string) {
    await invoke('colab_delete', { id });
    if (this.nb?.id === id) {
      this.nb = null;
      this.kernel = 'off';
    }
    await this.loadList();
  }

  async importFiles(paths?: string[]) {
    if (!paths) {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const r = await open({ multiple: true, title: 'Open notebooks', filters: [{ name: 'Notebooks', extensions: ['ipynb'] }] });
      paths = r == null ? [] : Array.isArray(r) ? r : [r];
    }
    paths = paths.filter((p) => p.toLowerCase().endsWith('.ipynb'));
    if (!paths.length) return;
    const res = await invoke<Array<{ id: string; lang: Lang; title: string } | { error: string }>>('colab_import', { paths });
    for (const r of res) if ('error' in r) store.toast('error', 'Couldn’t open a notebook', r.error);
    const ok = res.filter((r): r is { id: string; lang: Lang; title: string } => 'id' in r);
    await Promise.all([this.loadList('python'), this.loadList('rust')]);
    if (ok.length) await this.open(ok[ok.length - 1].id);
  }

  async exportFile() {
    const nb = this.nb;
    if (!nb?.id) return;
    await this.flush();
    const { save } = await import('@tauri-apps/plugin-dialog');
    const path = await save({ title: 'Save the notebook as', defaultPath: `${nb.title.replace(/[/\\:]/g, '-')}.ipynb`, filters: [{ name: 'Notebook', extensions: ['ipynb'] }] });
    if (!path) return;
    await invoke('colab_export', { id: nb.id, path });
    store.toast('success', 'Saved', 'A Jupyter notebook: it opens in Google Colab and Jupyter too.');
  }

  async openFolder() {
    const dir = await invoke<string>('colab_folder');
    const { revealItemInDir } = await import('@tauri-apps/plugin-opener');
    await revealItemInDir(dir).catch((e) => store.error(e, 'Couldn’t open the folder'));
  }

  // ---------------------------------------------------------------- cells

  cell(id: string | null) {
    return this.nb?.cells.find((c) => c.id === id) ?? null;
  }

  index(id: string | null) {
    return this.nb?.cells.findIndex((c) => c.id === id) ?? -1;
  }

  addCell(type: Cell['type'] = 'code', source = '', after: string | null = this.selected) {
    if (!this.nb) return null;
    const c = newCell(type, source);
    const at = after ? this.index(after) + 1 : this.nb.cells.length;
    this.nb.cells.splice(at <= 0 ? this.nb.cells.length : at, 0, c);
    this.selected = c.id;
    if (type === 'markdown' && !source) this.editing = new Set([...this.editing, c.id]);
    this.touch();
    return c;
  }

  deleteCell(id: string) {
    if (!this.nb) return;
    const i = this.index(id);
    if (i < 0) return;
    this.queue = this.queue.filter((q) => q !== id || this.queue[0] === id);
    this.nb.cells.splice(i, 1);
    if (!this.nb.cells.length) this.nb.cells.push(newCell());
    this.selected = this.nb.cells[Math.min(i, this.nb.cells.length - 1)].id;
    this.touch();
  }

  moveCell(id: string, by: -1 | 1) {
    if (!this.nb) return;
    const i = this.index(id);
    const j = i + by;
    if (i < 0 || j < 0 || j >= this.nb.cells.length) return;
    const [c] = this.nb.cells.splice(i, 1);
    this.nb.cells.splice(j, 0, c);
    this.touch();
  }

  setType(id: string, type: Cell['type']) {
    const c = this.cell(id);
    if (!c || c.type === type) return;
    c.type = type;
    c.outputs = [];
    c.count = null;
    c.state = 'idle';
    if (type === 'markdown') this.editing = new Set([...this.editing, id]);
    this.touch();
  }

  setEditing(id: string, on: boolean) {
    const s = new Set(this.editing);
    if (on) s.add(id);
    else s.delete(id);
    this.editing = s;
  }

  clearOutputs() {
    if (!this.nb) return;
    for (const c of this.nb.cells) if (c.type === 'code' && !this.queue.includes(c.id)) ((c.outputs = []), (c.state = 'idle'), (c.count = null), (c.ms = undefined));
    this.touch();
  }

  /** Code from Gilgamesh (or anywhere) as a new cell after the selected one; run it if asked. */
  insertCode(code: string, run = false) {
    const c = this.addCell('code', code.replace(/\n+$/, ''));
    if (c && run) this.run(c.id);
    return c;
  }

  // ---------------------------------------------------------------- running

  /** Puts a cell in the queue (a markdown cell just shows rendered). */
  run(id: string) {
    const c = this.cell(id);
    if (!c) return;
    if (c.type === 'markdown') {
      this.setEditing(id, false);
      return;
    }
    if (!c.source.trim() || this.queue.includes(id)) return;
    c.state = 'queued';
    this.queue = [...this.queue, id];
    void this.drain();
  }

  runAll() {
    for (const c of this.nb?.cells ?? []) this.run(c.id);
  }

  runAbove(id: string) {
    for (const c of this.nb?.cells ?? []) {
      if (c.id === id) break;
      this.run(c.id);
    }
  }

  private async drain() {
    if (this.draining) return;
    this.draining = true;
    try {
      while (this.queue.length && this.nb) {
        const nb = this.nb;
        const c = this.cell(this.queue[0]);
        if (!c) {
          this.queue = this.queue.slice(1);
          continue;
        }
        const ok = await this.exec(nb, c);
        this.queue = this.queue.slice(1);
        if (!ok && this.nb === nb) {
          // As in Jupyter: an error stops the cells queued after it.
          for (const q of this.queue) {
            const x = this.cell(q);
            if (x) x.state = x.outputs.length ? (x.outputs.some((o) => o.kind === 'error') ? 'error' : 'ok') : 'idle';
          }
          this.queue = [];
        }
      }
    } finally {
      this.draining = false;
    }
  }

  private async exec(nb: Notebook, c: Cell): Promise<boolean> {
    if (!nb.id) return false;
    c.state = 'running';
    c.outputs = [];
    c.ms = undefined;
    if (this.kernel === 'off' || this.kernel === 'dead') this.kernel = 'starting';
    const ch = new Channel<any>();
    ch.onmessage = (m) => this.onOutput(c, m);
    const started = performance.now();
    try {
      // The first run starts the kernel (a Rust one builds its first cell: a few seconds).
      const end = await invoke<any>('colab_run', { id: nb.id, lang: nb.lang, code: c.source, onEvent: ch });
      this.kernel = 'idle';
      c.count = ++this.count;
      c.ms = end.ms ?? Math.round(performance.now() - started);
      if (end.type === 'done') {
        if (end.data && Object.keys(end.data).length) c.outputs.push({ kind: 'data', data: end.data, result: true });
        c.state = 'ok';
      } else {
        c.outputs.push({ kind: 'error', errKind: end.kind ?? 'error', ename: end.ename, message: end.message ?? 'Error', traceback: end.traceback, errors: end.errors, line: end.line ?? end.errors?.[0]?.line ?? null });
        c.state = 'error';
        if (end.kind === 'died') this.kernel = 'dead';
      }
      // Build output is only worth keeping when the build failed.
      if (c.state === 'ok') c.outputs = c.outputs.filter((o) => o.kind !== 'build');
      void this.loadVars();
      return c.state === 'ok';
    } catch (e: any) {
      c.state = 'error';
      this.kernel = e?.code === 'setup' ? 'off' : 'dead';
      c.outputs.push({ kind: 'error', errKind: e?.code ?? 'kernel', message: e?.message ?? String(e) });
      if (e?.code === 'setup') void this.refresh();
      return false;
    } finally {
      if (this.nb === nb) this.touch();
    }
  }

  private onOutput(c: Cell, m: any) {
    if (this.kernel !== 'busy') this.kernel = 'busy';
    if (m.type === 'stdout' || m.type === 'stderr') {
      const last = c.outputs.at(-1);
      if (last?.kind === 'stream' && last.name === m.type) last.text += m.text;
      else c.outputs.push({ kind: 'stream', name: m.type, text: m.text });
    } else if (m.type === 'build') {
      const last = c.outputs.at(-1);
      if (last?.kind === 'build') last.text += m.text;
      else c.outputs.push({ kind: 'build', text: m.text });
    } else if (m.type === 'display') {
      c.outputs.push({ kind: 'data', data: m.data });
    }
  }

  interrupt() {
    const id = this.nb?.id;
    if (id && this.queue.length) {
      // Only the running cell is stopped; the rest of the queue goes.
      const rest = this.queue.slice(1);
      for (const q of rest) {
        const x = this.cell(q);
        if (x) x.state = 'idle';
      }
      this.queue = this.queue.slice(0, 1);
      void invoke('colab_interrupt', { id });
    }
  }

  async restart() {
    const nb = this.nb;
    if (!nb?.id) return;
    this.queue = [];
    this.kernel = 'starting';
    try {
      const info = await invoke<any>('colab_restart', { id: nb.id, lang: nb.lang });
      this.kernel = 'idle';
      this.kernelInfo = info.python ? `Python ${info.python}` : 'Rust (evcxr)';
      this.count = 0;
      this.vars = [];
      for (const c of nb.cells) if (c.state === 'running' || c.state === 'queued') c.state = 'idle';
    } catch (e) {
      this.kernel = 'dead';
      store.error(e, 'Couldn’t restart the kernel');
    }
  }

  async shutdown() {
    if (!this.nb?.id) return;
    await invoke('colab_shutdown', { id: this.nb.id });
    this.kernel = 'off';
    this.vars = [];
    this.queue = [];
  }

  async loadVars() {
    if (!this.nb?.id || this.kernel === 'off') return;
    try {
      this.vars = await invoke<Variable[]>('colab_vars', { id: this.nb.id });
    } catch {
      // Busy: next time.
    }
  }

  // ---------------------------------------------------------------- the vault (RAG)

  private async library(): Promise<T.Project> {
    const p = store.projects.find((p) => p.name === LIBRARY);
    if (p) return p;
    const created = await api.post<T.Project>('/v1/projects', { name: LIBRARY, color: '#0e7490', icon: 'book', description: 'Books, papers and docs about coding, maths and machine learning, for Colab; searchable from any chat' });
    store.projects = [...store.projects, created];
    return created;
  }

  /** Adds PDFs, books and docs to the vault's Colab Library, where every chat (and Gilgamesh) can search them. */
  async addToRag(paths?: string[]) {
    if (!paths) {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const r = await open({ multiple: true, title: 'Add to RAG (Colab Library)', filters: [{ name: 'Documents', extensions: ['pdf', 'epub', 'md', 'txt', 'html', 'htm', 'ipynb', 'py', 'rs', 'docx', 'mobi', 'azw3'] }] });
      paths = r == null ? [] : Array.isArray(r) ? r : [r];
    }
    if (!paths.length) return;
    this.adding = true;
    try {
      const p = await this.library();
      const res = await rad.uploadPaths(p.id, paths, 'colab');
      const added = res.filter((r) => !('error' in r)).length;
      for (const r of res) if ('error' in r) store.toast('error', 'Couldn’t add a file', r.error);
      if (added) store.toast('success', `${added} file${added > 1 ? 's' : ''} added to RAG`, 'In the Colab Library project. Once indexed, Gilgamesh and every chat can search them, offline too.');
    } catch (e) {
      store.error(e, 'Couldn’t add to RAG');
    } finally {
      this.adding = false;
    }
  }

  // ---------------------------------------------------------------- Gilgamesh

  find(id: string | undefined | null) {
    if (!id) return null;
    if (this.nb?.id === id) return { id, title: this.nb.title };
    const l = [...this.lists.python, ...this.lists.rust].find((n) => n.id === id);
    return l ? { id: l.id, title: l.title } : null;
  }

  /** The notebook for Gilgamesh: its cells, what they printed and their errors, the variables. */
  describe(): string {
    const nb = this.nb;
    if (!nb || store.route.kind !== 'colab') return '';
    const lang = nb.lang === 'rust' ? 'Rust' : 'Python';
    const sel = this.selected;
    const BUDGET = 9000;
    let used = 0;
    const cells = nb.cells.map((c, i) => {
      const mark = c.id === sel ? ' ← SELECTED' : '';
      const src = c.source.length > 1500 ? c.source.slice(0, 1500) + '\n…' : c.source;
      let s = c.type === 'markdown' ? `[${i + 1}] markdown${mark}:\n${src}` : `[${i + 1}] code${c.count ? ` (run ${c.count}${c.ms != null ? `, ${c.ms} ms` : ''})` : ' (not run)'}${c.state === 'running' ? ' RUNNING' : ''}${mark}:\n\`\`\`${nb.lang}\n${src}\n\`\`\``;
      const outs = c.outputs.map((o) => outputText(o)).filter(Boolean).join('\n');
      if (outs) s += `\nOutput:\n${outs.length > 1200 ? outs.slice(0, 1200) + '\n…' : outs}`;
      return s;
    });
    // The selected cell and the cells with errors come first if it's too long for all of them.
    const keep = new Set<number>();
    const order = nb.cells.map((c, i) => i).sort((a, b) => score(nb.cells[b], sel) - score(nb.cells[a], sel));
    for (const i of order) {
      if (used + cells[i].length > BUDGET && keep.size) continue;
      keep.add(i);
      used += cells[i].length;
    }
    const shown = cells.filter((_, i) => keep.has(i));
    const vars = this.vars.length ? `\nVariables: ${this.vars.slice(0, 30).map((v) => `${v.name}: ${v.type}${v.shape ? ` ${v.shape}` : ''}${v.dtype ? ` ${v.dtype}` : ''}`).join(', ')}` : '';
    const s = this.status;
    const offline = s?.offline ? `\nThe owner is OFFLINE: only packages already on this computer work (${nb.lang === 'python' ? 'the downloaded Python packs' : 'crates already in Colab’s store'}). Don’t suggest anything that needs the internet.` : '';
    const packs = s ? (nb.lang === 'python' ? s.python.packs : s.rust.packs).filter((p) => p.downloaded).map((p) => p.about) : [];
    return `THE OWNER'S ${lang.toUpperCase()} COLAB NOTEBOOK “${nb.title}” (open now; kernel ${this.kernel}). ${nb.cells.length} cells${shown.length < cells.length ? `, ${shown.length} shown` : ''}:
${shown.join('\n\n')}${vars}${offline}${packs.length ? `\nDownloaded for offline use: ${packs.join('; ')}.` : ''}
You are the owner's tutor here, for maths, machine learning, LLMs and ${lang}. Explain the idea and the maths (LaTeX in $…$ or $$…$$ is shown typeset), then the code. When you give code, put it in a fenced \`\`\`${nb.lang} block, short and complete enough to run as one cell in this notebook (it can use what earlier cells defined); the owner adds it with one click.${nb.lang === 'rust' ? ' Crates come in with `:dep name = "version"` at the top of a cell.' : ' Packages come in with `%pip install name`.'} When a cell failed, explain the error plainly and give the fixed cell. You can also act on the notebook: [colab: run] runs the selected cell, [colab: run all], [colab: stop], [colab: restart].`;
  }
}

function score(c: Cell, sel: string | null) {
  return (c.id === sel ? 4 : 0) + (c.state === 'error' ? 2 : 0) + (c.state === 'running' ? 3 : 0);
}

function outputText(o: Output): string {
  if (o.kind === 'stream') return o.text.trimEnd();
  if (o.kind === 'error') return `ERROR: ${errorText(o)}`;
  if (o.kind === 'data') {
    if (o.data['text/plain']) return o.data['text/plain'];
    if (o.data['image/png'] || o.data['image/svg+xml']) return '[a picture]';
    if (o.data['text/html']) return o.data['text/html'].includes('<svg') ? '[a chart]' : '[an HTML table or page]';
  }
  return '';
}

export const colab = new Colab();
export { TEMPLATES };
