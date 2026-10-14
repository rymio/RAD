<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { revealItemInDir } from '@tauri-apps/plugin-opener';
  import { store } from '../lib/state.svelte';
  import { speech } from '../lib/speech.svelte';
  import { formatBytes } from '../lib/util';
  import type { ClientConfig } from '../lib/types';
  import { Download, Trash2, Play, Square, Loader, Mic, FolderOpen, RotateCcw, CircleAlert } from '@lucide/svelte';

  interface Status {
    installed: boolean; hd: boolean; hd_bytes: number; cloning_installed: boolean; engine: boolean; dev_build: boolean; bytes: number; cloning_bytes: number; dir: string;
    downloading: { file?: string; done: number; total: number } | null;
    voices: { key: string; name: string; custom: boolean }[];
  }

  let st = $state<Status | null>(null);
  let progress = $state<{ done: number; total: number } | null>(null);
  let sample = $state('Hello! This is how RAD sounds when it reads your answers aloud.');
  let voiceName = $state('');
  let cloning = $state(false);

  const cfg = $derived(store.config);
  const pct = $derived(progress && progress.total ? Math.floor((progress.done / progress.total) * 100) : 0);

  async function refresh() {
    st = await invoke<Status>('tts_status').catch(() => null);
    if (st?.downloading) progress = st.downloading;
  }

  async function download(clone = false) {
    const upgrade = !clone && !!st?.installed && !st.hd;
    progress = { done: 0, total: clone ? (st?.cloning_bytes ?? 0) : upgrade ? (st?.hd_bytes ?? 0) : (st?.bytes ?? 0) };
    try {
      await invoke('tts_download', { cloning: clone });
      if (!clone && cfg && !cfg.tts_enabled) await set({ tts_enabled: true });
      if (upgrade) store.toast('success', 'Clearer voice installed', 'Press Play to hear the difference.');
      else store.toast('success', clone ? 'Voice cloning is ready' : 'Text to speech is ready', clone ? 'Make a voice from a recording below.' : 'Answers can now be read aloud.');
    } catch (e) {
      store.error(e, 'Download failed');
    } finally {
      progress = null;
      refresh();
    }
  }

  async function remove() {
    speech.stop();
    await invoke('tts_remove').catch((e) => store.error(e));
    await set({ tts_enabled: false, tts_auto_read: false });
    refresh();
  }

  function set(patch: Partial<ClientConfig>) {
    return store.saveConfig(patch);
  }

  async function test() {
    if (speech.speaking === 'test') return speech.stop();
    if (!cfg) return;
    try {
      await speech.speak('test', sample, cfg);
    } catch (e) {
      store.error(e, 'Speech failed');
    }
  }

  async function addVoice() {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const f = await open({ multiple: false, title: 'Choose a recording of the voice (5–20 seconds)', filters: [{ name: 'Audio', extensions: ['wav', 'mp3', 'flac'] }] });
    if (typeof f !== 'string') return;
    const name = voiceName.trim() || f.split(/[\\/]/).pop()!.replace(/\.[^.]+$/, '');
    cloning = true;
    try {
      const r = await invoke<{ key: string }>('tts_add_voice', { name, path: f });
      await refresh();
      await set({ tts_voice: r.key });
      voiceName = '';
      store.toast('success', 'Voice added', 'It’s now the selected voice — try it with “Play”.');
    } catch (e) {
      store.error(e, 'Couldn’t make a voice from that recording');
    } finally {
      cloning = false;
    }
  }

  async function deleteVoice(key: string) {
    await invoke('tts_delete_voice', { key }).catch((e) => store.error(e));
    if (cfg?.tts_voice === key) await set({ tts_voice: 'en_female' });
    refresh();
  }

  function resetVoice() {
    set({ tts_speed: 1, tts_volume: 1, tts_temperature: 0.8, tts_top_p: 1, tts_top_k: 50, tts_repetition_penalty: 1 });
  }

  onMount(() => {
    refresh();
    const un = listen<any>('tts-progress', (e) => {
      if (e.payload.done === true) progress = null;
      else progress = { done: e.payload.done, total: e.payload.total };
    });
    return () => un.then((u) => u());
  });
</script>

<h2>Voice</h2>
<p class="lead">
  RAD can read answers aloud with <strong>MioTTS-0.1B</strong>, a small text-to-speech model that runs on this computer — your text never leaves it.
  It speaks <strong>English and Japanese</strong>.
</p>

{#if st}
  <div class="card prov">
    <div class="ph">
      <div class="pn spacer">
        <div class="st">MioTTS-0.1B <span class="pill">Text to speech</span> {#if st.installed}<span class="pill success">Installed</span>{/if}</div>
        <div class="sd">Speech model (Q8, 125 MB) + MioCodec 44.1 kHz voice decoder (393 MB) + 4 voices · {formatBytes(st.bytes)} · Falcon-LLM licence (model), MIT (codec)</div>
      </div>
      {#if progress}
        <span class="sd">{pct}%</span>
      {:else if !st.installed}
        <button class="btn sm primary" onclick={() => download(false)}><Download size={13} /> Download</button>
      {:else}
        <button class="btn sm" onclick={() => revealItemInDir(st!.dir)}><FolderOpen size={13} /> Folder</button>
        <button class="icon-btn" onclick={remove} aria-label="Delete model"><Trash2 size={15} /></button>
      {/if}
    </div>
    {#if progress}<div class="progress"><div style="width:{pct}%"></div></div>{/if}
    {#if st.installed && !st.hd && !progress}
      <div class="test bad">
        <CircleAlert size={13} />
        <span class="spacer">This install uses the older 24 kHz voice decoder, which sounds breathy. The 44.1 kHz decoder reads the same speech much more clearly ({formatBytes(st.hd_bytes)}; the old one is removed afterwards).</span>
        <button class="btn sm primary" onclick={() => download(false)}><Download size={13} /> Get clearer voice</button>
      </div>
    {/if}
    {#if !st.engine}
      <div class="test bad">
        <CircleAlert size={13} />
        {#if st.dev_build}
          The speech engine isn’t built yet — run <code>./scripts/build-tts.sh</code>, then reopen this page.
        {:else}
          This copy of RAD was built without the speech engine. Rebuild it with <code>./scripts/build-app.sh</code> (it builds the engines and puts them inside the app), then replace the installed RAD.
        {/if}
      </div>
    {/if}
  </div>

  {#if st.installed && cfg}
    <div class="group">
      <div class="srow">
        <div><div class="st">Text to speech</div><div class="sd">Shows a speaker button on every answer.</div></div>
        <button class="switch" class:on={cfg.tts_enabled} onclick={() => set({ tts_enabled: !cfg.tts_enabled })} aria-label="Text to speech"></button>
      </div>
      <div class="srow">
        <div><div class="st">Read answers aloud automatically</div><div class="sd">Every new answer is spoken as soon as it’s complete.</div></div>
        <button class="switch" class:on={cfg.tts_auto_read} disabled={!cfg.tts_enabled} onclick={() => set({ tts_auto_read: !cfg.tts_auto_read })} aria-label="Read answers aloud"></button>
      </div>
      <div class="srow">
        <div><div class="st">Voice</div></div>
        <select class="select" style="width:220px" value={cfg.tts_voice} onchange={(e) => set({ tts_voice: (e.target as HTMLSelectElement).value })}>
          {#each st.voices as v}<option value={v.key}>{v.name}{v.custom ? ' (yours)' : ''}</option>{/each}
        </select>
      </div>
    </div>

    <h3>Voice parameters <button class="btn sm ghost" onclick={resetVoice}><RotateCcw size={12} /> Defaults</button></h3>
    <div class="group">
      {#each [
        { k: 'tts_speed', label: 'Speed', hint: 'Playback speed; pitch is kept.', min: 0.5, max: 2, step: 0.05, fmt: (v: number) => `${v.toFixed(2)}×` },
        { k: 'tts_volume', label: 'Volume', hint: '', min: 0, max: 1, step: 0.05, fmt: (v: number) => `${Math.round(v * 100)}%` },
        { k: 'tts_temperature', label: 'Expressiveness (temperature)', hint: 'Lower is steadier, higher is livelier. 0.8 is the model’s default.', min: 0.1, max: 1.5, step: 0.05, fmt: (v: number) => v.toFixed(2) },
        { k: 'tts_top_p', label: 'Top-p', hint: 'Limits sampling to the most likely sounds.', min: 0.1, max: 1, step: 0.05, fmt: (v: number) => v.toFixed(2) },
        { k: 'tts_top_k', label: 'Top-k', hint: '', min: 1, max: 100, step: 1, fmt: (v: number) => String(v) },
        { k: 'tts_repetition_penalty', label: 'Repetition penalty', hint: 'Raise a little if words stutter or repeat.', min: 1, max: 1.5, step: 0.01, fmt: (v: number) => v.toFixed(2) },
      ] as p (p.k)}
        {@const val = (cfg as any)[p.k] as number}
        <div class="srow">
          <div><div class="st">{p.label}</div>{#if p.hint}<div class="sd">{p.hint}</div>{/if}</div>
          <div class="slider">
            <input type="range" min={p.min} max={p.max} step={p.step} value={val} onchange={(e) => set({ [p.k]: Number((e.target as HTMLInputElement).value) } as Partial<ClientConfig>)} />
            <span class="val">{p.fmt(val)}</span>
          </div>
        </div>
      {/each}
      <div class="srow">
        <input class="input" style="flex:1" bind:value={sample} />
        <button class="btn sm primary" onclick={test} disabled={!sample.trim()}>
          {#if speech.speaking === 'test' && speech.loading}<Loader size={13} class="spin" /> Preparing{:else if speech.speaking === 'test'}<Square size={13} /> Stop{:else}<Play size={13} /> Play{/if}
        </button>
      </div>
    </div>

    <h3>Your own voice</h3>
    <p class="lead">Make a voice from a short, clear recording (5–20 seconds; WAV, MP3 or FLAC). Only use recordings of people who agreed to it.</p>
    {#if !st.cloning_installed}
      <div class="card prov">
        <div class="ph">
          <div class="pn spacer"><div class="st">WavLM Base+ <span class="pill">Voice cloning</span></div><div class="sd">Needed to make voices from recordings · {formatBytes(st.cloning_bytes)} · CC BY-SA 3.0</div></div>
          {#if progress}<span class="sd">{pct}%</span>{:else}<button class="btn sm" onclick={() => download(true)}><Download size={13} /> Download</button>{/if}
        </div>
      </div>
    {:else}
      <div class="group">
        <div class="srow">
          <input class="input" style="flex:1" placeholder="Name for the voice, e.g. Narrator" bind:value={voiceName} />
          <button class="btn sm" onclick={addVoice} disabled={cloning}>{#if cloning}<Loader size={13} class="spin" />{:else}<Mic size={13} />{/if} Choose recording…</button>
        </div>
        {#each st.voices.filter((v) => v.custom) as v (v.key)}
          <div class="srow">
            <div><div class="st">{v.name}</div></div>
            <button class="icon-btn" onclick={() => deleteVoice(v.key)} aria-label="Delete voice"><Trash2 size={15} /></button>
          </div>
        {/each}
      </div>
    {/if}
  {/if}
{:else}
  <Loader size={16} class="spin" />
{/if}

<style>
  .slider {
    display: flex;
    align-items: center;
    gap: 10px;
    flex: none;
  }
  .slider input {
    width: 200px;
    accent-color: var(--accent);
  }
  .val {
    width: 52px;
    text-align: right;
    font-size: 12.5px;
    color: var(--text-2);
    font-variant-numeric: tabular-nums;
  }
  h3 :global(.btn) {
    margin-left: 8px;
    vertical-align: middle;
  }
</style>
