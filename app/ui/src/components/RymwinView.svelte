<script lang="ts">
  import { onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { open as openDialog } from '@tauri-apps/plugin-dialog';
  import { rad, RadError } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import type { Document } from '../lib/types';
  import VaultMediaPicker from './VaultMediaPicker.svelte';
  import { FilePlus, Vault, RotateCw, Power, TriangleAlert, Loader, Network, SquareTerminal, Upload } from '@lucide/svelte';

  const ENGINE = 'rymwin' as const;

  let viewport: HTMLDivElement | undefined = $state();
  let canvas: HTMLCanvasElement | undefined = $state();
  let ctx: CanvasRenderingContext2D | null = null;
  let canvasCss = $state({ w: 0, h: 0 });
  let engine = $state<'starting' | 'running' | 'stopped' | 'missing'>('starting');
  let engineError = $state('');
  let cursor = $state('default');
  let dropOver = $state(false);
  let vaultOpen = $state(false);
  let importing = $state(false);

  const dpr = () => window.devicePixelRatio || 1;
  const send = (cmd: Record<string, unknown>) => rad.browserSend(ENGINE, cmd).catch(() => {});

  // ------------------------------------------------------------ frames

  let pulling = false;
  let again = false;
  async function pull() {
    if (pulling) {
      again = true;
      return;
    }
    pulling = true;
    try {
      do {
        again = false;
        draw(await rad.browserFrame(ENGINE));
      } while (again);
    } catch {
      /* engine gone; the 'exited' event explains */
    } finally {
      pulling = false;
    }
  }

  function draw(buf: ArrayBuffer) {
    if (!canvas || buf.byteLength < 16) return;
    const h = new DataView(buf);
    const [w, height, y0, rows] = [h.getUint32(0, true), h.getUint32(4, true), h.getUint32(8, true), h.getUint32(12, true)];
    if (!rows || !w) return;
    if (canvas.width !== w || canvas.height !== height) {
      canvas.width = w;
      canvas.height = height;
      canvasCss = { w: w / dpr(), h: height / dpr() };
      ctx = null;
    }
    ctx ??= canvas.getContext('2d', { alpha: false });
    ctx?.putImageData(new ImageData(new Uint8ClampedArray(buf, 16, rows * w * 4), w, rows), 0, y0);
  }

  // ------------------------------------------------------------ engine

  function viewportSize() {
    const r = viewport!.getBoundingClientRect();
    return { w: Math.max(1, Math.round(r.width * dpr())), h: Math.max(1, Math.round(r.height * dpr())) };
  }

  async function start() {
    if (!viewport) return;
    engine = 'starting';
    engineError = '';
    const { w, h } = viewportSize();
    try {
      await rad.browserStart(ENGINE, w, h, dpr());
      engine = 'running';
      canvas?.focus();
      flushPendingDrop();
    } catch (e) {
      engine = e instanceof RadError && (e.code === 'browser_missing' || e.code === 'rymnet_missing') ? 'missing' : 'stopped';
      engineError = e instanceof Error ? e.message : String(e);
    }
  }

  async function restart() {
    await rad.browserStop(ENGINE).catch(() => {});
    start();
  }

  function onEvent(ev: any) {
    if (ev.type === 'cursor') cursor = ev.cursor;
    else if (ev.type === 'open_url' && /^https?:/.test(ev.url)) {
      store.pendingBrowserUrl = ev.url;
      store.navigate({ kind: 'browser' });
    } else if (ev.type === 'exited') {
      engine = 'stopped';
      engineError = 'RYMWIN closed.';
    }
  }

  // ------------------------------------------------------------ files in

  function dropPaths(paths: string[]) {
    if (!paths.length) return;
    send({ cmd: 'drop', paths });
    store.toast('success', `Copying ${paths.length} item${paths.length > 1 ? 's' : ''} into RYMWIN`, 'They land in /Downloads. Log in to RYMWIN first if you haven’t.');
  }

  function flushPendingDrop() {
    const p = store.pendingRymwinDrop;
    if (!p) return;
    store.pendingRymwinDrop = null;
    // Give a freshly started desktop a moment to draw its first frame.
    setTimeout(() => dropPaths(p), 400);
  }

  async function addFiles() {
    const r = await openDialog({ multiple: true });
    const paths = Array.isArray(r) ? r : r ? [r] : [];
    dropPaths(paths.map(String));
  }

  async function addFolder() {
    const r = await openDialog({ directory: true });
    if (r) dropPaths([String(r)]);
  }

  async function addFromVault(docs: Document[]) {
    if (!docs.length) return;
    importing = true;
    try {
      const dir = await rad.rymnetTempDir();
      const used = new Set<string>();
      const paths: string[] = [];
      for (const d of docs) {
        let name = (d.original_name || d.title || `document-${d.id}`).replace(/[\/\\:]/g, '_');
        if (d.source_kind === 'note' && !/\.\w+$/.test(name)) name += '.md';
        while (used.has(name)) name = name.replace(/(\.\w+)?$/, (ext) => ` (${d.id})${ext}`);
        used.add(name);
        const dest = `${dir}/${name}`;
        await rad.exportDocument(d.id, dest);
        paths.push(dest);
      }
      dropPaths(paths);
    } catch (e) {
      store.error(e, 'Couldn’t copy from the vault');
    } finally {
      importing = false;
    }
  }

  // ------------------------------------------------------------ input

  function point(e: MouseEvent) {
    const r = canvas!.getBoundingClientRect();
    return { x: (e.clientX - r.left) * dpr(), y: (e.clientY - r.top) * dpr() };
  }

  const mods = (e: MouseEvent | KeyboardEvent) => ({ shift: e.shiftKey, ctrl: e.ctrlKey, alt: e.altKey, meta: e.metaKey });

  let moveQueued: MouseEvent | null = null;
  function onMove(e: MouseEvent) {
    if (!moveQueued)
      requestAnimationFrame(() => {
        if (moveQueued) send({ cmd: 'mouse_move', ...point(moveQueued) });
        moveQueued = null;
      });
    moveQueued = e;
  }

  function onDown(e: MouseEvent) {
    canvas?.focus();
    send({ cmd: 'mouse_button', action: 'down', button: e.button, ...point(e), ...mods(e) });
    const up = (u: MouseEvent) => {
      window.removeEventListener('mouseup', up);
      send({ cmd: 'mouse_button', action: 'up', button: u.button, ...point(u), ...mods(u) });
    };
    window.addEventListener('mouseup', up);
  }

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    const k = e.deltaMode === 0 ? dpr() : 1;
    send({ cmd: 'wheel', dx: e.deltaX * k, dy: e.deltaY * k, mode: e.deltaMode, ...point(e), ...mods(e) });
  }

  const isMac = document.documentElement.dataset.os === 'macos';

  function onKey(e: KeyboardEvent, state: 'down' | 'up') {
    if (e.isComposing) return;
    const mod = isMac ? e.metaKey : e.ctrlKey;
    if (mod && state === 'down') {
      const k = e.key.toLowerCase();
      if (k === 'c' || k === 'x') {
        e.preventDefault();
        return send({ cmd: k === 'c' ? 'copy' : 'cut' });
      }
      if (k === 'v') {
        e.preventDefault();
        return rad.rymwinPaste().catch(() => {});
      }
    }
    // ⌘A / ⌘Z / ⌘Y go to RYMWIN's editors; other app-wide shortcuts (⌘K, ⌘N, ⌘\…) stay with RAD.
    if (mod && e.key.length === 1 && !['a', 'z', 'y'].includes(e.key.toLowerCase())) return;
    e.preventDefault();
    e.stopPropagation();
    send({ cmd: 'key', state, key: e.key, code: e.code, repeat: e.repeat, ...mods(e) });
  }

  function onCompositionEnd(e: CompositionEvent) {
    if (e.data) send({ cmd: 'text', text: e.data });
  }

  onMount(() => {
    const un: Array<() => void> = [];
    listen<string>('browser-frame', (e) => e.payload === ENGINE && pull()).then((u) => un.push(u));
    listen<any>('browser-event', (e) => e.payload.engine === ENGINE && onEvent(e.payload)).then((u) => un.push(u));
    // Files dragged in from Finder (or anywhere outside RAD).
    getCurrentWebview()
      .onDragDropEvent((ev) => {
        if (store.route.kind !== 'rymwin' || engine !== 'running') return;
        const p = ev.payload;
        if (p.type === 'enter' || p.type === 'over') {
          if (!dropOver) send({ cmd: 'hover', active: true });
          dropOver = true;
        } else if (p.type === 'leave') {
          dropOver = false;
          send({ cmd: 'hover', active: false });
        } else if (p.type === 'drop') {
          dropOver = false;
          dropPaths(p.paths);
        }
      })
      .then((u) => un.push(u));
    start();

    let t: ReturnType<typeof setTimeout> | undefined;
    const ro = new ResizeObserver(() => {
      clearTimeout(t);
      t = setTimeout(() => {
        if (engine !== 'running' || !viewport) return;
        const { w, h } = viewportSize();
        send({ cmd: 'resize', w, h, scale: dpr() });
      }, 60);
    });
    if (viewport) ro.observe(viewport);
    return () => {
      ro.disconnect();
      clearTimeout(t);
      un.forEach((u) => u());
    };
  });

  // A request from Files or the assistant while the view is already open.
  $effect(() => {
    if (store.pendingRymwinDrop && engine === 'running') flushPendingDrop();
  });

  function openRymsh() {
    store.pendingTerminal = { kind: 'rymsh' };
    store.navigate({ kind: 'terminal' });
  }
</script>

<div class="rymwin">
  <header class="drag">
    <h1>RYMWIN</h1>
    <span class="muted no-drag">Rymnet Desktop</span>
    <span class="spacer"></span>
    <button class="btn sm ghost no-drag" disabled={engine !== 'running'} title="Copy files from this computer into RYMWIN’s /Downloads" onclick={addFiles}><FilePlus size={14} /> Add files</button>
    <button class="btn sm ghost no-drag" disabled={engine !== 'running'} title="Copy a folder into RYMWIN’s /Downloads" onclick={addFolder}><Upload size={14} /> Add folder</button>
    <button class="btn sm ghost no-drag" disabled={engine !== 'running' || importing} title="Copy documents from your RAD vault into RYMWIN’s /Downloads" onclick={() => (vaultOpen = true)}>
      {#if importing}<Loader size={14} class="spin" />{:else}<Vault size={14} />{/if} From vault
    </button>
    <button class="icon-btn no-drag" title="rymsh — the Rymnet shell, in Terminal" onclick={openRymsh}><SquareTerminal size={16} /></button>
    <button class="icon-btn no-drag" title="RYMNET — network, peers and assistant" onclick={() => store.navigate({ kind: 'rymnet' })}><Network size={16} /></button>
    <button class="icon-btn no-drag" title="Restart RYMWIN" onclick={restart}><RotateCw size={16} /></button>
  </header>

  <div class="viewport" bind:this={viewport}>
    <canvas
      bind:this={canvas}
      tabindex="0"
      style="cursor:{cursor};width:{canvasCss.w}px;height:{canvasCss.h}px"
      onmousemove={onMove}
      onmousedown={onDown}
      onmouseleave={() => send({ cmd: 'mouse_leave' })}
      onwheel={onWheel}
      oncontextmenu={(e) => e.preventDefault()}
      onkeydown={(e) => onKey(e, 'down')}
      onkeyup={(e) => onKey(e, 'up')}
      oncompositionend={onCompositionEnd}
      onfocus={() => send({ cmd: 'focus', focused: true })}
      onblur={() => send({ cmd: 'focus', focused: false })}
    ></canvas>

    {#if engine === 'starting'}
      <div class="notice"><div class="card"><Loader size={20} class="spin" /> Starting RYMWIN…</div></div>
    {:else if engine === 'missing' || engine === 'stopped'}
      <div class="notice">
        <div class="card">
          <TriangleAlert size={22} />
          <strong>{engine === 'missing' ? 'RYMWIN isn’t installed' : 'RYMWIN stopped'}</strong>
          <p>{engineError}</p>
          <button class="btn primary" onclick={start}><Power size={14} /> Start RYMWIN</button>
        </div>
      </div>
    {/if}
  </div>
</div>

<VaultMediaPicker bind:open={vaultOpen} kind="any" onpick={addFromVault} />

<style>
  .rymwin {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  header {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
    height: var(--titlebar-h);
    padding: 0 12px 0 calc(14px + var(--lead));
    border-bottom: 1px solid var(--border);
  }
  h1 {
    margin: 0;
    font-size: 15px;
    font-weight: 650;
  }
  .muted {
    color: var(--text-3);
    font-size: 12.5px;
  }
  .spacer {
    flex: 1;
  }
  .viewport {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: hidden;
    /* RYMWIN's desktop teal, shown while it starts. */
    background: #008080;
  }
  canvas {
    display: block;
    outline: none;
  }
  .notice {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    padding: 16px;
  }
  .card {
    max-width: 420px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    text-align: center;
    padding: 20px 22px;
    border-radius: var(--r);
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow-lg);
  }
  .card p {
    margin: 0;
    color: var(--text-2);
    font-size: 13px;
    white-space: pre-wrap;
  }
</style>
