<script lang="ts">
  // The waterfall: ~2 MHz of spectrum around the dongle's centre, newest at the top. Click a
  // signal to listen to it (scroll to fine-tune); FM broadcast, airband AM or narrow FM.
  import { onMount } from 'svelte';
  import { radio, bandOf, demodFor, mhz, type Demod } from '../lib/radio.svelte';
  import { palette, Waterfall, fit, tickStep, themeColors } from '../lib/radio/draw';
  import { Play, Square, Volume2, Star, X, Contrast } from '@lucide/svelte';

  let box = $state<HTMLDivElement | null>(null);
  let specCanvas = $state<HTMLCanvasElement | null>(null);
  let waterCanvas = $state<HTMLCanvasElement | null>(null);
  let freqText = $state((radio.settings.freq / 1e6).toFixed(3));
  let hover = $state<number | null>(null);
  let bookmarkName = $state('');
  let naming = $state(false);

  const s = $derived(radio.settings);
  const pal = $derived(palette(s.floor, s.ceiling));
  const band = $derived(bandOf(s.freq));
  const BW: Record<Demod, number> = { wfm: 200e3, nfm: 12.5e3, am: 10e3 };

  // The tuned frequency's place on the waterfall (when it's in view).
  const lo = $derived(radio.center - radio.rate / 2);
  const markLeft = $derived(((s.freq - BW[s.demod] / 2 - lo) / radio.rate) * 100);
  const markWidth = $derived((BW[s.demod] / radio.rate) * 100);
  const inView = $derived(radio.mode === 'spectrum' && s.freq > lo && s.freq < lo + radio.rate);

  $effect(() => {
    if (document.activeElement?.classList.contains('freq')) return;
    freqText = (s.freq / 1e6).toFixed(3);
  });

  let last: Float32Array | null = null;
  let lastCenter = 0;

  onMount(() => {
    const wf = new Waterfall(waterCanvas!);
    wf.clear();
    const off = radio.onRow((row, center) => {
      wf.push(row, pal, lastCenter !== 0 && center !== lastCenter);
      lastCenter = center;
      // A steadier trace: a little of the previous rows stays in.
      const db = Float32Array.from(row, (v) => v / 2 - 130);
      if (!last || last.length !== db.length) last = db;
      else for (let i = 0; i < db.length; i++) last[i] = last[i] * 0.5 + db[i] * 0.5;
      drawSpectrum();
    });
    const ro = new ResizeObserver(() => drawSpectrum());
    ro.observe(box!);
    return () => {
      off();
      ro.disconnect();
    };
  });

  function drawSpectrum() {
    const c = specCanvas;
    if (!c) return;
    const { w, h, dpr } = fit(c);
    const ctx = c.getContext('2d')!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    const col = themeColors(c);
    const top = s.ceiling + 10;
    const bottom = s.floor - 10;
    const y = (db: number) => ((top - db) / (top - bottom)) * (h - 18);
    ctx.font = '10px ui-sans-serif, system-ui';
    ctx.lineWidth = 1;
    // dB grid.
    for (let db = Math.ceil(bottom / 10) * 10; db <= top; db += 10) {
      ctx.strokeStyle = col.grid;
      ctx.beginPath();
      ctx.moveTo(0, y(db) + 0.5);
      ctx.lineTo(w, y(db) + 0.5);
      ctx.stroke();
      ctx.fillStyle = col.text;
      ctx.fillText(`${db}`, 4, y(db) - 2);
    }
    // Frequency ticks.
    const span = radio.rate;
    const step = tickStep(span, w);
    for (let f = Math.ceil(lo / step) * step; f < lo + span; f += step) {
      const x = ((f - lo) / span) * w;
      ctx.strokeStyle = col.grid;
      ctx.beginPath();
      ctx.moveTo(x + 0.5, 0);
      ctx.lineTo(x + 0.5, h - 16);
      ctx.stroke();
      ctx.fillStyle = col.text;
      const t = (f / 1e6).toFixed(step >= 1e6 ? 0 : step >= 100e3 ? 1 : step >= 10e3 ? 2 : 3);
      ctx.fillText(t, x - ctx.measureText(t).width / 2, h - 4);
    }
    if (!last) return;
    const n = last.length;
    ctx.beginPath();
    for (let i = 0; i < n; i++) {
      const x = (i / (n - 1)) * w;
      const yy = Math.max(0, Math.min(h - 18, y(last[i])));
      if (i === 0) ctx.moveTo(x, yy);
      else ctx.lineTo(x, yy);
    }
    ctx.strokeStyle = col.accent;
    ctx.lineWidth = 1.2;
    ctx.stroke();
    ctx.lineTo(w, h - 18);
    ctx.lineTo(0, h - 18);
    ctx.closePath();
    ctx.globalAlpha = 0.12;
    ctx.fillStyle = col.accent;
    ctx.fill();
    ctx.globalAlpha = 1;
  }

  function freqAt(e: MouseEvent) {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    return lo + ((e.clientX - r.left) / r.width) * radio.rate;
  }

  function snap(f: number) {
    const step = bandOf(f)?.step ?? 1e3;
    return Math.round(f / Math.min(step, 100e3)) * Math.min(step, 100e3);
  }

  function clickTune(e: MouseEvent) {
    if (radio.mode !== 'spectrum') return;
    const f = snap(freqAt(e));
    const b = bandOf(f);
    void radio.tune(f, b?.demod ?? s.demod);
  }

  function wheel(e: WheelEvent) {
    if (radio.mode !== 'spectrum') return;
    e.preventDefault();
    const step = Math.min(bandOf(s.freq)?.step ?? 1e3, 100e3);
    void radio.tune(s.freq + (e.deltaY > 0 ? -step : step), s.demod);
  }

  function enterFreq(e: Event) {
    e.preventDefault();
    const v = parseFloat(freqText.replace(',', '.'));
    if (!Number.isFinite(v)) return;
    const f = v < 2000 ? v * 1e6 : v;
    void radio.tune(f, demodFor(f) === s.demod || !bandOf(f) ? s.demod : demodFor(f));
  }

  const levelPct = $derived(radio.level == null ? 0 : Math.max(0, Math.min(100, ((radio.level + 100) / 90) * 100)));
  const sqPct = $derived(s.squelch == null ? null : ((s.squelch + 100) / 90) * 100);
</script>

<div class="wf">
  <div class="controls">
    <form class="fq" onsubmit={enterFreq}>
      <input class="input freq mono" bind:value={freqText} title="Frequency in MHz (Enter to tune)" />
      <span class="unit">MHz</span>
    </form>
    <div class="seg" title="How it's demodulated: WFM for FM broadcast, AM for airband and shortwave, NFM for two-way radio">
      {#each ['wfm', 'nfm', 'am'] as d (d)}
        <button class:on={s.demod === d} onclick={() => radio.tune(s.freq, d as Demod)}>{d.toUpperCase()}</button>
      {/each}
    </div>
    <button class="btn sm" class:primary={!radio.listening} disabled={radio.mode !== 'spectrum'} onclick={() => radio.listen(!radio.listening)} title={radio.listening ? 'Stop the sound' : 'Listen'}>
      {#if radio.listening}<Square size={12} /> Stop{:else}<Play size={12} /> Listen{/if}
    </button>
    <label class="vol" title="Volume"><Volume2 size={14} /><input type="range" min="0" max="1.5" step="0.05" value={s.volume} oninput={(e) => radio.setVolume(Number(e.currentTarget.value))} /></label>
    <label class="sq" title="Squelch: silence until a signal is stronger than this (for airband and two-way radio)">
      <input type="checkbox" checked={s.squelch != null} onchange={(e) => radio.setSquelch(e.currentTarget.checked ? Math.round((radio.level ?? -60) + 6) : null)} />
      Squelch
      {#if s.squelch != null}<input type="range" min="-100" max="-10" step="1" value={s.squelch} oninput={(e) => radio.setSquelch(Number(e.currentTarget.value))} />{/if}
    </label>
    <div class="meter" title={radio.level != null ? `${radio.level.toFixed(1)} dBFS${radio.open ? '' : ' (squelched)'}` : 'Signal strength (while listening)'}>
      <div class="lvl" class:shut={!radio.open} style:width="{levelPct}%"></div>
      {#if sqPct != null}<div class="sqmark" style:left="{sqPct}%"></div>{/if}
    </div>
    <span class="spacer"></span>
    {#if naming}
      <form class="fq" onsubmit={(e) => { e.preventDefault(); radio.bookmark(bookmarkName); naming = false; bookmarkName = ''; }}>
        <input class="input sm" placeholder="Name (e.g. Tower)" bind:value={bookmarkName} />
        <button class="btn sm">Save</button>
      </form>
    {:else}
      <button class="icon-btn sm" title="Bookmark this frequency" onclick={() => (naming = true)}><Star size={14} /></button>
    {/if}
  </div>
  {#if band}
    <div class="bandline"><strong>{band.name}</strong> · {band.what}</div>
  {/if}

  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="scope" bind:this={box} onclick={clickTune} onwheel={wheel} onmousemove={(e) => (hover = freqAt(e))} onmouseleave={() => (hover = null)} class:locked={radio.mode !== 'spectrum'}>
    <canvas class="spec" bind:this={specCanvas}></canvas>
    <canvas class="water" bind:this={waterCanvas} width="1024" height="600"></canvas>
    {#if inView}
      <div class="mark" style:left="{markLeft}%" style:width="max({markWidth}%, 2px)"></div>
    {/if}
    {#if hover != null}
      <div class="hover">{mhz(hover)}{bandOf(hover) ? ` · ${bandOf(hover)!.name}` : ''}</div>
    {/if}
  </div>

  <div class="foot">
    <div class="bms">
      {#each s.bookmarks as b (b.freq)}
        <span class="bm" class:on={Math.abs(b.freq - s.freq) < 500}>
          <button onclick={() => radio.tune(b.freq, b.demod, true)} title="{mhz(b.freq)} {b.demod.toUpperCase()}">{b.name}</button>
          <button class="x" title="Forget" onclick={() => radio.unbookmark(b.freq)}><X size={10} /></button>
        </span>
      {:else}
        <span class="muted small">Click a signal to tune; scroll to fine-tune. <Star size={11} /> keeps a frequency here.</span>
      {/each}
    </div>
    <label class="contrast" title="Waterfall contrast: the quietest and loudest levels shown">
      <Contrast size={13} />
      <input type="range" min="-130" max="-60" step="1" value={s.floor} oninput={(e) => radio.save({ floor: Number(e.currentTarget.value) })} />
      <input type="range" min="-70" max="0" step="1" value={s.ceiling} oninput={(e) => radio.save({ ceiling: Number(e.currentTarget.value) })} />
    </label>
  </div>
</div>

<style>
  .wf {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .controls {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }
  .fq {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .freq {
    width: 110px;
    font-size: 15px;
    font-weight: 600;
    text-align: right;
  }
  .unit {
    font-size: 12px;
    color: var(--text-3);
  }
  .seg {
    display: inline-flex;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    overflow: hidden;
  }
  .seg button {
    font: 11.5px var(--font);
    padding: 4px 9px;
    border: 0;
    background: var(--surface-2);
    color: var(--text-2);
    cursor: pointer;
  }
  .seg button.on {
    background: var(--accent);
    color: var(--on-accent);
  }
  .vol,
  .sq,
  .contrast {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 12px;
    color: var(--text-2);
  }
  .vol input,
  .sq input[type='range'] {
    width: 80px;
  }
  .meter {
    position: relative;
    width: 90px;
    height: 8px;
    border-radius: 4px;
    background: var(--surface-3);
    overflow: hidden;
  }
  .lvl {
    height: 100%;
    background: var(--success);
    transition: width 0.15s;
  }
  .lvl.shut {
    background: var(--text-4);
  }
  .sqmark {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 2px;
    background: var(--warning);
  }
  .spacer {
    flex: 1;
  }
  .bandline {
    padding: 4px 12px;
    font-size: 12px;
    color: var(--text-2);
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }
  .scope {
    flex: 1;
    min-height: 0;
    position: relative;
    display: flex;
    flex-direction: column;
    cursor: crosshair;
    background: #000008;
  }
  .scope.locked {
    cursor: default;
  }
  .spec {
    flex: none;
    height: 150px;
    width: 100%;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }
  .water {
    flex: 1;
    min-height: 0;
    width: 100%;
    image-rendering: pixelated;
  }
  .mark {
    position: absolute;
    top: 0;
    bottom: 0;
    background: color-mix(in oklab, var(--accent) 22%, transparent);
    border-left: 1px solid var(--accent);
    border-right: 1px solid var(--accent);
    pointer-events: none;
  }
  .hover {
    position: absolute;
    top: 6px;
    right: 8px;
    padding: 2px 8px;
    border-radius: 6px;
    font: 11.5px var(--mono);
    background: var(--glass);
    color: var(--text);
    pointer-events: none;
  }
  .foot {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 6px 12px;
    border-top: 1px solid var(--border);
    background: var(--surface);
  }
  .bms {
    flex: 1;
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    min-width: 0;
  }
  .bm {
    display: inline-flex;
    align-items: center;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--surface-2);
    font-size: 12px;
  }
  .bm.on {
    border-color: var(--accent);
  }
  .bm button {
    border: 0;
    background: transparent;
    color: var(--text-2);
    cursor: pointer;
    padding: 2px 4px 2px 9px;
    font: inherit;
  }
  .bm .x {
    padding: 2px 7px 2px 2px;
    display: grid;
    place-items: center;
  }
  .contrast input {
    width: 70px;
  }
</style>
