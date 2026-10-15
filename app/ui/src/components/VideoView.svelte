<script lang="ts">
  import { onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { store } from '../lib/state.svelte';
  import { rad } from '../lib/api';
  import { video, videoEngine as eng, fileItem, vaultItem, pickFiles, fmtTime, ext, VIDEO_EXT, SUBTITLE_EXT } from '../lib/media.svelte';
  import type * as T from '../lib/types';
  import VaultMediaPicker from './VaultMediaPicker.svelte';
  import {
    Play, Pause, Square, SkipBack, SkipForward, Volume2, VolumeX, Maximize, Minimize, Captions, AudioLines,
    Gauge, ListVideo, FilePlus, Vault, Film, Loader, RotateCw, X, Repeat, Check,
  } from '@lucide/svelte';

  let canvas = $state<HTMLCanvasElement>();
  let stage = $state<HTMLDivElement>();
  let ctx: CanvasRenderingContext2D | null = null;
  let stageSize = $state({ w: 0, h: 0 });
  let frameSize = $state({ w: 0, h: 0 });
  let seeking = $state<number | null>(null);
  let controlsVisible = $state(true);
  let hideTimer: ReturnType<typeof setTimeout> | undefined;
  let menu = $state<'audio' | 'subs' | 'speed' | null>(null);
  let queueOpen = $state(false);
  let vaultOpen = $state(false);
  let fullscreen = $state(false);
  let dropOver = $state(false);
  let hasPicture = $state(false);

  const st = $derived(eng.status);
  const cur = $derived(video.currentItem);
  const length = $derived(st.length || cur?.duration || 0);
  const shownTime = $derived(seeking ?? st.time);
  const busy = $derived(eng.engine === 'starting' || st.state === 'opening' || st.state === 'buffering');
  const aspect = $derived(st.aspect ?? (frameSize.h ? frameSize.w / frameSize.h : 16 / 9));
  // The picture fitted into the stage at the video's real shape.
  const fit = $derived.by(() => {
    const { w, h } = stageSize;
    if (!w || !h) return { w: 0, h: 0 };
    const fw = Math.min(w, h * aspect);
    return { w: Math.round(fw), h: Math.round(fw / aspect) };
  });
  const dpr = () => window.devicePixelRatio || 1;
  const maxBox = () => ({ max_w: Math.round(stageSize.w * dpr()), max_h: Math.round(stageSize.h * dpr()) });

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
        draw(await rad.mediaFrame('video'));
      } while (again);
    } catch {
      /* engine gone; the 'exited' event explains */
    } finally {
      pulling = false;
    }
  }

  function draw(buf: ArrayBuffer) {
    if (!canvas || buf.byteLength < 16) return;
    const v = new DataView(buf);
    const [w, h] = [v.getUint32(0, true), v.getUint32(4, true)];
    if (!w || !h || buf.byteLength < 16 + w * h * 4) return;
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w;
      canvas.height = h;
      frameSize = { w, h };
      ctx = null;
    }
    ctx ??= canvas.getContext('2d', { alpha: false });
    ctx?.putImageData(new ImageData(new Uint8ClampedArray(buf, 16, w * h * 4), w, h), 0, 0);
    hasPicture = true;
  }

  // A new file: clear the old picture.
  let shownKey: string | null = null;
  $effect(() => {
    if (video.current !== shownKey) {
      shownKey = video.current;
      hasPicture = false;
    }
  });

  // ------------------------------------------------------------ opening

  async function openFiles() {
    const paths = await pickFiles('video');
    if (paths.length) video.add(paths.map(fileItem), true);
  }
  function openVault(docs: T.Document[]) {
    if (docs.length) video.add(docs.map(vaultItem), true);
  }
  function play(key?: string) {
    video.play(key, maxBox());
  }

  async function addSubtitle() {
    const [p] = await pickFiles('subtitle', false);
    if (p) eng.send({ cmd: 'subtitle_file', path: p });
    menu = null;
  }

  async function onDrop(paths: string[]) {
    const subs = paths.filter((p) => SUBTITLE_EXT.includes(ext(p)));
    const vids = paths.filter((p) => VIDEO_EXT.includes(ext(p)));
    if (subs.length && st.has_media) eng.send({ cmd: 'subtitle_file', path: subs[0] });
    if (vids.length) {
      const items = vids.map(fileItem);
      video.items = [...video.items, ...items];
      video.save();
      play(items[0].key);
    }
  }

  // ------------------------------------------------------------ controls

  function poke() {
    controlsVisible = true;
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => {
      if (st.state === 'playing' && !menu && !queueOpen) controlsVisible = false;
    }, 2600);
  }

  function toggle() {
    if (!st.has_media) {
      if (video.items.length) play();
      else openFiles();
      return;
    }
    eng.send({ cmd: st.state === 'ended' || st.state === 'stopped' ? 'play' : 'toggle' });
    poke();
  }

  function seekBy(ms: number) {
    eng.send({ cmd: 'seek', ms: Math.max(0, Math.min(length || Infinity, st.time + ms)) });
    poke();
  }

  function seekCommit() {
    if (seeking !== null) eng.send({ cmd: 'seek', ms: Math.round(seeking) });
    seeking = null;
  }

  async function setFullscreen(on: boolean) {
    fullscreen = on;
    await getCurrentWindow().setFullscreen(on).catch(() => {});
    poke();
  }

  function onKey(e: KeyboardEvent) {
    const t = e.target as HTMLElement;
    if (t.closest('input, textarea, select, [contenteditable]') || e.metaKey || e.ctrlKey || e.altKey || store.paletteOpen || document.querySelector('[aria-modal="true"]')) return;
    const map: Record<string, () => void> = {
      ' ': toggle,
      k: toggle,
      arrowleft: () => seekBy(e.shiftKey ? -30_000 : -5_000),
      arrowright: () => seekBy(e.shiftKey ? 30_000 : 5_000),
      j: () => seekBy(-10_000),
      l: () => seekBy(10_000),
      arrowup: () => (video.setVolume(video.volume + 5), poke()),
      arrowdown: () => (video.setVolume(video.volume - 5), poke()),
      m: () => (video.setMuted(!video.muted), poke()),
      f: () => setFullscreen(!fullscreen),
      escape: () => (menu ? (menu = null) : fullscreen && setFullscreen(false)),
      n: () => video.step(1),
      p: () => video.step(-1),
      o: openFiles,
    };
    const f = map[e.key.toLowerCase()];
    if (!f) return;
    e.preventDefault();
    f();
  }

  const speeds = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 1.75, 2, 3];
  const trackName = (n: string) => n.replace(/^Track (\d+)\s*-\s*/, '').replace(/^\[(.+)\]$/, '$1') || n;

  onMount(() => {
    const un: Array<() => void> = [];
    listen<string>('media-frame', (e) => e.payload === 'video' && pull()).then((u) => un.push(u));
    getCurrentWebview()
      .onDragDropEvent((ev) => {
        const p = ev.payload;
        if (p.type === 'enter' || p.type === 'over') dropOver = true;
        else if (p.type === 'leave') dropOver = false;
        else if (p.type === 'drop') {
          dropOver = false;
          onDrop(p.paths);
        }
      })
      .then((u) => un.push(u));
    // Attaching after the frame listener is up makes the engine resend the last picture.
    video.load().then(() => eng.attach()).then(() => eng.engine === 'running' && video.sync());

    let t: ReturnType<typeof setTimeout> | undefined;
    const ro = new ResizeObserver(() => {
      if (!stage) return;
      const r = stage.getBoundingClientRect();
      stageSize = { w: r.width, h: r.height };
      // Later videos are decoded at the size they're shown at.
      clearTimeout(t);
      t = setTimeout(() => eng.send({ cmd: 'max', w: maxBox().max_w, h: maxBox().max_h }), 200);
    });
    if (stage) ro.observe(stage);
    poke();
    return () => {
      ro.disconnect();
      clearTimeout(t);
      clearTimeout(hideTimer);
      un.forEach((u) => u());
      // Leaving the view pauses the film (music keeps going in the Music view).
      if (eng.status.state === 'playing') eng.send({ cmd: 'pause' });
      if (fullscreen) getCurrentWindow().setFullscreen(false).catch(() => {});
    };
  });
</script>

<svelte:window onkeydown={onKey} />

<div class="videoview" class:fs={fullscreen}>
  {#if !fullscreen}
    <header class="drag">
      <h1 class="truncate">{cur ? cur.title : 'Video'}</h1>
      <span class="spacer"></span>
      <button class="btn sm ghost no-drag" onclick={openFiles}><FilePlus size={14} /> Open file</button>
      <button class="btn sm ghost no-drag" onclick={() => (vaultOpen = true)}><Vault size={14} /> From vault</button>
      <button class="icon-btn no-drag" class:active={queueOpen} title="Queue" onclick={() => (queueOpen = !queueOpen)}><ListVideo size={16} /></button>
    </header>
  {/if}

  <div class="body">
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="stage" class:drop={dropOver} class:nocursor={!controlsVisible} bind:this={stage} onmousemove={poke} onmouseleave={() => st.state === 'playing' && (controlsVisible = false)}>
      {#if eng.engine === 'missing' || (eng.engine === 'stopped' && eng.engineError)}
        <div class="center msg">
          <Film size={34} strokeWidth={1.4} />
          <h2>The media engine isn’t available</h2>
          <p>{eng.engineError}</p>
          <p class="hint">Video plays through libVLC. Install VLC (<code>brew install --cask vlc</code>), then build the engine with <code>./scripts/build-media.sh</code>.</p>
          <button class="btn primary" onclick={() => eng.start()}><RotateCw size={14} /> Try again</button>
        </div>
      {:else}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <canvas
          bind:this={canvas}
          class:hidden={!hasPicture || !st.has_media}
          style="width:{fit.w}px;height:{fit.h}px"
          onclick={toggle}
          ondblclick={() => setFullscreen(!fullscreen)}
        ></canvas>
        {#if !st.has_media}
          <div class="center msg">
            <Film size={40} strokeWidth={1.3} />
            <h2>Open a video</h2>
            <p>MP4, MKV, MOV, AVI, WMV, MPEG, WebM and more, from this computer or your vault. You can also drop a file here.</p>
            <div class="row">
              <button class="btn primary" onclick={openFiles}><FilePlus size={15} /> Open file…</button>
              <button class="btn" onclick={() => (vaultOpen = true)}><Vault size={15} /> From vault…</button>
            </div>
            {#if video.items.length}
              <button class="btn ghost sm" onclick={() => play()}><Play size={13} /> Resume “{video.currentItem?.title ?? video.items[0].title}”</button>
            {/if}
          </div>
        {:else if busy}
          <div class="center"><Loader size={28} class="spin" /></div>
        {:else if st.state === 'ended'}
          <div class="center">
            <button class="big" onclick={() => eng.send({ cmd: 'seek', ms: 0 })} title="Watch again"><RotateCw size={26} /></button>
          </div>
        {:else if !hasPicture && !st.video && (st.state === 'playing' || st.state === 'paused')}
          <div class="center msg">
            <AudioLines size={40} strokeWidth={1.3} />
            <h2 class="truncate">{cur?.title}</h2>
            <p>This file has no picture, only sound.</p>
          </div>
        {:else if !hasPicture && st.state !== 'error' && st.state !== 'stopped'}
          <div class="center"><Loader size={28} class="spin" /></div>
        {/if}

        {#if st.has_media}
          <div class="controls" class:hide={!controlsVisible}>
            <input
              class="seek"
              type="range"
              min="0"
              max={length || 1}
              step="100"
              value={shownTime}
              disabled={!st.seekable}
              style="--p:{length ? (shownTime / length) * 100 : 0}%"
              oninput={(e) => (seeking = +e.currentTarget.value)}
              onchange={seekCommit}
              aria-label="Position"
            />
            <div class="ctl">
              <button title={st.state === 'playing' ? 'Pause (Space)' : 'Play (Space)'} onclick={toggle}>
                {#if st.state === 'playing'}<Pause size={18} fill="currentColor" />{:else}<Play size={18} fill="currentColor" />{/if}
              </button>
              <button title="Stop" onclick={() => eng.send({ cmd: 'stop' })}><Square size={15} fill="currentColor" /></button>
              {#if video.items.length > 1}
                <button title="Previous (P)" onclick={() => video.step(-1)}><SkipBack size={16} fill="currentColor" /></button>
                <button title="Next (N)" onclick={() => video.step(1)}><SkipForward size={16} fill="currentColor" /></button>
              {/if}
              <div class="vol">
                <button title="Mute (M)" onclick={() => video.setMuted(!video.muted)}>
                  {#if video.muted || video.volume === 0}<VolumeX size={17} />{:else}<Volume2 size={17} />{/if}
                </button>
                <input type="range" min="0" max="100" value={video.volume} style="--p:{video.volume}%" oninput={(e) => video.setVolume(+e.currentTarget.value)} aria-label="Volume" />
              </div>
              <span class="time">{fmtTime(shownTime)} / {fmtTime(length)}</span>
              <span class="spacer"></span>

              <div class="pop">
                <button title="Speed" class:on={st.rate !== 1} onclick={() => (menu = menu === 'speed' ? null : 'speed')}>
                  <Gauge size={17} />{#if st.rate !== 1}<small>{st.rate}×</small>{/if}
                </button>
                {#if menu === 'speed'}
                  <div class="menu">
                    {#each speeds as s (s)}
                      <button onclick={() => (eng.send({ cmd: 'rate', value: s }), (menu = null))}>
                        <span class="ck">{#if Math.abs(st.rate - s) < 0.01}<Check size={13} />{/if}</span>{s === 1 ? 'Normal' : `${s}×`}
                      </button>
                    {/each}
                  </div>
                {/if}
              </div>
              {#if (eng.tracks?.audio.length ?? 0) > 2}
                <div class="pop">
                  <button title="Audio track" onclick={() => (menu = menu === 'audio' ? null : 'audio')}><AudioLines size={17} /></button>
                  {#if menu === 'audio'}
                    <div class="menu">
                      {#each eng.tracks?.audio ?? [] as t (t.id)}
                        <button onclick={() => (eng.send({ cmd: 'audio_track', id: t.id }), (menu = null))}>
                          <span class="ck">{#if eng.tracks?.audio_current === t.id}<Check size={13} />{/if}</span>{trackName(t.name)}
                        </button>
                      {/each}
                    </div>
                  {/if}
                </div>
              {/if}
              <div class="pop">
                <button title="Subtitles" class:on={(eng.tracks?.spu_current ?? -1) >= 0} onclick={() => (menu = menu === 'subs' ? null : 'subs')}><Captions size={17} /></button>
                {#if menu === 'subs'}
                  <div class="menu">
                    {#each eng.tracks?.spu.length ? eng.tracks.spu : [{ id: -1, name: 'Off' }] as t (t.id)}
                      <button onclick={() => (eng.send({ cmd: 'spu', id: t.id }), (menu = null))}>
                        <span class="ck">{#if eng.tracks?.spu_current === t.id || (t.id === -1 && !eng.tracks?.spu.length)}<Check size={13} />{/if}</span>{t.id === -1 ? 'Off' : trackName(t.name)}
                      </button>
                    {/each}
                    <div class="sep"></div>
                    <button onclick={addSubtitle}><span class="ck"></span>Load subtitle file…</button>
                  </div>
                {/if}
              </div>
              <button title="Repeat: {video.repeat}" class:on={video.repeat !== 'off'} onclick={() => ((video.repeat = video.repeat === 'off' ? 'one' : video.repeat === 'one' ? 'all' : 'off'), video.save())}>
                <Repeat size={16} />{#if video.repeat === 'one'}<small>1</small>{/if}
              </button>
              <button title={fullscreen ? 'Exit full screen (F)' : 'Full screen (F)'} onclick={() => setFullscreen(!fullscreen)}>
                {#if fullscreen}<Minimize size={17} />{:else}<Maximize size={17} />{/if}
              </button>
            </div>
          </div>
        {/if}
      {/if}
    </div>

    {#if queueOpen && !fullscreen}
      <aside class="queue">
        <div class="qh">
          <span>Queue</span>
          <span class="spacer"></span>
          <button class="icon-btn sm" title="Add files" onclick={async () => video.add((await pickFiles('video')).map(fileItem))}><FilePlus size={14} /></button>
          <button class="icon-btn sm" title="Close" onclick={() => (queueOpen = false)}><X size={14} /></button>
        </div>
        <div class="ql">
          {#each video.items as it, i (it.key)}
            <div class="qi" class:cur={it.key === video.current}>
              <button class="qt" onclick={() => play(it.key)} title={it.src.kind === 'file' ? it.src.path : 'From the vault'}>
                <span class="qn">{i + 1}</span>
                <span class="truncate">{it.title}</span>
                {#if it.src.kind === 'vault'}<Vault size={12} />{/if}
              </button>
              <button class="icon-btn sm" title="Remove" onclick={() => video.remove(new Set([it.key]))}><X size={13} /></button>
            </div>
          {:else}
            <div class="qe">Nothing queued.</div>
          {/each}
        </div>
      </aside>
    {/if}
  </div>
</div>

<VaultMediaPicker bind:open={vaultOpen} kind="video" multiple={false} onpick={openVault} />

<style>
  .videoview {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .videoview.fs {
    position: fixed;
    inset: 0;
    z-index: 100;
    background: #000;
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
    min-width: 0;
  }
  .spacer {
    flex: 1;
  }
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .stage {
    flex: 1;
    min-width: 0;
    position: relative;
    display: grid;
    place-items: center;
    background: #000;
    overflow: hidden;
  }
  .stage.drop {
    outline: 2px dashed var(--accent);
    outline-offset: -10px;
  }
  .stage.nocursor {
    cursor: none;
  }
  canvas {
    display: block;
    grid-area: 1 / 1;
  }
  canvas.hidden {
    visibility: hidden;
  }
  .center {
    grid-area: 1 / 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    color: #d8d8e2;
    z-index: 1;
  }
  .msg {
    max-width: 460px;
    text-align: center;
    padding: 0 20px;
  }
  .msg h2 {
    margin: 4px 0 0;
    font-size: 17px;
    color: #fff;
  }
  .msg p {
    margin: 0;
    color: #a0a0b0;
    line-height: 1.5;
  }
  .msg .hint {
    font-size: 12.5px;
  }
  .msg .row {
    display: flex;
    gap: 8px;
    margin-top: 6px;
  }
  .big {
    width: 64px;
    height: 64px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    color: #fff;
    background: rgba(255, 255, 255, 0.14);
    backdrop-filter: blur(8px);
  }

  .controls {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 0;
    padding: 28px 14px 10px;
    background: linear-gradient(180deg, transparent, rgba(0, 0, 0, 0.78));
    color: #fff;
    transition: opacity 260ms var(--ease);
    z-index: 2;
  }
  .controls.hide {
    opacity: 0;
    pointer-events: none;
  }
  input[type='range'] {
    -webkit-appearance: none;
    appearance: none;
    background: transparent;
    cursor: pointer;
  }
  input[type='range']::-webkit-slider-runnable-track {
    height: 4px;
    border-radius: 2px;
    background: linear-gradient(90deg, var(--accent) var(--p, 0%), rgba(255, 255, 255, 0.28) var(--p, 0%));
  }
  input[type='range']::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 13px;
    height: 13px;
    margin-top: -4.5px;
    border-radius: 50%;
    background: #fff;
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.5);
  }
  .seek {
    width: 100%;
    display: block;
    margin-bottom: 6px;
  }
  .ctl {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .ctl button {
    position: relative;
    display: inline-flex;
    align-items: center;
    gap: 3px;
    height: 34px;
    min-width: 34px;
    justify-content: center;
    padding: 0 6px;
    border-radius: 8px;
    color: #f2f2f6;
  }
  .ctl button:hover {
    background: rgba(255, 255, 255, 0.12);
  }
  .ctl button.on {
    color: var(--accent);
  }
  .ctl small {
    font-size: 11px;
    font-weight: 600;
  }
  .vol {
    display: flex;
    align-items: center;
  }
  .vol input {
    width: 90px;
  }
  .time {
    margin-left: 10px;
    font-size: 12.5px;
    font-variant-numeric: tabular-nums;
    color: #e2e2ea;
  }
  .pop {
    position: relative;
  }
  .menu {
    position: absolute;
    right: 0;
    bottom: 40px;
    min-width: 170px;
    max-height: 300px;
    overflow: auto;
    padding: 5px;
    border-radius: 10px;
    background: rgba(24, 24, 30, 0.94);
    backdrop-filter: blur(14px);
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.5);
    border: 1px solid rgba(255, 255, 255, 0.08);
  }
  .menu button {
    width: 100%;
    height: 30px;
    justify-content: flex-start;
    font-size: 13px;
    white-space: nowrap;
  }
  .ck {
    width: 18px;
    display: inline-flex;
  }
  .sep {
    height: 1px;
    margin: 4px 2px;
    background: rgba(255, 255, 255, 0.1);
  }

  .queue {
    width: 280px;
    flex: none;
    display: flex;
    flex-direction: column;
    border-left: 1px solid var(--border);
    background: var(--surface);
  }
  .qh {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 10px 8px 6px 14px;
    font-weight: 650;
    font-size: 13px;
  }
  .ql {
    flex: 1;
    overflow: auto;
    padding: 0 6px 10px;
  }
  .qi {
    display: flex;
    align-items: center;
    border-radius: var(--r-sm);
  }
  .qi:hover {
    background: var(--surface-hover);
  }
  .qi.cur {
    background: var(--accent-soft);
  }
  .qt {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 7px 8px;
    text-align: left;
    color: var(--text);
    font-size: 13px;
  }
  .qn {
    color: var(--text-3);
    font-size: 11.5px;
    min-width: 2ch;
    text-align: right;
  }
  .qe {
    padding: 20px;
    text-align: center;
    color: var(--text-3);
    font-size: 13px;
  }
</style>
