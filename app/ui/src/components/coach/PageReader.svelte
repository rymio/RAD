<script lang="ts">
  // Whole-page reading: read the full text in one go, then see every word graded in place,
  // each sentence's issues, and play back the whole reading (words light up as they are heard),
  // one sentence, or one word (reference, yours, reference).
  import { listen as onEvent, type UnlistenFn } from "@tauri-apps/api/event";
  import { api, issueNames, type PageProgress, type PageResult, type PageSentence, type WordResult } from "../../lib/coach/api";

  interface Props {
    sentences: string[];
    /** Microphone level, dB, while recording. */
    level: number | null;
    onError: (e: unknown) => void;
    onSpeak: (text: string) => void;
    onPractise: (index: number) => void;
  }
  let { sentences, level, onError, onSpeak, onPractise }: Props = $props();

  type Phase = "idle" | "recording" | "analysing";
  let phase = $state<Phase>("idle");
  let page = $state<PageResult | null>(null);
  let progress = $state<PageProgress | null>(null);
  let selected = $state<number | null>(null);
  let playing = $state<string | null>(null);
  let position = $state<number | null>(null);
  let startedAt = $state(0);
  let elapsed = $state(0);
  let timer: ReturnType<typeof setInterval> | undefined;
  let preparing = $state(false);
  let listenSpans = $state<[number, number][]>([]);

  /** The sentence being read aloud while listening to the whole page. */
  const reading = $derived(
    playing === "page_reference" && position !== null
      ? listenSpans.findIndex(([a, b]) => position! >= a && position! <= b + 0.45)
      : -1,
  );

  const words = (s: PageSentence): WordResult[] => s.result?.words ?? s.words;

  const summary = $derived.by(() => {
    if (!page) return null;
    const all = page.sentences.flatMap(words);
    const scored = all.filter((w) => !w.missed && w.score != null);
    const count = (g: string) => scored.filter((w) => w.grade === g).length;
    const features = new Map<string, number>();
    for (const s of page.sentences) for (const i of s.result?.issues ?? []) features.set(i.feature, (features.get(i.feature) ?? 0) + 1);
    return {
      total: all.length,
      score: scored.length ? Math.round((scored.reduce((a, w) => a + (w.score ?? 0), 0) / scored.length) * 100) : null,
      good: count("good"),
      fair: count("fair"),
      poor: count("poor"),
      missed: all.filter((w) => w.missed).length,
      skipped: page.sentences.filter((s) => !s.span).length,
      top: [...features.entries()].sort((a, b) => b[1] - a[1]).slice(0, 4),
    };
  });

  /** The word being heard during whole-page playback, as "sentence:word". */
  const speaking = $derived.by(() => {
    if (playing !== "page" || position === null || !page) return null;
    for (const s of page.sentences) {
      const i = s.word_spans.findIndex((sp) => sp && position! >= sp[0] && position! <= sp[1] + 0.05);
      if (i >= 0) return `${s.index}:${i}`;
    }
    return null;
  });

  const current = $derived(page && selected !== null ? page.sentences[selected] : null);

  function sentenceTone(s: PageSentence): string {
    if (!s.span) return "skipped";
    const worst = Math.max(0, ...(s.result?.issues ?? []).map((i) => i.severity));
    return worst >= 0.6 ? "bad" : worst >= 0.3 ? "meh" : "";
  }

  export async function toggle() {
    if (phase === "recording") return finish();
    if (phase !== "idle") return;
    await stop();
    page = null;
    selected = null;
    try {
      await api.startRecording(true);
      phase = "recording";
      startedAt = Date.now();
      elapsed = 0;
      timer = setInterval(() => (elapsed = (Date.now() - startedAt) / 1000), 250);
    } catch (e) {
      onError(e);
    }
  }

  export async function listen() {
    if (phase !== "idle" || preparing) return;
    if (playing === "page_reference") return stop();
    await stop();
    preparing = true;
    try {
      listenSpans = (await api.playPageReference()).sentences;
      playing = "page_reference";
    } catch (e) {
      onError(e);
    }
    preparing = false;
  }

  async function finish() {
    if (phase !== "recording") return;
    clearInterval(timer);
    phase = "analysing";
    progress = null;
    try {
      page = await api.stopPageRecording();
      selected = page.sentences.find((s) => (s.result?.issues.length ?? 0) > 0 || !s.span)?.index ?? 0;
    } catch (e) {
      onError(e);
    }
    phase = "idle";
    progress = null;
  }

  async function stop() {
    if (playing) await api.stopPlayback().catch(() => {});
    playing = null;
    position = null;
  }

  async function play(which: "page" | "reference" | "attempt", sentence?: number) {
    const key = sentence === undefined ? which : `${which}:${sentence}`;
    if (playing === key) return stop();
    await stop();
    try {
      await api.play(which, sentence);
      playing = key;
    } catch (e) {
      onError(e);
    }
  }

  async function playWord(s: PageSentence, w: WordResult) {
    selected = s.index;
    await stop();
    try {
      await api.playWord(w.index, w.comparable ? "compare" : "reference", s.index);
      playing = "word";
    } catch (e) {
      onError(e);
    }
  }

  function mmss(t: number) {
    const s = Math.floor(t);
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  }

  function wordLabel(w: WordResult) {
    if (w.missed) return `${w.display}, not heard`;
    const parts = [w.display];
    if (w.score != null) parts.push(`${Math.round(w.score * 100)}% like the reference`);
    parts.push(w.comparable ? "click to hear reference, yours, reference" : "click to hear the reference");
    return parts.join(", ");
  }

  $effect(() => {
    const offs: Promise<UnlistenFn>[] = [
      onEvent("coach-auto-stop", () => finish()),
      onEvent<PageProgress>("coach-page-progress", ({ payload }) => (progress = payload)),
      onEvent<{ which: string; position_s: number; done: boolean }>("coach-playback", ({ payload }) => {
        if (payload.done) {
          // Ignore the end of a clip that was replaced by a newer one.
          if (playing === payload.which || playing?.startsWith(payload.which + ":")) {
            playing = null;
            position = null;
          }
        } else if (payload.which === "page" || payload.which === "page_reference") position = payload.position_s;
      }),
    ];
    return () => {
      clearInterval(timer);
      offs.forEach((p) => p.then((off) => off()));
    };
  });
</script>

<section class="page-reader">
  <div class="toolbar">
    <button class="cbtn" onclick={listen} disabled={phase !== "idle" || preparing}>
      {preparing ? "Preparing the reading…" : playing === "page_reference" ? "Stop" : "▶ Listen"} <kbd>L</kbd>
    </button>
    <button class="record" class:on={phase === "recording"} onclick={toggle} disabled={phase === "analysing"} aria-pressed={phase === "recording"}>
      <span class="cdot" aria-hidden="true"></span>
      {phase === "recording" ? "Recording… press to finish" : phase === "analysing" ? "Assessing" : page ? "Read it again" : "Read the whole page"}
      <kbd>Space</kbd>
    </button>
    {#if phase === "recording"}
      <span class="clock">{mmss(elapsed)}</span>
      <span class="meter" aria-hidden="true"><span style:width="{Math.max(0, Math.min(100, ((level ?? -60) + 60) * 1.7))}%"></span></span>
    {:else if page}
      <button class="cbtn" onclick={() => play("page")}>{playing === "page" ? "Stop" : "▶ Hear my reading"}</button>
    {/if}
  </div>

  {#if phase === "recording"}
    <p class="chint">Read the page aloud at your own pace. Pausing is fine; recording stops after 6 seconds of silence, or press Space.</p>
  {:else if phase === "analysing"}
    <p class="chint" aria-live="polite">
      {#if progress && progress.stage === "Assessing"}
        Assessing sentence {progress.done + 1} of {progress.total}…
      {:else}
        {progress?.stage ?? "Listening back"}…
      {/if}
    </p>
    {#if progress?.total}
      <div class="bar" aria-hidden="true"><span style:width="{(progress.done / progress.total) * 100}%"></span></div>
    {/if}
  {:else if !page}
    <p class="chint">
      Press <kbd>L</kbd> to hear the whole page read by the reference voice, then press <kbd>Space</kbd> and read it aloud
      yourself. Every word is then marked in place: clear, close, needs work or skipped.
    </p>
  {/if}

  {#if summary}
    <div class="summary">
      {#if summary.score !== null}<div><strong>{summary.score}%</strong><span>like the reference</span></div>{/if}
      <div><strong class="good">{summary.good}</strong><span>clear</span></div>
      <div><strong class="fair">{summary.fair}</strong><span>close</span></div>
      <div><strong class="poor">{summary.poor}</strong><span>need work</span></div>
      <div><strong class="poor">{summary.missed}</strong><span>skipped{summary.skipped ? ` (${summary.skipped} sentence${summary.skipped > 1 ? "s" : ""})` : ""}</span></div>
      {#if summary.top.length}
        <div class="top">
          <span>Most common:</span>
          {#each summary.top as [f, n] (f)}<em>{issueNames[f] ?? f} ×{n}</em>{/each}
        </div>
      {/if}
    </div>
  {/if}

  <div class="layout" class:with-detail={!!current}>
    <article class="text" class:dim={phase !== "idle"}>
      {#if page}
        {#each page.sentences as s (s.index)}
          <span class="sentence {sentenceTone(s)}" class:selected={selected === s.index} class:reading={reading === s.index}>
            {#each words(s) as w (w.index)}
              <button
                class="word {w.missed ? 'missed' : (w.grade ?? '')}"
                class:speaking={speaking === `${s.index}:${w.index}`}
                title={wordLabel(w)}
                aria-label={wordLabel(w)}
                onclick={() => (w.missed ? (selected = s.index) : playWord(s, w))}
              >{w.display}</button>{" "}
            {/each}<button class="mark" onclick={() => (selected = s.index)} aria-label="Details for sentence {s.index + 1}">
              {(s.result?.issues.length ?? 0) || !s.span ? "●" : "✓"}
            </button>{" "}
          </span>
        {/each}
      {:else}
        {#each sentences as s, i (i)}
          <span class="sentence" class:reading={reading === i}>{s}</span>{" "}
        {/each}
      {/if}
    </article>

    {#if current}
      <aside class="detail" aria-live="polite">
        <h3>Sentence {current.index + 1}</h3>
        {#if !current.span}
          <p>This sentence was not heard. It may have been skipped.</p>
        {:else if current.error}
          <p>Could not assess this sentence: {current.error}</p>
        {:else if current.result}
          <blockquote>{current.result.coach.output.spoken}</blockquote>
          {#if current.result.issues.length}
            <ul>
              {#each current.result.issues as i (i.feature + (i.word ?? ""))}
                <li><strong>{issueNames[i.feature] ?? i.feature}:</strong> {i.detail}</li>
              {/each}
            </ul>
          {/if}
        {/if}
        <div class="actions">
          <button class="cbtn" onclick={() => play("reference", current.index)}>
            {playing === `reference:${current.index}` ? "Stop" : "▶ Reference"}
          </button>
          {#if current.span && !current.error}
            <button class="cbtn" onclick={() => play("attempt", current.index)}>
              {playing === `attempt:${current.index}` ? "Stop" : "▶ Yours"}
            </button>
          {/if}
          {#if current.result}
            <button class="cbtn quiet" onclick={() => onSpeak(current.result!.coach.output.spoken)}>Hear the feedback</button>
          {/if}
          <button class="cbtn quiet" onclick={() => onPractise(current.index)}>Practise this sentence</button>
        </div>
        <p class="chint">Click a word to hear the reference, then you, then the reference again.</p>
      </aside>
    {/if}
  </div>
</section>

<style>
  .page-reader {
    display: grid;
    gap: 15px;
  }
  .toolbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 11.25px;
  }
  .clock {
    font-variant-numeric: tabular-nums;
    color: var(--ink-soft);
  }
  .meter,
  .bar {
    display: inline-block;
    width: 140px;
    height: 6px;
    border-radius: 3px;
    background: var(--rule);
    overflow: hidden;
  }
  .bar {
    width: 100%;
    max-width: 450px;
  }
  .meter span,
  .bar span {
    display: block;
    height: 100%;
    background: var(--you);
    transition: width 120ms linear;
  }
  .chint {
    margin: 0;
    color: var(--ink-soft);
    font-size: var(--t-small);
  }
  .summary {
    display: flex;
    flex-wrap: wrap;
    gap: 7.5px 22.5px;
    align-items: baseline;
    padding: 11.25px 15px;
    border: 1px solid var(--rule);
    border-radius: var(--radius-panel);
    background: var(--paper-raised);
  }
  .summary div {
    display: flex;
    gap: 5.25px;
    align-items: baseline;
  }
  .summary strong {
    font-size: var(--t-h);
    font-variant-numeric: tabular-nums;
  }
  .summary span {
    color: var(--ink-soft);
    font-size: var(--t-small);
  }
  .summary .top em {
    font-style: normal;
    font-size: var(--t-small);
    padding: 1.5px 7.5px;
    border-radius: 999px;
    background: color-mix(in srgb, var(--off) 12%, transparent);
  }
  strong.good {
    color: var(--close);
  }
  strong.fair {
    color: var(--you);
  }
  strong.poor {
    color: var(--off);
  }
  .layout {
    display: grid;
    gap: 22.5px;
  }
  .layout.with-detail {
    grid-template-columns: minmax(0, 1fr) minmax(240px, 330px);
  }
  @media (max-width: 900px) {
    .layout.with-detail {
      grid-template-columns: 1fr;
    }
  }
  .text {
    font-family: var(--serif);
    font-size: 20.25px;
    line-height: 1.9;
    max-width: 42em;
    margin: 0;
    transition: opacity 200ms;
  }
  .text.dim {
    opacity: 0.6;
  }
  .sentence {
    border-radius: 6px;
    padding: 0.1em 0;
    box-decoration-break: clone;
    -webkit-box-decoration-break: clone;
  }
  .sentence.meh {
    background: color-mix(in srgb, var(--you) 9%, transparent);
  }
  .sentence.bad,
  .sentence.skipped {
    background: color-mix(in srgb, var(--off) 9%, transparent);
  }
  .sentence.reading {
    background: var(--reference-wash);
  }
  .sentence.selected {
    outline: 2px solid color-mix(in srgb, var(--reference) 55%, transparent);
    outline-offset: 2px;
  }
  .word,
  .mark {
    font: inherit;
    color: inherit;
    background: none;
    border: 0;
    padding: 0;
    margin: 0;
    cursor: pointer;
    border-radius: 4px;
    text-decoration-thickness: 0.07em;
    text-underline-offset: 0.2em;
  }
  .word:hover {
    background: color-mix(in srgb, var(--reference) 12%, transparent);
  }
  .word.speaking {
    background: var(--reference-wash);
  }
  .mark {
    font-family: var(--sans);
    font-size: 0.7em;
    color: var(--ink-soft);
    vertical-align: super;
  }
  .good {
    text-decoration: underline solid var(--close);
  }
  .fair {
    text-decoration: underline dashed var(--you);
    color: color-mix(in srgb, var(--ink) 80%, var(--you));
  }
  .poor {
    text-decoration: underline wavy var(--off);
    text-decoration-thickness: 0.05em;
    color: var(--off);
  }
  .missed {
    text-decoration: line-through var(--off);
    opacity: 0.55;
  }
  .detail {
    align-self: start;
    position: sticky;
    top: 15px;
    border-left: 3px solid var(--you);
    padding: 3.75px 0 3.75px 15px;
    display: grid;
    gap: 9px;
  }
  .detail h3 {
    margin: 0;
    font-size: var(--t-body);
  }
  .detail blockquote {
    margin: 0;
    font-family: var(--serif);
    font-style: italic;
    line-height: 1.5;
  }
  .detail ul {
    margin: 0;
    padding-left: 16.5px;
    font-size: var(--t-small);
    display: grid;
    gap: 4.5px;
  }
  .detail p {
    margin: 0;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 7.5px;
  }
</style>
