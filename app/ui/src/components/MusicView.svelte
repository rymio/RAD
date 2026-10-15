<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { store } from '../lib/state.svelte';
  import { rad } from '../lib/api';
  import { music, musicEngine as eng, fileItem, vaultItem, pickFiles, pickFolder, fmtTime, ext, AUDIO_EXT } from '../lib/media.svelte';
  import type * as T from '../lib/types';
  import VaultMediaPicker from './VaultMediaPicker.svelte';
  import {
    SkipBack, Play, Pause, Square, SkipForward, Eject, Shuffle, Repeat, Repeat1, Volume2, VolumeX,
    FilePlus, FolderPlus, Vault, Minus, Trash2, Search, Loader, Music, RotateCw,
  } from '@lucide/svelte';

  let showEq = $state(true);
  let showPl = $state(true);
  let remaining = $state(false);
  let vaultOpen = $state(false);
  let filter = $state('');
  let selected = $state<Set<string>>(new Set());
  let anchor = -1;
  let seeking = $state<number | null>(null);
  let dragFrom = $state<number | null>(null);
  let dropOver = $state(false);
  let listEl = $state<HTMLDivElement>();

  const st = $derived(eng.status);
  const cur = $derived(music.currentItem);
  const length = $derived(st.length || cur?.duration || 0);
  const shownTime = $derived(seeking ?? st.time);
  const playing = $derived(st.state === 'playing' || st.state === 'opening' || st.state === 'buffering');
  const totalMs = $derived(music.items.reduce((a, i) => a + (i.duration ?? 0), 0));
  const rows = $derived.by(() => {
    const q = filter.trim().toLowerCase();
    const all = music.items.map((it, i) => ({ it, i }));
    return q ? all.filter(({ it }) => `${it.artist ?? ''} ${it.title} ${it.album ?? ''}`.toLowerCase().includes(q)) : all;
  });

  const label = (it: T.MediaItem) => (it.artist ? `${it.artist} - ${it.title}` : it.title);
  const marquee = $derived(cur ? `${music.index + 1}. ${label(cur)} (${fmtTime(length)})` : 'RAD Music — open a file, a folder or something from your vault');
  const format = $derived(cur ? (cur.src.kind === 'file' ? ext(cur.src.path) : 'vault').toUpperCase() : '');
  const eqFreqs = $derived(eng.eqInfo?.freqs.length ? eng.eqInfo.freqs : [31.25, 62.5, 125, 250, 500, 1000, 2000, 4000, 8000, 16000]);
  const hz = (f: number) => (f >= 1000 ? `${+(f / 1000).toFixed(1)}K` : `${Math.round(f)}`);

  // ------------------------------------------------------------ adding

  async function addFiles(play = false) {
    const paths = await pickFiles('audio');
    music.add(paths.map(fileItem), play);
  }
  async function addFolder() {
    const paths = await pickFolder('audio');
    if (!paths.length) return store.toast('info', 'No music found', 'That folder has no audio files.');
    music.add(paths.map(fileItem));
    store.toast('success', 'Added to the playlist', `${paths.length} file${paths.length === 1 ? '' : 's'}`, 2500);
  }
  function addVault(docs: T.Document[]) {
    music.add(docs.map(vaultItem));
  }

  /** Dropped files and folders (folders are searched for audio). */
  async function addDropped(paths: string[]) {
    const files: string[] = [];
    for (const p of paths) {
      if (AUDIO_EXT.includes(ext(p))) files.push(p);
      else {
        const l = await rad.fsList(p).catch(() => null);
        if (l) for (const e of l.entries) if (!e.is_dir && !e.hidden && AUDIO_EXT.includes(ext(e.name))) files.push(e.path);
      }
    }
    if (files.length) music.add(files.map(fileItem));
  }

  // ------------------------------------------------------------ playlist

  function clickRow(e: MouseEvent, i: number) {
    const key = music.items[i].key;
    if (e.shiftKey && anchor >= 0) {
      const [a, b] = [Math.min(anchor, i), Math.max(anchor, i)];
      selected = new Set(music.items.slice(a, b + 1).map((x) => x.key));
    } else if (e.metaKey || e.ctrlKey) {
      const s = new Set(selected);
      s.has(key) ? s.delete(key) : s.add(key);
      selected = s;
      anchor = i;
    } else {
      selected = new Set([key]);
      anchor = i;
    }
  }

  function removeSelected() {
    if (!selected.size) return;
    music.remove(selected);
    selected = new Set();
  }

  function drop(to: number) {
    if (dragFrom !== null) music.move(dragFrom, to);
    dragFrom = null;
  }

  async function revealCurrent() {
    await tick();
    listEl?.querySelector('.row.current')?.scrollIntoView({ block: 'nearest' });
  }
  $effect(() => {
    if (music.current) revealCurrent();
  });

  // ------------------------------------------------------------ transport

  function seekCommit() {
    if (seeking !== null) eng.send({ cmd: 'seek', ms: Math.round(seeking) });
    seeking = null;
  }

  function setPreset(name: string) {
    const p = eng.eqInfo?.presets.find((x) => x.name === name);
    if (p) music.setEq({ preset: p.name, preamp: p.preamp, bands: [...p.bands], enabled: true });
  }

  function setBand(i: number, v: number) {
    const bands = [...music.eq.bands];
    bands[i] = v;
    music.setEq({ bands, preset: 'Custom' });
  }

  const eqPath = $derived.by(() => {
    const b = music.eq.bands;
    const n = Math.max(1, b.length - 1);
    return b.map((v, i) => `${((i / n) * 100).toFixed(1)},${(20 - v * 0.9).toFixed(1)}`).join(' ');
  });

  function onKey(e: KeyboardEvent) {
    const t = e.target as HTMLElement;
    if (t.closest('input, textarea, select, [contenteditable]') || e.metaKey || e.altKey || store.paletteOpen || document.querySelector('[aria-modal="true"]')) return;
    const k = e.key.toLowerCase();
    // Winamp's keys: Z X C V B = previous, play, pause, stop, next.
    const map: Record<string, () => void> = {
      ' ': () => music.toggle(),
      z: () => music.step(-1),
      x: () => (st.state === 'paused' ? eng.send({ cmd: 'play' }) : music.play(music.current ?? undefined)),
      c: () => eng.send({ cmd: 'toggle' }),
      v: () => music.stop(),
      b: () => music.step(1),
      arrowleft: () => eng.send({ cmd: 'seek', ms: Math.max(0, st.time - 5000) }),
      arrowright: () => eng.send({ cmd: 'seek', ms: st.time + 5000 }),
      arrowup: () => music.setVolume(music.volume + 5),
      arrowdown: () => music.setVolume(music.volume - 5),
      delete: removeSelected,
      backspace: removeSelected,
      enter: () => selected.size === 1 && music.play([...selected][0]),
      l: () => addFiles(true),
    };
    const f = map[k];
    if (!f) return;
    e.preventDefault();
    f();
  }

  onMount(() => {
    music.load().then(() => eng.attach()).then(() => eng.engine === 'running' && music.sync());
    let un: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((ev) => {
        const p = ev.payload;
        if (p.type === 'enter' || p.type === 'over') dropOver = true;
        else if (p.type === 'leave') dropOver = false;
        else if (p.type === 'drop') {
          dropOver = false;
          addDropped(p.paths);
        }
      })
      .then((u) => (un = u));
    return () => un?.();
  });
</script>

<svelte:window onkeydown={onKey} />

<div class="musicview">
  <header class="drag">
    <h1>Music</h1>
    <span class="spacer"></span>
    <button class="btn sm ghost no-drag" onclick={() => addFiles()}><FilePlus size={14} /> Add files</button>
    <button class="btn sm ghost no-drag" onclick={addFolder}><FolderPlus size={14} /> Add folder</button>
    <button class="btn sm ghost no-drag" onclick={() => (vaultOpen = true)}><Vault size={14} /> From vault</button>
  </header>

  <div class="stage" class:drop={dropOver}>
    {#if eng.engine === 'missing' || (eng.engine === 'stopped' && eng.engineError)}
      <div class="missing">
        <Music size={30} strokeWidth={1.5} />
        <h2>The media engine isn’t available</h2>
        <p>{eng.engineError}</p>
        <p class="hint">Music and Video play through libVLC. Install VLC (<code>brew install --cask vlc</code>), then build the engine with <code>./scripts/build-media.sh</code>.</p>
        <button class="btn primary" onclick={() => eng.start()}><RotateCw size={14} /> Try again</button>
      </div>
    {:else}
      <div class="amp">
        <!-- Main window -->
        <section class="win main">
          <div class="bar">
            <span class="logo">RAD&nbsp;AMP</span>
            <span class="grip"></span>
            <button class="tog" class:on={showEq} onclick={() => (showEq = !showEq)} title="Equalizer">EQ</button>
            <button class="tog" class:on={showPl} onclick={() => (showPl = !showPl)} title="Playlist">PL</button>
          </div>
          <div class="display">
            <div class="lcd clock">
              <span class="state" title={st.state}>
                {#if eng.engine === 'starting' || st.state === 'opening' || st.state === 'buffering'}<Loader size={13} class="spin" />
                {:else if st.state === 'playing'}<Play size={13} fill="currentColor" />
                {:else if st.state === 'paused'}<Pause size={13} fill="currentColor" />
                {:else}<Square size={11} fill="currentColor" />{/if}
              </span>
              <button class="time" class:blink={st.state === 'paused'} onclick={() => (remaining = !remaining)} title="Elapsed / remaining">
                {#if !st.has_media}&nbsp;&nbsp;:&nbsp;&nbsp;{:else if remaining && length}-{fmtTime(Math.max(0, length - shownTime))}{:else}{fmtTime(shownTime)}{/if}
              </button>
            </div>
            <div class="lcd info">
              <div class="marquee" title={marquee}><span class:run={marquee.length > 44}>{marquee}{#if marquee.length > 44}<i>&nbsp;***&nbsp;</i>{marquee}<i>&nbsp;***&nbsp;</i>{/if}</span></div>
              <div class="facts">
                <span>{format || '---'}</span>
                {#if cur?.album}<span class="truncate">{cur.album}</span>{/if}
                <span class="grow"></span>
                {#if st.rate !== 1}<span>{st.rate}×</span>{/if}
                <span class:lit={music.eq.enabled}>EQ</span>
                <span class:lit={music.shuffle}>SHUF</span>
                <span class:lit={music.repeat !== 'off'}>{music.repeat === 'one' ? 'REP1' : 'REP'}</span>
              </div>
            </div>
          </div>
          <input
            class="seek"
            type="range"
            min="0"
            max={length || 1}
            step="250"
            value={shownTime}
            disabled={!st.has_media || !st.seekable}
            style="--p:{length ? (shownTime / length) * 100 : 0}%"
            oninput={(e) => (seeking = +e.currentTarget.value)}
            onchange={seekCommit}
            aria-label="Position"
          />
          <div class="transport">
            <div class="keys">
              <button title="Previous (Z)" onclick={() => music.step(-1)}><SkipBack size={15} fill="currentColor" /></button>
              <button title="Play (X)" class:lit={playing} onclick={() => (st.state === 'paused' ? eng.send({ cmd: 'play' }) : music.play(music.current ?? undefined))}><Play size={15} fill="currentColor" /></button>
              <button title="Pause (C)" class:lit={st.state === 'paused'} onclick={() => eng.send({ cmd: 'toggle' })}><Pause size={15} fill="currentColor" /></button>
              <button title="Stop (V)" onclick={() => music.stop()}><Square size={13} fill="currentColor" /></button>
              <button title="Next (B)" onclick={() => music.step(1)}><SkipForward size={15} fill="currentColor" /></button>
              <button title="Open files (L)" onclick={() => addFiles(true)}><Eject size={15} /></button>
            </div>
            <div class="modes">
              <button class:lit={music.shuffle} title="Shuffle" onclick={() => ((music.shuffle = !music.shuffle), music.save())}><Shuffle size={14} /></button>
              <button
                class:lit={music.repeat !== 'off'}
                title="Repeat: {music.repeat}"
                onclick={() => ((music.repeat = music.repeat === 'off' ? 'all' : music.repeat === 'all' ? 'one' : 'off'), music.save())}
              >
                {#if music.repeat === 'one'}<Repeat1 size={14} />{:else}<Repeat size={14} />{/if}
              </button>
            </div>
            <div class="vol">
              <button title={music.muted ? 'Unmute' : 'Mute'} onclick={() => music.setMuted(!music.muted)}>
                {#if music.muted || music.volume === 0}<VolumeX size={15} />{:else}<Volume2 size={15} />{/if}
              </button>
              <input type="range" min="0" max="100" value={music.volume} style="--p:{music.volume}%" oninput={(e) => music.setVolume(+e.currentTarget.value)} aria-label="Volume" />
            </div>
          </div>
        </section>

        <!-- Equalizer -->
        {#if showEq}
          <section class="win eq">
            <div class="bar">
              <span class="logo">EQUALIZER</span>
              <span class="grip"></span>
              <button class="tog" class:on={music.eq.enabled} onclick={() => music.setEq({ enabled: !music.eq.enabled })}>ON</button>
              <select class="preset" value={music.eq.preset} onchange={(e) => setPreset(e.currentTarget.value)} aria-label="Preset">
                {#if !eng.eqInfo?.presets.some((p) => p.name === music.eq.preset)}<option value={music.eq.preset}>{music.eq.preset}</option>{/if}
                {#each eng.eqInfo?.presets ?? [] as p (p.name)}<option value={p.name}>{p.name}</option>{/each}
              </select>
              <button class="tog" onclick={() => music.setEq({ preamp: 0, bands: Array(10).fill(0), preset: 'Flat' })}>RESET</button>
            </div>
            <div class="eqbody" class:off={!music.eq.enabled}>
              <div class="band pre">
                <input type="range" class="v" min="-20" max="20" step="0.5" value={music.eq.preamp} oninput={(e) => music.setEq({ preamp: +e.currentTarget.value, preset: 'Custom' })} aria-label="Preamp" />
                <span>PRE</span>
              </div>
              <div class="scale"><span>+20</span><span>0</span><span>−20</span></div>
              <div class="bands">
                <svg class="curve" viewBox="0 0 100 40" preserveAspectRatio="none" aria-hidden="true"><polyline points={eqPath} /></svg>
                {#each music.eq.bands as b, i (i)}
                  <div class="band">
                    <input type="range" class="v" min="-20" max="20" step="0.5" value={b} oninput={(e) => setBand(i, +e.currentTarget.value)} aria-label="{hz(eqFreqs[i] ?? 0)} Hz" />
                    <span>{hz(eqFreqs[i] ?? 0)}</span>
                  </div>
                {/each}
              </div>
            </div>
          </section>
        {/if}

        <!-- Playlist -->
        {#if showPl}
          <section class="win pl">
            <div class="bar">
              <span class="logo">PLAYLIST</span>
              <span class="grip"></span>
              <label class="filter"><Search size={12} /><input placeholder="Filter" bind:value={filter} /></label>
            </div>
            <!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events -->
            <div class="list" bind:this={listEl} onclick={(e) => e.target === e.currentTarget && (selected = new Set())}>
              {#each rows as { it, i } (it.key)}
                <!-- svelte-ignore a11y_click_events_have_key_events -->
                <div
                  class="row"
                  class:current={it.key === music.current}
                  class:sel={selected.has(it.key)}
                  class:over={dragFrom !== null && dragFrom !== i}
                  role="option"
                  aria-selected={selected.has(it.key)}
                  tabindex="-1"
                  draggable={!filter}
                  ondragstart={(e) => ((dragFrom = i), e.dataTransfer?.setData('text/plain', String(i)))}
                  ondragover={(e) => dragFrom !== null && e.preventDefault()}
                  ondrop={(e) => (e.preventDefault(), drop(i))}
                  ondragend={() => (dragFrom = null)}
                  onclick={(e) => clickRow(e, i)}
                  ondblclick={() => music.play(it.key)}
                >
                  <span class="n">{i + 1}.</span>
                  <span class="t truncate">{label(it)}{#if it.src.kind === 'vault'}<em> ◆</em>{/if}</span>
                  <span class="d">{it.key === music.current && st.length ? fmtTime(st.length) : it.duration ? fmtTime(it.duration) : ''}</span>
                </div>
              {:else}
                <div class="empty">
                  {#if music.items.length}Nothing matches “{filter}”.{:else}Drop music here, or use <b>+ FILE</b>, <b>+ DIR</b> or <b>+ VAULT</b>. Plays MP3, FLAC, WAV, AAC, OGG, WMA and the sound of MP4/WMV videos.{/if}
                </div>
              {/each}
            </div>
            <div class="plfoot">
              <button onclick={() => addFiles()} title="Add files"><FilePlus size={12} /> FILE</button>
              <button onclick={addFolder} title="Add a folder"><FolderPlus size={12} /> DIR</button>
              <button onclick={() => (vaultOpen = true)} title="Add from the vault"><Vault size={12} /> VAULT</button>
              <button onclick={removeSelected} disabled={!selected.size} title="Remove selected (Delete)"><Minus size={12} /> REM</button>
              <button onclick={() => (music.clear(), (selected = new Set()))} disabled={!music.items.length} title="Clear the playlist"><Trash2 size={12} /> CLR</button>
              <span class="grow"></span>
              <span class="sum">{music.items.length} track{music.items.length === 1 ? '' : 's'} · {fmtTime(totalMs)}</span>
            </div>
          </section>
        {/if}
        {#if eng.version}<div class="credit">libVLC {eng.version.split(' ')[0]}</div>{/if}
      </div>
    {/if}
  </div>
</div>

<VaultMediaPicker bind:open={vaultOpen} kind="audio" onpick={addVault} />

<style>
  .musicview {
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
  .spacer,
  .grow {
    flex: 1;
  }
  .stage {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 18px 16px 24px;
    display: flex;
    justify-content: center;
    transition: background var(--t);
  }
  .stage.drop {
    background: var(--accent-softer);
    outline: 2px dashed var(--accent-ring);
    outline-offset: -8px;
  }

  /* ---------------------------------------------------------- the skin */
  .amp {
    --skin: #262634;
    --skin-hi: #3a3a4e;
    --skin-lo: #14141c;
    --lcd: #050806;
    --lcd-on: #48f07a;
    --lcd-dim: #1d5a30;
    --lcd-glow: 0 0 6px rgba(72, 240, 122, 0.45);
    --pl-text: #39e26a;
    --pl-cur: #ffffff;
    --pl-sel: #1b2fb8;
    width: 100%;
    max-width: 720px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-height: 100%;
    font-family: var(--mono);
  }
  .win {
    background: linear-gradient(180deg, var(--skin-hi), var(--skin) 26px, var(--skin-lo));
    border: 1px solid #0a0a10;
    border-radius: 6px;
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.08), 0 8px 24px rgba(0, 0, 0, 0.25);
    padding: 0 8px 8px;
    color: #c9c9d6;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 26px;
  }
  .logo {
    font-size: 10.5px;
    letter-spacing: 0.18em;
    font-weight: 700;
    color: #e8e8f2;
    text-shadow: 0 1px 0 #000;
  }
  .grip {
    flex: 1;
    height: 6px;
    background: repeating-linear-gradient(180deg, rgba(255, 255, 255, 0.13) 0 1px, transparent 1px 3px);
    opacity: 0.7;
  }
  .tog {
    font: 700 9.5px var(--mono);
    letter-spacing: 0.08em;
    padding: 2px 6px;
    border-radius: 3px;
    color: #8f8fa3;
    background: #15151d;
    border: 1px solid #000;
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.06);
  }
  .tog.on {
    color: var(--lcd-on);
    text-shadow: var(--lcd-glow);
  }
  .lcd {
    background: var(--lcd);
    border: 1px solid #000;
    border-radius: 3px;
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.04), inset 0 2px 8px rgba(0, 0, 0, 0.8);
    color: var(--lcd-on);
    text-shadow: var(--lcd-glow);
  }

  /* main window */
  .display {
    display: grid;
    grid-template-columns: 150px 1fr;
    gap: 6px;
  }
  .clock {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 10px;
  }
  .state {
    display: flex;
    width: 14px;
  }
  .time {
    flex: 1;
    text-align: right;
    font: 600 30px/1 var(--mono);
    letter-spacing: 0.02em;
    color: inherit;
    font-variant-numeric: tabular-nums;
  }
  .time.blink {
    animation: blink 1s steps(2, start) infinite;
  }
  @keyframes blink {
    to {
      visibility: hidden;
    }
  }
  .info {
    min-width: 0;
    padding: 7px 10px 6px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .marquee {
    overflow: hidden;
    white-space: nowrap;
    font-size: 13px;
  }
  .marquee span {
    display: inline-block;
  }
  .marquee span.run {
    animation: scroll 18s linear infinite;
  }
  .marquee i {
    font-style: normal;
  }
  @keyframes scroll {
    to {
      transform: translateX(-50%);
    }
  }
  .facts {
    display: flex;
    gap: 10px;
    font-size: 10px;
    letter-spacing: 0.08em;
    min-width: 0;
  }
  .facts span:not(.grow) {
    color: var(--lcd-on);
  }
  .facts span:not(.lit):is(:nth-last-child(-n + 3)) {
    color: var(--lcd-dim);
    text-shadow: none;
  }

  input[type='range'] {
    -webkit-appearance: none;
    appearance: none;
    background: transparent;
    cursor: pointer;
  }
  input[type='range']::-webkit-slider-runnable-track {
    height: 6px;
    border-radius: 3px;
    border: 1px solid #000;
    background: linear-gradient(90deg, #2fa653 var(--p, 0%), #101016 var(--p, 0%));
  }
  input[type='range']::-webkit-slider-thumb {
    -webkit-appearance: none;
    width: 22px;
    height: 12px;
    margin-top: -4px;
    border-radius: 2px;
    border: 1px solid #000;
    background: linear-gradient(180deg, #e4e4ee, #8d8da0);
  }
  input[type='range']:disabled {
    cursor: default;
    opacity: 0.55;
  }
  .seek {
    width: 100%;
    margin: 10px 0 8px;
  }
  .transport {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }
  .keys,
  .modes {
    display: flex;
    gap: 3px;
  }
  .transport button {
    display: grid;
    place-items: center;
    width: 32px;
    height: 26px;
    border-radius: 3px;
    color: #d4d4e0;
    background: linear-gradient(180deg, #4a4a60, #2a2a38);
    border: 1px solid #000;
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.12);
  }
  .transport button:hover {
    background: linear-gradient(180deg, #56566e, #30303f);
  }
  .transport button:active {
    background: linear-gradient(180deg, #24242f, #3a3a4c);
  }
  .transport button.lit {
    color: var(--lcd-on);
  }
  .vol {
    flex: 1;
    min-width: 140px;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .vol input {
    flex: 1;
  }

  /* equalizer */
  .preset {
    font: 600 10.5px var(--mono);
    color: #d4d4e0;
    background: #15151d;
    border: 1px solid #000;
    border-radius: 3px;
    padding: 1px 4px;
    max-width: 150px;
  }
  .eqbody {
    display: flex;
    gap: 8px;
    padding: 8px 6px 2px;
    background: #101016;
    border: 1px solid #000;
    border-radius: 3px;
    transition: opacity var(--t);
  }
  .eqbody.off {
    opacity: 0.5;
  }
  .band {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 4px;
    font-size: 9.5px;
    color: #9a9aae;
  }
  .band.pre {
    padding-right: 6px;
    border-right: 1px solid #23232e;
  }
  .scale {
    display: flex;
    flex-direction: column;
    justify-content: space-between;
    font-size: 9px;
    color: #6d6d80;
    padding: 2px 0 18px;
  }
  .bands {
    flex: 1;
    position: relative;
    display: flex;
    justify-content: space-between;
  }
  .curve {
    position: absolute;
    inset: 4px 10px 22px;
    width: calc(100% - 20px);
    height: calc(100% - 26px);
    pointer-events: none;
    opacity: 0.4;
  }
  .curve polyline {
    fill: none;
    stroke: var(--lcd-on);
    stroke-width: 1.2;
    vector-effect: non-scaling-stroke;
  }
  input.v {
    writing-mode: vertical-lr;
    direction: rtl;
    width: 14px;
    height: 104px;
  }
  input.v::-webkit-slider-runnable-track {
    width: 6px;
    height: 100%;
    background: linear-gradient(0deg, #1d5a30, #2fa653 50%, #d0a52a);
  }
  input.v::-webkit-slider-thumb {
    width: 14px;
    height: 9px;
    margin: 0 0 0 -5px;
  }

  /* playlist */
  .pl {
    flex: 1;
    min-height: 240px;
    display: flex;
    flex-direction: column;
  }
  .filter {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 0 6px;
    height: 18px;
    background: #050508;
    border: 1px solid #000;
    border-radius: 3px;
    color: #6d6d80;
  }
  .filter input {
    width: 120px;
    border: 0;
    outline: none;
    background: transparent;
    color: var(--pl-text);
    font: 11px var(--mono);
  }
  .list {
    flex: 1;
    min-height: 0;
    overflow: auto;
    background: #000;
    border: 1px solid #000;
    border-radius: 3px;
    padding: 3px 0;
    font-size: 12px;
  }
  .row {
    display: grid;
    grid-template-columns: auto 1fr auto;
    gap: 6px;
    padding: 2px 8px;
    color: var(--pl-text);
    cursor: default;
    user-select: none;
  }
  .row.sel {
    background: var(--pl-sel);
  }
  .row.current {
    color: var(--pl-cur);
  }
  .row.over:hover {
    box-shadow: inset 0 2px 0 var(--lcd-on);
  }
  .n {
    min-width: 2.5ch;
    text-align: right;
    opacity: 0.85;
  }
  .row em {
    font-style: normal;
    opacity: 0.6;
    font-size: 10px;
  }
  .d {
    font-variant-numeric: tabular-nums;
  }
  .empty {
    padding: 22px 16px;
    color: #4fa76a;
    line-height: 1.6;
    text-align: center;
  }
  .plfoot {
    display: flex;
    align-items: center;
    gap: 4px;
    margin-top: 6px;
    flex-wrap: wrap;
  }
  .plfoot button {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    font: 700 9.5px var(--mono);
    letter-spacing: 0.06em;
    padding: 3px 7px;
    color: #c9c9d6;
    background: linear-gradient(180deg, #4a4a60, #2a2a38);
    border: 1px solid #000;
    border-radius: 3px;
  }
  .plfoot button:disabled {
    opacity: 0.45;
  }
  .sum {
    font-size: 10.5px;
    color: var(--lcd-on);
    background: #000;
    border: 1px solid #000;
    border-radius: 3px;
    padding: 2px 7px;
  }
  .credit {
    text-align: right;
    font-size: 10px;
    color: var(--text-4);
  }

  /* engine missing */
  .missing {
    align-self: center;
    max-width: 460px;
    text-align: center;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    color: var(--text-2);
  }
  .missing h2 {
    margin: 4px 0 0;
    font-size: 16px;
    color: var(--text);
  }
  .missing p {
    margin: 0;
  }
  .missing .hint {
    font-size: 12.5px;
    color: var(--text-3);
  }

  @media (max-width: 560px) {
    .display {
      grid-template-columns: 1fr;
    }
  }
</style>
