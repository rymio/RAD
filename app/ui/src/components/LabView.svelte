<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { ask } from '@tauri-apps/plugin-dialog';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { api, rad } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import { renderMarkdown } from '../lib/markdown';
  import { formatBytes, timeAgo } from '../lib/util';
  import * as L from '../lib/zxlab';
  import * as G from '../lib/games';
  import * as Z from '../lib/zx';
  import { LabAgent } from '../lib/labAgent.svelte';
  import type { FrameStats, LabLine, LabNotes, LabSummary } from '../lib/types';
  import ZxBoard from './ZxBoard.svelte';
  import Modal from './Modal.svelte';
  import Menu from './Menu.svelte';
  import {
    Microscope, Upload, Play, Loader, Sparkles, ArrowUp, Square, Eraser, Wrench, Search, Tag, MessageSquare, CornerDownRight, Copy,
    Trash2, MoreHorizontal, Pencil, CircleCheck, CircleX, FileCode, Save, BrainCircuit, RefreshCw, Lock, Unlock, CassetteTape, Cpu,
  } from '@lucide/svelte';

  type Tab = 'overview' | 'code' | 'basic' | 'graphics' | 'text' | 'tape' | 'patches' | 'cheats' | 'board';
  const TABS: [Tab, string][] = [
    ['overview', 'Overview'], ['code', 'Code'], ['basic', 'BASIC'], ['graphics', 'Graphics'], ['text', 'Text'], ['tape', 'Tape'],
    ['patches', 'Patches'], ['cheats', 'Cheats'], ['board', 'Board'],
  ];

  let projects = $state<L.LabProject[]>([]);
  let current = $state<string | null>(read('rad-lab-current'));
  let st = $state<L.LabState | null>(null);
  let summary = $state<LabSummary | null>(null);
  let analysing = $state(false);
  let tab = $state<Tab>('overview');
  let seconds = $state(15);
  let dropping = $state(false);
  const project = $derived(projects.find((p) => p.id === current) ?? null);

  function read(k: string) {
    try {
      return localStorage.getItem(k);
    } catch {
      return null;
    }
  }

  // ------------------------------------------------------------------ projects

  async function refresh() {
    projects = await L.projects().catch((e) => (store.error(e, 'Couldn’t read the lab projects'), projects));
  }

  async function open(id: string) {
    current = id;
    try {
      localStorage.setItem('rad-lab-current', id);
    } catch {
      /* private mode */
    }
    summary = null;
    lines = [];
    st = await L.load(id).catch((e) => (store.error(e, 'Couldn’t open the project'), null));
    if (!st) return;
    agent = new LabAgent(io(id));
    agent.restore(st.chat as any);
    await analyse();
  }

  async function analyse() {
    if (!current || !st) return;
    analysing = true;
    try {
      summary = await rad.zxlabOpen(current, st.file, st.data, seconds);
      codeFrom = L.hex4(summary.range[0]);
      codeTo = L.hex4(Math.min(summary.range[1], 0x10000) - 1);
      gfxAddr = L.hex4(summary.range[0]);
      if (tab === 'code') loadListing();
      loadScreens();
    } catch (e) {
      store.error(e, 'Couldn’t analyse the game');
    } finally {
      analysing = false;
    }
  }

  async function addFiles(paths: string[]) {
    const ok = paths.filter((p) => /\.(tzx|tap|z80|sna)$/i.test(p));
    if (!ok.length) return paths.length && store.toast('error', 'Not a Spectrum file', 'Use TZX, TAP, Z80 or SNA.');
    const files = await rad.gameReadFiles(ok.slice(0, 1)).catch((e) => (store.error(e), []));
    const f = files[0];
    if (!f || f.error || f.data === undefined) return store.toast('error', 'Couldn’t read the file', f?.error ?? '');
    const id = await L.create(f.name, f.data, f.size);
    await refresh();
    tab = 'overview';
    open(id);
  }

  async function pick() {
    const { open: o } = await import('@tauri-apps/plugin-dialog');
    const r = await o({ multiple: false, title: 'Take a Spectrum game apart', filters: [{ name: 'ZX Spectrum tapes and snapshots', extensions: ['tzx', 'tap', 'z80', 'sna'] }] });
    if (typeof r === 'string') addFiles([r]);
  }

  async function removeProject() {
    if (!project) return;
    if (!(await ask(`Delete the lab project “${project.name}” with its notes and patches?`, { title: 'Delete project', kind: 'warning' }))) return;
    await L.remove(project.id).catch((e) => store.error(e));
    rad.zxlabClose(project.id).catch(() => {});
    current = null;
    st = null;
    summary = null;
    await refresh();
  }

  async function renameProject() {
    if (!project) return;
    const n = window.prompt('Name', project.name)?.trim();
    if (n && n !== project.name) await L.rename(project.id, n);
    refresh();
  }

  function play(name: string, data: string, title: string) {
    store.pendingZx = { name, data, title };
    store.navigate({ kind: 'zx' });
  }

  function playOriginal() {
    if (st && project) play(st.file, st.data, project.name);
  }

  // ------------------------------------------------------------------ notes

  async function setNotes(n: LabNotes) {
    if (!st || !current) return;
    st.notes = n;
    await L.saveNotes(current, n);
    if (tab === 'code') loadListing();
  }

  async function renameLabel(addr: number, now: string | null) {
    const n = window.prompt(`Name for ${L.hex4(addr)}`, now ?? '')?.trim();
    if (n === undefined || !st) return;
    const notes = $state.snapshot(st.notes) as LabNotes;
    if (n) notes.labels[addr] = n.replace(/[^A-Za-z0-9_]/g, '_').replace(/^([0-9])/, '_$1');
    else delete notes.labels[addr];
    setNotes(notes);
  }

  async function editComment(addr: number, now: string | null) {
    const c = window.prompt(`Comment for ${L.hex4(addr)}`, now ?? '');
    if (c === null || !st) return;
    const notes = $state.snapshot(st.notes) as LabNotes;
    if (c.trim()) notes.comments[addr] = c.trim();
    else delete notes.comments[addr];
    setNotes(notes);
  }

  // ------------------------------------------------------------------ code

  let codeFrom = $state('$8000');
  let codeTo = $state('$8fff');
  let lines = $state<LabLine[]>([]);
  let loadingCode = $state(false);
  let selected = $state<number | null>(null);
  let xrefs = $state<{ addr: number; text: string }[] | null>(null);
  let find = $state('');
  let codeBox: HTMLDivElement | undefined = $state();

  async function loadListing() {
    if (!current || !st || !summary) return;
    const from = L.parseAddr(codeFrom);
    const to = L.parseAddr(codeTo);
    if (from == null || to == null || to < from) return store.toast('error', 'Range', 'Use addresses like $8000.');
    loadingCode = true;
    try {
      lines = await rad.zxlabListing(current, from, to + 1, $state.snapshot(st.notes) as LabNotes);
    } catch (e) {
      store.error(e, 'Couldn’t list the code');
    } finally {
      loadingCode = false;
    }
  }

  async function goTo(addr: number) {
    const from = L.parseAddr(codeFrom) ?? 0;
    const to = L.parseAddr(codeTo) ?? 0;
    tab = 'code';
    if (addr < from || addr > to || !lines.length) {
      codeFrom = L.hex4(Math.max(0, addr - 0x40));
      codeTo = L.hex4(Math.min(0xffff, addr + 0x3c0));
      await loadListing();
    }
    selected = lines.reduce((best, l) => (l.addr <= addr ? l.addr : best), lines[0]?.addr ?? addr);
    xrefs = null;
    await tick();
    document.getElementById(`L${selected}`)?.scrollIntoView({ block: 'center' });
  }

  function doFind(e: Event) {
    e.preventDefault();
    const q = find.trim();
    if (!q) return;
    const a = L.parseAddr(q);
    if (a != null) return goTo(a);
    const ql = q.toLowerCase();
    const hit = lines.find((l) => l.label?.toLowerCase().includes(ql) || l.text.toLowerCase().includes(ql) || l.comment?.toLowerCase().includes(ql));
    const fromNotes = Object.entries(st?.notes.labels ?? {}).find(([, n]) => n.toLowerCase().includes(ql));
    if (hit) goTo(hit.addr);
    else if (fromNotes) goTo(+fromNotes[0]);
    else store.toast('info', 'Not found', q, 2000);
  }

  async function showXrefs(addr: number) {
    if (!current || !st) return;
    xrefs = await rad.zxlabXrefs(current, addr, $state.snapshot(st.notes) as LabNotes).catch(() => []);
  }

  const selLine = $derived(lines.find((l) => l.addr === selected) ?? null);

  /** The code around the selection, for the AI. */
  function focusText(): string {
    if (tab !== 'code' || !lines.length) return '';
    const i = Math.max(0, lines.findIndex((l) => l.addr === selected));
    return lines
      .slice(Math.max(0, i - 30), i + 40)
      .map((l) => `${l.label ? l.label + ':\n' : ''}${L.hex4(l.addr).slice(1)}  ${l.text}${l.comment ? ' ; ' + l.comment : ''}${l.addr === selected ? '   <== selected' : ''}`)
      .join('\n');
  }

  // Export
  let exportOpen = $state(false);
  let exported = $state<{ source: string; verified: boolean; note?: string } | null>(null);

  async function exportSource() {
    if (!current || !st || !project) return;
    const from = L.parseAddr(codeFrom);
    const to = L.parseAddr(codeTo);
    if (from == null || to == null) return;
    exportOpen = true;
    exported = null;
    exported = await rad.zxlabSource(current, from, to + 1, $state.snapshot(st.notes) as LabNotes, project.name).catch((e) => (store.error(e), null));
  }

  async function exportToGames() {
    if (!exported || !project || !summary) return;
    let src = exported.source;
    if (!/^start:/m.test(src) && summary.entry != null) src += `\n; Where the game's code starts (RAD's lab found it running here).\nstart EQU $${summary.entry.toString(16).padStart(4, '0')}\n`;
    const g = await G.createWithFiles(`${project.name} (disassembly)`, 'zx-asm', [{ path: 'main.asm', content: src }]);
    exportOpen = false;
    store.toast('success', 'Assembler project made', `${g.name} — in Games. Note: only this range is in it; the rest of memory starts empty.`, 6000);
    store.navigate({ kind: 'games' });
  }

  async function basicToGames() {
    if (!summary?.basic || !project) return;
    const g = await G.createWithFiles(`${project.name} (BASIC)`, 'zx-basic', [{ path: 'main.bas', content: summary.basic }]);
    store.toast('success', 'BASIC project made', g.name, 4000);
    store.navigate({ kind: 'games' });
  }

  // ------------------------------------------------------------------ graphics

  let gfxAddr = $state('$8000');
  let gfxWidth = $state(1);
  let gfxRows = $state(256);
  let gfxZoom = $state(3);
  let gfxMode = $state<'rows' | 'chars'>('chars');
  let gfxCanvas: HTMLCanvasElement | undefined = $state();
  let gfxHover = $state<string>('');
  let screens = $state<{ title: string; bytes: Uint8Array }[]>([]);

  async function drawGfx() {
    if (!current || !gfxCanvas) return;
    const a = L.parseAddr(gfxAddr) ?? 0x8000;
    const w = Math.max(1, Math.min(32, gfxWidth));
    const rows = Math.max(8, Math.min(1024, gfxRows));
    const len = Math.min(0x10000 - a, w * rows);
    const bytes = Uint8Array.from(atob(await rad.zxlabMemory(current, a, len)), (c) => c.charCodeAt(0));
    const ctx = gfxCanvas.getContext('2d')!;
    // chars: 8-byte characters, w per row; rows: plain lines of w bytes.
    const cols = gfxMode === 'chars' ? w * 8 : w * 8;
    const h = gfxMode === 'chars' ? Math.ceil(len / (8 * w)) * 8 : Math.ceil(len / w);
    gfxCanvas.width = cols;
    gfxCanvas.height = Math.max(8, h);
    const img = ctx.createImageData(cols, Math.max(8, h));
    const put = (x: number, y: number, on: boolean, data: boolean) => {
      const i = (y * cols + x) * 4;
      const v = on ? [240, 240, 250] : data ? [24, 24, 34] : [12, 12, 16];
      img.data.set([v[0], v[1], v[2], 255], i);
    };
    for (let i = 0; i < len; i++) {
      const byte = bytes[i];
      let x: number, y: number;
      if (gfxMode === 'chars') {
        const ch = Math.floor(i / 8);
        x = (ch % w) * 8;
        y = Math.floor(ch / w) * 8 + (i % 8);
      } else {
        x = (i % w) * 8;
        y = Math.floor(i / w);
      }
      for (let b = 0; b < 8; b++) put(x + b, y, !!(byte & (0x80 >> b)), true);
    }
    ctx.putImageData(img, 0, 0);
  }

  function gfxMove(e: MouseEvent) {
    if (!gfxCanvas) return;
    const r = gfxCanvas.getBoundingClientRect();
    const x = Math.floor(((e.clientX - r.left) / r.width) * gfxCanvas.width);
    const y = Math.floor(((e.clientY - r.top) / r.height) * gfxCanvas.height);
    const a = L.parseAddr(gfxAddr) ?? 0;
    const w = gfxWidth;
    const off = gfxMode === 'chars' ? (Math.floor(y / 8) * w + Math.floor(x / 8)) * 8 + (y % 8) : y * w + Math.floor(x / 8);
    gfxHover = L.hex4(a + off);
  }

  function gfxPage(dir: number) {
    const a = L.parseAddr(gfxAddr) ?? 0;
    gfxAddr = L.hex4(Math.max(0, Math.min(0xffff, a + dir * gfxWidth * gfxRows)));
    drawGfx();
  }

  async function loadScreens() {
    if (!current || !summary) return;
    const out: { title: string; bytes: Uint8Array }[] = [];
    for (const b of summary.tape.filter((b) => b.screen)) {
      const d = Uint8Array.from(atob(await rad.zxlabTapeBlock(current, b.index)), (c) => c.charCodeAt(0));
      out.push({ title: `Loading screen (tape block ${b.index})`, bytes: d });
    }
    const s = Uint8Array.from(atob(await rad.zxlabMemory(current, 0x4000, 6912)), (c) => c.charCodeAt(0));
    out.push({ title: 'The screen when the game started', bytes: s });
    screens = out;
  }

  function screenCanvas(node: HTMLCanvasElement, bytes: Uint8Array) {
    const draw = (b: Uint8Array) => node.getContext('2d')?.putImageData(L.screenToImage(b), 0, 0);
    draw(bytes);
    return { update: draw };
  }

  // ------------------------------------------------------------------ patches

  let pForm = $state({ addr: '', mode: 'asm' as 'asm' | 'hex', code: '', note: '' });
  let pResult = $state<string | null>(null);
  let testing = $state(false);
  let testOut = $state<{ crashed: boolean; screen: string[] } | null>(null);

  function patchHere(addr: number) {
    tab = 'patches';
    pForm = { addr: L.hex4(addr), mode: 'asm', code: '', note: '' };
  }

  async function addPatch(e: Event) {
    e.preventDefault();
    if (!current || !st) return;
    const addr = L.parseAddr(pForm.addr);
    if (addr == null) return (pResult = 'The address isn’t valid (e.g. $8123).');
    let bytes = '';
    if (pForm.mode === 'hex') {
      bytes = pForm.code.replace(/[^0-9a-fA-F]/g, '').toLowerCase();
      if (!bytes || bytes.length % 2) return (pResult = 'Give bytes as hex pairs, e.g. 00 00 c9.');
    } else {
      const r = await rad.zxlabAssemblePatch(current, addr, pForm.code, $state.snapshot(st.notes) as LabNotes);
      if (!r.ok || !r.bytes) return (pResult = r.messages || 'It didn’t assemble.');
      if (r.splits != null && !(await ask(`This patch ends inside the instruction at ${L.hex4(r.splits)}, which would break it. Add it anyway?`, { title: 'Patch', kind: 'warning' }))) return;
      bytes = r.bytes;
    }
    await addPatchObj({ id: `p${Date.now()}`, addr, bytes, note: pForm.note.trim() || `Patch at ${L.hex4(addr)}`, source: pForm.mode === 'asm' ? pForm.code : '', enabled: true });
    pResult = `Added: ${bytes.length / 2} bytes at ${L.hex4(addr)}.`;
    pForm = { addr: '', mode: 'asm', code: '', note: '' };
  }

  async function addPatchObj(p: L.LabPatch) {
    if (!st || !current) return;
    st.patches = [...st.patches, p];
    await L.savePatches(current, $state.snapshot(st.patches) as L.LabPatch[]);
  }

  async function togglePatch(p: L.LabPatch) {
    if (!st || !current) return;
    p.enabled = !p.enabled;
    await L.savePatches(current, $state.snapshot(st.patches) as L.LabPatch[]);
  }

  async function deletePatch(p: L.LabPatch) {
    if (!st || !current) return;
    st.patches = st.patches.filter((x) => x.id !== p.id);
    await L.savePatches(current, $state.snapshot(st.patches) as L.LabPatch[]);
  }

  const enabledPatches = () => (st?.patches ?? []).filter((p) => p.enabled).map((p) => ({ addr: p.addr, bytes: p.bytes }));

  async function testPatched() {
    if (!current) return;
    testing = true;
    try {
      const r = await rad.zxlabBuild(current, enabledPatches(), true);
      testOut = r.check ?? null;
    } catch (e) {
      store.error(e, 'Couldn’t build it');
    } finally {
      testing = false;
    }
  }

  async function playPatched() {
    if (!current || !project) return;
    try {
      const r = await rad.zxlabBuild(current, enabledPatches(), false);
      play(r.file.name, r.file.data, `${project.name} (patched)`);
    } catch (e) {
      store.error(e, 'Couldn’t build it');
    }
  }

  // ------------------------------------------------------------------ cheats (live memory search)

  let cheatBase = $state<Uint8Array | null>(null);
  let candidates = $state<number[]>([]);
  let liveMem = $state<Uint8Array | null>(null);
  let locks = $state<{ addr: number; value: number }[]>([]);
  let equalsN = $state('');

  async function readLive(): Promise<Uint8Array | null> {
    try {
      return Uint8Array.from(atob(await rad.zxMemory()), (c) => c.charCodeAt(0));
    } catch (e) {
      store.error(e, 'Play the game first (the Spectrum isn’t on)');
      return null;
    }
  }

  async function startSearch() {
    const m = await readLive();
    if (!m) return;
    cheatBase = m;
    liveMem = m;
    candidates = [];
    for (let a = 0x5b00; a < 0x10000; a++) candidates.push(a);
  }

  async function filter(kind: 'down' | 'up' | 'same' | 'changed' | 'equals') {
    if (!cheatBase) return;
    const m = await readLive();
    if (!m) return;
    const n = parseInt(equalsN, 10);
    candidates = candidates.filter((a) => {
      const was = cheatBase![a];
      const now = m[a];
      return kind === 'down' ? now < was : kind === 'up' ? now > was : kind === 'same' ? now === was : kind === 'changed' ? now !== was : now === n;
    });
    cheatBase = m;
    liveMem = m;
  }

  async function lockAt(addr: number, value: number) {
    await rad.zxLock(addr, value).catch((e) => store.error(e));
    locks = [...locks.filter((l) => l.addr !== addr), { addr, value }];
  }

  async function unlock(addr: number) {
    await rad.zxLock(addr, null).catch(() => {});
    locks = locks.filter((l) => l.addr !== addr);
  }

  // ------------------------------------------------------------------ board (live)

  let stats = $state<FrameStats | null>(null);
  let boardCanvas: HTMLCanvasElement | undefined = $state();
  let lut = new Uint32Array(16);
  let frame: Uint8Array | null = null;
  let size: [number, number] = [352, 288];
  let boardOn = false;
  let raf = 0;
  const held = new Set<string>();

  function onZx(m: ArrayBuffer | Record<string, any>) {
    if (m instanceof ArrayBuffer) frame = new Uint8Array(m);
    else if (m.size) {
      size = m.size;
      lut = new Uint32Array((m.palette as number[][]).map(([r, g, b]) => (0xff << 24) | (b << 16) | (g << 8) | r));
    } else if (m.stats) stats = m.stats;
  }

  function drawBoardScreen() {
    if (frame && boardCanvas) {
      const ctx = boardCanvas.getContext('2d');
      const [w, h] = size;
      if (ctx && frame.length === w * h) {
        const img = ctx.createImageData(w, h);
        const px = new Uint32Array(img.data.buffer);
        for (let i = 0; i < frame.length; i++) px[i] = lut[frame[i] & 15];
        ctx.putImageData(img, 0, 0);
      }
      frame = null;
    }
    raf = requestAnimationFrame(drawBoardScreen);
  }

  async function boardStart() {
    if (boardOn) return;
    boardOn = true;
    try {
      await rad.zxAttach('48', onZx);
      await rad.zxStats(true);
      await rad.zxControl('resume');
    } catch (e) {
      store.error(e);
    }
    raf = requestAnimationFrame(drawBoardScreen);
  }

  function boardStop() {
    if (!boardOn) return;
    boardOn = false;
    cancelAnimationFrame(raf);
    rad.zxStats(false).catch(() => {});
    rad.zxControl('pause').catch(() => {});
    held.clear();
    rad.zxKeys(0).catch(() => {});
    rad.zxJoystick(0).catch(() => {});
    stats = null;
  }

  function boardKey(e: KeyboardEvent, down: boolean) {
    if (tab !== 'board' || e.metaKey) return;
    const t = e.target as HTMLElement;
    if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA')) return;
    if (!Z.isMapped(e.code, 'kempston')) return;
    e.preventDefault();
    if (down) held.add(e.code);
    else held.delete(e.code);
    const { keys, joy } = Z.mapKeys(held, 'kempston');
    rad.zxKeys(keys).catch(() => {});
    rad.zxJoystick(joy).catch(() => {});
  }

  /** The name of the routine the CPU is in (the nearest function or label below the PC). */
  const fnName = $derived.by(() => {
    if (!stats || !summary) return null;
    const pc = stats.pc;
    if (pc < 0x4000) return 'the ROM (interrupt, BASIC or a ROM routine)';
    let best = -1;
    let name: string | null = null;
    for (const [a, n] of Object.entries(st?.notes.labels ?? {})) if (+a <= pc && +a > best) [best, name] = [+a, n];
    for (const a of summary.function_addrs) if (a <= pc && a > best) [best, name] = [a, st?.notes.labels[a] ?? `sub_${a.toString(16).padStart(4, '0')}`];
    return name ? `${name} (${L.hex4(pc)})` : L.hex4(pc);
  });

  $effect(() => {
    if (tab === 'board' && summary) boardStart();
    else boardStop();
  });

  // ------------------------------------------------------------------ the AI

  let agent = $state<LabAgent | null>(null);
  let text = $state('');
  let thread: HTMLDivElement | undefined = $state();

  function io(id: string) {
    return {
      id,
      name: () => projects.find((p) => p.id === id)?.name ?? 'the game',
      summary: () => summary,
      notes: () => (st ? ($state.snapshot(st.notes) as LabNotes) : { labels: {}, comments: {} }),
      patches: () => (st ? ($state.snapshot(st.patches) as L.LabPatch[]) : []),
      focus: focusText,
      setNotes,
      addPatch: addPatchObj,
      persist: (turns: unknown[]) => L.saveChat(id, turns).catch(() => {}),
    };
  }

  function send(t = text) {
    const q = t.trim();
    if (!q || !agent || agent.busy) return;
    text = '';
    agent.ask(q);
  }

  const PROMPTS = ['Explain how this game works', 'What does the main loop do?', 'How does it draw on the screen?', 'Find where the lives are kept and give me infinite lives'];

  $effect(() => {
    agent?.items.length;
    agent?.status;
    tick().then(() => thread?.scrollTo({ top: thread.scrollHeight }));
  });

  // ------------------------------------------------------------------ save to RAD (searchable)

  let saving = $state(false);

  async function saveToRad() {
    if (!project || !st || !summary || !current) return;
    saving = true;
    try {
      const projs = await api.get<any[]>('/v1/projects');
      let p = projs.find((x) => x.name === 'ZX Spectrum Lab');
      p ??= await api.post<any>('/v1/projects', { name: 'ZX Spectrum Lab', color: '#d7a700' });
      const labels = Object.entries(st.notes.labels).sort((a, b) => +a[0] - +b[0]);
      const comments = Object.entries(st.notes.comments).sort((a, b) => +a[0] - +b[0]);
      const md = [
        `# ${project.name} — how it works`,
        `Taken apart in RAD's Spectrum lab (${new Date().toLocaleDateString()}). File: ${project.file}.`,
        `## Overview`,
        `- Machine: ZX Spectrum ${summary.model}K`,
        `- Code starts at ${summary.entry != null ? L.hex4(summary.entry) : 'unknown'}; program ${L.hex4(summary.range[0])}–${L.hex4(summary.range[1])}; ${summary.functions} functions`,
        `- Ports: ${Object.keys(summary.ports).join(', ') || 'none'}`,
        `- ROM routines used: ${summary.rom_calls.map((r) => `${r.name ?? L.hex4(r.addr)}`).join(', ') || 'none'}`,
        summary.basic ? `## BASIC\n\`\`\`\n${summary.basic}\n\`\`\`` : '',
        labels.length ? `## Names\n${labels.map(([a, n]) => `- ${L.hex4(+a)} **${n}**${st!.notes.comments[+a] ? ` — ${st!.notes.comments[+a]}` : ''}`).join('\n')}` : '',
        comments.length ? `## Comments\n${comments.map(([a, c]) => `- ${L.hex4(+a)}: ${c}`).join('\n')}` : '',
        st.patches.length ? `## Patches\n${st.patches.map((x) => `- ${L.hex4(x.addr)} ${x.note} (${x.bytes})${x.source ? `\n\`\`\`\n${x.source}\n\`\`\`` : ''}`).join('\n')}` : '',
        agent?.explanations().length ? `## Explanations\n${agent.explanations().join('\n\n---\n\n')}` : '',
      ]
        .filter(Boolean)
        .join('\n\n');
      const note = { title: `${project.name} (Spectrum lab)`, content: md, tags: ['zx-spectrum', 'reverse-engineering'] };
      if (st.ragNote) {
        await api.put(`/v1/notes/${st.ragNote}`, note).catch(async () => {
          const n = await api.post<any>(`/v1/projects/${p.id}/notes`, note);
          await L.setRagNote(current!, String(n.id));
          st!.ragNote = String(n.id);
        });
      } else {
        const n = await api.post<any>(`/v1/projects/${p.id}/notes`, note);
        await L.setRagNote(current, String(n.id));
        st.ragNote = String(n.id);
      }
      store.toast('success', 'Saved to RAD', 'In the “ZX Spectrum Lab” project — ask the chat about it.', 4000);
    } catch (e) {
      store.error(e, 'Couldn’t save it');
    } finally {
      saving = false;
    }
  }

  // ------------------------------------------------------------------

  const MAP_COLORS: Record<string, string> = {
    rom: '#4b4b5a', screen: '#2f7bd8', sysvars: '#7a6a9a', basic: '#c9a227', code: '#2fbf71', reachable: '#1f7a4a', data: '#d07a2a', variables: '#e0475a', empty: '#1b1b22',
  };

  $effect(() => {
    if (tab === 'code' && summary && !lines.length) loadListing();
    if (tab === 'graphics' && summary) tick().then(drawGfx);
  });

  onMount(() => {
    refresh().then(() => {
      if (current && projects.some((p) => p.id === current)) open(current);
    });
    const kd = (e: KeyboardEvent) => boardKey(e, true);
    const ku = (e: KeyboardEvent) => boardKey(e, false);
    window.addEventListener('keydown', kd, true);
    window.addEventListener('keyup', ku, true);
    const unDrop = getCurrentWebview().onDragDropEvent((ev) => {
      const p = ev.payload;
      if (p.type === 'enter' || p.type === 'over') dropping = true;
      else if (p.type === 'leave') dropping = false;
      else if (p.type === 'drop') {
        dropping = false;
        addFiles(p.paths);
      }
    });
    return () => {
      boardStop();
      window.removeEventListener('keydown', kd, true);
      window.removeEventListener('keyup', ku, true);
      unDrop.then((u) => u());
    };
  });
</script>

<div class="lv" class:drop={dropping}>
  <header class="drag">
    <div class="ttl no-drag"><Microscope size={16} /> Spectrum Lab</div>
    {#if projects.length}
      <select class="select no-drag pick" value={current} onchange={(e) => open((e.currentTarget as HTMLSelectElement).value)}>
        {#each projects as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
      </select>
    {/if}
    {#if analysing}<span class="muted small no-drag"><Loader size={12} class="spin" /> Running and tracing it…</span>{/if}
    <span class="spacer"></span>
    {#if project}
      <button class="btn sm no-drag" onclick={playOriginal} title="Play the original in the ZX Spectrum view"><Play size={13} /> Play</button>
      <button class="btn sm no-drag" onclick={saveToRad} disabled={saving || !summary} title="Save the notes and explanations to RAD, so the chat can answer questions about this game">
        {#if saving}<Loader size={13} class="spin" />{:else}<BrainCircuit size={13} />{/if} Save to RAD
      </button>
      <div class="no-drag">
        <Menu items={[{ label: 'Analyse again', icon: RefreshCw, onclick: analyse }, { label: 'Rename…', icon: Pencil, onclick: renameProject }, null, { label: 'Delete project', icon: Trash2, danger: true, onclick: removeProject }]}>
          {#snippet trigger(toggle)}<button class="icon-btn" onclick={toggle} aria-label="Project menu"><MoreHorizontal size={16} /></button>{/snippet}
        </Menu>
      </div>
    {/if}
    <button class="btn sm primary no-drag" onclick={pick}><Upload size={13} /> Open a game</button>
  </header>

  {#if !project}
    <div class="center">
      <div class="intro">
        <div class="big"><Microscope size={30} /></div>
        <h2>Take a Spectrum game apart</h2>
        <p>Open a TZX, TAP, Z80 or SNA file. RAD runs it and watches the CPU to tell code from graphics, lists the BASIC and the machine code with names, finds the text and pictures, and shows the data flowing on the board while you play. The AI explains it all and helps you change it — then you play your version.</p>
        <button class="btn primary" onclick={pick}><Upload size={14} /> Open a game</button>
        <p class="muted small">Or drop a file here. Use games you own or that are free to share.</p>
      </div>
    </div>
  {:else}
    <div class="body">
      <section class="work">
        <nav class="tabs">
          {#each TABS as [t, label] (t)}
            <button class:on={tab === t} onclick={() => (tab = t)} disabled={!summary && t !== 'overview'}>{label}{#if t === 'patches' && st?.patches.length} <span class="cnt">{st.patches.length}</span>{/if}</button>
          {/each}
        </nav>

        <div class="pane">
          {#if !summary}
            <div class="center note">{#if analysing}<Loader size={16} class="spin" /> Loading the game and tracing {seconds} seconds of it…{:else}Couldn’t analyse it yet.{/if}</div>
          {:else if tab === 'overview'}
            <div class="ov">
              <div class="mapwrap">
                <div class="map" title="Memory, 256 bytes per square: $0000 top-left to $FFFF bottom-right">
                  {#each summary.map as m, i}<button class="cell" style="background:{MAP_COLORS[m] ?? '#222'}" title="{L.hex4(i * 256)}: {m}" onclick={() => (i >= 0x40 ? goTo(i * 256) : null)}></button>{/each}
                </div>
                <div class="legend">{#each Object.entries(MAP_COLORS) as [k, c]}<span><i style="background:{c}"></i>{k}</span>{/each}</div>
              </div>
              <div class="facts">
                <div class="fact"><span>Machine</span><strong>{summary.model}K</strong></div>
                <div class="fact"><span>Code starts</span><button class="link" onclick={() => summary?.entry != null && goTo(summary.entry)}>{summary.entry != null ? L.hex4(summary.entry) : '—'}</button></div>
                <div class="fact"><span>Program</span><strong>{L.hex4(summary.range[0])}–{L.hex4(summary.range[1] - 1)}</strong></div>
                <div class="fact"><span>Functions</span><strong>{summary.functions}</strong></div>
                <div class="fact"><span>Code seen running</span><strong>{formatBytes(summary.code_bytes)}</strong></div>
                <div class="fact"><span>Reachable code</span><strong>{formatBytes(summary.reachable_bytes)}</strong></div>
                <div class="fact"><span>Changing bytes</span><strong>{summary.variables}</strong></div>
                <div class="fact"><span>Traced</span><strong>{summary.seconds} s · {(summary.steps / 1e6).toFixed(1)}M instructions</strong></div>
              </div>
              <div class="cols">
                <div>
                  <div class="section-title">Hardware it uses</div>
                  {#each Object.entries(summary.ports) as [p, c]}<div class="small">{p} <span class="muted">×{c}</span></div>{:else}<div class="small muted">No ports from its own code.</div>{/each}
                </div>
                <div>
                  <div class="section-title">ROM routines it calls</div>
                  {#each summary.rom_calls as r}<div class="small"><button class="link" onclick={() => goTo(r.addr)}>{r.name ?? L.hex4(r.addr)}</button> <span class="muted">×{r.count}{r.what ? ` — ${r.what}` : ''}</span></div>{:else}<div class="small muted">None.</div>{/each}
                </div>
              </div>
              {#if screens.length}
                <div class="screens">{#each screens as s}<figure><canvas width="256" height="192" use:screenCanvas={s.bytes}></canvas><figcaption>{s.title}</figcaption></figure>{/each}</div>
              {/if}
              <div class="row"><span class="muted small">Trace for</span><input class="input sm num" type="number" min="2" max="120" bind:value={seconds} /><span class="muted small">seconds</span><button class="btn sm" onclick={analyse} disabled={analysing}><RefreshCw size={12} /> Analyse again</button><span class="muted small">Longer runs find more code (RAD presses keys now and then to get past menus).</span></div>
            </div>
          {:else if tab === 'code'}
            <div class="codebar">
              <input class="input sm addr" bind:value={codeFrom} title="From" />–<input class="input sm addr" bind:value={codeTo} title="To" />
              <button class="btn sm" onclick={loadListing}>Show</button>
              <form class="findf" onsubmit={doFind}><Search size={12} /><input bind:value={find} placeholder="Address, label or text" /></form>
              <span class="spacer"></span>
              {#if loadingCode}<Loader size={13} class="spin" />{/if}
              <button class="btn sm" onclick={exportSource}><FileCode size={13} /> Export source</button>
            </div>
            <div class="codeview" bind:this={codeBox}>
              {#each lines as l (l.addr)}
                {#if l.label}<div class="lbl" id="lbl{l.addr}">{l.label}:</div>{/if}
                <button id="L{l.addr}" class="ln {l.kind}" class:sel={selected === l.addr} class:var={l.variable} onclick={() => ((selected = l.addr), (xrefs = null))}>
                  <span class="a">{L.hex4(l.addr).slice(1)}</span>
                  <span class="b">{l.bytes.slice(0, 4).map((b) => L.hex2(b)).join(' ')}</span>
                  <span class="t">{l.text}</span>
                  {#if l.comment}<span class="c">; {l.comment}</span>{/if}
                </button>
              {/each}
            </div>
            {#if selLine}
              <div class="selbar">
                <strong class="mono">{L.hex4(selLine.addr)}</strong>
                <span class="muted small">{selLine.kind === 'code' ? 'code (seen running)' : selLine.kind === 'reachable' ? 'code (not seen running yet)' : selLine.variable ? 'data that changes (a variable)' : selLine.kind}</span>
                <span class="spacer"></span>
                {#if selLine.target != null}<button class="btn sm ghost" onclick={() => goTo(selLine!.target!)}><CornerDownRight size={12} /> Go to {L.hex4(selLine.target)}</button>{/if}
                <button class="btn sm ghost" onclick={() => renameLabel(selLine!.addr, selLine!.label)}><Tag size={12} /> Name</button>
                <button class="btn sm ghost" onclick={() => editComment(selLine!.addr, selLine!.comment)}><MessageSquare size={12} /> Comment</button>
                <button class="btn sm ghost" onclick={() => showXrefs(selLine!.addr)}><Search size={12} /> Who uses it</button>
                <button class="btn sm ghost" onclick={() => patchHere(selLine!.addr)}><Wrench size={12} /> Patch</button>
                <button class="btn sm primary" onclick={() => send(`Explain the code at ${L.hex4(selLine!.addr)} (selected).`)}><Sparkles size={12} /> Explain</button>
              </div>
              {#if xrefs}
                <div class="xrefs">{#each xrefs as x}<button class="link mono" onclick={() => goTo(x.addr)}>{L.hex4(x.addr)} {x.text}</button>{:else}<span class="muted small">Nothing in the program refers to it directly.</span>{/each}</div>
              {/if}
            {/if}
          {:else if tab === 'basic'}
            {#if summary.basic}
              <div class="row"><button class="btn sm" onclick={basicToGames}><FileCode size={13} /> Open as a BASIC project</button><span class="muted small">Edit and run it in Games.</span></div>
              <pre class="listing selectable">{summary.basic}</pre>
            {:else}
              <div class="center note">No BASIC program in memory — it’s all machine code (or a snapshot).</div>
            {/if}
          {:else if tab === 'graphics'}
            <div class="codebar">
              <input class="input sm addr" bind:value={gfxAddr} onchange={drawGfx} title="Address" />
              <label class="small">Width <input class="input sm num" type="number" min="1" max="32" bind:value={gfxWidth} onchange={drawGfx} /> bytes</label>
              <label class="small">Rows <input class="input sm num" type="number" min="8" max="1024" bind:value={gfxRows} onchange={drawGfx} /></label>
              <select class="select sm" bind:value={gfxMode} onchange={drawGfx}><option value="chars">8×8 characters</option><option value="rows">Plain rows</option></select>
              <label class="small">Zoom <input type="range" min="1" max="8" bind:value={gfxZoom} /></label>
              <button class="btn sm ghost" onclick={() => gfxPage(-1)}>‹</button><button class="btn sm ghost" onclick={() => gfxPage(1)}>›</button>
              <span class="spacer"></span>
              <span class="muted small mono">{gfxHover}</span>
            </div>
            <div class="row quick">
              <span class="muted small">Jump to:</span>
              <button class="btn sm ghost" onclick={() => ((gfxAddr = L.hex4(summary!.range[0])), drawGfx())}>Program</button>
              <button class="btn sm ghost" onclick={() => ((gfxAddr = L.hex4(summary!.udg)), (gfxWidth = 1), drawGfx())}>UDGs</button>
              <button class="btn sm ghost" onclick={() => ((gfxAddr = '$3d00'), (gfxWidth = 16), (gfxMode = 'chars'), drawGfx())}>ROM font</button>
              <span class="muted small">Sprites are often 2–4 bytes wide: try widths until shapes appear.</span>
            </div>
            <div class="gfx"><canvas bind:this={gfxCanvas} style="width:{(gfxCanvas?.width ?? 8) * gfxZoom}px" onmousemove={gfxMove} onclick={() => gfxHover && goTo(L.parseAddr(gfxHover)!)}></canvas></div>
            {#if screens.length}
              <div class="screens">{#each screens as s}<figure><canvas width="256" height="192" use:screenCanvas={s.bytes}></canvas><figcaption>{s.title}</figcaption></figure>{/each}</div>
            {/if}
          {:else if tab === 'text'}
            <div class="tlist">
              {#each summary.strings as s}<button class="trow" onclick={() => goTo(s.addr)}><span class="mono muted">{L.hex4(s.addr)}</span> <span class="selectable">{s.text}</span></button>{:else}<div class="muted small">No text found.</div>{/each}
            </div>
          {:else if tab === 'tape'}
            {#if summary.tape.length}
              <table class="grid">
                <thead><tr><th>#</th><th>Block</th><th>Bytes</th><th>Loader</th></tr></thead>
                <tbody>
                  {#each summary.tape as b}
                    <tr>
                      <td>{b.index}</td>
                      <td>{#if b.header}<CassetteTape size={12} /> {['Program', 'Number array', 'Character array', 'Bytes'][b.header.kind] ?? 'Header'} “{b.header.name}” — {b.header.length} bytes{b.header.kind === 3 ? ` to ${L.hex4(b.header.param1)}` : b.header.kind === 0 ? `, starts at line ${b.header.param1}` : ''}{:else if b.screen}A loading screen{:else if b.flag === 0xff}Data{:else}Custom block (flag {b.flag}){/if}</td>
                      <td>{b.length}</td>
                      <td>{b.speed === 'standard' ? 'ROM' : 'custom / turbo'}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            {:else}
              <div class="center note">A snapshot: no tape.</div>
            {/if}
          {:else if tab === 'patches'}
            <form class="pform" onsubmit={addPatch}>
              <div class="row">
                <input class="input sm addr" bind:value={pForm.addr} placeholder="$8123" required />
                <select class="select sm" bind:value={pForm.mode}><option value="asm">Assembler</option><option value="hex">Hex bytes</option></select>
                <input class="input sm grow" bind:value={pForm.note} placeholder="What it does, e.g. Infinite lives" />
                <button class="btn sm primary"><Wrench size={12} /> Add patch</button>
              </div>
              <textarea class="code" bind:value={pForm.code} rows="4" spellcheck="false" placeholder={pForm.mode === 'asm' ? '    nop\n    nop        ; the program’s labels can be used' : '00 00 c9'}></textarea>
              {#if pResult}<div class="small muted pre">{pResult}</div>{/if}
            </form>
            <div class="plist">
              {#each st?.patches ?? [] as p (p.id)}
                <div class="prow">
                  <input type="checkbox" checked={p.enabled} onchange={() => togglePatch(p)} title="Use this patch" />
                  <button class="link mono" onclick={() => goTo(p.addr)}>{L.hex4(p.addr)}</button>
                  <span class="grow">{p.note}</span>
                  <span class="mono muted small">{p.bytes.match(/../g)?.slice(0, 8).join(' ')}{p.bytes.length > 16 ? '…' : ''}</span>
                  <button class="icon-btn sm" title="Delete" onclick={() => deletePatch(p)}><Trash2 size={12} /></button>
                </div>
              {:else}
                <div class="muted small">No patches yet. Add one here, from the Code tab, or ask the AI.</div>
              {/each}
            </div>
            <div class="row">
              <button class="btn sm" onclick={testPatched} disabled={testing}>{#if testing}<Loader size={12} class="spin" />{:else}<CircleCheck size={12} />{/if} Test</button>
              <button class="btn sm primary" onclick={playPatched}><Play size={12} /> Play the patched game</button>
            </div>
            {#if testOut}
              <div class="test" class:bad={testOut.crashed}>
                <div>{#if testOut.crashed}<CircleX size={13} /> It crashed (reset to the start-up screen) within 3 seconds.{:else}<CircleCheck size={13} /> It ran for 3 seconds.{/if}</div>
                {#if testOut.screen.length}<pre class="mono">{testOut.screen.join('\n')}</pre>{/if}
              </div>
            {/if}
          {:else if tab === 'cheats'}
            <p class="small muted">Find where the game keeps things like lives, energy or time: play it, start a search, play on (lose a life), then keep only the bytes that went down. A few rounds narrow it to one.</p>
            <div class="row">
              <button class="btn sm" onclick={playOriginal}><Play size={12} /> Play</button>
              <button class="btn sm primary" onclick={startSearch}>Start a search</button>
              {#if cheatBase}
                <button class="btn sm" onclick={() => filter('down')}>Went down</button>
                <button class="btn sm" onclick={() => filter('up')}>Went up</button>
                <button class="btn sm" onclick={() => filter('same')}>Stayed the same</button>
                <button class="btn sm" onclick={() => filter('changed')}>Changed</button>
                <input class="input sm num" bind:value={equalsN} placeholder="n" /><button class="btn sm" onclick={() => filter('equals')} disabled={!equalsN}>Equals</button>
                <span class="muted small">{candidates.length} candidates</span>
              {/if}
            </div>
            {#if cheatBase && candidates.length <= 200}
              <div class="clist">
                {#each candidates.slice(0, 60) as a (a)}
                  <div class="crow">
                    <button class="link mono" onclick={() => goTo(a)}>{L.hex4(a)}</button>
                    <span class="mono">= {liveMem?.[a]}</span>
                    {#if st?.notes.labels[a]}<span class="muted small">{st.notes.labels[a]}</span>{/if}
                    <span class="spacer"></span>
                    <button class="btn sm ghost" onclick={() => lockAt(a, liveMem?.[a] ?? 0)}><Lock size={11} /> Hold at {liveMem?.[a]}</button>
                    <button class="btn sm ghost" onclick={() => renameLabel(a, st?.notes.labels[a] ?? null)}><Tag size={11} /> Name</button>
                    <button class="btn sm ghost" onclick={() => send(`The byte at ${L.hex4(a)} seems to be a game value (it changed when I played). Find the code that changes it and explain it.`)}><Sparkles size={11} /> Ask AI</button>
                  </div>
                {/each}
              </div>
            {/if}
            {#if locks.length}
              <div class="section-title">Held values</div>
              {#each locks as l}<div class="crow"><span class="mono">{L.hex4(l.addr)} = {l.value}</span><button class="btn sm ghost" onclick={() => unlock(l.addr)}><Unlock size={11} /> Release</button><button class="btn sm ghost" onclick={() => addPatchObj({ id: `p${Date.now()}`, addr: l.addr, bytes: L.hex2(l.value), note: `Start with ${l.value} at ${L.hex4(l.addr)}`, source: '', enabled: true })}><Save size={11} /> As a patch</button></div>{/each}
            {/if}
          {:else if tab === 'board'}
            <div class="boardtab">
              <div class="bscreen">
                <canvas bind:this={boardCanvas} width="352" height="288"></canvas>
                <div class="muted small">Play here (arrows + Tab = joystick) or in the ZX Spectrum view, and watch the board.</div>
                <button class="btn sm" onclick={playOriginal}><Play size={12} /> Load the game</button>
              </div>
              <div class="bdiag"><ZxBoard {stats} fn={fnName} live={!!stats} /></div>
            </div>
          {/if}
        </div>
      </section>

      <aside class="chat">
        <div class="ch"><Sparkles size={13} class="spark" /> <span class="title">Lab AI</span><span class="spacer"></span>
          <button class="icon-btn sm" title="Clear the conversation" disabled={!agent || agent.busy || !agent.items.length} onclick={() => agent?.clear()}><Eraser size={12} /></button>
        </div>
        <div class="thread" bind:this={thread}>
          {#if agent && !agent.items.length}
            <div class="empty">
              Ask about the game: how it works, what a routine does, where the score is kept — or what to change. The AI reads the code, names things as it goes, and can write patches that RAD tests.
              <div class="ideas">{#each PROMPTS as p}<button class="chip" onclick={() => send(p)} disabled={!summary}>{p}</button>{/each}</div>
            </div>
          {/if}
          {#each agent?.items ?? [] as it, i (i)}
            {#if it.kind === 'user'}
              <div class="user">{it.text}</div>
            {:else if it.kind === 'say'}
              <div class="say md selectable">{@html renderMarkdown(it.text)}</div>
            {:else if it.kind === 'tool'}
              <details class="tool" class:bad={it.ok === false}><summary><Cpu size={11} /> {it.title}</summary>{#if it.detail}<pre>{it.detail}</pre>{/if}</details>
            {:else}
              <div class="err">{it.text}</div>
            {/if}
          {/each}
          {#if agent?.busy && agent.status}<div class="status"><Loader size={12} class="spin" /> {agent.status}</div>{/if}
        </div>
        <div class="inrow">
          <textarea rows="2" bind:value={text} placeholder={summary ? 'Ask about the game…' : 'Waiting for the analysis…'} onkeydown={(e) => { if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) { e.preventDefault(); send(); } }} disabled={!summary}></textarea>
          {#if agent?.busy}<button class="send stop" onclick={() => agent?.stop()}><Square size={12} /></button>{:else}<button class="send" disabled={!text.trim() || !summary} onclick={() => send()}><ArrowUp size={15} /></button>{/if}
        </div>
      </aside>
    </div>
  {/if}
</div>

<Modal bind:open={exportOpen} title="Assembly source" width={760}>
  {#if !exported}
    <div class="center"><Loader size={16} class="spin" /></div>
  {:else}
    <div class="row">
      {#if exported.verified}<span class="ok"><CircleCheck size={13} /> Checked: it assembles back to exactly the same bytes</span>{:else}<span class="warnc"><CircleX size={13} /> Not checked{exported.note ? `: ${exported.note.slice(0, 200)}` : ''}</span>{/if}
    </div>
    <pre class="srcview selectable">{exported.source}</pre>
  {/if}
  {#snippet footer()}
    <span class="spacer"></span>
    <button class="btn ghost" onclick={() => exported && navigator.clipboard.writeText(exported.source).then(() => store.toast('success', 'Copied', undefined, 1500))}><Copy size={13} /> Copy</button>
    <button class="btn primary" onclick={exportToGames} disabled={!exported}><FileCode size={13} /> Open as an assembler project</button>
  {/snippet}
</Modal>

<style>
  .lv {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .lv.drop {
    outline: 2px dashed var(--accent);
    outline-offset: -6px;
  }
  header {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    height: var(--titlebar-h);
    padding: 0 12px 0 calc(14px + var(--lead));
    border-bottom: 1px solid var(--border);
  }
  .ttl {
    display: flex;
    align-items: center;
    gap: 7px;
    font-weight: 650;
    font-size: 14px;
  }
  .ttl :global(svg) {
    color: var(--accent);
  }
  .pick {
    max-width: 240px;
    height: 28px;
    padding-top: 0;
    padding-bottom: 0;
    font-size: 12.5px;
  }
  .small {
    font-size: 12px;
  }
  .mono {
    font-family: var(--mono);
  }
  .center {
    flex: 1;
    display: grid;
    place-items: center;
    padding: 24px;
  }
  .note {
    display: flex;
    gap: 8px;
    align-items: center;
    color: var(--text-2);
    font-size: 13px;
  }
  .intro {
    max-width: 520px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    text-align: center;
  }
  .intro p {
    margin: 0;
    color: var(--text-2);
    font-size: 13px;
    line-height: 1.5;
  }
  .intro h2 {
    margin: 0;
  }
  .big {
    display: grid;
    place-items: center;
    width: 60px;
    height: 60px;
    border-radius: 16px;
    background: var(--accent-soft, var(--surface-2));
    color: var(--accent);
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .work {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .tabs {
    flex: none;
    display: flex;
    gap: 2px;
    padding: 6px 10px 0;
    border-bottom: 1px solid var(--border);
    overflow-x: auto;
  }
  .tabs button {
    padding: 6px 12px;
    border: none;
    border-bottom: 2px solid transparent;
    background: none;
    color: var(--text-2);
    font-size: 13px;
    white-space: nowrap;
  }
  .tabs button.on {
    color: var(--text);
    border-bottom-color: var(--accent);
    font-weight: 600;
  }
  .tabs .cnt {
    font-size: 11px;
    color: var(--accent);
  }
  .pane {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }
  .grow {
    flex: 1;
    min-width: 0;
  }
  .num {
    width: 70px;
  }
  .addr {
    width: 80px;
    font-family: var(--mono);
  }
  .input.sm,
  .select.sm {
    height: 28px;
    padding-top: 0;
    padding-bottom: 0;
    font-size: 12.5px;
  }
  .link {
    border: none;
    background: none;
    padding: 0;
    color: var(--accent);
    font: inherit;
    text-align: left;
  }
  .ov {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }
  .mapwrap {
    display: flex;
    gap: 14px;
    align-items: flex-start;
    flex-wrap: wrap;
  }
  .map {
    display: grid;
    grid-template-columns: repeat(16, 14px);
    gap: 2px;
  }
  .cell {
    width: 14px;
    height: 14px;
    border: none;
    border-radius: 2px;
    padding: 0;
  }
  .legend {
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 11.5px;
    color: var(--text-2);
  }
  .legend i {
    display: inline-block;
    width: 10px;
    height: 10px;
    border-radius: 2px;
    margin-right: 6px;
    vertical-align: -1px;
  }
  .facts {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(170px, 1fr));
    gap: 8px;
  }
  .fact {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    font-size: 13px;
  }
  .fact span {
    font-size: 11.5px;
    color: var(--text-3);
  }
  .cols {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 16px;
  }
  .screens {
    display: flex;
    gap: 14px;
    flex-wrap: wrap;
  }
  .screens figure {
    margin: 0;
  }
  .screens canvas {
    width: 384px;
    image-rendering: pixelated;
    border-radius: 4px;
  }
  figcaption {
    font-size: 11.5px;
    color: var(--text-3);
  }
  .codebar {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }
  .findf {
    display: flex;
    align-items: center;
    gap: 5px;
    height: 28px;
    padding: 0 10px;
    border: 1px solid var(--border);
    border-radius: 999px;
    color: var(--text-3);
  }
  .findf input {
    border: none;
    outline: none;
    background: none;
    color: var(--text);
    width: 170px;
    font-size: 12.5px;
  }
  .codeview {
    flex: 1;
    min-height: 200px;
    overflow: auto;
    background: #12121a;
    border-radius: var(--r-sm);
    padding: 6px 0;
    font-family: var(--mono);
    font-size: 12px;
  }
  .lbl {
    color: #ffd75e;
    padding: 6px 10px 0;
  }
  .ln {
    display: flex;
    gap: 12px;
    width: 100%;
    padding: 1px 10px;
    border: none;
    background: none;
    color: #e6e6ef;
    text-align: left;
    font: inherit;
    white-space: pre;
  }
  .ln:hover {
    background: rgba(255, 255, 255, 0.05);
  }
  .ln.sel {
    background: rgba(120, 140, 255, 0.25);
  }
  .ln .a {
    color: #7d7d96;
    flex: none;
    width: 34px;
  }
  .ln .b {
    color: #55556a;
    flex: none;
    width: 86px;
    overflow: hidden;
  }
  .ln .t {
    flex: none;
    min-width: 230px;
  }
  .ln .c {
    color: #7ec4a0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .ln.reachable .t {
    color: #9a9ab5;
  }
  .ln.data .t,
  .ln.text .t {
    color: #e8a96a;
  }
  .ln.var .a::after {
    content: '•';
    color: #ff6b7f;
    margin-left: 3px;
  }
  .selbar {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }
  .xrefs {
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: 12px;
    max-height: 140px;
    overflow: auto;
  }
  .listing,
  .srcview {
    margin: 0;
    padding: 12px;
    border-radius: var(--r-sm);
    background: #12121a;
    color: #e6e6ef;
    font-family: var(--mono);
    font-size: 12px;
    white-space: pre-wrap;
  }
  .srcview {
    max-height: 60vh;
    overflow: auto;
    white-space: pre;
  }
  .quick {
    gap: 4px;
  }
  .gfx {
    overflow: auto;
    background: #0c0c10;
    border-radius: var(--r-sm);
    padding: 8px;
    max-height: 60vh;
  }
  .gfx canvas {
    image-rendering: pixelated;
    cursor: crosshair;
  }
  .tlist {
    display: flex;
    flex-direction: column;
  }
  .trow {
    display: flex;
    gap: 10px;
    padding: 3px 4px;
    border: none;
    background: none;
    color: var(--text);
    text-align: left;
    font-size: 12.5px;
  }
  .trow:hover {
    background: var(--surface-hover);
  }
  .grid {
    border-collapse: collapse;
    font-size: 12.5px;
  }
  .grid td,
  .grid th {
    padding: 5px 10px;
    border-bottom: 1px solid var(--border);
    text-align: left;
  }
  .pform {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .code {
    resize: vertical;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    background: var(--surface-2);
    color: var(--text);
    font-family: var(--mono);
    font-size: 12px;
  }
  .pre {
    white-space: pre-wrap;
  }
  .plist,
  .clist {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .prow,
  .crow {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 6px;
    border-radius: var(--r-sm);
    font-size: 12.5px;
  }
  .prow:hover,
  .crow:hover {
    background: var(--surface-hover);
  }
  .test {
    padding: 8px 10px;
    border-radius: var(--r-sm);
    background: var(--success-soft);
    font-size: 12.5px;
  }
  .test.bad {
    background: var(--danger-soft);
  }
  .test pre {
    margin: 6px 0 0;
    font-size: 11px;
  }
  .boardtab {
    flex: 1;
    min-height: 0;
    display: flex;
    gap: 14px;
  }
  .bscreen {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 6px;
    align-items: flex-start;
  }
  .bscreen canvas {
    width: 352px;
    image-rendering: pixelated;
    border-radius: 4px;
    background: #000;
  }
  .bdiag {
    flex: 1;
    min-width: 0;
  }
  .chat {
    flex: none;
    width: 380px;
    display: flex;
    flex-direction: column;
    border-left: 1px solid var(--border);
  }
  .ch {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
    height: 36px;
    padding: 0 10px 0 12px;
    border-bottom: 1px solid var(--border);
    font-size: 12.5px;
  }
  .ch :global(.spark) {
    color: var(--accent);
  }
  .title {
    font-weight: 600;
  }
  .thread {
    flex: 1;
    overflow: auto;
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    font-size: 13px;
  }
  .empty {
    color: var(--text-3);
    font-size: 12.5px;
    line-height: 1.5;
  }
  .ideas {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 8px;
  }
  .chip {
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--surface);
    color: var(--text-2);
    font-size: 12px;
    padding: 4px 10px;
    text-align: left;
  }
  .user {
    align-self: flex-end;
    max-width: 90%;
    padding: 6px 10px;
    border-radius: 12px;
    background: var(--accent);
    color: white;
    white-space: pre-wrap;
  }
  .say {
    line-height: 1.5;
  }
  .tool summary {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 11.5px;
    color: var(--text-3);
    cursor: pointer;
  }
  .tool.bad summary {
    color: var(--warning);
  }
  .tool pre {
    margin: 4px 0 0;
    font-size: 11px;
    white-space: pre-wrap;
    max-height: 160px;
    overflow: auto;
  }
  .err {
    color: var(--danger);
    font-size: 12.5px;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-3);
  }
  .inrow {
    flex: none;
    display: flex;
    align-items: flex-end;
    gap: 8px;
    padding: 8px 10px;
    border-top: 1px solid var(--border);
  }
  .inrow textarea {
    flex: 1;
    resize: none;
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 7px 10px;
    background: var(--surface-2);
    color: var(--text);
    font: inherit;
    font-size: 13px;
    outline: none;
  }
  .send {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: 50%;
    border: none;
    background: var(--accent);
    color: white;
  }
  .send:disabled {
    opacity: 0.4;
  }
  .send.stop {
    background: var(--danger);
  }
  .ok {
    color: var(--success);
    display: flex;
    gap: 5px;
    align-items: center;
    font-size: 12.5px;
  }
  .warnc {
    color: var(--warning);
    display: flex;
    gap: 5px;
    align-items: center;
    font-size: 12.5px;
  }
</style>
