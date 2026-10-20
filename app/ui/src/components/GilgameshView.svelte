<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { store } from '../lib/state.svelte';
  import { formatBytes } from '../lib/util';
  import { gilgamesh } from '../lib/gilgamesh/gilgamesh.svelte';
  import { prepareAvatar, avatarImage } from '../lib/gilgamesh/face';
  import { Head } from '../lib/gilgamesh/head';
  import { Mic, MicOff, ImageUp, SlidersHorizontal, RotateCcw, Loader, Download, Headphones, SendHorizontal, MessageSquareText, Hand, Square, X, CircleAlert, ScanFace } from '@lucide/svelte';

  interface TtsStatus { installed: boolean; engine: boolean; dev_build: boolean; bytes: number; downloading: { done: number; total: number } | null; voices: { key: string; name: string }[] }
  interface SttStatus { installed: boolean; bytes: number; downloading: { done: number; total: number } | null }

  let tts = $state<TtsStatus | null>(null);
  let stt = $state<SttStatus | null>(null);
  let ttsProgress = $state<{ done: number; total: number } | null>(null);
  let sttProgress = $state<{ done: number; total: number } | null>(null);
  let canvas = $state<HTMLCanvasElement | null>(null);
  let fileInput = $state<HTMLInputElement | null>(null);
  let preparing = $state(false);
  let warnings = $state<string[]>([]);
  let showSettings = $state(false);
  let showTranscript = $state(true);
  let typed = $state('');
  let headError = $state<string | null>(null);
  let transcriptEl = $state<HTMLDivElement | null>(null);

  const g = gilgamesh;
  const ready = $derived(!!tts?.installed && !!tts?.engine && !!stt?.installed);
  const ptt = $derived(g.settings.mode === 'push_to_talk');
  const pct = (p: { done: number; total: number } | null) => (p && p.total ? Math.floor((p.done / p.total) * 100) : 0);

  const phaseLabel = $derived.by(() => {
    switch (g.phase) {
      case 'off': return 'Not listening';
      case 'starting': return 'Starting…';
      case 'listening': return ptt ? (g.holding ? 'Listening…' : 'Hold Space to talk') : 'Listening';
      case 'hearing': return 'Hearing you…';
      case 'thinking': return 'Thinking…';
      case 'speaking': return g.settings.headphones || ptt ? 'Speaking — talk to interrupt' : 'Speaking';
    }
  });

  async function refresh() {
    [tts, stt] = await Promise.all([invoke<TtsStatus>('tts_status').catch(() => null), invoke<SttStatus>('stt_status').catch(() => null)]);
    if (tts?.downloading) ttsProgress = tts.downloading;
    if (stt?.downloading) sttProgress = stt.downloading;
  }

  onMount(() => {
    g.load().catch((e) => store.error(e, 'Couldn’t load Gilgamesh'));
    refresh();
    const un: Array<() => void> = [];
    listen<any>('tts-progress', (e) => !e.payload.done && (ttsProgress = e.payload)).then((u) => un.push(u));
    listen<any>('stt-progress', (e) => !e.payload.done && (sttProgress = e.payload)).then((u) => un.push(u));
    return () => {
      un.forEach((u) => u());
      // Leaving the view ends the conversation: the microphone is never left on unseen.
      g.stop();
    };
  });

  // The talking head lives as long as the canvas and the avatar.
  $effect(() => {
    const a = g.avatar;
    const c = canvas;
    if (!a || !c) return;
    let head: Head | null = null;
    let dead = false;
    headError = null;
    avatarImage(a)
      .then((img) => {
        if (dead) return;
        head = new Head(c, a, img);
        g.head = head;
        if (g.phase !== 'off') head.attention = 'listening';
      })
      .catch((e) => (headError = e?.message ?? String(e)));
    return () => {
      dead = true;
      head?.destroy();
      if (g.head === head) g.head = null;
    };
  });

  $effect(() => {
    void g.turns.length;
    void g.turns.at(-1)?.text;
    transcriptEl?.scrollTo({ top: transcriptEl.scrollHeight });
  });

  async function downloadTts() {
    ttsProgress = { done: 0, total: tts?.bytes ?? 0 };
    try {
      await invoke('tts_download', { cloning: false });
    } catch (e) {
      store.error(e, 'Download failed');
    } finally {
      ttsProgress = null;
      refresh();
    }
  }

  async function downloadStt() {
    sttProgress = { done: 0, total: stt?.bytes ?? 0 };
    try {
      await invoke('stt_download');
    } catch (e) {
      store.error(e, 'Download failed');
    } finally {
      sttProgress = null;
      refresh();
    }
  }

  async function pickPhoto(e: Event) {
    const f = (e.target as HTMLInputElement).files?.[0];
    (e.target as HTMLInputElement).value = '';
    if (!f) return;
    preparing = true;
    warnings = [];
    try {
      const r = await prepareAvatar(f);
      await g.setAvatar(r.avatar);
      warnings = r.warnings;
    } catch (err) {
      store.error(err, 'Couldn’t use that photo');
    } finally {
      preparing = false;
    }
  }

  async function removePhoto() {
    g.stop();
    await g.setAvatar(null).catch((e) => store.error(e));
    showSettings = false;
  }

  function toggle() {
    if (g.phase === 'off') g.start();
    else g.stop();
  }

  function sendTyped(e: Event) {
    e.preventDefault();
    const t = typed.trim();
    if (!t) return;
    typed = '';
    g.ask(t);
  }

  const typing = (t: EventTarget | null) => t instanceof HTMLElement && (t.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(t.tagName));

  function onKey(e: KeyboardEvent) {
    if (typing(e.target)) return;
    if (e.code === 'Space' && ptt && g.phase !== 'off') {
      e.preventDefault();
      if (!e.repeat) g.press(true);
    } else if (e.key === 'Escape' && (g.phase === 'speaking' || g.phase === 'thinking')) {
      g.interrupt();
    }
  }
  function onKeyUp(e: KeyboardEvent) {
    if (e.code === 'Space' && ptt) g.press(false);
  }
</script>

<svelte:window onkeydown={onKey} onkeyup={onKeyUp} onblur={() => g.press(false)} />

<div class="gil">
  <header class="drag">
    <h1>Gilgamesh</h1>
    {#if g.avatar}<span class="phase" data-phase={g.phase}><i></i>{phaseLabel}</span>{/if}
    <span class="spacer"></span>
    {#if g.avatar}
      <button class="btn sm ghost no-drag" title="Start a new conversation" onclick={() => g.newConversation()}><RotateCcw size={14} /> New conversation</button>
      <button class="icon-btn no-drag" class:active={showTranscript} title="Transcript" onclick={() => (showTranscript = !showTranscript)}><MessageSquareText size={17} /></button>
      <button class="icon-btn no-drag" class:active={showSettings} title="Settings" onclick={() => (showSettings = !showSettings)}><SlidersHorizontal size={17} /></button>
    {/if}
  </header>

  <input bind:this={fileInput} type="file" accept="image/png,image/jpeg,image/webp" hidden onchange={pickPhoto} />

  {#if !g.loaded}
    <div class="center"><Loader size={20} class="spin" /></div>
  {:else if !g.avatar}
    <div class="center">
      <div class="setup">
        <div class="glyph"><ScanFace size={30} /></div>
        <h2>Give Gilgamesh a face</h2>
        <p>Choose a photo of a face: a clear, well-lit portrait looking at the camera, with the mouth closed, works best. Gilgamesh will talk with it — lips, eyes and expressions move as he speaks.</p>
        <button class="btn primary lg" disabled={preparing} onclick={() => fileInput?.click()}>
          {#if preparing}<Loader size={16} class="spin" /> Finding the face…{:else}<ImageUp size={16} /> Choose a photo{/if}
        </button>
        <p class="fine">Use a photo of yourself, or of someone who has agreed to it. The photo is processed on this computer and kept only in your encrypted vault.</p>
      </div>
    </div>
  {:else}
    <div class="main">
      <section class="stage">
        <div class="frame" style="aspect-ratio:{g.avatar.w} / {g.avatar.h}; --lvl:{Math.min(1, g.level * 12)}" data-phase={g.phase}>
          <canvas bind:this={canvas}></canvas>
          {#if headError}<div class="head-error"><CircleAlert size={16} /> {headError}</div>{/if}
        </div>

        <div class="captions">
          {#if g.heard}
            <p class="you">{g.heard}</p>
          {:else if g.saying}
            <p class="him">{g.saying}</p>
          {:else if g.error}
            <p class="err"><CircleAlert size={14} /> {g.error}</p>
          {/if}
        </div>

        {#if warnings.length}
          <div class="note">
            {#each warnings as w}<span>{w}</span>{/each}
            <button class="icon-btn sm" title="Dismiss" onclick={() => (warnings = [])}><X size={13} /></button>
          </div>
        {/if}

        {#if !ready}
          <div class="needs">
            {#if tts && !tts.engine}
              <div class="need"><CircleAlert size={15} /> <span class="spacer">{tts.dev_build ? 'The speech engine isn’t built yet. Run ./scripts/build-tts.sh.' : 'This copy of RAD was built without the speech engine.'}</span></div>
            {:else if tts && !tts.installed}
              <div class="need">
                <span class="spacer"><strong>Gilgamesh’s voice</strong> · MioTTS, {formatBytes(tts.bytes)}</span>
                {#if ttsProgress}<span class="bar"><i style="width:{pct(ttsProgress)}%"></i></span><span class="pct">{pct(ttsProgress)}%</span>
                {:else}<button class="btn sm primary" onclick={downloadTts}><Download size={14} /> Download</button>{/if}
              </div>
            {/if}
            {#if stt && !stt.installed}
              <div class="need">
                <span class="spacer"><strong>Speech recognition</strong> · Parakeet, 25 languages, {formatBytes(stt.bytes)}</span>
                {#if sttProgress}<span class="bar"><i style="width:{pct(sttProgress)}%"></i></span><span class="pct">{pct(sttProgress)}%</span>
                {:else}<button class="btn sm primary" onclick={downloadStt}><Download size={14} /> Download</button>{/if}
              </div>
            {/if}
            <p class="fine">Both run on this computer; downloaded once.</p>
          </div>
        {/if}

        <div class="controls">
          <button class="talk" class:on={g.phase !== 'off'} class:held={g.holding} disabled={!ready || g.phase === 'starting'} onclick={toggle} title={g.phase === 'off' ? 'Start talking with Gilgamesh' : 'End the conversation (microphone off)'}>
            {#if g.phase === 'starting'}<Loader size={22} class="spin" />{:else if g.phase === 'off'}<Mic size={22} />{:else}<MicOff size={22} />{/if}
          </button>
          {#if g.phase === 'speaking' || g.phase === 'thinking'}
            <button class="btn sm ghost" title="Stop (Esc)" onclick={() => g.interrupt()}><Square size={13} /> Stop</button>
          {/if}
        </div>
        <p class="hint">
          {#if g.phase === 'off'}{ready ? 'Press the microphone to start talking.' : 'Download the models above to start.'}
          {:else if ptt}Hold <span class="kbd">Space</span> while you talk.
          {:else if g.settings.headphones}Just talk — you can interrupt him at any time.
          {:else}Just talk. He doesn’t listen while he speaks; press <span class="kbd">Esc</span> to stop him.{/if}
        </p>

        <form class="type" onsubmit={sendTyped}>
          <input class="input" placeholder="Or type to Gilgamesh…" bind:value={typed} disabled={!tts?.installed} />
          <button class="icon-btn" title="Send" disabled={!typed.trim()}><SendHorizontal size={16} /></button>
        </form>
      </section>

      {#if showTranscript}
        <aside class="transcript">
          <div class="th">Conversation</div>
          <div class="turns selectable" bind:this={transcriptEl}>
            {#each g.turns as t}
              <div class="turn {t.who}"><span class="who">{t.who === 'you' ? 'You' : 'Gilgamesh'}</span><p>{t.text}</p></div>
            {:else}
              <p class="empty">What you say and his answers appear here. The conversation is also saved as the “Gilgamesh” chat, so he remembers it.</p>
            {/each}
          </div>
        </aside>
      {/if}

      {#if showSettings}
        <div class="settings">
          <div class="sh"><strong>Gilgamesh</strong><button class="icon-btn sm" title="Close" onclick={() => (showSettings = false)}><X size={14} /></button></div>
          <span class="label">Talking</span>
          <div class="seg">
            <button class:on={!ptt} onclick={() => (g.stop(), g.saveSettings({ mode: 'hands_free' }))}><Mic size={14} /> Hands-free</button>
            <button class:on={ptt} onclick={() => (g.stop(), g.saveSettings({ mode: 'push_to_talk' }))}><Hand size={14} /> Push to talk</button>
          </div>
          <label class="check">
            <input type="checkbox" checked={g.settings.headphones} onchange={(e) => g.saveSettings({ headphones: (e.target as HTMLInputElement).checked })} />
            <span><Headphones size={14} /> I’m using headphones <small>The microphone stays on while he speaks, so you can interrupt him by talking.</small></span>
          </label>
          <span class="label">Voice</span>
          <select class="select" value={g.settings.voice || 'en_male'} onchange={(e) => g.saveSettings({ voice: (e.target as HTMLSelectElement).value })}>
            {#each tts?.voices ?? [] as v}<option value={v.key}>{v.name}</option>{/each}
          </select>
          <small class="fine">Speed and volume are shared with read-aloud (Settings → Voice), where you can also make a voice from a recording.</small>
          <span class="label">Face</span>
          <div class="row">
            <button class="btn sm" onclick={() => fileInput?.click()} disabled={preparing}>{#if preparing}<Loader size={14} class="spin" />{:else}<ImageUp size={14} />{/if} Change photo</button>
            <button class="btn sm ghost danger" onclick={removePhoto}>Remove</button>
          </div>
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .gil {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    position: relative;
  }
  header {
    flex: none;
    display: flex;
    align-items: center;
    gap: 10px;
    height: var(--titlebar-h);
    padding: 0 12px 0 calc(14px + var(--lead));
    border-bottom: 1px solid var(--border);
  }
  h1 {
    margin: 0;
    font-size: 15px;
    font-weight: 650;
  }
  .phase {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    font-size: 12px;
    color: var(--text-2);
    padding: 3px 10px;
    border-radius: 99px;
    background: var(--surface-2);
  }
  .phase i {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--text-4);
  }
  .phase[data-phase='listening'] i,
  .phase[data-phase='hearing'] i {
    background: var(--success);
  }
  .phase[data-phase='hearing'] i,
  .phase[data-phase='thinking'] i {
    animation: pulse 1s infinite;
  }
  .phase[data-phase='thinking'] i {
    background: var(--warning);
  }
  .phase[data-phase='speaking'] i {
    background: var(--accent);
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
  .center {
    flex: 1;
    display: grid;
    place-items: center;
    padding: 24px;
  }
  .setup {
    max-width: 440px;
    text-align: center;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
  }
  .setup h2 {
    margin: 4px 0 0;
    font-size: 19px;
  }
  .setup p {
    margin: 0;
    color: var(--text-2);
    line-height: 1.5;
  }
  .glyph {
    width: 62px;
    height: 62px;
    border-radius: 18px;
    display: grid;
    place-items: center;
    color: var(--accent);
    background: var(--accent-soft);
  }
  .fine {
    font-size: 12px;
    color: var(--text-3);
    line-height: 1.45;
  }
  .main {
    flex: 1;
    min-height: 0;
    display: flex;
    position: relative;
  }
  .stage {
    flex: 1;
    min-width: 0;
    overflow: auto;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    padding: 22px 20px 18px;
  }
  .frame {
    position: relative;
    height: min(58vh, 620px);
    max-width: 100%;
    border-radius: var(--r-xl);
    overflow: hidden;
    background: var(--surface-3);
    flex: none;
    box-shadow: var(--shadow-lg), 0 0 0 calc(var(--lvl) * 6px) color-mix(in oklab, var(--success) 35%, transparent);
    transition: box-shadow 90ms linear;
  }
  .frame[data-phase='off'],
  .frame[data-phase='speaking'],
  .frame[data-phase='thinking'] {
    box-shadow: var(--shadow-lg);
  }
  canvas {
    width: 100%;
    height: 100%;
    display: block;
  }
  .head-error {
    position: absolute;
    inset: auto 12px 12px;
    display: flex;
    gap: 8px;
    align-items: center;
    padding: 8px 10px;
    border-radius: var(--r-sm);
    background: var(--glass);
    color: var(--danger);
    font-size: 13px;
  }
  .captions {
    min-height: 46px;
    max-width: 620px;
    text-align: center;
  }
  .captions p {
    margin: 0;
    font-size: 15px;
    line-height: 1.45;
  }
  .captions .you {
    color: var(--text-2);
    font-style: italic;
  }
  .captions .err {
    color: var(--danger);
    font-size: 13px;
    display: inline-flex;
    gap: 6px;
    align-items: center;
  }
  .note {
    display: flex;
    align-items: center;
    gap: 10px;
    max-width: 560px;
    padding: 8px 10px 8px 14px;
    border-radius: var(--r);
    background: var(--warning-soft);
    font-size: 12.5px;
    color: var(--text-2);
  }
  .note span {
    flex: 1;
  }
  .needs {
    width: min(520px, 100%);
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .need {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--surface);
    font-size: 13px;
  }
  .need strong {
    font-weight: 600;
  }
  .bar {
    width: 120px;
    height: 6px;
    border-radius: 3px;
    background: var(--surface-3);
    overflow: hidden;
  }
  .bar i {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width var(--t);
  }
  .pct {
    width: 36px;
    text-align: right;
    font-size: 12px;
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
  }
  .controls {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .talk {
    width: 58px;
    height: 58px;
    border-radius: 50%;
    border: none;
    display: grid;
    place-items: center;
    background: var(--accent);
    color: var(--on-accent);
    cursor: pointer;
    box-shadow: var(--shadow);
    transition: transform var(--t-fast) var(--ease), background var(--t);
  }
  .talk:hover:not(:disabled) {
    transform: scale(1.05);
  }
  .talk.on {
    background: var(--danger);
  }
  .talk.held {
    background: var(--success);
    transform: scale(1.08);
  }
  .talk:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .hint {
    margin: 0;
    font-size: 12.5px;
    color: var(--text-3);
  }
  .type {
    width: min(520px, 100%);
    display: flex;
    gap: 6px;
    margin-top: 4px;
  }
  .type .input {
    flex: 1;
  }
  .transcript {
    width: 320px;
    flex: none;
    border-left: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .th {
    padding: 12px 14px 8px;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-3);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .turns {
    flex: 1;
    overflow: auto;
    padding: 0 14px 14px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .turn .who {
    font-size: 11.5px;
    font-weight: 600;
    color: var(--text-3);
  }
  .turn.gilgamesh .who {
    color: var(--accent);
  }
  .turn p {
    margin: 2px 0 0;
    font-size: 13.5px;
    line-height: 1.5;
    white-space: pre-wrap;
  }
  .empty {
    font-size: 13px;
    color: var(--text-3);
    line-height: 1.5;
  }
  .settings {
    position: absolute;
    top: 10px;
    right: 12px;
    width: 300px;
    padding: 12px 14px 14px;
    border-radius: var(--r-lg);
    background: var(--surface);
    border: 1px solid var(--border);
    box-shadow: var(--shadow-lg);
    display: flex;
    flex-direction: column;
    gap: 8px;
    z-index: 5;
  }
  .sh {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  .label {
    margin-top: 6px;
    font-size: 12px;
    font-weight: 600;
    color: var(--text-3);
  }
  .seg {
    display: flex;
    padding: 3px;
    gap: 3px;
    border-radius: var(--r-sm);
    background: var(--surface-2);
  }
  .seg button {
    flex: 1;
    display: inline-flex;
    gap: 6px;
    align-items: center;
    justify-content: center;
    border: none;
    background: none;
    padding: 6px;
    border-radius: var(--r-xs);
    font: inherit;
    font-size: 12.5px;
    color: var(--text-2);
    cursor: pointer;
  }
  .seg button.on {
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow-sm);
  }
  .check {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    font-size: 13px;
    cursor: pointer;
  }
  .check span {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .check small {
    color: var(--text-3);
    font-size: 12px;
    line-height: 1.4;
  }
  .row {
    display: flex;
    gap: 8px;
  }
  :global(.spin) {
    animation: spin 1s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
