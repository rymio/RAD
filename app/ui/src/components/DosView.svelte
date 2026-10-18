<script lang="ts">
  import { onMount } from 'svelte';
  import { ask } from '@tauri-apps/plugin-dialog';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { rad } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import { formatBytes } from '../lib/util';
  import * as D from '../lib/dos';
  import Menu from './Menu.svelte';
  import Modal from './Modal.svelte';
  import {
    Computer, Power, RotateCcw, Pause, Play, Volume2, VolumeX, Settings, Disc, Save, Eject, FolderOpen, FolderPlus, FolderInput, Folder, FileText,
    ChevronRight, HardDrive, Trash2, MoreHorizontal, Loader, MousePointer, Keyboard, CornerLeftUp, Upload,
  } from '@lucide/svelte';

  let canvas: HTMLCanvasElement | undefined = $state();
  let screenBox: HTMLDivElement | undefined = $state();
  let size = $state<[number, number]>([640, 400]);
  let scale = $state(1);
  let img: ImageData | null = null;

  let status = $state<{ engine: boolean; running: boolean; hdd: string; used: number } | null>(null);
  let on = $state(false);
  let starting = $state(false);
  let paused = $state(false);
  let settings = $state<D.DosSettings>(D.loadSettings());
  let settingsOpen = $state(false);
  let draft = $state<D.DosSettings>(D.loadSettings());
  let volume = $state(Number(read('rad-dos-volume') ?? 0.8));
  let muted = $state(read('rad-dos-muted') === '1');
  let captured = $state(false);
  let drive = $state<string | null>(null);
  let dropping = $state(false);

  function read(k: string) {
    try {
      return localStorage.getItem(k);
    } catch {
      return null;
    }
  }
  function write(k: string, v: string) {
    try {
      localStorage.setItem(k, v);
    } catch {
      /* private mode */
    }
  }

  // ------------------------------------------------------------------ power

  async function refreshStatus() {
    status = await rad.dosStatus().catch(() => null);
    on = !!status?.running;
  }

  async function powerOn() {
    if (starting) return;
    starting = true;
    try {
      await rad.dosStart({ ...settings });
      on = true;
      paused = false;
      drive = null;
      rad.dosSend({ cmd: 'volume', value: muted ? 0 : volume }).catch(() => {});
      canvas?.focus();
    } catch (e) {
      store.error(e, 'Couldn’t turn the PC on');
    } finally {
      starting = false;
    }
  }

  async function powerOff() {
    if (!(await ask('Turn the PC off? Anything not saved in a running program is lost (files on drive C: stay).', { title: 'Turn off', kind: 'warning' }))) return;
    await rad.dosStop().catch(() => {});
    on = false;
    refreshStatus();
  }

  function send(cmd: Record<string, unknown>) {
    if (on) rad.dosSend(cmd).catch(() => {});
  }

  async function reset() {
    if (!(await ask('Restart the PC (like pressing its reset button)?', { title: 'Reset', kind: 'warning' }))) return;
    send({ cmd: 'reset' });
  }

  function togglePause() {
    paused = !paused;
    send({ cmd: 'pause', on: paused });
  }

  function setVolume(v: number) {
    volume = v;
    write('rad-dos-volume', String(v));
    send({ cmd: 'volume', value: muted ? 0 : v });
  }

  function toggleMute() {
    muted = !muted;
    write('rad-dos-muted', muted ? '1' : '0');
    send({ cmd: 'volume', value: muted ? 0 : volume });
  }

  async function applySettings() {
    settings = { ...draft };
    D.saveSettings(settings);
    settingsOpen = false;
    if (on && (await ask('Restart the PC with the new settings now?', { title: 'PC settings' }))) powerOn();
  }

  // ------------------------------------------------------------------ screen

  async function pull() {
    const buf = await rad.dosFrame().catch(() => null);
    if (!buf || !canvas) return;
    const next = D.decodeFrame(buf, img);
    if (!next) return;
    img = next;
    if (size[0] !== img.width || size[1] !== img.height) {
      size = [img.width, img.height];
      fit();
    }
    canvas.getContext('2d')?.putImageData(img, 0, 0);
  }

  /** The picture fills the space at 4:3, as on a PC monitor (320×200 and 640×400 are stretched). */
  const shown = $derived.by(() => {
    const [w, h] = size;
    const tall = w / h > 1.4 ? (w * 3) / 4 : h;
    return [w * scale, tall * scale];
  });

  function fit() {
    if (!screenBox) return;
    const r = screenBox.getBoundingClientRect();
    const [w, h] = size;
    const tall = w / h > 1.4 ? (w * 3) / 4 : h;
    scale = Math.max(0.5, Math.min((r.width - 16) / w, (r.height - 16) / tall));
  }

  // ------------------------------------------------------------------ keyboard and mouse

  const held = new Set<string>();

  function typingInField(e: KeyboardEvent) {
    const t = e.target as HTMLElement | null;
    return !!t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT' || t.isContentEditable);
  }

  function onKeyDown(e: KeyboardEvent) {
    if (!on || e.metaKey || typingInField(e) || settingsOpen) return;
    const k = D.pcKey(e.code);
    if (k == null) return;
    e.preventDefault();
    if (held.has(e.code)) return;
    held.add(e.code);
    send({ cmd: 'key', code: k, down: true });
  }

  function onKeyUp(e: KeyboardEvent) {
    if (!held.delete(e.code)) return;
    e.preventDefault();
    const k = D.pcKey(e.code);
    if (k != null) send({ cmd: 'key', code: k, down: false });
  }

  function releaseAll() {
    held.clear();
    mouse.buttons = 0;
    send({ cmd: 'release_all' });
  }

  const mouse = { dx: 0, dy: 0, buttons: 0, wheel: 0, dirty: false };

  function capture() {
    canvas?.focus();
    if (!on || captured) return;
    canvas?.requestPointerLock?.();
  }

  function onPointerLock() {
    captured = document.pointerLockElement === canvas;
    if (!captured) releaseMouseButtons();
  }

  function releaseMouseButtons() {
    if (mouse.buttons) {
      mouse.buttons = 0;
      mouse.dirty = true;
    }
  }

  function onMouseMove(e: MouseEvent) {
    if (!on || (!captured && !(e.target === canvas))) return;
    mouse.dx += e.movementX;
    mouse.dy += e.movementY;
    mouse.dirty = true;
  }

  function onMouseButton(e: MouseEvent, down: boolean) {
    if (!captured) return;
    const bit = e.button === 0 ? 1 : e.button === 2 ? 2 : e.button === 1 ? 4 : 0;
    mouse.buttons = down ? mouse.buttons | bit : mouse.buttons & ~bit;
    mouse.dirty = true;
    e.preventDefault();
  }

  function onWheel(e: WheelEvent) {
    if (!captured) return;
    mouse.wheel = Math.sign(e.deltaY);
    mouse.dirty = true;
  }

  function flushMouse() {
    if (mouse.dirty && on) {
      send({ cmd: 'mouse', dx: Math.round(mouse.dx), dy: Math.round(mouse.dy), buttons: mouse.buttons, wheel: mouse.wheel });
      mouse.dx = mouse.dy = mouse.wheel = 0;
      mouse.dirty = false;
    }
    raf = requestAnimationFrame(flushMouse);
  }
  let raf = 0;

  // ------------------------------------------------------------------ drives A: and D:

  async function insertImage(path?: string) {
    if (!path) {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const r = await open({ multiple: false, title: 'Put a floppy or CD image in the drive', filters: [{ name: 'Floppy and CD images', extensions: ['img', 'ima', 'vfd', 'iso', 'cue', 'chd'] }] });
      if (typeof r !== 'string') return;
      path = r;
    }
    if (!on) await powerOn();
    try {
      await rad.dosSend({ cmd: 'insert', path });
      D.rememberImage(path);
      drive = path.split('/').pop() ?? path;
      const cd = /\.(iso|cue|chd)$/i.test(path);
      store.toast('success', cd ? 'CD in drive D:' : 'Floppy in drive A:', `Type ${cd ? 'D:' : 'A:'} at the prompt to use it.`, 3500);
      canvas?.focus();
    } catch (e) {
      store.error(e, 'Couldn’t insert it');
    }
  }

  function eject() {
    send({ cmd: 'eject' });
    drive = null;
  }

  const recentMenu = $derived([
    { label: 'Choose an image…', icon: Disc, onclick: () => insertImage() },
    ...(drive ? [{ label: 'Eject', icon: Eject, onclick: eject }] : []),
    ...(D.recentImages().length ? [null] : []),
    ...D.recentImages().slice(0, 8).map((p) => ({ label: p.split('/').pop() ?? p, icon: Save, onclick: () => insertImage(p) })),
  ]);

  // ------------------------------------------------------------------ drive C:

  let cwd = $state<string[]>([]);
  let files = $state<{ name: string; dir: boolean; size: number }[]>([]);
  let importing = $state(false);

  async function listFiles() {
    files = await rad.dosFiles(cwd.join('\\')).catch(() => []);
    refreshStatus();
  }

  function openDir(name: string) {
    cwd = [...cwd, name];
    listFiles();
  }

  function up(to = cwd.length - 1) {
    cwd = cwd.slice(0, Math.max(0, to));
    listFiles();
  }

  const isProgram = (n: string) => /\.(exe|com|bat)$/i.test(n);

  /** Types the commands to run a program at the DOS prompt. */
  function runProgram(name: string) {
    if (!on) return store.toast('info', 'The PC is off', 'Turn it on first.', 2500);
    const dir = cwd.length ? '\\' + cwd.join('\\') : '\\';
    send({ cmd: 'type', text: `C:\ncd ${dir}\n${name.replace(/\.(exe|com|bat)$/i, '')}\n` });
    canvas?.focus();
  }

  async function importPaths(paths: string[]) {
    if (!paths.length) return;
    importing = true;
    try {
      const into = cwd.length ? cwd.join('\\') : 'GAMES';
      const made = await rad.dosImport(paths, into);
      store.toast('success', 'Added to drive C:', `${[...new Set(made)].join(', ')}${on ? ' — type RESCAN at the DOS prompt to see new files.' : ''}`, 6000);
      if (!cwd.length) cwd = ['GAMES'];
      await listFiles();
    } catch (e) {
      store.error(e, 'Couldn’t add it');
    } finally {
      importing = false;
    }
  }

  async function pickImport(folders: boolean) {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const r = await open(
      folders
        ? { multiple: true, directory: true, title: 'Copy folders onto drive C:' }
        : { multiple: true, title: 'Copy games and files onto drive C: (ZIPs are unpacked)' },
    );
    importPaths(r == null ? [] : Array.isArray(r) ? r : [r]);
  }

  async function newFolder() {
    const n = window.prompt('New folder name (8 letters or fewer is best for DOS)')?.trim().toUpperCase().replace(/[^A-Z0-9_-]/g, '');
    if (!n) return;
    await rad.dosMkdir([...cwd, n].join('\\')).catch((e) => store.error(e));
    listFiles();
  }

  async function remove(f: { name: string; dir: boolean }) {
    if (!(await ask(`Delete ${f.dir ? 'the folder' : ''} C:\\${[...cwd, f.name].join('\\')}${f.dir ? ' and everything in it' : ''}?`, { title: 'Delete', kind: 'warning' }))) return;
    await rad.dosDelete([...cwd, f.name].join('\\')).catch((e) => store.error(e));
    listFiles();
  }

  async function reveal() {
    if (!status?.hdd) return;
    const { revealItemInDir } = await import('@tauri-apps/plugin-opener');
    revealItemInDir(status.hdd).catch((e) => store.error(e));
  }

  onMount(() => {
    refreshStatus().then(() => {
      if (status?.engine && !status.running) powerOn();
      else if (status?.running) {
        send({ cmd: 'pause', on: false });
        send({ cmd: 'volume', value: muted ? 0 : volume });
        pull();
      }
    });
    listFiles();
    const un: (() => void)[] = [];
    listen('dos-frame', () => pull()).then((u) => un.push(u));
    listen<Record<string, any>>('dos-event', (e) => {
      const v = e.payload;
      if (v.event === 'exit') {
        on = false;
        captured = false;
        document.exitPointerLock?.();
        refreshStatus();
      } else if (v.event === 'error') store.toast('error', 'DOS', v.message ?? '');
      else if (v.event === 'message' && v.text) store.toast('info', 'DOS', v.text, 3000);
    }).then((u) => un.push(u));
    const ro = new ResizeObserver(fit);
    if (screenBox) ro.observe(screenBox);
    window.addEventListener('keydown', onKeyDown, true);
    window.addEventListener('keyup', onKeyUp, true);
    window.addEventListener('blur', releaseAll);
    document.addEventListener('pointerlockchange', onPointerLock);
    window.addEventListener('mousemove', onMouseMove);
    raf = requestAnimationFrame(flushMouse);
    const unDrop = getCurrentWebview().onDragDropEvent((ev) => {
      const p = ev.payload;
      if (p.type === 'enter' || p.type === 'over') dropping = true;
      else if (p.type === 'leave') dropping = false;
      else if (p.type === 'drop') {
        dropping = false;
        const images = p.paths.filter((x) => /\.(img|ima|vfd|iso|cue|chd)$/i.test(x));
        if (images.length) insertImage(images[0]);
        const rest = p.paths.filter((x) => !images.includes(x));
        if (rest.length) importPaths(rest);
      }
    });
    return () => {
      un.forEach((u) => u());
      ro.disconnect();
      cancelAnimationFrame(raf);
      window.removeEventListener('keydown', onKeyDown, true);
      window.removeEventListener('keyup', onKeyUp, true);
      window.removeEventListener('blur', releaseAll);
      document.removeEventListener('pointerlockchange', onPointerLock);
      window.removeEventListener('mousemove', onMouseMove);
      document.exitPointerLock?.();
      releaseAll();
      // Out of sight: the PC waits (and goes quiet).
      if (on) rad.dosSend({ cmd: 'pause', on: true }).catch(() => {});
      unDrop.then((u) => u());
    };
  });
</script>

<div class="dv" class:drop={dropping}>
  <header class="drag">
    <div class="ttl no-drag"><Computer size={16} /> DOS PC</div>
    <span class="pill no-drag" class:ok={on}>{on ? (paused ? 'Paused' : 'On') : 'Off'}</span>
    <span class="muted small no-drag">{settings.memory} MB · {D.SPEEDS.find((s) => s[0] === settings.speed)?.[1].split(' (')[0] ?? settings.speed} · {settings.sound === 'none' ? 'no sound card' : settings.sound.toUpperCase()}</span>
    <span class="spacer"></span>
    <div class="no-drag">
      <Menu items={recentMenu}>
        {#snippet trigger(toggle)}<button class="btn sm" onclick={toggle} title="Floppy (A:) and CD (D:) images"><Disc size={13} /> {drive ?? 'Insert disk'}</button>{/snippet}
      </Menu>
    </div>
    <button class="icon-btn no-drag" title={paused ? 'Resume' : 'Pause'} onclick={togglePause} disabled={!on}>{#if paused}<Play size={15} />{:else}<Pause size={15} />{/if}</button>
    <button class="icon-btn no-drag" title="Reset" onclick={reset} disabled={!on}><RotateCcw size={15} /></button>
    <div class="vol no-drag">
      <button class="icon-btn" title={muted ? 'Sound on' : 'Mute'} onclick={toggleMute}>{#if muted || volume === 0}<VolumeX size={15} />{:else}<Volume2 size={15} />{/if}</button>
      <input type="range" min="0" max="1" step="0.05" value={volume} oninput={(e) => setVolume(+(e.currentTarget as HTMLInputElement).value)} disabled={muted} />
    </div>
    <button class="icon-btn no-drag" title="PC settings" onclick={() => ((draft = { ...settings }), (settingsOpen = true))}><Settings size={15} /></button>
    {#if on}
      <button class="btn sm no-drag" onclick={powerOff}><Power size={13} /> Turn off</button>
    {:else}
      <button class="btn sm primary no-drag" onclick={powerOn} disabled={starting || status?.engine === false}>{#if starting}<Loader size={13} class="spin" />{:else}<Power size={13} />{/if} Turn on</button>
    {/if}
  </header>

  <div class="body">
    <aside class="disk">
      <div class="dh">
        <HardDrive size={14} />
        <span class="section-title">Drive C:</span>
        <span class="spacer"></span>
        <span class="muted small" title="Space used on drive C:">{status ? formatBytes(status.used) : ''}</span>
      </div>
      <div class="crumbs">
        <button class="link" onclick={() => up(0)}>C:\</button>
        {#each cwd as c, i}<ChevronRight size={11} /><button class="link" onclick={() => up(i + 1)}>{c}</button>{/each}
      </div>
      <div class="flist">
        {#if cwd.length}<button class="fi" onclick={() => up()}><CornerLeftUp size={13} /> <span>..</span></button>{/if}
        {#each files as f (f.name)}
          <div class="fr">
            {#if f.dir}
              <button class="fi" onclick={() => openDir(f.name)}><Folder size={13} /> <span class="truncate">{f.name}</span><span class="spacer"></span><span class="muted sz">{formatBytes(f.size)}</span></button>
            {:else}
              <button class="fi" class:prog={isProgram(f.name)} ondblclick={() => isProgram(f.name) && runProgram(f.name)} title={isProgram(f.name) ? 'Double-click to run it' : ''}>
                <FileText size={13} /> <span class="truncate">{f.name}</span><span class="spacer"></span><span class="muted sz">{formatBytes(f.size)}</span>
              </button>
              {#if isProgram(f.name)}<button class="btn xs run" onclick={() => runProgram(f.name)} title="Type its name at the DOS prompt"><Play size={10} /> Run</button>{/if}
            {/if}
            <Menu items={[{ label: 'Delete', icon: Trash2, danger: true, onclick: () => remove(f) }]}>
              {#snippet trigger(toggle)}<button class="icon-btn sm more" onclick={toggle} aria-label="More"><MoreHorizontal size={12} /></button>{/snippet}
            </Menu>
          </div>
        {:else}
          <div class="empty">
            {#if cwd.length}This folder is empty.{:else}The hard disk is empty. Add a game (a ZIP or folder) below, or put in an install floppy or CD and install from DOS.{/if}
          </div>
        {/each}
      </div>
      <div class="dactions">
        <button class="btn sm primary" onclick={() => pickImport(false)} disabled={importing}>{#if importing}<Loader size={12} class="spin" />{:else}<Upload size={12} />{/if} Add games or files</button>
        <button class="btn sm" onclick={() => pickImport(true)} disabled={importing} title="Copy a folder from this Mac"><FolderInput size={12} /> Folder</button>
        <button class="icon-btn sm" onclick={newFolder} title="New folder"><FolderPlus size={13} /></button>
        <button class="icon-btn sm" onclick={reveal} title="Show drive C: in Finder"><FolderOpen size={13} /></button>
      </div>
    </aside>

    <section class="main">
      <div class="screen" bind:this={screenBox}>
        {#if status?.engine === false}
          <div class="off">
            <Computer size={30} />
            <p>The DOS engine isn’t built yet.<br />Run <code>./scripts/build-dos.sh</code>, then come back.</p>
          </div>
        {:else}
          <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
          <canvas
            bind:this={canvas}
            width={size[0]}
            height={size[1]}
            tabindex="0"
            style="width:{shown[0]}px;height:{shown[1]}px"
            class:dim={!on}
            onclick={capture}
            onmousedown={(e) => onMouseButton(e, true)}
            onmouseup={(e) => onMouseButton(e, false)}
            onwheel={onWheel}
            oncontextmenu={(e) => e.preventDefault()}
          ></canvas>
          {#if !on}
            <div class="overlay"><button class="btn primary" onclick={powerOn} disabled={starting}><Power size={15} /> Turn on</button></div>
          {:else if paused}
            <div class="overlay" role="button" tabindex="-1" onclick={togglePause} onkeydown={() => {}}><Play size={42} /> Paused</div>
          {/if}
        {/if}
      </div>
      <div class="bar">
        <MousePointer size={13} />
        <span class="muted">{captured ? 'The mouse is the PC’s — press Esc to get it back' : 'Click the screen to use the mouse in DOS'}</span>
        <span class="spacer"></span>
        <Keyboard size={13} />
        <span class="muted">Keys go to the PC while this view is open (shortcuts with ⌘ or the Windows key stay with RAD)</span>
      </div>
    </section>
  </div>
</div>

<Modal bind:open={settingsOpen} title="PC settings" width={480}>
  <div class="settings">
    <label><span>Memory</span>
      <select class="select" bind:value={draft.memory}>{#each ['4', '8', '16', '32', '64'] as m}<option value={m}>{m} MB{m === '8' ? ' (plenty for DOS games)' : ''}</option>{/each}</select>
    </label>
    <label><span>Speed</span>
      <select class="select" bind:value={draft.speed}>{#each D.SPEEDS as [v, l]}<option value={v}>{l}</option>{/each}</select>
    </label>
    <label><span>Sound card</span>
      <select class="select" bind:value={draft.sound}>
        <option value="sb16">Sound Blaster 16</option><option value="sbpro2">Sound Blaster Pro 2</option><option value="sbpro1">Sound Blaster Pro</option>
        <option value="sb2">Sound Blaster 2.0</option><option value="sb1">Sound Blaster 1.0</option><option value="none">None</option>
      </select>
    </label>
    <label class="chk"><input type="checkbox" bind:checked={draft.gus} /> Gravis Ultrasound too (port 240, IRQ 5, DMA 3)</label>
    <label><span>Graphics card</span>
      <select class="select" bind:value={draft.graphics}>
        <option value="svga">SVGA</option><option value="vga">VGA</option><option value="ega">EGA</option><option value="cga">CGA</option><option value="tandy">Tandy</option><option value="hercules">Hercules</option>
      </select>
    </label>
    {#if draft.graphics === 'svga'}
      <label><span>SVGA chip</span>
        <select class="select" bind:value={draft.svga}>
          <option value="svga_s3">S3 Trio64 (VESA 2.0)</option><option value="vesa_nolfb">S3 Trio64, no linear frame buffer</option><option value="vesa_oldvbe">S3 Trio64, VESA 1.3</option>
          <option value="svga_et4000">Tseng ET4000</option><option value="svga_et3000">Tseng ET3000</option><option value="svga_paradise">Paradise PVGA1A</option>
        </select>
      </label>
      <label><span>Video memory</span>
        <select class="select" bind:value={draft.svgamem}>{#each ['1', '2', '4', '8'] as m}<option value={m}>{m} MB</option>{/each}</select>
      </label>
    {/if}
    <label><span>Keyboard layout</span>
      <select class="select" bind:value={draft.keyboard}>
        {#each [['us', 'US'], ['uk', 'UK'], ['gr', 'German'], ['fr', 'French'], ['sp', 'Spanish'], ['it', 'Italian'], ['sv', 'Swedish'], ['dk', 'Danish'], ['nl', 'Dutch'], ['no', 'Norwegian'], ['su', 'Finnish'], ['pl', 'Polish'], ['ru', 'Russian']] as [v, l]}<option value={v}>{l}</option>{/each}
      </select>
    </label>
    <p class="muted small">The Sound Blaster is at port 220, IRQ 7, DMA 1 (high DMA 5) — choose those in a game’s SETUP. Changes take effect when the PC restarts.</p>
  </div>
  {#snippet footer()}
    <button class="btn ghost" onclick={() => (draft = { ...D.DEFAULTS })}>Defaults</button>
    <span class="spacer"></span>
    <button class="btn" onclick={() => (settingsOpen = false)}>Cancel</button>
    <button class="btn primary" onclick={applySettings}>Save</button>
  {/snippet}
</Modal>

<style>
  .dv {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .dv.drop {
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
  .pill {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--text-3);
  }
  .pill.ok {
    background: color-mix(in oklab, #2fbf71 20%, transparent);
    color: #2fbf71;
  }
  .small {
    font-size: 12px;
  }
  .vol {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .vol input {
    width: 80px;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .disk {
    flex: none;
    width: 260px;
    display: flex;
    flex-direction: column;
    border-right: 1px solid var(--border);
    min-height: 0;
  }
  .dh {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 36px;
    padding: 0 12px;
  }
  .dh :global(svg) {
    color: var(--text-3);
  }
  .crumbs {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 2px;
    padding: 0 12px 6px;
    font-family: var(--mono);
    font-size: 12px;
  }
  .crumbs :global(svg) {
    color: var(--text-3);
  }
  .link {
    border: none;
    background: none;
    padding: 0;
    color: var(--accent);
    font: inherit;
  }
  .flist {
    flex: 1;
    overflow: auto;
    padding: 0 6px 8px;
  }
  .fr {
    display: flex;
    align-items: center;
    border-radius: var(--r-sm);
  }
  .fr:hover {
    background: var(--surface-hover);
  }
  .fi {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 7px;
    height: 28px;
    padding: 0 8px;
    border: none;
    background: transparent;
    color: var(--text);
    font-size: 12.5px;
    font-family: var(--mono);
    text-align: left;
    border-radius: var(--r-sm);
  }
  .fi :global(svg) {
    flex: none;
    color: var(--text-3);
  }
  .fi.prog {
    color: var(--accent);
  }
  .sz {
    font-size: 11px;
    font-family: var(--font);
  }
  .run {
    flex: none;
    height: 22px;
    padding: 0 7px;
    font-size: 11px;
    display: flex;
    align-items: center;
    gap: 3px;
  }
  .more {
    opacity: 0;
  }
  .fr:hover .more {
    opacity: 1;
  }
  .empty {
    padding: 8px 10px;
    font-size: 12.5px;
    color: var(--text-3);
    line-height: 1.5;
  }
  .dactions {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 10px;
    border-top: 1px solid var(--border);
  }
  .main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .screen {
    flex: 1;
    min-height: 0;
    position: relative;
    display: grid;
    place-items: center;
    background: #050507;
    overflow: hidden;
  }
  canvas {
    image-rendering: pixelated;
    outline: none;
    box-shadow: 0 6px 30px rgba(0, 0, 0, 0.5);
    background: black;
  }
  canvas.dim {
    opacity: 0.35;
  }
  .overlay {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
    background: rgba(0, 0, 0, 0.4);
    color: white;
    font-size: 22px;
    font-weight: 600;
  }
  .off {
    text-align: center;
    color: var(--text-3);
  }
  .off code {
    font-size: 12px;
  }
  .bar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 7px;
    height: 36px;
    padding: 0 12px;
    border-top: 1px solid var(--border);
    font-size: 12px;
  }
  .bar :global(svg) {
    color: var(--text-3);
  }
  .settings {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .settings label {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 13px;
  }
  .settings label span {
    width: 120px;
    flex: none;
    color: var(--text-2);
  }
  .settings select {
    flex: 1;
  }
  .settings .chk {
    padding-left: 130px;
    color: var(--text-2);
  }
  .settings p {
    margin: 4px 0 0;
    line-height: 1.5;
  }
</style>
