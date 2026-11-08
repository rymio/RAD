<script lang="ts">
  // Australian voice chooser: macOS voices and neural voices (Chatterbox, cloned from a clip).
  // A neural voice is selected as "neural:<id>"; a macOS voice by its `say` name.
  import { open } from "@tauri-apps/plugin-dialog";
  import { api, type NeuralVoice, type VoiceInfo } from "../../lib/coach/api";

  interface Props {
    voices: VoiceInfo[];
    neuralVoices: NeuralVoice[];
    neuralReady: boolean;
    voice: string;
    onUse: () => void;
    onError: (e: unknown) => void;
  }
  let { voices = $bindable(), neuralVoices = $bindable(), neuralReady, voice = $bindable(), onUse, onError }: Props =
    $props();

  const quality = { premium: "Premium", enhanced: "Enhanced", compact: "basic" } as const;

  function label(v: VoiceInfo) {
    const base = v.name.split(" (")[0];
    const parts = [v.gender, quality[v.quality]].filter(Boolean);
    return `${base} — ${parts.join(", ")}`;
  }

  const onlyBasic = $derived(voices.every((v) => v.quality === "compact"));
  const noMale = $derived(!voices.some((v) => v.gender === "male"));
  const neuralId = $derived(voice.startsWith("neural:") ? Number(voice.slice(7)) : null);

  let busy = $state<string | null>(null);
  let adding = $state(false);
  let newName = $state("");
  let newPath = $state<string | null>(null);
  let consent = $state(false);

  async function refresh() {
    try {
      voices = await api.listVoices();
      if (!voice.startsWith("neural:") && !voices.some((v) => v.name === voice)) voice = voices[0]?.name ?? voice;
    } catch (e) {
      onError(e);
    }
  }

  async function preview() {
    if (neuralId === null) return api.previewVoice(voice).catch(onError);
    busy = "Generating sample… (the first one loads the model and takes a while)";
    await api.previewNeuralVoice(neuralId).catch(onError);
    busy = null;
  }

  async function pickClip() {
    const path = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "Audio", extensions: ["wav", "mp3", "m4a", "aac", "flac", "mp4"] }],
    });
    if (typeof path === "string") newPath = path;
  }

  async function addVoice() {
    if (!newPath) return;
    try {
      const record = consent ? `Speaker permission confirmed in app, ${new Date().toISOString()}` : "";
      neuralVoices = await api.addNeuralVoice(newName, newPath, record);
      const added = neuralVoices[neuralVoices.length - 1];
      if (added) voice = `neural:${added.id}`;
      adding = false;
      newName = "";
      newPath = null;
      consent = false;
    } catch (e) {
      onError(e);
    }
  }

  async function removeVoice() {
    if (neuralId === null) return;
    try {
      neuralVoices = await api.deleteNeuralVoice(neuralId);
      voice = voices[0]?.name ?? "Karen";
    } catch (e) {
      onError(e);
    }
  }
</script>

<div class="picker">
  <select bind:value={voice} aria-label="Australian voice">
    {#if neuralVoices.length}
      <optgroup label="Neural voices">
        {#each neuralVoices as n (n.id)}
          <option value={`neural:${n.id}`}>{n.name} — neural</option>
        {/each}
      </optgroup>
    {/if}
    <optgroup label="macOS voices">
      {#each voices as v (v.name)}
        <option value={v.name}>{label(v)}</option>
      {/each}
    </optgroup>
  </select>
  <button class="cbtn quiet" onclick={preview} disabled={!!busy} title="Hear a sample">▶ Preview</button>
  <button class="cbtn" onclick={onUse} disabled={!!busy}>Use this voice</button>
  {#if neuralId !== null}
    <button class="cbtn quiet" onclick={removeVoice}>Remove</button>
  {/if}
</div>
{#if busy}<p class="get-more">{busy}</p>{/if}

{#if neuralReady}
  {#if adding}
    <div class="add">
      <input placeholder="Name, e.g. Jack" bind:value={newName} aria-label="Voice name" />
      <button class="cbtn quiet" onclick={pickClip}>{newPath ? newPath.split("/").pop() : "Choose clip…"}</button>
      <label><input type="checkbox" bind:checked={consent} /> I have the speaker's permission to copy their voice</label>
      <span class="actions">
        <button class="cbtn" onclick={addVoice} disabled={!newPath || !newName.trim() || !consent}>Add voice</button>
        <button class="cbtn quiet" onclick={() => (adding = false)}>Cancel</button>
      </span>
      <p class="get-more">Use 5–20 seconds of one Australian speaker talking clearly with no music. The voice copies their accent.</p>
    </div>
  {:else}
    <button class="cbtn quiet more" onclick={() => (adding = true)}>+ Add neural voice…</button>
  {/if}
{:else}
  <p class="get-more">For a natural voice copied from any Australian speaker, set up <b>Chatterbox neural voice</b> under Models.</p>
{/if}

{#if onlyBasic || noMale}
  <p class="get-more">
    {#if onlyBasic}Only the basic macOS voice is installed, which sounds robotic.{/if}
    {#if noMale}No male Australian macOS voice is installed.{/if}
    Download free Australian voices (Lee is male; pick <b>Premium</b> for the most natural sound) in
    System Settings › Accessibility › Spoken Content › System Voice › Manage Voices… › English (Australia).
    <span class="actions">
      <button class="cbtn quiet" onclick={() => api.openVoiceSettings().catch(onError)}>Open System Settings</button>
      <button class="cbtn quiet" onclick={refresh}>I've downloaded them, refresh</button>
    </span>
  </p>
{:else}
  <button class="cbtn quiet more" onclick={refresh} title="Re-read installed voices">Refresh voices</button>
{/if}

<style>
  .picker {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
  }
  select,
  input:not([type]) {
    border: 1px solid var(--rule);
    background: var(--paper-raised);
    border-radius: 8px;
    padding: 6.75px 9px;
  }
  .add {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    align-items: center;
    margin-top: 8px;
  }
  .add label {
    font-size: 13.5px;
  }
  .get-more {
    margin: 8px 0 0;
    font-size: 12.75px;
    color: var(--ink-soft);
    line-height: 1.45;
    flex-basis: 100%;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 6px;
  }
  .more {
    margin-top: 6px;
  }
</style>
