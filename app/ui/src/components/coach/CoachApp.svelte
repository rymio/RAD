<script lang="ts">
  import { listen, type UnlistenFn } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";
  import {
    api,
    message,
    type AttemptResult,
    type Bootstrap,
    type LivePayload,
    type ModelInfo,
    type ModelProgress,
    type PlaybackPayload,
    type SentenceRef,
    type TextDetail,
  } from "../../lib/coach/api";
  import Library from "./Library.svelte";
  import Melody from "./Melody.svelte";
  import PaceBar from "./PaceBar.svelte";
  import CoachPanel from "./CoachPanel.svelte";
  import SentenceWords from "./SentenceWords.svelte";
  import VowelChart from "./VowelChart.svelte";
  import Calibration from "./Calibration.svelte";
  import VoicePicker from "./VoicePicker.svelte";
  import PageReader from "./PageReader.svelte";
  import type { CalibrationInfo, NeuralVoice, VoiceInfo, VowelChartData } from "../../lib/coach/api";
  import type { EngineState } from "../../lib/coach/api";
  import { coach } from "../../lib/coach.svelte";
  import { gilgamesh } from "../../lib/gilgamesh/gilgamesh.svelte";

  type Phase = "idle" | "loading" | "recording" | "analysing";

  let boot = $state<Bootstrap | null>(null);
  let text = $state<TextDetail | null>(null);
  let index = $state(0);
  let reference = $state<SentenceRef | null>(null);
  let phase = $state<Phase>("idle");
  let error = $state<string | null>(null);

  let live = $state<{ t: number; st: number | null }[]>([]);
  let liveRate = $state<number | null>(null);
  let result = $state<AttemptResult | null>(null);
  let playing = $state<"reference" | "attempt" | "word" | null>(null);
  let playhead = $state<number | null>(null);

  let voice = $state("Karen");
  let voices = $state<VoiceInfo[]>([]);
  let neuralVoices = $state<NeuralVoice[]>([]);
  let changeOpen = $state(false);
  let mode = $state<"sentence" | "page">("sentence");
  let pageReader = $state<{ toggle: () => Promise<void>; listen: () => Promise<void> } | null>(null);

  async function setMode(m: "sentence" | "page") {
    if (mode === m || phase === "recording" || phase === "analysing") return;
    await stopAll();
    mode = m;
  }
  let changeRef = $state<HTMLElement | null>(null);

  function openVoiceChange() {
    changeOpen = true;
    requestAnimationFrame(() => changeRef?.scrollIntoView({ behavior: "smooth", block: "start" }));
  }
  let newOpen = $state(false);
  let newTitle = $state("");
  let newBody = $state("");
  let modelsOpen = $state(false);
  let models = $state<ModelInfo[]>([]);
  let progress = $state<Record<string, ModelProgress>>({});

  let onset: number | null = null;

  let view = $state<"melody" | "vowels">("melody");
  let chart = $state<VowelChartData | null>(null);
  let preparing = $state<{ done: number; total: number } | null>(null);
  let showYours = $state(false);
  let calOpen = $state(false);
  let calInfo = $state<CalibrationInfo | null>(null);
  let calSheet = $state<{ finish: () => Promise<void> } | null>(null);
  let liveDb = $state<number | null>(null);

  async function loadChart() {
    if (view !== "vowels" || !text?.reference || !reference) return;
    const c = await guard(() => api.vowelChart());
    if (!c) return;
    chart = c;
    const needsMore = c.analysed < c.sentences && c.status.state !== "needs_words";
    if (needsMore && !preparing) {
      preparing = { done: c.analysed, total: c.sentences };
      chart = (await guard(() => api.prepareReference())) ?? chart;
      preparing = null;
    }
  }

  function setView(v: "melody" | "vowels") {
    view = v;
    loadChart();
  }

  async function openCalibration() {
    await stopAll();
    calInfo = (await guard(() => api.calibrationInfo())) ?? null;
    if (calInfo) calOpen = true;
  }

  function calibrationSaved(info: CalibrationInfo) {
    calOpen = false;
    calInfo = info;
    if (boot) boot.calibrated = info.calibrated;
    result = null;
    loadChart();
  }

  const hasNext = $derived(!!text && index < text.sentences.length - 1);

  async function guard<T>(fn: () => Promise<T>): Promise<T | undefined> {
    try {
      error = null;
      return await fn();
    } catch (e) {
      error = message(e);
      return undefined;
    }
  }

  let starting = $state(true);

  async function start() {
    // In RAD, Coach starts the first time it's opened (its database and speech engines).
    const opened = await guard(() => api.open());
    starting = false;
    if (opened === undefined) return;
    boot = (await guard(() => api.bootstrap())) ?? null;
    if (boot) {
      voices = boot.voices;
      neuralVoices = boot.neural_voices;
      const known = [...voices.map((v) => v.name), ...neuralVoices.map((n) => `neural:${n.id}`)];
      voice = known.includes(boot.voice) ? boot.voice : (voices[0]?.name ?? "Karen");
      if (boot.texts.length) await openText(boot.texts[0].id);
    }
  }

  async function refreshTexts() {
    const b = await guard(() => api.bootstrap());
    if (b && boot) {
      boot.texts = b.texts;
      boot.documents = b.documents;
    }
  }

  async function show(t: TextDetail | undefined) {
    if (!t) return;
    text = t;
    if (t.document) mode = "page"; // a PDF page is read as a whole page
    await goTo(0);
  }

  async function openText(id: number) {
    await stopAll();
    await show(await guard(() => api.openText(id)));
  }

  async function openDocument(id: number) {
    await stopAll();
    await show(await guard(() => api.openDocument(id)));
    await refreshTexts();
  }

  let importing = $state(false);

  async function importPdf() {
    const path = await open({ multiple: false, directory: false, filters: [{ name: "PDF", extensions: ["pdf"] }] });
    if (typeof path !== "string") return;
    await stopAll();
    importing = true;
    const t = await guard(() => api.importPdf(path));
    importing = false;
    await refreshTexts();
    await show(t);
  }

  async function goTo(i: number) {
    if (!text || i < 0 || i >= text.sentences.length) return;
    await stopAll();
    index = i;
    result = null;
    live = [];
    liveRate = null;
    reference = null;
    if (!text.reference) return;
    phase = "loading";
    reference = (await guard(() => api.loadSentence(i))) ?? null;
    phase = "idle";
    loadChart();
  }

  async function stopAll() {
    if (playing) await api.stopPlayback().catch(() => {});
    playing = null;
    playhead = null;
  }

  async function listenRef() {
    if (!reference || phase !== "idle") return;
    if (playing === "reference") return stopAll();
    await stopAll();
    if ((await guard(() => api.play("reference"))) !== undefined) playing = "reference";
  }

  async function hearMe() {
    if (!result || phase !== "idle") return;
    if (playing === "attempt") return stopAll();
    await stopAll();
    if ((await guard(() => api.play("attempt"))) !== undefined) playing = "attempt";
  }

  async function playWord(i: number, mode: "compare" | "reference") {
    if (phase !== "idle") return;
    await stopAll();
    if ((await guard(() => api.playWord(i, mode))) !== undefined) playing = "word";
  }

  async function reloadSpeech() {
    const status = await guard(() => api.reloadSpeech());
    if (status && boot) boot.speech = status;
    models = (await guard(() => api.modelsStatus())) ?? models;
    const b = await guard(() => api.bootstrap());
    if (b && boot) boot.neural_ready = b.neural_ready;
    if (text?.reference) await goTo(index);
  }

  function engineText(e: EngineState | undefined): string {
    if (!e) return "";
    switch (e.state) {
      case "ready":
        return "Ready";
      case "missing_model":
        return "Model not installed";
      case "not_built":
        return "Not in this build";
      case "failed":
        return "Failed: " + e.detail;
    }
  }

  async function toggleRecord() {
    if (phase === "recording") return finishRecording();
    if (phase !== "idle" || !reference) return;
    await stopAll();
    live = [];
    liveRate = null;
    onset = null;
    result = null;
    const device = await guard(() => api.startRecording());
    if (device !== undefined) phase = "recording";
  }

  async function finishRecording() {
    if (phase !== "recording") return;
    phase = "analysing";
    const r = await guard(() => api.stopRecording());
    result = r ?? null;
    phase = "idle";
    if (r) {
      refreshTexts();
      loadChart();
    }
  }

  async function attachNarration() {
    const path = await open({
      multiple: false,
      directory: false,
      filters: [{ name: "Audio", extensions: ["wav", "mp3", "m4a", "aac", "flac", "mp4"] }],
    });
    if (typeof path !== "string") return;
    phase = "loading";
    const t = await guard(() => api.attachNarration(path));
    phase = "idle";
    if (t) {
      text = t;
      await goTo(0);
    }
  }

  async function useVoice() {
    phase = "loading";
    const t = await guard(() =>
      voice.startsWith("neural:") ? api.useNeuralVoice(Number(voice.slice(7))) : api.useSystemVoice(voice),
    );
    phase = "idle";
    if (t) {
      text = t;
      await goTo(index);
    }
  }

  async function createText() {
    const t = await guard(() => api.createText(newTitle, newBody));
    if (!t) return;
    newOpen = false;
    newTitle = "";
    newBody = "";
    text = t;
    await refreshTexts();
    await goTo(0);
  }

  async function deleteText() {
    if (text?.document) {
      const doc = text.document;
      if (!confirm(`Delete the PDF "${doc.title}" (all ${doc.count} pages) and its practice history?`)) return;
      const docs = await guard(() => api.deleteDocument(doc.id));
      if (!docs || !boot) return;
      boot.documents = docs;
      text = null;
      reference = null;
      return;
    }
    if (!text || !confirm(`Delete "${text.title}" and its practice history?`)) return;
    const list = await guard(() => api.deleteText(text!.id));
    if (!list || !boot) return;
    boot.texts = list;
    text = null;
    reference = null;
    if (list.length) await openText(list[0].id);
  }

  async function showModels() {
    models = (await guard(() => api.modelsStatus())) ?? [];
    modelsOpen = true;
  }

  async function useCoachModel(id: string) {
    const name = await guard(() => api.loadCoach(id));
    if (name && boot) boot.coach = name;
  }

  function onKey(e: KeyboardEvent) {
    const tag = (e.target as HTMLElement)?.tagName;
    if (newOpen || modelsOpen || calOpen || tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return;
    if (mode === "page") {
      if (e.code === "Space") {
        e.preventDefault();
        pageReader?.toggle();
      } else if (e.key.toLowerCase() === "l") pageReader?.listen();
      return;
    }
    if (e.code === "Space") {
      e.preventDefault();
      toggleRecord();
    } else if (e.key === "ArrowRight") goTo(index + 1);
    else if (e.key === "ArrowLeft") goTo(index - 1);
    else if (e.key.toLowerCase() === "l") listenRef();
    else if (e.key.toLowerCase() === "h") hearMe();
    else if (e.key.toLowerCase() === "v") setView(view === "melody" ? "vowels" : "melody");
  }

  $effect(() => {
    const offs: Promise<UnlistenFn>[] = [
      listen<LivePayload>("coach-live", ({ payload }) => {
        // Reading aloud (a sentence, a page or calibration): Gilgamesh mustn't hear it as talk
        // to him, nor be heard in the recording.
        if (gilgamesh.phase !== "off") gilgamesh.stop();
        const lastFrame = payload.frames[payload.frames.length - 1];
        if (lastFrame) liveDb = lastFrame.db;
        if (payload.onset === null) return;
        onset ??= payload.onset;
        const pts = payload.frames.filter((f) => f.t >= onset!).map((f) => ({ t: f.t - onset!, st: f.st }));
        live = live.concat(pts);
        liveRate = payload.rate_sps;
      }),
      listen("coach-auto-stop", () => (calOpen ? calSheet?.finish() : finishRecording())),
      listen<{ done: number; total: number }>("coach-vowel-progress", ({ payload }) => (preparing = payload)),
      listen<PlaybackPayload>("coach-playback", ({ payload }) => {
        if (payload.done) {
          if (playing === payload.which) {
            playing = null;
            playhead = null;
          }
          return;
        }
        if (payload.which === "reference" && reference) playhead = payload.position_s - reference.onset_s;
      }),
      listen<ModelProgress>("coach-model-progress", async ({ payload }) => {
        progress = { ...progress, [payload.id]: payload };
        if (payload.done) {
          if (payload.error) error = payload.error;
          models = (await api.modelsStatus().catch(() => models)) ?? models;
        }
      }),
    ];
    start();
    return () => offs.forEach((p) => p.then((off) => off()));
  });
  // What Gilgamesh sees, and the controls he uses ([coach: …]).
  $effect(() => {
    coach.ready = !!boot;
    coach.text = text;
    coach.index = index;
    coach.reference = reference;
    coach.result = result;
    coach.phase = phase;
    coach.mode = mode;
    coach.view = view;
    coach.chart = chart;
    coach.calibrated = boot?.calibrated ?? false;
    coach.coachName = boot?.coach ?? "rules";
    coach.texts = (boot?.texts ?? []).map((t) => ({ id: t.id, title: t.title }));
    coach.documents = (boot?.documents ?? []).map((d) => ({ id: d.id, title: d.title }));
  });

  $effect(() => {
    coach.open = true;
    coach.controls = {
      listen: listenRef,
      goTo,
      hearMe,
      setView,
      setMode,
      addText: async (title, body) => {
        await stopAll();
        const t = await guard(() => api.createText(title, body));
        if (!t) return;
        await refreshTexts();
        mode = "sentence";
        await show(t);
      },
      openByTitle: async (title) => {
        const q = title.toLowerCase().trim();
        const pick = <T extends { title: string }>(list: T[]) =>
          list.find((x) => x.title.toLowerCase() === q) ?? list.find((x) => x.title.toLowerCase().includes(q));
        const t = pick(boot?.texts ?? []);
        if (t) return openText(t.id).then(() => true);
        const d = pick(boot?.documents ?? []);
        if (d) return openDocument(d.id).then(() => true);
        return false;
      },
    };
    return () => {
      coach.open = false;
      coach.ready = false;
      coach.controls = null;
      void stopAll();
      // Leaving Coach mid-reading: stop the microphone (the attempt is dropped).
      if (phase === "recording") void api.stopRecording().catch(() => {});
    };
  });
</script>

<svelte:window onkeydown={onKey} />

<div class="shell">
  <Library
    texts={boot?.texts ?? []}
    documents={boot?.documents ?? []}
    activeId={text?.document ? null : (text?.id ?? null)}
    activeDocId={text?.document?.id ?? null}
    onOpen={openText}
    onOpenDocument={openDocument}
    onNew={() => (newOpen = true)}
    onImportPdf={importPdf}
    onModels={showModels}
    coachName={boot?.coach ?? "rules"}
    calibrated={boot?.calibrated ?? false}
    onCalibrate={openCalibration}
  />

  <main>
    {#if error}
      <div class="error" role="alert">
        <span>{error}</span>
        <button class="cbtn quiet" onclick={() => (error = null)}>Dismiss</button>
      </div>
    {/if}

    {#if importing}
      <p class="importing" aria-live="polite">Reading the PDF…</p>
    {/if}
    {#if starting}
      <p class="importing" aria-live="polite">Starting Coach…</p>
    {:else if !text}
      <section class="empty">
        <h1>Pick a text to practise, or add your own.</h1>
        <div class="ref-actions">
          <button class="cbtn" onclick={() => (newOpen = true)}>New text</button>
          <button class="cbtn" onclick={importPdf}>Open PDF…</button>
        </div>
      </section>
    {:else}
      <header class="bar">
        <div>
          <h2>{text.document ? text.document.title : text.title}</h2>
          {#if text.document}
            {@const d = text.document}
            <nav class="pager" aria-label="Pages">
              <button class="cbtn quiet" disabled={d.prev === null} onclick={() => d.prev !== null && openText(d.prev)} aria-label="Previous page">‹</button>
              <span>Page {d.page_no} <span class="of">({d.index + 1} of {d.count})</span></span>
              <button class="cbtn quiet" disabled={d.next === null} onclick={() => d.next !== null && openText(d.next)} aria-label="Next page">›</button>
            </nav>
          {/if}
          {#if text.reference}
            <span class="source">Reference: {text.reference.label}</span>
            <button class="change-voice" onclick={openVoiceChange}>Change voice</button>
          {/if}
        </div>
        <div class="mode-switch" role="tablist" aria-label="How to practise">
          <button role="tab" aria-selected={mode === "sentence"} class:on={mode === "sentence"} onclick={() => setMode("sentence")}>Sentence by sentence</button>
          <button role="tab" aria-selected={mode === "page"} class:on={mode === "page"} onclick={() => setMode("page")}>Whole page</button>
        </div>
        {#if mode === "sentence"}
        <ol class="steps" aria-label="Sentences">
          {#each text.sentences as s, i (i)}
            <li>
              <button
                class:current={i === index}
                onclick={() => goTo(i)}
                aria-label="Sentence {i + 1}: {s}"
                aria-current={i === index ? "step" : undefined}
              ></button>
            </li>
          {/each}
        </ol>
        <span class="count">Sentence {index + 1} of {text.sentences.length}</span>
        {/if}
        <button class="cbtn quiet" onclick={deleteText}>{text.document ? "Delete PDF" : "Delete text"}</button>
      </header>

      {#if mode === "page" && text.reference}
        {#key text.id}
          <PageReader
            bind:this={pageReader}
            sentences={text.sentences}
            level={liveDb}
            onError={(e) => (error = message(e))}
            onSpeak={(t) => guard(() => api.speak(t))}
            onPractise={(i) => {
              mode = "sentence";
              goTo(i);
            }}
          />
        {/key}
      {:else}
      <SentenceWords
        text={text.sentences[index]}
        words={reference?.words ?? []}
        results={result?.words.length ? result.words : null}
        dim={phase === "loading"}
        onPlay={playWord}
      />

      {#if !text.reference}
        <section class="needs-ref">
          <p>
            This text needs an Australian reading to compare against. A person's narration is best; a macOS voice
            works as a stand-in.
          </p>
          <div class="ref-actions">
            <button class="cbtn" onclick={attachNarration}>Attach narration…</button>
            <span class="or">or choose an Australian voice:</span>
          </div>
          <VoicePicker bind:voices bind:neuralVoices neuralReady={boot?.neural_ready ?? false} bind:voice onUse={useVoice} onError={(e) => (error = message(e))} />
        </section>
      {:else}
        <div class="view-switch" role="tablist" aria-label="Graph">
          <button role="tab" aria-selected={view === "melody"} class:on={view === "melody"} onclick={() => setView("melody")}>Melody</button>
          <button role="tab" aria-selected={view === "vowels"} class:on={view === "vowels"} onclick={() => setView("vowels")}>Vowels <kbd>V</kbd></button>
        </div>

        {#if view === "melody"}
          <Melody
            {reference}
            {live}
            overlay={result?.comparison.overlay ?? null}
            tempo={result?.comparison.tempo ?? null}
            {playhead}
            recording={phase === "recording"}
          />
        {:else}
          <section class="vowels" aria-live="polite">
            {#if preparing}
              <p class="notice">Measuring the reference speaker's vowels: sentence {preparing.done} of {preparing.total}.</p>
            {/if}
            {#if chart}
              {#if chart.status.state === "needs_words"}
                <p class="notice">
                  Vowels are found word by word, so this needs Whisper.
                  <button class="cbtn quiet" onclick={showModels}>Open Models</button>
                </p>
              {:else if chart.status.state === "needs_calibration"}
                <p class="notice">
                  This is the Australian reference's vowel map. Set up your voice (eight short sentences) to see your
                  vowels on it and get vowel feedback.
                  <button class="cbtn" onclick={openCalibration}>Set up your voice</button>
                </p>
              {:else if chart.status.state === "needs_reference" && !preparing}
                <p class="notice">
                  {chart.status.prepared
                    ? `This text has too few different vowels for a fair comparison (it needs ${chart.status.missing.join(", ")}). Add a few more sentences to it.`
                    : "Still measuring the reference."}
                </p>
              {/if}
              {#if chart.reference.length}
                <VowelChart data={chart} {showYours} />
                <label class="toggle">
                  <input type="checkbox" bind:checked={showYours} disabled={!chart.yours.length} />
                  Show my usual vowels (from calibration)
                </label>
              {/if}
            {/if}
          </section>
        {/if}

        <div class="controls">
          <button class="cbtn" onclick={listenRef} disabled={!reference || phase !== "idle"}>
            {playing === "reference" ? "Stop" : "Listen"} <kbd>L</kbd>
          </button>
          <button
            class="record"
            class:on={phase === "recording"}
            onclick={toggleRecord}
            disabled={!reference || phase === "loading" || phase === "analysing"}
            aria-pressed={phase === "recording"}
          >
            <span class="cdot" aria-hidden="true"></span>
            {phase === "recording" ? "Listening… press to finish" : phase === "analysing" ? "Analysing" : "Read it"}
            <kbd>Space</kbd>
          </button>
          <button class="cbtn" onclick={hearMe} disabled={!result || phase !== "idle"}>
            {playing === "attempt" ? "Stop" : "Hear yourself"} <kbd>H</kbd>
          </button>
          <PaceBar
            reference={reference?.articulation_sps ?? null}
            you={phase === "recording" ? liveRate : (result?.comparison.attempt_rate.articulation_sps ?? null)}
          />
        </div>

        <CoachPanel
          {result}
          analysing={phase === "analysing"}
          recording={phase === "recording"}
          onSpeak={(t) => guard(() => api.speak(t))}
          onAgain={toggleRecord}
          onNext={() => goTo(index + 1)}
          {hasNext}
          onCalibrate={openCalibration}
          onShowVowels={() => setView("vowels")}
        />
      {/if}
      {/if}

      {#if text.reference}
        <details class="change-ref" bind:open={changeOpen} bind:this={changeRef}>
          <summary>Change voice or reference</summary>
          <div class="ref-actions">
            <button class="cbtn" onclick={attachNarration}>Attach narration…</button>
          </div>
          <VoicePicker bind:voices bind:neuralVoices neuralReady={boot?.neural_ready ?? false} bind:voice onUse={useVoice} onError={(e) => (error = message(e))} />
        </details>
      {/if}
    {/if}
  </main>
</div>

{#if calOpen && calInfo}
  <Calibration bind:this={calSheet} info={calInfo} level={liveDb} onClose={() => (calOpen = false)} onSaved={calibrationSaved} />
{/if}

{#if newOpen}
  <div class="scrim" role="presentation" onclick={(e) => e.target === e.currentTarget && (newOpen = false)}>
    <form class="sheet" onsubmit={(e) => { e.preventDefault(); createText(); }}>
      <h2>New text</h2>
      <label>
        <span>Title</span>
        <input bind:value={newTitle} placeholder="Optional" />
      </label>
      <label>
        <span>Text to practise</span>
        <textarea bind:value={newBody} rows="9" placeholder="Paste a paragraph or a whole page. Practise it sentence by sentence, or read it all in one go with Whole page."></textarea>
      </label>
      <div class="sheet-actions">
        <button type="button" class="cbtn quiet" onclick={() => (newOpen = false)}>Cancel</button>
        <button type="submit" class="cbtn primary" disabled={!newBody.trim()}>Add text</button>
      </div>
    </form>
  </div>
{/if}

{#if modelsOpen}
  <div class="scrim" role="presentation" onclick={(e) => e.target === e.currentTarget && (modelsOpen = false)}>
    <section class="sheet wide" aria-label="Models">
      <h2>Models</h2>
      <p class="chint">
        Pitch and pace need no models. Downloads stay in {boot?.data_dir}/models.
        {#if !boot?.llm_supported}This build uses measured feedback; rebuild with <code>--features metal</code> to run a local coach model.{/if}
      </p>
      <dl class="engines">
        <div><dt>Word timings (Whisper)</dt><dd>{engineText(boot?.speech.words)}</dd></div>
        <div><dt>Voice detection (Silero)</dt><dd>{engineText(boot?.speech.vad)}</dd></div>
        <div><dt>Pronunciation scoring (wav2vec2)</dt><dd>{engineText(boot?.speech.phonemes)}</dd></div>
      </dl>
      <table>
        <tbody>
          {#each models as m (m.id)}
            {@const p = progress[m.id]}
            <tr>
              <td>
                <strong>{m.name}</strong>
                <div class="chint">{({ coach: "Coach", asr: "Speech recognition", vad: "Voice detection", phonemes: "Pronunciation scoring", tts: "Reference voice" } as const)[m.role]}, needed from {m.milestone}, about {m.size_mb >= 1000 ? (m.size_mb / 1000).toFixed(1) + " GB" : m.size_mb + " MB"}</div>
                {#if m.note && m.status.state !== "ready"}<div class="chint note">{m.note}</div>{/if}
              </td>
              <td class="state">
                {#if p && !p.done}
                  {p.total ? Math.round((p.downloaded / p.total) * 100) + "%" : Math.round(p.downloaded / 1e6) + " MB"}
                {:else if m.status.state === "ready"}
                  Downloaded
                {:else if m.status.state === "partial"}
                  Paused
                {:else}
                  {m.note ? "Not set up" : "Not downloaded"}
                {/if}
              </td>
              <td class="act">
                {#if m.status.state === "ready"}
                  {#if m.role === "coach" && boot?.llm_supported}
                    <button class="cbtn" onclick={() => useCoachModel(m.id)}>Use as coach</button>
                  {/if}
                {:else if m.note}
                  <span class="chint">Set up by script</span>
                {:else if !(p && !p.done)}
                  <button class="cbtn" onclick={() => api.downloadModel(m.id).catch((e) => (error = message(e)))}>
                    {m.status.state === "partial" ? "Resume" : "Download"}
                  </button>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
      <div class="sheet-actions">
        <button class="cbtn quiet" onclick={reloadSpeech}>Reload speech models</button>
        <button class="cbtn" onclick={() => (modelsOpen = false)}>Done</button>
      </div>
    </section>
  </div>
{/if}

<style>
  .shell {
    display: grid;
    /* In RAD, Coach shares the window with RAD's sidebar and Gilgamesh: the texts rail gives way first. */
    grid-template-columns: clamp(200px, 24%, 264px) 1fr;
    height: 100%;
  }
  main {
    position: relative;
    overflow-y: auto;
    padding: 24px clamp(22.5px, 4vw, 52.5px) 45px;
  }
  h2 {
    font-size: var(--t-body);
    font-weight: 650;
    margin: 0;
  }
  .bar {
    display: flex;
    flex-wrap: wrap;
    row-gap: 10px;
    align-items: center;
    gap: 18.75px;
    margin-bottom: 24px;
  }
  .count,
  .bar > .cbtn {
    white-space: nowrap;
  }
  .source,
  .count {
    font-size: var(--t-small);
    color: var(--ink-soft);
  }
  .steps {
    display: flex;
    gap: 6px;
    list-style: none;
    margin: 0 0 0 auto;
    padding: 0;
  }
  .steps button {
    width: 26px;
    height: 8px;
    border-radius: 4px;
    border: 0;
    background: var(--rule);
    padding: 0;
  }
  .steps button.current {
    background: var(--reference);
    width: 40px;
  }
  .view-switch,
  .mode-switch {
    display: inline-flex;
    gap: 2px;
    padding: 3px;
    border-radius: var(--radius-control);
    background: var(--rule);
    margin-bottom: 11.25px;
  }
  .view-switch button,
  .mode-switch button {
    white-space: nowrap;
    border: 0;
    background: transparent;
    border-radius: var(--radius-control);
    padding: 4.5px 13.5px;
    font-weight: 600;
    color: var(--ink-soft);
  }
  .view-switch button.on,
  .mode-switch button.on {
    background: var(--paper-raised);
    color: var(--ink);
  }
  .vowels .notice {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 11.25px;
    color: var(--ink-soft);
    max-width: 720px;
    margin: 0 0 15px;
  }
  .toggle {
    display: inline-flex;
    gap: 7.5px;
    align-items: center;
    font-size: var(--t-small);
    color: var(--ink-soft);
    margin-top: 7.5px;
  }
  .controls {
    display: flex;
    align-items: center;
    gap: 11.25px;
    flex-wrap: wrap;
    margin: 21px 0 30px;
  }
  .needs-ref {
    max-width: 600px;
    border-left: 3px solid var(--reference);
    padding-left: 18.75px;
  }
  .needs-ref p {
    margin: 0 0 15px;
    color: var(--ink-soft);
  }
  .ref-actions {
    display: flex;
    gap: 9px;
    align-items: center;
    flex-wrap: wrap;
  }
  .or {
    color: var(--ink-soft);
  }
  .importing {
    color: var(--ink-soft);
    margin: 0 0 15px;
  }
  .pager {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-right: 11.25px;
  }
  .pager .of {
    color: var(--ink-soft);
    font-size: var(--t-small);
  }
  .change-voice {
    margin-left: 7.5px;
    background: none;
    border: 0;
    padding: 0;
    color: var(--reference);
    font: inherit;
    text-decoration: underline;
    text-underline-offset: 0.15em;
    cursor: pointer;
  }
  .change-ref {
    margin-top: 37.5px;
    color: var(--ink-soft);
  }
  .change-ref summary {
    cursor: pointer;
    margin-bottom: 11.25px;
  }
  .empty {
    display: grid;
    place-items: start;
    gap: 15px;
    margin-top: 20vh;
  }
  .empty h1 {
    font-family: var(--serif);
    font-weight: 500;
    font-size: var(--t-h);
    margin: 0;
  }
  .error {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 15px;
    background: color-mix(in srgb, var(--off) 12%, var(--paper));
    border-left: 3px solid var(--off);
    padding: 7.5px 7.5px 7.5px 15px;
    margin-bottom: 18.75px;
    border-radius: 0 8px 8px 0;
  }
  .scrim {
    position: fixed;
    inset: 0;
    background: rgba(20, 30, 30, 0.35);
    display: grid;
    place-items: center;
    z-index: 450;
  }
  .sheet {
    background: var(--paper-raised);
    border-radius: var(--radius-panel);
    padding: 22.5px;
    width: min(560px, 92vw);
    display: grid;
    gap: 15px;
    box-shadow: 0 24px 60px rgba(10, 20, 20, 0.28);
  }
  .sheet.wide {
    width: min(760px, 94vw);
  }
  .sheet h2 {
    font-size: var(--t-lead);
  }
  .sheet label {
    display: grid;
    gap: 4.5px;
    font-weight: 600;
  }
  .sheet input,
  .sheet textarea {
    border: 1px solid var(--rule);
    border-radius: 8px;
    padding: 8.25px 10.5px;
    background: var(--paper);
    font-weight: 400;
  }
  .sheet textarea {
    font-family: var(--serif);
    resize: vertical;
  }
  .sheet-actions {
    display: flex;
    justify-content: flex-end;
    gap: 7.5px;
  }
  .cbtn.primary {
    background: var(--ink);
    color: var(--paper);
    border-color: var(--ink);
  }
  .chint {
    font-size: var(--t-small);
    color: var(--ink-soft);
    margin: 0;
  }
  table {
    width: 100%;
    border-collapse: collapse;
  }
  td {
    border-top: 1px solid var(--rule);
    padding: 10.5px 7.5px;
    vertical-align: middle;
  }
  .state {
    white-space: nowrap;
    color: var(--ink-soft);
    font-variant-numeric: tabular-nums;
  }
  .act {
    text-align: right;
    white-space: nowrap;
  }
  code {
    font-size: 0.85em;
  }
  .engines {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 15px;
    margin: 0;
  }
  .engines dt {
    font-size: var(--t-small);
    color: var(--ink-soft);
  }
  .engines dd {
    margin: 0;
    font-weight: 600;
  }
  .note {
    margin-top: 3.75px;
    max-width: 510px;
  }
</style>
