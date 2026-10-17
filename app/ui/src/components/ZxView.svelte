<script lang="ts">
  import { onMount } from 'svelte';
  import { ask } from '@tauri-apps/plugin-dialog';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { rad } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import { formatBytes, timeAgo } from '../lib/util';
  import * as Z from '../lib/zx';
  import Menu from './Menu.svelte';
  import {
    Joystick, Upload, RotateCcw, Pause, Play, Square, Rewind, Eject, Save, Volume2, VolumeX, Zap, Loader, Trash2, Pencil, MoreHorizontal,
    CassetteTape, History, Keyboard, Gamepad2,
  } from '@lucide/svelte';

  let canvas: HTMLCanvasElement | undefined = $state();
  let screenBox: HTMLDivElement | undefined = $state();
  let size = $state<[number, number]>([352, 288]);
  let lut = new Uint32Array(16);
  let pending: Uint8Array | null = null;
  let scale = $state(2);

  type Status = { model: '48' | '128'; turbo: boolean; fast_load: boolean; tape: { playing: boolean; progress: number; names: string[] } | null };
  let status = $state<Status | null>(null);
  let paused = $state(false);
  let volume = $state(Number(read('rad-zx-volume') ?? 0.8));
  let muted = $state(read('rad-zx-muted') === '1');
  let arrows = $state<Z.ArrowMode>((read('rad-zx-arrows') as Z.ArrowMode) || 'kempston');

  let games = $state<Z.ZxGame[]>([]);
  let current = $state<string | null>(null);
  let saves = $state<Z.ZxState[]>([]);
  let loading = $state<string | null>(null);
  let dropping = $state(false);
  /** A program from the Games view that's playing (not in the library). */
  let fromGames = $state<string | null>(null);
  const game = $derived(games.find((g) => g.id === current) ?? null);

  const isSnap = (file: string) => /\.(z80|sna)$/i.test(file);

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
      // Little-endian ABGR for ImageData.
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

  const held = new Set<string>();
  let sentKeys = -1;
  let sentJoy = -1;
  let padJoy = 0;

  function sendInput() {
    const { keys, joy } = Z.mapKeys(held, arrows);
    if (keys !== sentKeys) {
      sentKeys = keys;
      rad.zxKeys(keys).catch(() => {});
    }
    const j = joy | padJoy;
    if (j !== sentJoy) {
      sentJoy = j;
      rad.zxJoystick(j).catch(() => {});
    }
  }

  function typingInField(e: KeyboardEvent) {
    const t = e.target as HTMLElement | null;
    return !!t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT' || t.isContentEditable);
  }

  function onKeyDown(e: KeyboardEvent) {
    if (e.metaKey || typingInField(e) || !Z.isMapped(e.code, arrows)) return;
    e.preventDefault();
    if (!held.has(e.code)) {
      held.add(e.code);
      sendInput();
    }
  }

  function onKeyUp(e: KeyboardEvent) {
    if (held.delete(e.code)) {
      e.preventDefault();
      sendInput();
    }
  }

  function releaseAll() {
    held.clear();
    sendInput();
  }

  /** A connected gamepad drives the Kempston joystick (stick or d-pad, A/B to fire). */
  function pollPad() {
    const pads = navigator.getGamepads?.() ?? [];
    let j = 0;
    for (const p of pads) {
      if (!p) continue;
      const [x = 0, y = 0] = p.axes;
      const b = (i: number) => !!p.buttons[i]?.pressed;
      if (y < -0.5 || b(12)) j |= Z.JOY.UP;
      if (y > 0.5 || b(13)) j |= Z.JOY.DOWN;
      if (x < -0.5 || b(14)) j |= Z.JOY.LEFT;
      if (x > 0.5 || b(15)) j |= Z.JOY.RIGHT;
      if (b(0) || b(1) || b(2) || b(3)) j |= Z.JOY.FIRE;
    }
    if (j !== padJoy) {
      padJoy = j;
      sendInput();
    }
  }

  function setArrows(a: Z.ArrowMode) {
    arrows = a;
    write('rad-zx-arrows', a);
    releaseAll();
  }

  // ------------------------------------------------------------------ library

  async function refreshGames() {
    games = await Z.games().catch((e) => (store.error(e, 'Couldn’t read the game library'), games));
    if (current) saves = await Z.states(current).catch(() => []);
  }

  async function addFiles(paths: string[]) {
    const ok = paths.filter((p) => /\.(tzx|tap|z80|sna)$/i.test(p));
    if (!ok.length) {
      if (paths.length) store.toast('error', 'Not a Spectrum file', 'Use TZX, TAP, Z80 or SNA files.');
      return;
    }
    const read = await rad.gameReadFiles(ok).catch((e) => (store.error(e, 'Couldn’t read the files'), []));
    let first: string | null = null;
    for (const f of read) {
      if (f.error || f.data === undefined) {
        store.toast('error', `${f.name} wasn’t added`, f.error ?? '');
        continue;
      }
      const id = await Z.addGame(f.name, f.data, f.size, status?.model ?? '48').catch((e) => (store.error(e, 'Couldn’t save the game'), null));
      first ??= id;
    }
    await refreshGames();
    if (first) play(first);
  }

  async function pickFiles() {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const r = await open({ multiple: true, title: 'Add Spectrum games', filters: [{ name: 'ZX Spectrum tapes and snapshots', extensions: ['tzx', 'tap', 'z80', 'sna'] }] });
    addFiles(r == null ? [] : Array.isArray(r) ? r : [r]);
  }

  async function play(id: string) {
    loading = id;
    fromGames = null;
    try {
      const g = await Z.gameData(id);
      if (!isSnap(g.file)) await rad.zxControl(g.model === '128' ? 'model128' : 'model48');
      const r = await rad.zxLoad(g.file, g.data);
      current = id;
      paused = false;
      saves = await Z.states(id).catch(() => []);
      if (r.kind === 'tape') store.toast('info', 'Loading from tape', status?.fast_load ? 'Fast loading — a few seconds.' : 'At real speed — tape loading takes minutes.', 3000);
      refreshGames();
      canvas?.focus();
    } catch (e) {
      store.error(e, 'Couldn’t load the game');
    } finally {
      loading = null;
    }
  }

  async function setModel(m: '48' | '128') {
    if (status?.model === m) return;
    if (game && !isSnap(game.file)) {
      await Z.setGame(game.id, { model: m }).catch(() => {});
      await refreshGames();
      play(game.id);
    } else {
      await rad.zxControl(m === '128' ? 'model128' : 'model48');
    }
  }

  async function renameGame(g: Z.ZxGame) {
    const n = window.prompt('Game name', g.name)?.trim();
    if (!n || n === g.name) return;
    await Z.setGame(g.id, { name: n }).catch((e) => store.error(e));
    refreshGames();
  }

  async function removeGame(g: Z.ZxGame) {
    if (!(await ask(`Remove “${g.name}”${g.states ? ` and its ${g.states} saved game${g.states > 1 ? 's' : ''}` : ''} from the library?`, { title: 'Remove game', kind: 'warning' }))) return;
    await Z.removeGame(g.id).catch((e) => store.error(e));
    if (current === g.id) current = null;
    refreshGames();
  }

  async function saveState() {
    if (!current) return;
    try {
      const data = await rad.zxSaveState();
      const name = new Date().toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
      await Z.saveState(current, data, name);
      store.toast('success', 'Game saved', name, 2000);
      refreshGames();
    } catch (e) {
      store.error(e, 'Couldn’t save');
    }
  }

  async function loadState(s: Z.ZxState) {
    try {
      await rad.zxLoad('state.z80', await Z.stateData(s.id));
      paused = false;
      canvas?.focus();
    } catch (e) {
      store.error(e, 'Couldn’t load the save');
    }
  }

  async function removeState(s: Z.ZxState) {
    await Z.removeState(s.id).catch((e) => store.error(e));
    refreshGames();
  }

  // ------------------------------------------------------------------ controls

  function control(a: string, v?: number) {
    rad.zxControl(a, v).catch((e) => store.error(e));
  }

  function togglePause() {
    paused = !paused;
    control(paused ? 'pause' : 'resume');
  }

  function setVolume(v: number) {
    volume = v;
    write('rad-zx-volume', String(v));
    control('volume', muted ? 0 : v);
  }

  function toggleMute() {
    muted = !muted;
    write('rad-zx-muted', muted ? '1' : '0');
    control('volume', muted ? 0 : volume);
  }

  // A program sent from the Games view (Run on Spectrum).
  let attached = $state(false);
  $effect(() => {
    const p = store.pendingZx;
    if (!p || !attached) return;
    store.pendingZx = null;
    (async () => {
      try {
        await rad.zxControl('model48');
        await rad.zxLoad(p.name, p.data);
        current = null;
        fromGames = p.title;
        paused = false;
        canvas?.focus();
      } catch (e) {
        store.error(e, 'Couldn’t load the game');
      }
    })();
  });

  const ARROWS: Record<Z.ArrowMode, string> = { kempston: 'Kempston joystick', cursor: 'Cursor joystick', qaop: 'Q A O P keys', keys: 'Spectrum cursor keys' };

  onMount(() => {
    rad.zxAttach('48', onMsg).then(() => {
      control('resume');
      control('volume', muted ? 0 : volume);
      attached = true;
    }).catch((e) => store.error(e, 'Couldn’t start the Spectrum'));
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
        addFiles(p.paths);
      }
    });
    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
      window.removeEventListener('keydown', onKeyDown, true);
      window.removeEventListener('keyup', onKeyUp, true);
      window.removeEventListener('blur', releaseAll);
      releaseAll();
      // Out of sight: the Spectrum waits (and goes quiet).
      rad.zxControl('pause').catch(() => {});
      unDrop.then((u) => u());
    };
  });
</script>

<div class="zv">
  <header class="drag">
    <div class="ttl no-drag"><Joystick size={16} /> ZX Spectrum</div>
    <div class="seg no-drag" title="Machine">
      <button class:on={status?.model === '48'} onclick={() => setModel('48')}>48K</button>
      <button class:on={status?.model === '128'} onclick={() => setModel('128')}>128K</button>
    </div>
    {#if game}<span class="playing no-drag truncate">{game.name}</span>{:else if fromGames}<span class="playing no-drag truncate">{fromGames} <span class="muted">· from Games</span></span>{/if}
    <span class="spacer"></span>
    <button class="icon-btn no-drag" title={paused ? 'Resume' : 'Pause'} onclick={togglePause}>{#if paused}<Play size={15} />{:else}<Pause size={15} />{/if}</button>
    <button class="icon-btn no-drag" title="Reset the Spectrum" onclick={() => control('reset')}><RotateCcw size={15} /></button>
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
          <div class="gi" class:on={current === g.id}>
            <button class="gb" onclick={() => play(g.id)} title={`${g.file} · ${formatBytes(g.size)}`}>
              {#if loading === g.id}<Loader size={13} class="spin" />{:else if isSnap(g.file)}<History size={13} />{:else}<CassetteTape size={13} />{/if}
              <span class="truncate spacer">{g.name}</span>
              <span class="badge">{isSnap(g.file) ? 'snap' : g.model + 'K'}</span>
            </button>
            <Menu items={[{ label: 'Rename…', icon: Pencil, onclick: () => renameGame(g) }, null, { label: 'Remove', icon: Trash2, danger: true, onclick: () => removeGame(g) }]}>
              {#snippet trigger(toggle)}<button class="icon-btn sm more" onclick={toggle} aria-label="Game menu"><MoreHorizontal size={12} /></button>{/snippet}
            </Menu>
          </div>
          {#if current === g.id && saves.length}
            <div class="saves">
              {#each saves as s (s.id)}
                <div class="si">
                  <button class="sb" onclick={() => loadState(s)} title="Carry on from this save"><Save size={11} /> <span class="truncate">{s.name}</span></button>
                  <button class="icon-btn sm more" title="Delete this save" onclick={() => removeState(s)}><Trash2 size={11} /></button>
                </div>
              {/each}
            </div>
          {/if}
        {:else}
          <div class="empty">
            Drop TZX, TAP, Z80 or SNA files here, or use <strong>Add games</strong>. They’re kept in your encrypted storage.
          </div>
        {/each}
      </div>
    </aside>

    <section class="main">
      <div class="screen" bind:this={screenBox} class:drop={dropping}>
        <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
        <canvas bind:this={canvas} width={size[0]} height={size[1]} tabindex="0" style="width:{size[0] * scale}px;height:{size[1] * scale}px"></canvas>
        {#if paused}<div class="overlay" role="button" tabindex="-1" onclick={togglePause} onkeydown={() => {}}><Play size={42} /> Paused</div>{/if}
        {#if status?.turbo}<div class="turbo"><Zap size={13} /> Fast loading…</div>{/if}
      </div>

      <div class="bar">
        <div class="tape">
          <CassetteTape size={14} />
          {#if status?.tape}
            <span class="tn truncate" title={status.tape.names.join('\n')}>{status.tape.names[0] ?? 'Tape'}{status.tape.names.length > 1 ? ` + ${status.tape.names.length - 1}` : ''}</span>
            <div class="prog"><div style="width:{Math.round(status.tape.progress * 100)}%"></div></div>
            {#if status.tape.playing}
              <button class="icon-btn sm" title="Stop the tape" onclick={() => control('tape_stop')}><Square size={12} /></button>
            {:else}
              <button class="icon-btn sm" title="Play the tape" onclick={() => control('tape_play')}><Play size={12} /></button>
            {/if}
            <button class="icon-btn sm" title="Rewind" onclick={() => control('tape_rewind')}><Rewind size={12} /></button>
            <button class="icon-btn sm" title="Eject" onclick={() => control('eject')}><Eject size={12} /></button>
            <label class="chk" title="Run at top speed while the Spectrum reads the tape"><input type="checkbox" checked={status.fast_load} onchange={(e) => control((e.currentTarget as HTMLInputElement).checked ? 'fast_on' : 'fast_off')} /> Fast load</label>
          {:else}
            <span class="muted">No tape</span>
          {/if}
        </div>
        <span class="spacer"></span>
        <div class="keys">
          <Keyboard size={13} />
          <span class="muted">Shift = CAPS · Ctrl/Alt = SYMBOL · Esc = BREAK</span>
          <Gamepad2 size={13} />
          <select class="select" value={arrows} onchange={(e) => setArrows((e.currentTarget as HTMLSelectElement).value as Z.ArrowMode)} title="What the arrow keys (and Tab, to fire) do">
            {#each Object.entries(ARROWS) as [k, label]}<option value={k}>Arrows: {label}</option>{/each}
          </select>
        </div>
      </div>
    </section>
  </div>
</div>

<style>
  .zv {
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
    gap: 10px;
    height: 42px;
    padding: 0 12px;
    border-top: 1px solid var(--border);
    font-size: 12px;
  }
  .tape,
  .keys {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .tape :global(svg),
  .keys :global(svg) {
    flex: none;
    color: var(--text-3);
  }
  .tn {
    max-width: 200px;
  }
  .prog {
    width: 120px;
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
</style>
