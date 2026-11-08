<script lang="ts">
  // The melody: the reference sentence as a ribbon (pitch = height, loudness = thickness,
  // so stressed syllables swell), with your voice traced over it in gold.
  import type { SentenceRef, OverlayPoint, TempoPoint } from "../../lib/coach/api";

  interface Props {
    reference: SentenceRef | null;
    /** Live points, seconds from your speech onset. */
    live: { t: number; st: number | null }[];
    /** After an attempt: your pitch mapped onto the reference timeline. */
    overlay: OverlayPoint[] | null;
    tempo: TempoPoint[] | null;
    /** Playhead in seconds from reference onset, or null. */
    playhead: number | null;
    recording: boolean;
  }

  let { reference, live, overlay, tempo, playhead, recording }: Props = $props();

  let canvas: HTMLCanvasElement;
  let wrap: HTMLDivElement;
  let width = $state(800);
  const height = 270;
  const pad = { left: 44, right: 20, top: 18, bottom: 34 };

  $effect(() => {
    const ro = new ResizeObserver(([entry]) => (width = Math.max(320, entry.contentRect.width)));
    ro.observe(wrap);
    return () => ro.disconnect();
  });

  // Coach's colours are set on its own root (.coach-app) in RAD, so read them where the canvas is.
  function css(name: string): string {
    return getComputedStyle(wrap ?? document.documentElement).getPropertyValue(name).trim();
  }

  function stRange(): [number, number] {
    const vals: number[] = [];
    reference?.contour.forEach((v) => v !== null && vals.push(v));
    live.forEach((p) => p.st !== null && vals.push(p.st));
    overlay?.forEach((p) => p.attempt_st !== null && vals.push(p.attempt_st));
    let lo = Math.min(-6, ...vals);
    let hi = Math.max(6, ...vals);
    lo = Math.max(lo, -14);
    hi = Math.min(hi, 14);
    return [Math.floor(lo - 1), Math.ceil(hi + 1)];
  }

  function draw() {
    if (!canvas) return;
    const dpr = window.devicePixelRatio || 1;
    canvas.width = width * dpr;
    canvas.height = height * dpr;
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, width, height);

    const ink = css("--ink-soft");
    const rule = css("--rule");
    const refColour = css("--reference");
    const refWash = css("--reference-wash");
    const you = css("--you");

    const refDur = reference ? reference.duration_s : 3;
    const liveDur = live.length ? live[live.length - 1].t : 0;
    const span = Math.max(refDur * 1.12, liveDur + 0.3, 1.5);
    const [lo, hi] = stRange();
    const plotW = width - pad.left - pad.right;
    const plotH = height - pad.top - pad.bottom;
    const X = (t: number) => pad.left + (t / span) * plotW;
    const Y = (st: number) => pad.top + ((hi - st) / (hi - lo)) * plotH;

    // Grid: one line per 3 semitones, the speaker's median emphasised.
    ctx.font = "11.5px " + css("--sans");
    ctx.textAlign = "right";
    ctx.textBaseline = "middle";
    for (let s = Math.ceil(lo / 3) * 3; s <= hi; s += 3) {
      ctx.strokeStyle = rule;
      ctx.lineWidth = s === 0 ? 1.25 : 0.75;
      ctx.setLineDash(s === 0 ? [] : [2, 4]);
      ctx.beginPath();
      ctx.moveTo(pad.left, Y(s));
      ctx.lineTo(width - pad.right, Y(s));
      ctx.stroke();
      ctx.fillStyle = ink;
      ctx.fillText(s === 0 ? "mid" : (s > 0 ? "+" : "") + s, pad.left - 6, Y(s));
    }
    ctx.setLineDash([]);
    // Time ticks every second.
    ctx.textAlign = "center";
    ctx.textBaseline = "top";
    for (let t = 0; t <= span; t += 1) {
      ctx.fillStyle = ink;
      ctx.fillText(t + " s", X(t), height - pad.bottom + 8);
    }

    // Reference ribbon.
    if (reference) {
      const c = reference.contour;
      const n = c.length;
      const half = (i: number) => 2.5 + 11 * Math.pow(1 + (reference.intensity[i] ?? -40) / 40, 1.4);
      let i = 0;
      while (i < n) {
        if (c[i] === null) {
          i++;
          continue;
        }
        const start = i;
        while (i < n && c[i] !== null) i++;
        if (i - start < 2) continue;
        ctx.beginPath();
        for (let k = start; k < i; k++) ctx.lineTo(X(k / 100), Y(c[k]!) - half(k));
        for (let k = i - 1; k >= start; k--) ctx.lineTo(X(k / 100), Y(c[k]!) + half(k));
        ctx.closePath();
        ctx.fillStyle = refWash;
        ctx.fill();
        ctx.beginPath();
        for (let k = start; k < i; k++) ctx.lineTo(X(k / 100), Y(c[k]!));
        ctx.strokeStyle = refColour;
        ctx.lineWidth = 1.75;
        ctx.lineJoin = "round";
        ctx.stroke();
      }
    }

    // Your voice: aligned overlay after an attempt, otherwise the live trace.
    const trace: { t: number; st: number | null }[] = overlay
      ? overlay.map((p) => ({ t: p.t, st: p.attempt_st }))
      : live;
    ctx.strokeStyle = you;
    ctx.lineWidth = 3;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    let drawing = false;
    let prev: number | null = null;
    ctx.beginPath();
    for (const p of trace) {
      if (p.st === null || (prev !== null && Math.abs(p.st - prev) > 4)) {
        drawing = false;
        prev = p.st;
        if (p.st === null) continue;
      }
      if (!drawing) {
        ctx.moveTo(X(p.t), Y(p.st));
        drawing = true;
      } else ctx.lineTo(X(p.t), Y(p.st));
      prev = p.st;
    }
    ctx.stroke();
    if (recording && live.length) {
      const last = [...live].reverse().find((p) => p.st !== null);
      const lt = live[live.length - 1].t;
      ctx.strokeStyle = you;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(X(lt), pad.top);
      ctx.lineTo(X(lt), height - pad.bottom);
      ctx.stroke();
      if (last && lt - last.t < 0.15) {
        ctx.fillStyle = you;
        ctx.beginPath();
        ctx.arc(X(last.t), Y(last.st!), 6, 0, Math.PI * 2);
        ctx.fill();
      }
    }

    // Playhead during reference playback.
    if (playhead !== null && playhead >= 0 && playhead <= span) {
      ctx.strokeStyle = refColour;
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      ctx.moveTo(X(playhead), pad.top);
      ctx.lineTo(X(playhead), height - pad.bottom);
      ctx.stroke();
    }

    // Tempo strip: where you dragged or rushed, on the reference timeline.
    if (tempo && tempo.length) {
      const y = height - pad.bottom - 7;
      for (const p of tempo) {
        const w = Math.max(2, X(0.1) - X(0));
        const slow = p.ratio > 1.2;
        const fast = p.ratio < 0.83;
        if (!slow && !fast) continue;
        ctx.fillStyle = slow ? refColour : you;
        ctx.globalAlpha = Math.min(0.85, 0.3 + Math.abs(Math.log(p.ratio)));
        ctx.fillRect(X(p.t) - w / 2, y, w + 0.5, 5);
      }
      ctx.globalAlpha = 1;
    }
  }

  $effect(() => {
    // Track every input so the canvas redraws when any changes.
    void [reference, live, live.length, overlay, tempo, playhead, recording, width];
    draw();
  });

  // Redraw when RAD's theme changes (light, dark or OLED: data-theme on <html>).
  $effect(() => {
    const mo = new MutationObserver(() => draw());
    mo.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme", "class", "style"] });
    return () => mo.disconnect();
  });
</script>

<div class="melody" bind:this={wrap}>
  <canvas bind:this={canvas} style:height="{height}px" style:width="{width}px" aria-label="Pitch melody: reference ribbon in blue, your voice in gold"></canvas>
  <div class="legend">
    <span class="key ref">Australian reference: height is pitch, thickness is loudness</span>
    <span class="key you">You</span>
    {#if tempo && tempo.length}
      <span class="key slow">You dragged</span>
      <span class="key fast">You rushed</span>
    {/if}
  </div>
</div>

<style>
  .melody {
    width: 100%;
  }
  canvas {
    display: block;
  }
  .legend {
    display: flex;
    flex-wrap: wrap;
    gap: 5.25px 18.75px;
    padding-left: 44px;
    font-size: var(--t-small);
    color: var(--ink-soft);
  }
  .key::before {
    content: "";
    display: inline-block;
    width: 18px;
    height: 6px;
    border-radius: 3px;
    margin-right: 6.75px;
    vertical-align: middle;
  }
  .ref::before {
    background: var(--reference-wash);
    border-top: 2px solid var(--reference);
    height: 8px;
  }
  .you::before {
    background: var(--you);
    height: 3px;
  }
  .slow::before {
    background: var(--reference);
    height: 5px;
    width: 10px;
  }
  .fast::before {
    background: var(--you);
    height: 5px;
    width: 10px;
  }
</style>
