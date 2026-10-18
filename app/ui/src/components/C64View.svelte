<script lang="ts">
  import { onMount } from 'svelte';
  import { ask } from '@tauri-apps/plugin-dialog';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { rad } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import { formatBytes } from '../lib/util';
  import * as C from '../lib/c64';
  import Menu from './Menu.svelte';
  import Modal from './Modal.svelte';
  import {
    Keyboard, Upload, RotateCcw, Pause, Play, Square, Rewind, Eject, Save, Volume2, VolumeX, Zap, Loader, Trash2, Pencil, MoreHorizontal,
    CassetteTape, History, Gamepad2, HardDrive, Cpu, FileCode, Disc, KeyRound, Search, FolderOpen,
  } from '@lucide/svelte';

  let canvas: HTMLCanvasElement | undefined = $state();
  let screenBox: HTMLDivElement | undefined = $state();
  let size = $state<[number, number]>([392, 272]);
  let lut = new Uint32Array(16);
  let pending: Uint8Array | null = null;
  let scale = $state(2);

  type Status = {
    roms: 'open' | 'commodore'; turbo: boolean; starting: boolean; fast_load: boolean; joy_port: 1 | 2; cartridge: boolean;
    tape: { playing: boolean; progress: number; name: string } | null; disk: { name: string; files: string[] } | null;
  };
  let status = $state<Status | null>(null);
  let paused = $state(false);
  let volume = $state(Number(read('rad-c64-volume') ?? 0.8));
  let muted = $state(read('rad-c64-muted') === '1');
  let arrows = $state<C.ArrowMode>((read('rad-c64-arrows') as C.ArrowMode) || 'joystick');

  let games = $state<C.C64Game[]>([]);
  let current = $state<string | null>(null);
  let saves = $state<C.C64State[]>([]);
  let loading = $state<string | null>(null);
  let dropping = $state(false);
  let fromGames = $state<string | null>(null);
  const game = $derived(games.find((g) => g.id === current) ?? null);

  const ICON = { program: FileCode, tape: CassetteTape, disk: Disc, cartridge: Cpu };
  const BADGE = { program: 'prg', tape: 'tape', disk: 'disk', cartridge: 'cart' };

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

  // ------------------------------------------------------------------ screen

  function onMsg(m: ArrayBuffer | Record<string, any>) {
    if (m instanceof ArrayBuffer) {
      pending = new Uint8Array(m);
      return;
    }
    if (m.size) {
      size = m.size;
      const pal = m.palette as number[][];
      lut = new Uint32Array(pal.map(([r, g, b]) => (0xff << 24) | (b << 16) | (g << 8) | r));
      fit();
    }
    if (m.status) status = m.status;
  }

  function draw() {
    if (pending && canvas) {
      const [w, h] = size;
      const ctx = canvas.getContext('2d');
      if (ctx && pending.length === w * h) {
        const img = ctx.createImageData(w, h);
        const px = new Uint32Array(img.data.buffer);
        for (let i = 0; i < pending.length; i++) px[i] = lut[pending[i] & 15];
        ctx.putImageData(img, 0, 0);
      }
      pending = null;
    }
    pollPad();
    raf = requestAnimationFrame(draw);
  }
  let raf = 0;

  function fit() {
    if (!screenBox) return;
    const r = screenBox.getBoundingClientRect();
    scale = Math.max(1, Math.min((r.width - 16) / size[0], (r.height - 16) / size[1]));
  }

  // ------------------------------------------------------------------ keyboard and joystick

  const kb = new C.Keyboard();
  let sentKeys = '';
  let sentJoy = -1;
  let padJoy = 0;

  function sendInput() {
    const { keys, joy } = kb.state(arrows);
    const k = keys.join(',');
    if (k !== sentKeys) {
      sentKeys = k;
      rad.c64Keys(keys).catch(() => {});
    }
    const j = joy | padJoy;
    if (j !== sentJoy) {
      sentJoy = j;
      rad.c64Joystick(j).catch(() => {});
    }
  }

  function typingInField(e: KeyboardEvent) {
    const t = e.target as HTMLElement | null;
    return !!t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT' || t.isContentEditable);
  }

  function onKeyDown(e: KeyboardEvent) {
    if (e.metaKey || typingInField(e) || romsOpen) return;
    if (e.repeat && kb.held.has(e.code)) {
      e.preventDefault();
      return;
    }
    if (!kb.down(e, arrows)) return;
    e.preventDefault();
    sendInput();
  }

  function onKeyUp(e: KeyboardEvent) {
    if (kb.up(e.code)) {
      e.preventDefault();
      sendInput();
    }
  }

  function releaseAll() {
    kb.clear();
    sendInput();
  }

  /** A connected gamepad drives the joystick (stick or d-pad, any face button fires). */
  function pollPad() {
    const pads = navigator.getGamepads?.() ?? [];
    let j = 0;
    for (const p of pads) {
      if (!p) continue;
      const [x = 0, y = 0] = p.axes;
      const b = (i: number) => !!p.buttons[i]?.pressed;
      if (y < -0.5 || b(12)) j |= C.JOY.UP;
      if (y > 0.5 || b(13)) j |= C.JOY.DOWN;
      if (x < -0.5 || b(14)) j |= C.JOY.LEFT;
      if (x > 0.5 || b(15)) j |= C.JOY.RIGHT;
      if (b(0) || b(1) || b(2) || b(3)) j |= C.JOY.FIRE;
    }
    if (j !== padJoy) {
      padJoy = j;
      sendInput();
    }
  }

  function setArrows(a: C.ArrowMode) {
    arrows = a;
    write('rad-c64-arrows', a);
    releaseAll();
  }

  function setPort(p: 1 | 2) {
    rad.c64JoystickPort(p).catch((e) => store.error(e));
  }

  // ------------------------------------------------------------------ library

  async function refreshGames() {
    games = await C.games().catch((e) => (store.error(e, 'Couldn’t read the game library'), games));
    if (current) saves = await C.states(current).catch(() => []);
  }

  async function addFiles(paths: string[]) {
    const re = new RegExp(`\\.(${C.C64_EXTS.join('|')})$`, 'i');
    const ok = paths.filter((p) => re.test(p));
    if (!ok.length) {
      if (paths.length) store.toast('error', 'Not a C64 file', 'Use PRG, P00, T64, TAP, D64 or CRT files.');
      return;
    }
    const files = await rad.gameReadFiles(ok).catch((e) => (store.error(e, 'Couldn’t read the files'), []));
    let first: string | null = null;
    for (const f of files) {
      if (f.error || f.data === undefined) {
        store.toast('error', `${f.name} wasn’t added`, f.error ?? '');
        continue;
      }
      const id = await C.addGame(f.name, f.data, f.size).catch((e) => (store.error(e, 'Couldn’t save the game'), null));
      first ??= id;
    }
    await refreshGames();
    if (first) play(first);
  }

  async function pickFiles() {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const r = await open({ multiple: true, title: 'Add C64 games', filters: [{ name: 'C64 programs, tapes, disks and cartridges', extensions: C.C64_EXTS }] });
    addFiles(r == null ? [] : Array.isArray(r) ? r : [r]);
  }

  async function play(id: string) {
    loading = id;
    fromGames = null;
    try {
      const g = await C.gameData(id);
      const r = await rad.c64Load(g.file, g.data);
      current = id;
      paused = false;
      saves = await C.states(id).catch(() => []);
      if (r.kind === 'tape') store.toast('info', 'Loading from tape', status?.fast_load ? 'Fast loading — tapes still take a little while.' : 'At real speed — tape loading takes minutes.', 3000);
      if (r.kind === 'disk' && !(r.names ?? []).length) store.toast('info', 'That disk has no programs', 'Nothing to load from it.', 3000);
      refreshGames();
      canvas?.focus();
    } catch (e) {
      store.error(e, 'Couldn’t load the game');
    } finally {
      loading = null;
    }
  }

  /** Puts a disk in the drive without restarting (multi-disk games ask for "disk 2"). */
  async function insertDisk(g: C.C64Game) {
    try {
      const d = await C.gameData(g.id, false);
      await rad.c64InsertDisk(g.file, d.data);
      store.toast('success', 'Disk inserted', g.name, 2000);
      canvas?.focus();
    } catch (e) {
      store.error(e, 'Couldn’t insert the disk');
    }
  }

  async function renameGame(g: C.C64Game) {
    const n = window.prompt('Game name', g.name)?.trim();
    if (!n || n === g.name) return;
    await C.renameGame(g.id, n).catch((e) => store.error(e));
    refreshGames();
  }

  async function removeGame(g: C.C64Game) {
    if (!(await ask(`Remove “${g.name}”${g.states ? ` and its ${g.states} saved game${g.states > 1 ? 's' : ''}` : ''} from the library?`, { title: 'Remove game', kind: 'warning' }))) return;
    await C.removeGame(g.id).catch((e) => store.error(e));
    if (current === g.id) current = null;
    refreshGames();
  }

  async function saveState() {
    if (!current) return;
    try {
      const data = await rad.c64SaveState();
      const name = new Date().toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
      await C.saveState(current, data, name);
      store.toast('success', 'Game saved', name, 2000);
      refreshGames();
    } catch (e) {
      store.error(e, 'Couldn’t save');
    }
  }

  async function loadState(s: C.C64State) {
    try {
      await rad.c64Load('state.c64s', await C.stateData(s.id));
      paused = false;
      canvas?.focus();
    } catch (e) {
      store.error(e, 'Couldn’t load the save');
    }
  }

  async function removeState(s: C.C64State) {
    await C.removeState(s.id).catch((e) => store.error(e));
    refreshGames();
  }

  // ------------------------------------------------------------------ ROMs

  let romsOpen = $state(false);
  let romsBusy = $state(false);

  async function useRomFiles(paths: string[]) {
    if (!paths.length) return;
    romsBusy = true;
    try {
      const r = await rad.c64ReadRoms(paths);
      const { chars, basic, kernal } = r.roms;
      if (!chars || !basic || !kernal) {
        const missing = [!kernal && 'KERNAL', !basic && 'BASIC', !chars && 'character'].filter(Boolean).join(', ');
        store.toast('error', 'Some ROMs are missing', `Still needed: ${missing} ROM.${r.skipped.length ? ` Not ROMs: ${r.skipped.join(', ')}.` : ''}`);
        return;
      }
      await C.saveRoms(chars, basic, kernal);
      store.toast('success', 'C64 ROMs added', 'Kept in your encrypted storage. BASIC programs run now.', 3000);
      romsOpen = false;
      control('reset');
    } catch (e) {
      store.error(e, 'Couldn’t use those ROMs');
    } finally {
      romsBusy = false;
    }
  }

  async function pickRoms() {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const r = await open({ multiple: true, title: 'Choose the C64’s KERNAL, BASIC and character ROMs' });
    useRomFiles(r == null ? [] : Array.isArray(r) ? r : [r]);
  }

  async function findRoms() {
    const paths = await rad.c64FindRoms().catch(() => []);
    if (!paths.length) {
      store.toast('info', 'No VICE installation found', 'Choose the ROM files yourself instead.', 3000);
      return;
    }
    useRomFiles(paths);
  }

  async function freeRoms() {
    await C.removeRoms().catch((e) => store.error(e));
    romsOpen = false;
    control('reset');
  }

  // ------------------------------------------------------------------ controls

  function control(a: string, v?: number) {
    rad.c64Control(a, v).catch((e) => store.error(e));
  }

  function togglePause() {
    paused = !paused;
    control(paused ? 'pause' : 'resume');
  }

  function setVolume(v: number) {
    volume = v;
    write('rad-c64-volume', String(v));
    control('volume', muted ? 0 : v);
  }

  function toggleMute() {
    muted = !muted;
    write('rad-c64-muted', muted ? '1' : '0');
    control('volume', muted ? 0 : volume);
  }

  // A program sent from the Games view (Run on C64).
  let attached = $state(false);
  $effect(() => {
    const p = store.pendingC64;
    if (!p || !attached) return;
    store.pendingC64 = null;
    (async () => {
      try {
        await rad.c64Load(p.name, p.data);
        current = null;
        fromGames = p.title;
        paused = false;
        canvas?.focus();
      } catch (e) {
        store.error(e, 'Couldn’t load the game');
      }
    })();
  });

  onMount(() => {
    (async () => {
      await C.loadRoms().catch((e) => store.error(e, 'Couldn’t read your C64 ROMs'));
      await rad.c64Attach(onMsg);
      control('resume');
      control('volume', muted ? 0 : volume);
      attached = true;
    })().catch((e) => store.error(e, 'Couldn’t start the C64'));
    refreshGames();
    raf = requestAnimationFrame(draw);
    const ro = new ResizeObserver(fit);
    if (screenBox) ro.observe(screenBox);
    window.addEventListener('keydown', onKeyDown, true);
    window.addEventListener('keyup', onKeyUp, true);
    window.addEventListener('blur', releaseAll);
    const unDrop = getCurrentWebview().onDragDropEvent((ev) => {
      const p = ev.payload;
      if (p.type === 'enter' || p.type === 'over') dropping = true;
      else if (p.type === 'leave') dropping = false;
      else if (p.type === 'drop') {
        dropping = false;
        if (romsOpen) useRomFiles(p.paths);
        else addFiles(p.paths);
      }
    });
    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
      window.removeEventListener('keydown', onKeyDown, true);
      window.removeEventListener('keyup', onKeyUp, true);
      window.removeEventListener('blur', releaseAll);
      releaseAll();
      rad.c64Control('pause').catch(() => {});
      unDrop.then((u) => u());
    };
  });
</script>

<div class="cv">
  <header class="drag">
    <div class="ttl no-drag"><Keyboard size={16} /> Commodore 64</div>
    <div class="seg no-drag" title="Which joystick port the joystick is in (most games use port 2)">
      <button class:on={status?.joy_port === 1} onclick={() => setPort(1)}>Port 1</button>
      <button class:on={status?.joy_port !== 1} onclick={() => setPort(2)}>Port 2</button>
    </div>
    {#if game}<span class="playing no-drag truncate">{game.name}</span>{:else if fromGames}<span class="playing no-drag truncate">{fromGames} <span class="muted">· from Games</span></span>{/if}
    <span class="spacer"></span>
    <button class="romchip no-drag" class:free={status?.roms === 'open'} onclick={() => (romsOpen = true)} title="Which ROMs the C64 runs">
      <KeyRound size={12} /> {status?.roms === 'commodore' ? 'C64 ROMs' : 'Free ROMs'}
    </button>
    <button class="icon-btn no-drag" title={paused ? 'Resume' : 'Pause'} onclick={togglePause}>{#if paused}<Play size={15} />{:else}<Pause size={15} />{/if}</button>
    <button class="icon-btn no-drag" title="Reset the C64" onclick={() => control('reset')}><RotateCcw size={15} /></button>
    <button class="icon-btn no-drag" title="Save the game here (a save state)" disabled={!current} onclick={saveState}><Save size={15} /></button>
    <div class="vol no-drag">
      <button class="icon-btn" title={muted ? 'Sound on' : 'Mute'} onclick={toggleMute}>{#if muted || volume === 0}<VolumeX size={15} />{:else}<Volume2 size={15} />{/if}</button>
      <input type="range" min="0" max="1" step="0.05" value={volume} oninput={(e) => setVolume(+(e.currentTarget as HTMLInputElement).value)} disabled={muted} />
    </div>
    <button class="btn sm primary no-drag" onclick={pickFiles}><Upload size={13} /> Add games</button>
  </header>

  <div class="body">
    <aside class="lib">
      <div class="lh"><span class="section-title">Library</span><span class="muted cnt">{games.length || ''}</span></div>
      <div class="ll">
        {#each games as g (g.id)}
          {@const kind = C.kindOf(g.file)}
          {@const Icon = ICON[kind]}
          <div class="gi" class:on={current === g.id}>
            <button class="gb" onclick={() => play(g.id)} title={`${g.file} · ${formatBytes(g.size)}`}>
              {#if loading === g.id}<Loader size={13} class="spin" />{:else}<Icon size={13} />{/if}
              <span class="truncate spacer">{g.name}</span>
              <span class="badge">{BADGE[kind]}</span>
            </button>
            <Menu
              items={[
                ...(kind === 'disk' ? [{ label: 'Insert disk (no restart)', icon: HardDrive, onclick: () => insertDisk(g) }, null] : []),
                { label: 'Rename…', icon: Pencil, onclick: () => renameGame(g) },
                null,
                { label: 'Remove', icon: Trash2, danger: true, onclick: () => removeGame(g) },
              ]}
            >
              {#snippet trigger(toggle)}<button class="icon-btn sm more" onclick={toggle} aria-label="Game menu"><MoreHorizontal size={12} /></button>{/snippet}
            </Menu>
          </div>
          {#if current === g.id && saves.length}
            <div class="saves">
              {#each saves as s (s.id)}
                <div class="si">
                  <button class="sb" onclick={() => loadState(s)} title="Carry on from this save"><History size={11} /> <span class="truncate">{s.name}</span></button>
                  <button class="icon-btn sm more" title="Delete this save" onclick={() => removeState(s)}><Trash2 size={11} /></button>
                </div>
              {/each}
            </div>
          {/if}
        {:else}
          <div class="empty">
            Drop PRG, T64, TAP, D64 or CRT files here, or use <strong>Add games</strong>. They’re kept in your encrypted storage.
          </div>
        {/each}
      </div>
    </aside>

    <section class="main">
      <div class="screen" bind:this={screenBox} class:drop={dropping}>
        <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
        <canvas bind:this={canvas} width={size[0]} height={size[1]} tabindex="0" style="width:{size[0] * scale}px;height:{size[1] * scale}px"></canvas>
        {#if paused}<div class="overlay" role="button" tabindex="-1" onclick={togglePause} onkeydown={() => {}}><Play size={42} /> Paused</div>{/if}
        {#if status?.turbo}<div class="turbo"><Zap size={13} /> {status.tape?.playing ? 'Fast loading…' : 'Starting…'}</div>{/if}
      </div>

      <div class="bar">
        <div class="dev">
          <CassetteTape size={14} />
          {#if status?.tape}
            <span class="tn truncate" title={status.tape.name}>{status.tape.name}</span>
            <div class="prog"><div style="width:{Math.round(status.tape.progress * 100)}%"></div></div>
            {#if status.tape.playing}
              <button class="icon-btn sm" title="Stop the tape" onclick={() => control('tape_stop')}><Square size={12} /></button>
            {:else}
              <button class="icon-btn sm" title="Press play" onclick={() => control('tape_play')}><Play size={12} /></button>
            {/if}
            <button class="icon-btn sm" title="Rewind" onclick={() => control('tape_rewind')}><Rewind size={12} /></button>
            <button class="icon-btn sm" title="Eject the tape" onclick={() => control('eject')}><Eject size={12} /></button>
            <label class="chk" title="Run at top speed while the tape plays"><input type="checkbox" checked={status.fast_load} onchange={(e) => control((e.currentTarget as HTMLInputElement).checked ? 'fast_on' : 'fast_off')} /> Fast</label>
          {:else}
            <span class="muted">No tape</span>
          {/if}
        </div>
        <div class="dev">
          <HardDrive size={14} />
          {#if status?.disk}
            <span class="tn truncate" title={status.disk.files.join('\n')}>{status.disk.name}</span>
            <button class="icon-btn sm" title="Take the disk out" onclick={() => control('eject_disk')}><Eject size={12} /></button>
          {:else}
            <span class="muted">No disk</span>
          {/if}
        </div>
        {#if status?.cartridge}
          <div class="dev">
            <Cpu size={14} /> <span>Cartridge</span>
            <button class="icon-btn sm" title="Pull the cartridge out (resets)" onclick={() => control('eject_cart')}><Eject size={12} /></button>
          </div>
        {/if}
        <span class="spacer"></span>
        <div class="keys">
          <span class="muted" title="PageUp is RESTORE · Home is CLR/HOME · Insert is INST · F1–F8 are the function keys">Esc = RUN/STOP · Alt = C= · Ctrl = CTRL</span>
          <Gamepad2 size={13} />
          <select class="select" value={arrows} onchange={(e) => setArrows((e.currentTarget as HTMLSelectElement).value as C.ArrowMode)} title="What the arrow keys (and Tab, to fire) do">
            <option value="joystick">Arrows: joystick</option>
            <option value="keys">Arrows: cursor keys</option>
          </select>
        </div>
      </div>
    </section>
  </div>
</div>

<Modal bind:open={romsOpen} title="C64 ROMs" width={500}>
  <div class="roms">
    {#if status?.roms === 'commodore'}
      <p>RAD is using <strong>your own C64 ROMs</strong> (kept in your encrypted storage).</p>
    {:else}
      <p>
        RAD is using the free <strong>Open ROMs</strong> from the MEGA65 project. Machine-code games and programs run, but their BASIC can’t run BASIC
        programs yet (no numbers or variables).
      </p>
      <p>
        The C64’s own ROMs are still under copyright, so RAD can’t include them. If you have them — from a VICE installation, C64 Forever, or a real
        C64 — add them here: the KERNAL (8K), BASIC (8K) and character (4K) ROMs.
      </p>
    {/if}
    <div class="ra">
      <button class="btn sm" disabled={romsBusy} onclick={findRoms}><Search size={13} /> Find VICE’s ROMs</button>
      <button class="btn sm" disabled={romsBusy} onclick={pickRoms}><FolderOpen size={13} /> Choose ROM files…</button>
      {#if status?.roms === 'commodore'}<button class="btn sm" onclick={freeRoms}>Use the free Open ROMs</button>{/if}
    </div>
    <p class="muted small">You can also drop the three files here.</p>
  </div>
</Modal>

<style>
  .cv {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
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
  .seg {
    display: flex;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    overflow: hidden;
  }
  .seg button {
    height: 26px;
    padding: 0 10px;
    border: none;
    background: transparent;
    color: var(--text-2);
    font-size: 12px;
    font-weight: 600;
  }
  .seg button.on {
    background: var(--accent);
    color: white;
  }
  .playing {
    font-size: 13px;
    color: var(--text-2);
    max-width: 260px;
  }
  .romchip {
    display: flex;
    align-items: center;
    gap: 5px;
    height: 24px;
    padding: 0 9px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-2);
    font-size: 11.5px;
  }
  .romchip.free {
    color: #c98a00;
    border-color: color-mix(in oklab, #c98a00 45%, transparent);
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
  .lib {
    flex: none;
    width: 230px;
    display: flex;
    flex-direction: column;
    border-right: 1px solid var(--border);
    min-height: 0;
  }
  .lh {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 36px;
    padding: 0 12px;
  }
  .cnt {
    font-size: 11px;
  }
  .ll {
    flex: 1;
    overflow: auto;
    padding: 0 6px 10px;
  }
  .gi,
  .si {
    display: flex;
    align-items: center;
    border-radius: var(--r-sm);
  }
  .gi:hover,
  .si:hover {
    background: var(--surface-hover);
  }
  .gi.on {
    background: var(--surface-active, var(--surface-hover));
  }
  .gb,
  .sb {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 7px;
    height: 30px;
    padding: 0 8px;
    border: none;
    background: transparent;
    color: var(--text);
    font-size: 13px;
    text-align: left;
  }
  .gb :global(svg),
  .sb :global(svg) {
    flex: none;
    color: var(--text-3);
  }
  .badge {
    flex: none;
    font-size: 10.5px;
    padding: 1px 5px;
    border-radius: 4px;
    background: var(--surface-2);
    color: var(--text-3);
  }
  .more {
    opacity: 0;
  }
  .gi:hover .more,
  .si:hover .more {
    opacity: 1;
  }
  .saves {
    margin: 0 0 4px 18px;
  }
  .sb {
    height: 24px;
    font-size: 12px;
    color: var(--text-2);
  }
  .empty {
    padding: 8px 10px;
    font-size: 12.5px;
    color: var(--text-3);
    line-height: 1.5;
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
    background: #0b0b10;
    overflow: hidden;
  }
  .screen.drop {
    outline: 2px dashed var(--accent);
    outline-offset: -8px;
  }
  canvas {
    image-rendering: pixelated;
    outline: none;
    box-shadow: 0 6px 30px rgba(0, 0, 0, 0.5);
  }
  .overlay {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
    background: rgba(0, 0, 0, 0.45);
    color: white;
    font-size: 22px;
    font-weight: 600;
    cursor: pointer;
  }
  .turbo {
    position: absolute;
    top: 12px;
    right: 14px;
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 4px 10px;
    border-radius: 999px;
    background: rgba(0, 0, 0, 0.6);
    color: #ffd75e;
    font-size: 12px;
  }
  .bar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 14px;
    height: 42px;
    padding: 0 12px;
    border-top: 1px solid var(--border);
    font-size: 12px;
  }
  .dev,
  .keys {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .dev :global(svg),
  .keys :global(svg) {
    flex: none;
    color: var(--text-3);
  }
  .tn {
    max-width: 160px;
  }
  .prog {
    width: 90px;
    height: 4px;
    border-radius: 2px;
    background: var(--surface-3);
    overflow: hidden;
  }
  .prog div {
    height: 100%;
    background: var(--accent);
  }
  .chk {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--text-2);
  }
  .keys select {
    height: 26px;
    padding-top: 0;
    padding-bottom: 0;
    font-size: 12px;
  }
  .roms p {
    margin: 0 0 10px;
    font-size: 13px;
    line-height: 1.5;
  }
  .ra {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin: 14px 0 8px;
  }
  .small {
    font-size: 12px;
  }
</style>
