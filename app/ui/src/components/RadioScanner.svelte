<script lang="ts">
  // The scanner: sweeps a band over and over (2 MHz at a time) and lists what transmits: each
  // frequency with its band, how wide, how strong and how often it was heard. FM broadcast and
  // airband can be listened to from the list; anything can be opened in the waterfall.
  import { onMount } from 'svelte';
  import { radio, PRESETS, mhz, bandOf } from '../lib/radio.svelte';
  import { palette, Waterfall, fit, tickStep, themeColors } from '../lib/radio/draw';
  import { Play, AudioWaveform, Star, Trash2, Download, Square, ScanSearch } from '@lucide/svelte';

  let waterCanvas = $state<HTMLCanvasElement | null>(null);
  let axis = $state<HTMLCanvasElement | null>(null);
  let fromText = $state((radio.settings.scanFrom / 1e6).toString());
  let toText = $state((radio.settings.scanTo / 1e6).toString());
  let activeOnly = $state(false);
  let sort = $state<'freq' | 'snr'>('freq');

  const s = $derived(radio.settings);
  const pal = $derived(palette(s.floor, s.ceiling));
  const scanning = $derived(radio.on && radio.mode === 'scan');
  const preset = $derived(PRESETS.find((p) => p.from === s.scanFrom && p.to === s.scanTo)?.id ?? 'custom');
  const list = $derived(
    radio.signals.filter((x) => !activeOnly || x.active).sort((a, b) => (sort === 'snr' ? b.snr - a.snr : a.freq - b.freq)),
  );

  onMount(() => {
    const wf = new Waterfall(waterCanvas!);
    wf.clear();
    let lastRange = '';
    const off = radio.onSweep((row, from, to) => {
      const r = `${from}-${to}`;
      wf.push(row, pal, lastRange !== '' && r !== lastRange);
      lastRange = r;
    });
    const ro = new ResizeObserver(drawAxis);
    ro.observe(axis!);
    return () => {
      off();
      ro.disconnect();
    };
  });

  $effect(() => {
    void s.scanFrom;
    void s.scanTo;
    void radio.signals;
    drawAxis();
  });

  function drawAxis() {
    const c = axis;
    if (!c) return;
    const { w, h, dpr } = fit(c);
    const ctx = c.getContext('2d')!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    const col = themeColors(c);
    const span = s.scanTo - s.scanFrom;
    const step = tickStep(span, w);
    ctx.font = '10px ui-sans-serif, system-ui';
    for (let f = Math.ceil(s.scanFrom / step) * step; f <= s.scanTo; f += step) {
      const x = ((f - s.scanFrom) / span) * w;
      ctx.fillStyle = col.grid;
      ctx.fillRect(x, 0, 1, 5);
      ctx.fillStyle = col.text;
      const t = (f / 1e6).toFixed(step >= 1e6 ? 0 : 1);
      ctx.fillText(t, Math.min(w - ctx.measureText(t).width, Math.max(0, x - ctx.measureText(t).width / 2)), h - 3);
    }
    // What was found, as ticks.
    for (const sig of radio.signals) {
      const x = ((sig.freq - s.scanFrom) / span) * w;
      ctx.fillStyle = sig.active ? col.accent : col.text;
      ctx.fillRect(x - 1, 5, 2, 6);
    }
  }

  function choose(id: string) {
    const p = PRESETS.find((x) => x.id === id);
    if (!p) return;
    fromText = (p.from / 1e6).toString();
    toText = (p.to / 1e6).toString();
    void radio.scan(p.from, p.to);
  }

  function go(e?: Event) {
    e?.preventDefault();
    const a = parseFloat(fromText) * 1e6;
    const b = parseFloat(toText) * 1e6;
    if (!(a > 0 && b > a)) return;
    void radio.scan(a, b);
  }

  function ago(t: number) {
    const s = Math.round((Date.now() - t) / 1000);
    return s < 60 ? `${s}s` : s < 3600 ? `${Math.round(s / 60)}m` : `${Math.round(s / 3600)}h`;
  }
</script>

<div class="sc">
  <form class="controls" onsubmit={go}>
    <select class="input sm" value={preset} onchange={(e) => choose(e.currentTarget.value)}>
      {#each PRESETS as p (p.id)}<option value={p.id}>{p.name}</option>{/each}
      <option value="custom">Custom range</option>
    </select>
    <input class="input sm mono num" bind:value={fromText} title="From (MHz)" /> – <input class="input sm mono num" bind:value={toText} title="To (MHz)" /> MHz
    <label class="thr" title="How far above the noise a signal must stand to be listed">
      Sensitivity
      <input type="range" min="5" max="25" step="1" value={30 - s.threshold} onchange={(e) => { radio.save({ threshold: 30 - Number(e.currentTarget.value) }); go(); }} />
    </label>
    {#if scanning}
      <button type="button" class="btn sm" onclick={() => radio.tune(s.freq, s.demod)}><Square size={12} /> Stop</button>
    {:else}
      <button class="btn sm primary"><ScanSearch size={13} /> Scan</button>
    {/if}
    <span class="spacer"></span>
    {#if scanning && radio.scanStep}
      <div class="prog" title="This sweep"><div style:width="{((radio.scanStep.step + 1) / radio.scanStep.steps) * 100}%"></div></div>
    {/if}
    <span class="muted small">{radio.sweeps} sweeps · {radio.signals.length} found</span>
  </form>

  <div class="view">
    <canvas class="water" bind:this={waterCanvas} width="2048" height="300"></canvas>
    <canvas class="axis" bind:this={axis}></canvas>
  </div>

  <div class="listbar">
    <span class="section-title">Frequencies</span>
    <label class="small"><input type="checkbox" bind:checked={activeOnly} /> On now only</label>
    <label class="small">Sort <select class="input sm" bind:value={sort}><option value="freq">by frequency</option><option value="snr">by strength</option></select></label>
    <span class="spacer"></span>
    <button class="icon-btn sm" title="Save the list (CSV)" disabled={!radio.signals.length} onclick={() => radio.export('signals')}><Download size={13} /></button>
    <button class="icon-btn sm" title="Forget what was found" disabled={!radio.signals.length} onclick={() => radio.clearSignals()}><Trash2 size={13} /></button>
  </div>
  <div class="list">
    <table>
      <thead><tr><th>Frequency</th><th>What</th><th>Width</th><th>Strength</th><th>Heard</th><th>Last</th><th></th></tr></thead>
      <tbody>
        {#each list as x (x.freq)}
          <tr class:dim={!x.active}>
            <td class="mono f">{mhz(x.freq, x.band)}</td>
            <td title={x.band?.what ?? ''}>{x.band?.name ?? '—'}</td>
            <td class="mono">{x.bw >= 1e6 ? `${(x.bw / 1e6).toFixed(1)} MHz` : `${Math.round(x.bw / 1e3)} kHz`}</td>
            <td><span class="bar"><span style:width="{Math.min(100, (x.snr / 50) * 100)}%"></span></span> <span class="mono small">{x.snr.toFixed(0)} dB</span></td>
            <td class="mono small">{x.hits}/{radio.sweeps}</td>
            <td class="small muted">{x.active ? 'now' : ago(x.last)}</td>
            <td class="acts">
              {#if x.band?.listen}
                <button class="btn sm primary" onclick={() => radio.tune(x.freq, x.band!.demod!, true)} title="Listen ({x.band!.demod!.toUpperCase()})"><Play size={11} /> Listen</button>
              {/if}
              <button class="icon-btn sm" title="Open in the waterfall" onclick={() => radio.tune(x.freq, bandOf(x.freq)?.demod ?? 'nfm')}><AudioWaveform size={13} /></button>
              <button class="icon-btn sm" title="Bookmark" onclick={() => { radio.save({ freq: x.freq, demod: x.band?.demod ?? 'nfm' }); radio.bookmark(x.band ? `${x.band.name} ${mhz(x.freq, x.band)}` : mhz(x.freq)); }}><Star size={12} /></button>
            </td>
          </tr>
        {:else}
          <tr><td colspan="7" class="empty">{scanning ? 'Listening across the band… transmitters show up here as they are heard (airband and two-way radio talk only now and then, so let it run).' : 'Pick a band and press Scan.'}</td></tr>
        {/each}
      </tbody>
    </table>
  </div>
</div>

<style>
  .controls :global(select.input),
  .listbar :global(select.input) {
    width: auto;
  }
  .sc {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .controls {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
    font-size: 12.5px;
    color: var(--text-2);
  }
  .num {
    width: 76px;
  }
  .thr {
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }
  .thr input {
    width: 90px;
  }
  .spacer {
    flex: 1;
  }
  .prog {
    width: 120px;
    height: 6px;
    border-radius: 3px;
    background: var(--surface-3);
    overflow: hidden;
  }
  .prog div {
    height: 100%;
    background: var(--accent);
  }
  .view {
    flex: none;
    height: 34%;
    min-height: 120px;
    display: flex;
    flex-direction: column;
    background: #000008;
  }
  .water {
    flex: 1;
    min-height: 0;
    width: 100%;
    image-rendering: pixelated;
  }
  .axis {
    flex: none;
    height: 26px;
    width: 100%;
    background: var(--surface);
  }
  .listbar {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 6px 12px;
    border-top: 1px solid var(--border);
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }
  .listbar label {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    color: var(--text-2);
  }
  .list {
    flex: 1;
    min-height: 0;
    overflow: auto;
    background: var(--surface);
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12.5px;
  }
  th {
    position: sticky;
    top: 0;
    background: var(--surface-2);
    text-align: left;
    font-weight: 500;
    color: var(--text-3);
    padding: 5px 10px;
    border-bottom: 1px solid var(--border);
  }
  td {
    padding: 4px 10px;
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
  }
  tr.dim td {
    color: var(--text-3);
  }
  .f {
    font-weight: 600;
  }
  .bar {
    display: inline-block;
    width: 60px;
    height: 6px;
    border-radius: 3px;
    background: var(--surface-3);
    overflow: hidden;
    vertical-align: middle;
  }
  .bar span {
    display: block;
    height: 100%;
    background: var(--success);
  }
  .acts {
    text-align: right;
  }
  .acts > :global(*) {
    vertical-align: middle;
  }
  .empty {
    color: var(--text-3);
    padding: 18px 12px;
    white-space: normal;
  }
</style>
