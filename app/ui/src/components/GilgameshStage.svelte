<script lang="ts">
  // Gilgamesh's own window: only his picture, as big as the window, on black (made for a second
  // monitor, full screen). The main window does the talking and listening and sends what moves
  // him (lib/gilgamesh/stage.svelte.ts). Double-click or F: full screen; Esc: leave it.
  import { onMount } from 'svelte';
  import { emit, listen } from '@tauri-apps/api/event';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { Head } from '../lib/gilgamesh/head';
  import { avatarImage, type Avatar } from '../lib/gilgamesh/face';
  import { STAGE_EVENT, type StageMsg } from '../lib/gilgamesh/stage.svelte';
  import { Maximize, Minimize, X, ScanFace } from '@lucide/svelte';

  const win = getCurrentWindow();
  const FS_KEY = 'gilgamesh-stage-fullscreen';

  let avatar = $state<Avatar | null>(null);
  let canvas = $state<HTMLCanvasElement | null>(null);
  let full = $state(false);
  let error = $state<string | null>(null);
  let head: Head | null = null;
  /** The speech clock: the main window's audio time, carried on between its updates. */
  let clock = { base: 0, at: 0 };
  /** What came before the picture was ready (applied once it is). */
  let pending: { mood?: any; attention?: any } = {};

  function apply(m: StageMsg) {
    if (m.t === 'face') {
      avatar = m.avatar as Avatar;
      return;
    }
    if (!head) {
      if (m.t === 'mood') pending.mood = m.v;
      if (m.t === 'attention') pending.attention = m.v;
      return;
    }
    switch (m.t) {
      case 'mood': head.mood = m.v; break;
      case 'attention': head.attention = m.v; break;
      case 'speech':
        if (!m.track) {
          head.speech = null;
          break;
        }
        clock = { base: m.time ?? 0, at: performance.now() };
        head.speech = {
          track: { fps: m.track.fps, jaw: Float32Array.from(m.track.jaw), wide: Float32Array.from(m.track.wide), round: Float32Array.from(m.track.round), energy: Float32Array.from(m.track.energy) },
          time: () => clock.base + (performance.now() - clock.at) / 1000,
        };
        break;
      case 'sync': clock = { base: m.time, at: performance.now() }; break;
      case 'blink': head.blink(); break;
      case 'gesture': head.gesture(m.v); break;
      case 'ack': head.acknowledge(); break;
      case 'see': head.see(m.x, m.y); break;
    }
  }

  // The picture lives as long as the canvas and the face.
  $effect(() => {
    const a = avatar;
    const c = canvas;
    if (!a || !c) return;
    let h: Head | null = null;
    let dead = false;
    error = null;
    avatarImage(a)
      .then((img) => {
        if (dead) return;
        h = new Head(c, a, img);
        if (pending.mood) h.mood = pending.mood;
        if (pending.attention) h.attention = pending.attention;
        pending = {};
        head = h;
      })
      .catch((e) => (error = e?.message ?? String(e)));
    return () => {
      dead = true;
      if (head === h) head = null;
      h?.destroy();
    };
  });

  async function setFull(on: boolean) {
    await win.setFullscreen(on);
    full = on;
    try {
      localStorage.setItem(FS_KEY, on ? '1' : '');
    } catch {
      /* not kept: fine */
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape' && full) void setFull(false);
    else if (e.key.toLowerCase() === 'f' && !e.metaKey && !e.ctrlKey) void setFull(!full);
  }

  onMount(() => {
    document.title = 'Gilgamesh';
    const un: (() => void)[] = [];
    listen<StageMsg>(STAGE_EVENT, (e) => apply(e.payload)).then((u) => {
      un.push(u);
      // Ready: the main window sends his face and how he is now.
      void emit(`${STAGE_EVENT}-ready`);
    });
    // Back to full screen if that's how it was left (the window reopens where it was).
    win.isFullscreen().then(async (f) => {
      let want = false;
      try {
        want = localStorage.getItem(FS_KEY) === '1';
      } catch {
        /* not kept */
      }
      if (want && !f) await setFull(true);
      else full = f;
    });
    return () => un.forEach((u) => u());
  });
</script>

<svelte:window onkeydown={onKey} />

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="stage" class:full ondblclick={() => setFull(!full)}>
  {#if avatar}
    <div class="frame" style="aspect-ratio:{avatar.w} / {avatar.h}; width:min(100vw, calc(100vh * {avatar.w} / {avatar.h}))">
      {#key avatar}<canvas bind:this={canvas}></canvas>{/key}
    </div>
  {:else}
    <div class="wait"><ScanFace size={28} /><span>Waiting for Gilgamesh…</span></div>
  {/if}
  {#if error}<div class="err">{error}</div>{/if}
  <div class="tools">
    <button title={full ? 'Leave full screen (Esc)' : 'Full screen (F or double-click)'} onclick={() => setFull(!full)}>
      {#if full}<Minimize size={16} />{:else}<Maximize size={16} />{/if}
    </button>
    <button title="Close: Gilgamesh goes back to the main window" onclick={() => win.close()}><X size={16} /></button>
  </div>
</div>

<style>
  :global(html),
  :global(body) {
    background: #000 !important;
  }
  .stage {
    position: fixed;
    inset: 0;
    display: grid;
    place-items: center;
    background: #000;
    overflow: hidden;
    cursor: default;
  }
  .stage.full {
    cursor: none;
  }
  canvas {
    width: 100%;
    height: 100%;
    display: block;
  }
  .wait {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    color: #777;
    font-size: 14px;
  }
  .err {
    position: absolute;
    bottom: 16px;
    left: 50%;
    transform: translateX(-50%);
    color: #e5534b;
    font-size: 13px;
  }
  .tools {
    position: absolute;
    top: 14px;
    right: 14px;
    display: flex;
    gap: 6px;
    opacity: 0;
    transition: opacity 200ms;
  }
  .stage:hover .tools {
    opacity: 1;
  }
  .stage.full:hover .tools {
    opacity: 0;
  }
  .stage.full .tools:hover {
    opacity: 1;
  }
  .tools button {
    width: 32px;
    height: 32px;
    display: grid;
    place-items: center;
    border-radius: 8px;
    color: #ddd;
    background: rgba(255, 255, 255, 0.12);
    cursor: pointer;
  }
  .tools button:hover {
    background: rgba(255, 255, 255, 0.22);
  }
</style>
