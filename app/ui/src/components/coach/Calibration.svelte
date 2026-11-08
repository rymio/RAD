<script lang="ts">
  // Voice calibration: read eight short sentences so the app learns where your vowels sit.
  import { untrack } from "svelte";
  import { api, message, type CalibrationInfo, type TakeResult } from "../../lib/coach/api";

  interface Props {
    info: CalibrationInfo;
    /** Latest microphone level in dBFS while recording. */
    level: number | null;
    onClose: () => void;
    onSaved: (info: CalibrationInfo) => void;
  }
  let { info, level, onClose, onSaved }: Props = $props();

  // The sheet owns its progress after opening; the prop is only the starting point.
  const initial = untrack(() => info);
  function firstUnread(i: CalibrationInfo): number {
    const k = i.passage.findIndex((_, j) => !i.taken.includes(j));
    return k === -1 ? 0 : k;
  }
  let current = $state(firstUnread(initial));
  let phase = $state<"idle" | "recording" | "measuring">("idle");
  let takes = $state<Record<number, TakeResult["words"]>>({});
  let latest = $state<CalibrationInfo>(initial);
  let error = $state<string | null>(null);
  let saving = $state(false);

  const levelPct = $derived(level === null ? 0 : Math.max(0, Math.min(100, ((level + 60) / 60) * 100)));

  async function record() {
    if (phase === "recording") return finish();
    if (phase !== "idle") return;
    error = null;
    try {
      await api.startRecording();
      phase = "recording";
    } catch (e) {
      error = message(e);
    }
  }

  /** Called on auto-stop or when the learner presses the button again. */
  export async function finish() {
    if (phase !== "recording") return;
    phase = "measuring";
    try {
      const r = await api.calibrationTake(current);
      takes = { ...takes, [r.index]: r.words };
      latest = r.info;
      const allMeasured = r.words.every((w) => w.measured);
      if (allMeasured) {
        const next = latest.passage.findIndex((_, i) => i > current && !latest.taken.includes(i));
        if (next !== -1) current = next;
      }
    } catch (e) {
      error = message(e);
    }
    phase = "idle";
  }

  async function save() {
    saving = true;
    error = null;
    try {
      onSaved(await api.calibrationSave());
    } catch (e) {
      error = message(e);
    }
    saving = false;
  }

  function onKey(e: KeyboardEvent) {
    if (e.code === "Space") {
      e.preventDefault();
      record();
    } else if (e.key === "ArrowRight" && phase === "idle") current = Math.min(latest.passage.length - 1, current + 1);
    else if (e.key === "ArrowLeft" && phase === "idle") current = Math.max(0, current - 1);
    else if (e.key === "Escape" && phase === "idle") onClose();
  }

  const normalisingDone = $derived(new Set(latest.coverage.have.map(([set]) => set)));
</script>

<svelte:window onkeydown={onKey} />

<div class="scrim" role="presentation">
  <section class="sheet" aria-label="Set up your voice">
    <header>
      <h2>Set up your voice</h2>
      <button class="cbtn quiet" onclick={onClose} disabled={phase !== "idle"}>Close</button>
    </header>
    <p class="lead">
      Read each sentence aloud at a natural pace. This maps where your vowels sit, so they can be compared fairly
      with the Australian reference whatever your voice. It takes about two minutes and you only do it once.
    </p>

    {#if !latest.available}
      <p class="warn">Calibration needs Whisper word timings. Download Whisper small.en in Models, then choose Reload speech models.</p>
    {/if}

    <ol class="steps" aria-label="Calibration sentences">
      {#each latest.passage as s, i (i)}
        <li>
          <button
            class:current={i === current}
            class:done={latest.taken.includes(i)}
            onclick={() => phase === "idle" && (current = i)}
            aria-label="Sentence {i + 1}: {s}{latest.taken.includes(i) ? ', read' : ''}"
            aria-current={i === current ? "step" : undefined}
          ></button>
        </li>
      {/each}
    </ol>

    <p class="sentence">{latest.passage[current]}</p>

    <div class="controls">
      <button class="record" class:on={phase === "recording"} onclick={record} disabled={phase === "measuring" || !latest.available}>
        <span class="cdot" aria-hidden="true"></span>
        {phase === "recording" ? "Listening… press to finish" : phase === "measuring" ? "Measuring" : takes[current] ? "Read it again" : "Read it"}
        <kbd>Space</kbd>
      </button>
      <div class="meter" aria-label="Microphone level" role="meter" aria-valuenow={Math.round(levelPct)} aria-valuemin="0" aria-valuemax="100">
        <div class="fill" style:width="{phase === 'recording' ? levelPct : 0}%"></div>
      </div>
    </div>

    {#if takes[current]}
      <ul class="chips">
        {#each takes[current] as w (w.word)}
          <li class:missed={!w.measured}>
            {w.word}
            <span>{w.measured ? `${w.keyword} vowel measured` : "not caught, read it again"}</span>
          </li>
        {/each}
      </ul>
    {/if}

    {#if error}<p class="warn" role="alert">{error}</p>{/if}

    <footer>
      <div class="coverage">
        <span class="clabel">Vowels mapped:</span>
        {#each ["see", "sit", "bed", "cat", "car", "hot", "cup", "saw", "put", "bird"] as k (k)}
          <span class="kw" class:have={latest.coverage.have.some(([, kw]) => kw === k)}>{k}</span>
        {/each}
      </div>
      <button class="cbtn primary" onclick={save} disabled={!latest.coverage.ready || saving || phase !== "idle"}>
        {saving ? "Saving" : "Save calibration"}
      </button>
    </footer>
    <span class="sr-only">{normalisingDone.size} vowel categories measured</span>
  </section>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: rgba(20, 30, 30, 0.35);
    display: grid;
    place-items: center;
    z-index: 460;
  }
  .sheet {
    background: var(--paper-raised);
    border-radius: var(--radius-panel);
    padding: 22.5px 26.25px;
    width: min(760px, 94vw);
    display: grid;
    gap: 15px;
    box-shadow: 0 24px 60px rgba(10, 20, 20, 0.28);
  }
  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
  h2 {
    margin: 0;
    font-size: var(--t-lead);
  }
  .lead {
    margin: 0;
    color: var(--ink-soft);
    max-width: 600px;
  }
  .warn {
    margin: 0;
    color: var(--off);
  }
  .steps {
    display: flex;
    gap: 6px;
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .steps button {
    width: 34px;
    height: 8px;
    border-radius: 4px;
    border: 0;
    padding: 0;
    background: var(--rule);
  }
  .steps button.done {
    background: color-mix(in srgb, var(--close) 70%, var(--rule));
  }
  .steps button.current {
    background: var(--reference);
    width: 48px;
  }
  .sentence {
    font-family: var(--serif);
    font-size: clamp(27px, 3vw, 36px);
    font-variation-settings: "opsz" 72;
    line-height: 1.25;
    margin: 3.75px 0;
  }
  .controls {
    display: flex;
    align-items: center;
    gap: 15px;
  }
  .record {
    display: inline-flex;
    align-items: center;
    gap: 9.75px;
    border: 0;
    border-radius: var(--radius-control);
    padding: 12px 21px;
    font-size: var(--t-lead);
    font-weight: 650;
    background: var(--you);
    color: #1d1a10;
  }
  .record:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .record kbd {
    color: #1d1a10;
    border-color: rgba(29, 26, 16, 0.3);
  }
  .record .cdot {
    width: 12px;
    height: 12px;
    border-radius: 50%;
    background: #1d1a10;
  }
  .record.on .cdot {
    animation: pulse 1s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      transform: scale(0.55);
      opacity: 0.6;
    }
  }
  .meter {
    flex: 1;
    max-width: 260px;
    height: 10px;
    border-radius: 5px;
    background: var(--rule);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: var(--close);
    transition: width 80ms linear;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 7.5px;
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .chips li {
    border: 1px solid color-mix(in srgb, var(--close) 60%, var(--rule));
    border-radius: 999px;
    padding: 3.75px 11.25px;
    font-weight: 600;
  }
  .chips li span {
    font-weight: 400;
    font-size: var(--t-small);
    color: var(--ink-soft);
    margin-left: 5.25px;
  }
  .chips li.missed {
    border-color: var(--off);
  }
  footer {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 15px;
    border-top: 1px solid var(--rule);
    padding-top: 15px;
    flex-wrap: wrap;
  }
  .coverage {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    align-items: baseline;
    font-size: var(--t-small);
    color: var(--ink-soft);
  }
  .coverage .kw {
    font-family: var(--serif);
    font-style: italic;
    font-size: 14.25px;
    opacity: 0.45;
  }
  .coverage .kw.have {
    opacity: 1;
    color: var(--close);
  }
  .cbtn.primary {
    background: var(--ink);
    color: var(--paper);
    border-color: var(--ink);
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0 0 0 0);
  }
</style>
