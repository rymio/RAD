<script lang="ts">
  import { issueNames, type AttemptResult } from "../../lib/coach/api";

  interface Props {
    result: AttemptResult | null;
    analysing: boolean;
    recording: boolean;
    onSpeak: (text: string) => void;
    onAgain: () => void;
    onNext: () => void;
    hasNext: boolean;
    onCalibrate: () => void;
    onShowVowels: () => void;
  }
  let { result, analysing, recording, onSpeak, onAgain, onNext, hasNext, onCalibrate, onShowVowels }: Props = $props();

  const names = issueNames;

  let measures = $derived.by(() => {
    if (!result) return [];
    const c = result.comparison;
    const flagged = new Set(result.issues.map((i) => i.feature));
    const pct = (v: number) => `${Math.round(v * 100)}%`;
    return [
      { feature: "INTONATION", label: "Tune match", value: c.contour_corr === null ? "–" : pct(Math.max(0, c.contour_corr)) },
      { feature: "PACE", label: "Pace", value: `${c.attempt_rate.articulation_sps.toFixed(1)} vs ${c.reference_rate.articulation_sps.toFixed(1)}` },
      { feature: "PAUSES", label: "Pauses", value: `${c.attempt_rate.pause_count} vs ${c.reference_rate.pause_count}` },
      { feature: "FINAL_RISE", label: "Ending", value: `${c.final_rise ? "rises" : "falls"}, reference ${c.reference_final_rise ? "rises" : "falls"}` },
      { feature: "PITCH_RANGE", label: "Pitch range", value: pct(c.range_ratio) + " of reference" },
    ].map((m) => ({ ...m, flagged: flagged.has(m.feature) }));
  });
</script>

<section class="coach" aria-live="polite">
  {#if recording}
    <p class="waiting">Read the sentence now. Recording stops by itself about a second after you finish.</p>
  {:else if analysing}
    <p class="waiting">Listening back…</p>
  {:else if result}
    <blockquote>{result.coach.output.spoken}</blockquote>
    <div class="actions">
      <button class="cbtn" onclick={() => onSpeak(result.coach.output.spoken)}>Hear the feedback</button>
      {#if result.coach.output.next === "next_sentence" && hasNext}
        <button class="cbtn primary" onclick={onNext}>Next sentence <kbd>→</kbd></button>
      {:else}
        <button class="cbtn primary" onclick={onAgain}>Try again <kbd>Space</kbd></button>
      {/if}
      <span class="source">
        {result.coach.source === "llm" ? "Local coach model" : "Measured feedback"}
      </span>
    </div>
    <dl class="measures">
      {#each measures as m (m.feature)}
        <div class:flagged={m.flagged}>
          <dt>{m.label}</dt>
          <dd>{m.value}</dd>
        </div>
      {/each}
    </dl>
    {#if result.words.some((w) => w.comparable)}
      <p class="chint">Click any word in the sentence to hear the reference, then you, then the reference again.</p>
    {/if}
    {#if result.vowels.status.state === "needs_calibration"}
      <p class="chint">
        Vowel feedback is off until you set up your voice.
        <button class="link" onclick={onCalibrate}>Set up your voice</button>
      </p>
    {:else if result.vowels.status.state === "ready" && result.vowels.vowels.some((v) => v.target)}
      <p class="chint">
        Your Australian target vowels were measured.
        <button class="link" onclick={onShowVowels}>See them on the vowel chart</button>
      </p>
    {/if}
    {#if result.heard}
      <p class="chint">Heard: “{result.heard}”</p>
    {/if}
    {#if result.issues.length}
      <ul class="issues">
        {#each result.issues as issue (issue.feature)}
          <li>
            <span class="sev" style:--w="{Math.round(issue.severity * 100)}%" aria-hidden="true"></span>
            <strong>{names[issue.feature] ?? issue.feature}:</strong>
            {issue.detail}
          </li>
        {/each}
      </ul>
    {/if}
  {:else}
    <p class="waiting">
      Listen to the sentence, then press <kbd>Space</kbd> and read it aloud. Recording stops by itself when you finish.
    </p>
  {/if}
</section>

<style>
  .coach {
    border-left: 3px solid var(--you);
    padding: 3.75px 0 3.75px 18.75px;
    max-width: 930px;
  }
  blockquote {
    margin: 0 0 15px;
    font-family: var(--serif);
    font-size: var(--t-lead);
    font-style: italic;
    line-height: 1.55;
    max-width: 38em;
  }
  .waiting {
    margin: 0;
    color: var(--ink-soft);
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 9px;
    flex-wrap: wrap;
  }
  .cbtn.primary {
    background: var(--ink);
    color: var(--paper);
    border-color: var(--ink);
  }
  .cbtn.primary kbd {
    color: var(--paper);
    border-color: color-mix(in srgb, var(--paper) 40%, transparent);
  }
  .source {
    font-size: var(--t-small);
    color: var(--ink-soft);
    margin-left: auto;
  }
  .measures {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(150px, 1fr));
    gap: 11.25px 22.5px;
    margin: 18.75px 0 0;
  }
  .measures div {
    border-top: 1px solid var(--rule);
    padding-top: 6px;
  }
  .measures dt {
    font-size: var(--t-small);
    color: var(--ink-soft);
  }
  .measures dd {
    margin: 0;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }
  .measures .flagged {
    border-top: 2px solid var(--off);
  }
  .measures .flagged dd {
    color: var(--off);
  }
  .chint {
    font-size: var(--t-small);
    color: var(--ink-soft);
    margin: 13.5px 0 0;
  }
  .link {
    background: none;
    border: 0;
    padding: 0;
    color: var(--reference);
    font: inherit;
    text-decoration: underline;
    text-underline-offset: 0.15em;
  }
  .issues {
    list-style: none;
    padding: 0;
    margin: 15px 0 0;
    font-size: var(--t-small);
    color: var(--ink-soft);
  }
  .issues li {
    margin-bottom: 3.75px;
  }
  .issues strong {
    color: var(--ink);
    font-weight: 600;
  }
  .sev {
    display: inline-block;
    width: 36px;
    height: 5px;
    border-radius: 3px;
    margin-right: 7.5px;
    vertical-align: middle;
    background: linear-gradient(to right, var(--off) var(--w), var(--rule) var(--w));
  }
</style>
