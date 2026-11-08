<script lang="ts">
  // Vowel chart: tongue position as a map. Left is front of the mouth, right is back;
  // top is close (tongue high), bottom is open. Blue areas are where the Australian
  // reference's vowels sit; arrows show how diphthongs move. Gold is you.
  import type { ChartWord, Cloud, Point2, VowelChartData } from "../../lib/coach/api";

  interface Props {
    data: VowelChartData;
    showYours: boolean;
  }
  let { data, showYours }: Props = $props();

  const W = 640;
  const H = 420;
  const pad = { left: 52, right: 24, top: 30, bottom: 34 };

  let extent = $derived.by(() => {
    const vals: number[] = [];
    const add = (p: Point2 | null) => p && vals.push(Math.abs(p[0]), Math.abs(p[1]));
    data.reference.forEach((c) => (add(c.centre), add(c.onset), add(c.offset)));
    data.words.forEach((w) => (w.reference.forEach(add), w.you?.forEach(add)));
    if (showYours) data.yours.forEach((c) => add(c.centre));
    return Math.min(3.4, Math.max(2.2, ...vals) + 0.4);
  });

  const plotW = W - pad.left - pad.right;
  const plotH = H - pad.top - pad.bottom;
  // z[0] = openness (F1), z[1] = frontness (F2).
  const X = (p: Point2) => pad.left + ((extent - p[1]) / (2 * extent)) * plotW;
  const Y = (p: Point2) => pad.top + ((p[0] + extent) / (2 * extent)) * plotH;
  const unit = $derived(plotW / (2 * extent));

  function arrow(a: Point2, b: Point2): string {
    return `M${X(a)} ${Y(a)} L${X(b)} ${Y(b)}`;
  }

  function cloudLabelPoint(c: Cloud): Point2 {
    return c.onset ?? c.centre;
  }

  const gradeText = { good: "Close", fair: "A bit off", poor: "Off" } as const;

  let sentenceTargets = $derived(new Set(data.words.map((w) => w.set)));
  let scored = $derived(data.words.filter((w) => w.you));

  /** All labels placed in one pass, each nudged away from labels already placed. */
  let placement = $derived.by(() => {
    const placed: { x: number; y: number; w: number }[] = [];
    const clear = (x: number, y: number, w: number) =>
      !placed.some((p) => x < p.x + p.w + 4 && p.x < x + w + 4 && Math.abs(p.y - y) < 14);
    const place = (x: number, y: number, text: string, step: number) => {
      const w = text.length * 7.5;
      let tries = 0;
      while (!clear(x, y, w) && tries++ < 6) y += step;
      placed.push({ x, y, w });
      return { x, y };
    };
    const clouds = data.reference.map((c) => place(X(cloudLabelPoint(c)) + 8, Y(cloudLabelPoint(c)) - 8, c.keyword, -15));
    const words = data.words.map((w) => {
      const anchor = w.you ? w.you[0] : w.reference[0];
      return place(X(anchor) + 9, Y(anchor) + 16, w.word, 15);
    });
    return { clouds, words };
  });
</script>

<figure class="chart">
  <svg viewBox="0 0 {W} {H}" role="img" aria-label="Vowel chart: Australian reference in blue, your vowels in gold">
    <defs>
      <marker id="ref-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
        <path d="M0 0L10 5L0 10z" fill="var(--reference)" />
      </marker>
      <marker id="you-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
        <path d="M0 0L10 5L0 10z" fill="var(--you)" />
      </marker>
    </defs>

    <!-- frame and axis words -->
    <rect x={pad.left} y={pad.top} width={plotW} height={plotH} rx="10" class="frame" />
    <line x1={X([0, 0])} x2={X([0, 0])} y1={pad.top} y2={pad.top + plotH} class="grid" />
    <line x1={pad.left} x2={pad.left + plotW} y1={Y([0, 0])} y2={Y([0, 0])} class="grid" />
    <text x={pad.left + 8} y={pad.top - 10} class="axis">front of mouth</text>
    <text x={pad.left + plotW - 8} y={pad.top - 10} class="axis" text-anchor="end">back</text>
    <text x={pad.left - 10} y={pad.top + 14} class="axis" text-anchor="end">close</text>
    <text x={pad.left - 10} y={pad.top + plotH - 4} class="axis" text-anchor="end">open</text>

    <!-- reference categories -->
    {#each data.reference as c, ci (c.set)}
      {@const focus = sentenceTargets.has(c.set)}
      <g class="cloud" class:focus>
        {#if c.onset && c.offset}
          <!-- gliding vowel: where it starts and where it heads; this sentence's own words draw their own arrows -->
          {#if !focus}
            <circle cx={X(c.onset)} cy={Y(c.onset)} r="4" class="glide-start" />
            <path d={arrow(c.onset, c.offset)} class="ref-move" marker-end="url(#ref-arrow)" />
          {/if}
        {:else}
          <ellipse
            cx={X(c.centre)}
            cy={Y(c.centre)}
            rx={Math.max(10, c.spread[1] * unit)}
            ry={Math.max(10, c.spread[0] * unit)}
          />
        {/if}
        <text x={placement.clouds[ci].x} y={placement.clouds[ci].y} class="kw">{c.keyword}</text>
      </g>
    {/each}

    <!-- your usual vowels from calibration -->
    {#if showYours}
      {#each data.yours as c (c.set)}
        <g class="yours">
          {#if c.onset && c.offset}
            <path d={arrow(c.onset, c.offset)} marker-end="url(#you-arrow)" />
          {:else}
            <ellipse cx={X(c.centre)} cy={Y(c.centre)} rx={Math.max(8, c.spread[1] * unit)} ry={Math.max(8, c.spread[0] * unit)} />
          {/if}
        </g>
      {/each}
    {/if}

    <!-- this sentence -->
    {#each data.words as w (w.word_index)}
      <g class="word" class:target={w.target}>
        {#if w.reference.length === 2}
          <path d={arrow(w.reference[0], w.reference[1])} class="ref-word" marker-end="url(#ref-arrow)" />
        {:else}
          <circle cx={X(w.reference[0])} cy={Y(w.reference[0])} r="5" class="ref-dot" />
        {/if}
        {#if w.you}
          {#if w.you.length === 2}
            <path d={arrow(w.you[0], w.you[1])} class="you-word" marker-end="url(#you-arrow)" />
          {:else}
            <circle cx={X(w.you[0])} cy={Y(w.you[0])} r="6" class="you-dot" />
          {/if}
        {/if}
      </g>
    {/each}
    {#each data.words as w, i (w.word_index)}
      <text x={placement.words[i].x} y={placement.words[i].y} class="word-label" class:ref={!w.you} class:cmuted={!w.target}>{w.word}</text>
    {/each}
  </svg>
  <figcaption>
    <span class="key ref">Australian reference: shaded areas are where each vowel sits, arrows show vowels that glide</span>
    <span class="key you">You</span>
  </figcaption>
</figure>

{#if scored.length || data.rhotic.length}
  <ul class="verdicts">
    {#each scored as w (w.word_index)}
      <li class:target={w.target}>
        <span class="w">{w.word}</span>
        <span class="k">{w.keyword} vowel</span>
        <span class="g {w.grade}">{w.grade ? gradeText[w.grade] : ""}</span>
        <span class="h">{w.hint ?? ""}</span>
      </li>
    {/each}
    {#each data.rhotic as r (r.word_index)}
      <li class:target={r.flagged}>
        <span class="w">{r.word}</span>
        <span class="k">the R</span>
        <span class="g {r.flagged ? 'poor' : 'good'}">{r.flagged ? "Pronounced" : "Dropped"}</span>
        <span class="h">{r.hint ?? ""}</span>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .chart {
    margin: 0;
  }
  svg {
    width: 100%;
    max-width: 760px;
    height: auto;
    display: block;
    font-family: var(--sans);
  }
  .frame {
    fill: color-mix(in srgb, var(--paper-raised) 70%, transparent);
    stroke: var(--rule);
  }
  .grid {
    stroke: var(--rule);
    stroke-dasharray: 2 5;
  }
  .axis {
    font-size: 12px;
    fill: var(--ink-soft);
  }
  .cloud ellipse {
    fill: var(--reference-wash);
    stroke: none;
  }
  .cloud.focus ellipse {
    stroke: var(--reference);
    stroke-width: 1.25;
  }
  .glide-start {
    fill: var(--reference);
    opacity: 0.5;
  }
  .ref-move {
    fill: none;
    stroke: var(--reference);
    stroke-width: 1.25;
    opacity: 0.7;
  }
  .kw {
    font-family: var(--serif);
    font-style: italic;
    font-size: 14px;
    fill: var(--ink-soft);
  }
  .cloud.focus .kw {
    fill: var(--reference);
    font-weight: 600;
  }
  .yours ellipse {
    fill: none;
    stroke: var(--you);
    stroke-dasharray: 3 3;
    opacity: 0.7;
  }
  .yours path {
    fill: none;
    stroke: var(--you);
    stroke-dasharray: 3 3;
    opacity: 0.7;
  }
  .ref-word {
    fill: none;
    stroke: var(--reference);
    stroke-width: 2.5;
  }
  .ref-dot {
    fill: var(--reference);
  }
  .you-word {
    fill: none;
    stroke: var(--you);
    stroke-width: 3;
  }
  .you-dot {
    fill: var(--you);
    stroke: var(--paper);
    stroke-width: 2;
  }
  .word-label {
    font-size: 13px;
    font-weight: 600;
    fill: var(--ink);
  }
  .word-label.ref {
    fill: var(--reference);
  }
  .word:not(.target),
  .word-label.cmuted {
    opacity: 0.55;
  }
  figcaption {
    display: flex;
    flex-wrap: wrap;
    gap: 5.25px 18.75px;
    padding-left: 52px;
    font-size: var(--t-small);
    color: var(--ink-soft);
  }
  .key::before {
    content: "";
    display: inline-block;
    width: 18px;
    height: 8px;
    border-radius: 4px;
    margin-right: 6.75px;
    vertical-align: middle;
  }
  .key.ref::before {
    background: var(--reference-wash);
    border-top: 2px solid var(--reference);
  }
  .key.you::before {
    background: var(--you);
    height: 3px;
  }
  .verdicts {
    list-style: none;
    margin: 18.75px 0 0;
    padding: 0;
    max-width: 930px;
  }
  .verdicts li {
    display: grid;
    grid-template-columns: 105px 105px 97.5px 1fr;
    gap: 11.25px;
    align-items: baseline;
    border-top: 1px solid var(--rule);
    padding: 7.5px 0;
    color: var(--ink-soft);
  }
  .verdicts li.target {
    color: var(--ink);
  }
  .w {
    font-family: var(--serif);
    font-size: 15.75px;
  }
  .k {
    font-size: var(--t-small);
  }
  .g {
    font-weight: 600;
    font-size: var(--t-small);
  }
  .g.good {
    color: var(--close);
  }
  .g.fair {
    color: var(--you);
  }
  .g.poor {
    color: var(--off);
  }
  .h {
    font-size: var(--t-small);
  }
</style>
