<script lang="ts">
  // Gilgamesh in the sidebar: his face, what he's saying or hearing, and the microphone, so you
  // can talk with him from any view. His full view is a click on the face.
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { store } from '../lib/state.svelte';
  import { gilgamesh } from '../lib/gilgamesh/gilgamesh.svelte';
  import { transcripts, clock } from '../lib/gilgamesh/transcripts.svelte';
  import { avatarImage } from '../lib/gilgamesh/face';
  import { Head } from '../lib/gilgamesh/head';
  import { sight } from '../lib/gilgamesh/sight.svelte';
  import { stage } from '../lib/gilgamesh/stage.svelte';
  import { Mic, MicOff, Eye, EyeOff, Square, Loader, ScanFace, SendHorizontal, Camera, CameraOff, MonitorUp } from '@lucide/svelte';

  const g = gilgamesh;
  let canvas = $state<HTMLCanvasElement | null>(null);
  let ready = $state<boolean | null>(null);
  let typed = $state('');

  const ptt = $derived(g.settings.mode === 'push_to_talk');
  const on = $derived(g.phase !== 'off');
  const label = $derived.by(() => {
    switch (g.phase) {
      case 'off': return 'Gilgamesh';
      case 'starting': return 'Starting…';
      case 'listening': return ptt ? 'Hold to talk' : 'Listening';
      case 'hearing': return 'Hearing you…';
      case 'thinking': return g.status || 'Thinking…';
      case 'speaking': return 'Speaking';
    }
  });
  const caption = $derived(g.heard || g.saying || g.error || '');

  onMount(async () => {
    const [tts, stt] = await Promise.all([invoke<any>('tts_status').catch(() => null), invoke<any>('stt_status').catch(() => null)]);
    ready = !!tts?.installed && !!tts?.engine && !!stt?.installed;
  });

  // His face lives as long as the canvas and the avatar.
  $effect(() => {
    const a = g.avatar;
    const c = canvas;
    if (!a || !c) return;
    let head: Head | null = null;
    let off = () => {};
    let dead = false;
    avatarImage(a)
      .then((img) => {
        if (dead) return;
        head = new Head(c, a, img);
        off = g.head.add(head);
      })
      .catch(() => {});
    return () => {
      dead = true;
      off();
      head?.destroy();
    };
  });

  function toggleMic() {
    // The voice and speech recognition are downloaded in his view.
    if (!ready) return store.navigate({ kind: 'gilgamesh' });
    if (on) g.stop();
    else g.start();
  }

  function send(e: Event) {
    e.preventDefault();
    const t = typed.trim();
    if (!t) return;
    typed = '';
    g.ask(t);
  }
</script>

<div class="dock" data-phase={g.phase}>
  {#if g.settings.dock}
    {#if g.avatar}
      <button class="pic" title="Open Gilgamesh" style="--lvl:{on ? Math.min(1, g.level * 12) : 0}" onclick={() => store.navigate({ kind: 'gilgamesh' })}>
        {#key g.avatar}<canvas bind:this={canvas}></canvas>{/key}
      </button>
    {:else if g.loaded}
      <button class="noface" onclick={() => store.navigate({ kind: 'gilgamesh' })}><ScanFace size={16} /> Give Gilgamesh a face</button>
    {/if}
  {/if}
  {#if transcripts.recording}
    <!-- He's writing down what he hears: the microphone is his, not the conversation's. -->
    <div class="bar rec">
      <span class="dot" class:paused={transcripts.paused}></span>
      <button class="lbl truncate link" title="Open the transcript" onclick={() => store.navigate({ kind: 'gilgamesh' })}>{transcripts.paused ? 'Transcript paused' : 'Transcribing'} · {clock(transcripts.elapsed)}</button>
      <button class="icon-btn sm" title="Stop and save the transcript" onclick={() => transcripts.stop()}><Square size={12} /></button>
    </div>
  {/if}
  {#if caption}
    <p class="cap" class:you={!!g.heard} class:err={!g.heard && !g.saying}>{caption}</p>
  {/if}
  <div class="bar">
    <span class="dot"></span>
    <span class="lbl truncate" title={label}>{label}</span>
    {#if g.phase === 'speaking' || g.phase === 'thinking'}
      <button class="icon-btn sm" title="Stop him" onclick={() => g.interrupt()}><Square size={12} /></button>
    {/if}
    {#if on && ptt}
      <button class="hold" class:held={g.holding} title="Hold while you talk" onpointerdown={() => g.press(true)} onpointerup={() => g.press(false)} onpointerleave={() => g.press(false)}>Talk</button>
    {/if}
    {#if sight.on}
      <!-- His camera is on: he can see. Click: sight off. -->
      <button class="icon-btn sm seeing" title="Gilgamesh can see{sight.present ? (sight.who === 'owner' ? ' you' : sight.who === 'stranger' ? ' someone he doesn’t know' : ' someone') : ' (nobody there)'} — turn his sight off" onclick={() => sight.saveSettings({ mode: 'off' })}><Camera size={14} /></button>
    {:else if sight.error && sight.settings.mode !== 'off'}
      <button class="icon-btn sm" title={sight.error} onclick={() => store.navigate({ kind: 'gilgamesh' })}><CameraOff size={14} /></button>
    {/if}
    {#if g.avatar}
      <button class="icon-btn sm" class:seeing={stage.open} title={stage.open ? 'Close his own window' : 'Open him in a window of his own (e.g. on another monitor)'} onclick={() => stage.toggle().catch((e) => store.error(e, 'Couldn’t open his window'))}><MonitorUp size={14} /></button>
    {/if}
    <button class="icon-btn sm" title={g.settings.dock ? 'Hide his face' : 'Show his face'} onclick={() => g.saveSettings({ dock: !g.settings.dock })}>
      {#if g.settings.dock}<EyeOff size={14} />{:else}<Eye size={14} />{/if}
    </button>
    <button class="mic" class:on disabled={g.phase === 'starting'} title={ready === false ? 'Set up his voice first' : on ? 'Microphone off' : 'Talk with Gilgamesh'} onclick={toggleMic}>
      {#if g.phase === 'starting'}<Loader size={14} class="spin" />{:else if on}<MicOff size={14} />{:else}<Mic size={14} />{/if}
    </button>
  </div>
  <form class="type" onsubmit={send}>
    <input class="input" placeholder="Ask Gilgamesh…" bind:value={typed} disabled={ready === false} />
    {#if typed.trim()}<button class="icon-btn sm" title="Send"><SendHorizontal size={13} /></button>{/if}
  </form>
</div>

<style>
  .seeing {
    color: var(--accent);
  }
  .dock {
    margin: 0 10px 8px;
    padding: 8px;
    border-radius: var(--r-lg);
    background: var(--surface);
    border: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .pic {
    position: relative;
    height: 150px;
    padding: 0;
    border: none;
    border-radius: var(--r);
    overflow: hidden;
    background: var(--surface-3);
    cursor: pointer;
    box-shadow: 0 0 0 calc(var(--lvl) * 4px) color-mix(in oklab, var(--success) 35%, transparent);
    transition: box-shadow 90ms linear;
  }
  /* The picture is 4:5; the face is a little above its middle. */
  canvas {
    position: absolute;
    left: 0;
    top: 50%;
    width: 100%;
    aspect-ratio: 720 / 900;
    transform: translateY(-46%);
    display: block;
    animation: fade-in 450ms var(--ease) both;
  }
  @keyframes fade-in {
    from {
      opacity: 0;
    }
  }
  .noface {
    display: flex;
    gap: 8px;
    align-items: center;
    justify-content: center;
    padding: 10px;
    border: 1px dashed var(--border-strong);
    border-radius: var(--r);
    background: none;
    font: inherit;
    font-size: 12.5px;
    color: var(--text-2);
    cursor: pointer;
  }
  .cap {
    margin: 0;
    font-size: 12px;
    line-height: 1.4;
    color: var(--text);
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .cap.you {
    color: var(--text-2);
    font-style: italic;
  }
  .cap.err {
    color: var(--danger);
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--text-4);
    flex: none;
    margin: 0 2px;
  }
  .dock[data-phase='listening'] .dot,
  .dock[data-phase='hearing'] .dot {
    background: var(--success);
  }
  .dock[data-phase='thinking'] .dot {
    background: var(--warning);
  }
  .dock[data-phase='speaking'] .dot {
    background: var(--accent);
  }
  .dock[data-phase='hearing'] .dot,
  .dock[data-phase='thinking'] .dot {
    animation: pulse 1s infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
  .lbl {
    flex: 1;
    min-width: 0;
    font-size: 12.5px;
    font-weight: 600;
    color: var(--text-2);
  }
  .rec .dot {
    background: var(--danger);
    animation: rec 1s ease-in-out infinite;
  }
  .rec .dot.paused {
    animation: none;
    background: var(--text-4);
  }
  @keyframes rec {
    50% {
      opacity: 0.3;
    }
  }
  .link {
    border: none;
    background: none;
    padding: 0;
    text-align: left;
    font: inherit;
    font-size: 12.5px;
    font-weight: 600;
    color: var(--text-2);
    cursor: pointer;
  }
  .mic {
    width: 28px;
    height: 28px;
    flex: none;
    border-radius: 50%;
    border: none;
    display: grid;
    place-items: center;
    background: var(--accent);
    color: var(--on-accent);
    cursor: pointer;
  }
  .mic.on {
    background: var(--danger);
  }
  .mic:disabled {
    opacity: 0.5;
    cursor: default;
  }
  .hold {
    border: none;
    border-radius: 99px;
    padding: 4px 10px;
    font: inherit;
    font-size: 12px;
    background: var(--surface-3);
    color: var(--text);
    cursor: pointer;
    user-select: none;
  }
  .hold.held {
    background: var(--success);
    color: var(--on-accent);
  }
  .type {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .type .input {
    flex: 1;
    min-width: 0;
    height: 28px;
    font-size: 12.5px;
  }
</style>
