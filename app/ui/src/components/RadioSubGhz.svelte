<script lang="ts">
  // Sub-GHz: the dongle sits on one frequency (433.92, 868, 915/923 MHz, a Meshtastic channel…)
  // and every short transmission in the ~2 MHz around it is caught and identified by its
  // modulation: OOK remotes and sensors (with their pulses and bits), FSK, LoRa chirps (spreading
  // factor and bandwidth: Meshtastic, LoRaWAN, ExpressLRS). Above the list: signs of drones.
  import { onMount } from 'svelte';
  import { radio, WATCH, mhz, type Burst } from '../lib/radio.svelte';
  import { palette, Waterfall } from '../lib/radio/draw';
  import { Antenna, Download, Trash2, Plane, Network, Info } from '@lucide/svelte';

  let waterCanvas = $state<HTMLCanvasElement | null>(null);
  let freqText = $state((radio.settings.watch / 1e6).toFixed(3));
  let kind = $state<string>('all');
  let now = $state(Date.now());

  const s = $derived(radio.settings);
  const pal = $derived(palette(s.floor, s.ceiling));
  const preset = $derived(WATCH.find((w) => Math.abs(w.freq - s.watch) < 1e3)?.id ?? 'custom');
  const list = $derived(radio.bursts.filter((b) => kind === 'all' || b.kind === kind).slice(-500).reverse());
  const sel = $derived(radio.bursts.find((b) => b.n === radio.selectedBurst) ?? null);
  // Re-read every couple of seconds so old signs fade.
  const drones = $derived((void now, radio.drones));
  const lora = $derived((void now, radio.lora));

  onMount(() => {
    const wf = new Waterfall(waterCanvas!);
    wf.clear();
    const off = radio.onRow((row) => {
      if (radio.mode === 'bursts') wf.push(row, pal);
    });
    const t = setInterval(() => (now = Date.now()), 2000);
    return () => {
      off();
      clearInterval(t);
    };
  });

  function go(e?: Event) {
    e?.preventDefault();
    const v = parseFloat(freqText);
    if (!Number.isFinite(v)) return;
    void radio.watch(v < 2000 ? v * 1e6 : v);
  }

  function choose(id: string) {
    const w = WATCH.find((x) => x.id === id);
    if (!w) return;
    freqText = (w.freq / 1e6).toFixed(3);
    void radio.watch(w.freq);
  }

  const dur = (us: number) => (us >= 1e6 ? `${(us / 1e6).toFixed(2)} s` : us >= 1000 ? `${(us / 1000).toFixed(1)} ms` : `${us} µs`);
  const time = (t: number) => new Date(t).toLocaleTimeString(undefined, { hour12: false });

  /** The pulse train of an OOK burst, as rectangles. */
  function train(b: Burst) {
    const total = b.pulses.reduce((a, x) => a + x, 0) || 1;
    let x = 0;
    return b.pulses.map((d, i) => {
      const r = { x: (x / total) * 1000, w: Math.max(0.5, (d / total) * 1000), on: i % 2 === 0 };
      x += d;
      return r;
    });
  }
</script>

<div class="sg">
  <form class="controls" onsubmit={go}>
    <Antenna size={14} />
    <select class="input sm" value={preset} onchange={(e) => choose(e.currentTarget.value)}>
      {#each WATCH as w (w.id)}<option value={w.id}>{w.name}</option>{/each}
      <option value="custom">Custom frequency</option>
    </select>
    <input class="input sm mono num" bind:value={freqText} title="Centre (MHz); about ±1 MHz around it is watched" /> MHz
    <button class="btn sm primary">{radio.mode === 'bursts' ? 'Move' : 'Watch'}</button>
    <span class="spacer"></span>
    <span class="muted small">{radio.bursts.length} transmissions</span>
    <button class="icon-btn sm" title="Save the list (CSV)" disabled={!radio.bursts.length} onclick={() => radio.export('bursts')}><Download size={13} /></button>
    <button class="icon-btn sm" title="Clear" disabled={!radio.bursts.length} onclick={() => radio.clearBursts()}><Trash2 size={13} /></button>
  </form>

  <canvas class="water" bind:this={waterCanvas} width="1024" height="160"></canvas>

  <div class="cards">
    <div class="card" class:alert={drones.some((d) => d.level === 'likely')}>
      <div class="ch"><Plane size={13} /> Drones & RC</div>
      {#each drones as d (d.what)}
        <div class="hint"><span class="lvl {d.level}">{d.level}</span> {d.what}<small>{d.evidence}</small></div>
      {:else}
        <div class="muted small">No signs right now. Watch 868 / 915 MHz for RC control links (ExpressLRS, Crossfire), 433 / 915 MHz for telemetry, 1280 MHz for 1.2/1.3 GHz FPV video, or sweep 1080–1360 MHz in the scanner.</div>
      {/each}
      <div class="note"><Info size={11} /> DJI and most consumer drones, and Remote ID, are on 2.4 / 5.8 GHz: above this dongle’s 1.766 GHz.</div>
    </div>
    <div class="card">
      <div class="ch"><Network size={13} /> LoRa & mesh</div>
      {#each lora as n (n.sf + '/' + n.bw)}
        <div class="hint"><b>SF{n.sf} / {n.bw / 1e3} kHz</b> · {n.count} packets · {n.freqs.slice(0, 4).map((f) => mhz(f)).join(', ')}<small>{n.guess}</small></div>
      {:else}
        <div class="muted small">No LoRa yet. Meshtastic: EU 869.525, US 906.875, ANZ 919.875 MHz; Asia (AS923) around 920–925 MHz. Packets are identified (spreading factor, bandwidth), not decoded.</div>
      {/each}
    </div>
  </div>

  <div class="listbar">
    <span class="section-title">Transmissions</span>
    <select class="input sm" bind:value={kind}>
      <option value="all">All kinds</option>
      <option value="ook">OOK (remotes, sensors)</option>
      <option value="fsk">FSK</option>
      <option value="lora">LoRa</option>
      <option value="carrier">Carriers</option>
      <option value="wide">Wideband</option>
      <option value="unknown">Not identified</option>
    </select>
  </div>
  <div class="split">
    <div class="list">
      <table>
        <thead><tr><th>Time</th><th>Frequency</th><th>Kind</th><th>Width</th><th>Length</th><th>SNR</th><th>What it probably is</th></tr></thead>
        <tbody>
          {#each list as b (b.n)}
            <tr class="k-{b.kind}" class:on={b.n === radio.selectedBurst} onclick={() => (radio.selectedBurst = b.n === radio.selectedBurst ? null : b.n)}>
              <td class="mono small">{time(b.t)}</td>
              <td class="mono">{(b.freq / 1e6).toFixed(4)}</td>
              <td><span class="tag">{b.kind === 'lora' ? `LoRa SF${b.sf}` : b.kind.toUpperCase()}</span></td>
              <td class="mono">{(b.bw / 1e3).toFixed(0)} kHz</td>
              <td class="mono">{dur(b.dur_us)}</td>
              <td class="mono">{b.snr.toFixed(0)}</td>
              <td class="truncate g" title={b.guess}>{b.guess}</td>
            </tr>
          {:else}
            <tr><td colspan="7" class="empty">{radio.mode === 'bursts' ? 'Listening… devices like weather stations send every 30–60 s, remotes only when pressed: let it run, or press a remote near the antenna.' : 'Pick a frequency and press Watch.'}</td></tr>
          {/each}
        </tbody>
      </table>
    </div>
    {#if sel}
      <div class="detail">
        <h3>{mhz(sel.freq)} · {time(sel.t)}</h3>
        <p class="mod">{sel.modulation}</p>
        <p>{sel.guess}</p>
        <div class="kv">
          <span>Width</span><b>{(sel.bw / 1e3).toFixed(0)} kHz</b>
          <span>Length</span><b>{dur(sel.dur_us)}</b>
          <span>Strength</span><b>{sel.db.toFixed(0)} dBFS, {sel.snr.toFixed(0)} dB over the noise</b>
          {#if sel.sf}<span>LoRa</span><b>SF{sel.sf}, {(sel.lora_bw ?? 0) / 1e3} kHz (symbol {((2 ** sel.sf / (sel.lora_bw ?? 1)) * 1e3).toFixed(2)} ms)</b>{/if}
          {#if sel.baud}<span>Rate</span><b>{sel.baud.toLocaleString()} baud</b>{/if}
          {#if sel.deviation}<span>Deviation</span><b>±{(sel.deviation / 1e3).toFixed(1)} kHz</b>{/if}
        </div>
        {#if sel.pulses.length}
          <div class="sub">Pulses ({sel.pulses.length / 2 | 0})</div>
          <svg class="train" viewBox="0 0 1000 30" preserveAspectRatio="none">
            {#each train(sel) as r, i (i)}
              {#if r.on}<rect x={r.x} y="4" width={r.w} height="22" />{/if}
            {/each}
            <line x1="0" y1="26" x2="1000" y2="26" />
          </svg>
          <div class="mono tiny">{sel.pulses.slice(0, 60).join(' ')}{sel.pulses.length > 60 ? ' …' : ''} µs</div>
        {/if}
        {#if sel.bits}
          <div class="sub">Bits</div>
          <div class="mono tiny sel">{sel.bits}</div>
        {/if}
      </div>
    {/if}
  </div>
</div>

<style>
  .controls :global(select.input),
  .listbar :global(select.input) {
    width: auto;
  }
  .sg {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--surface);
  }
  .controls {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
    font-size: 12.5px;
    color: var(--text-2);
  }
  .num {
    width: 90px;
  }
  .spacer {
    flex: 1;
  }
  .water {
    flex: none;
    width: 100%;
    height: 110px;
    image-rendering: pixelated;
    background: #000008;
  }
  .cards {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
  }
  .card {
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    padding: 8px 10px;
    font-size: 12.5px;
    background: var(--surface-2);
    display: flex;
    flex-direction: column;
    gap: 5px;
    max-height: 150px;
    overflow: auto;
  }
  .card.alert {
    border-color: var(--warning);
    background: var(--warning-soft);
  }
  .ch {
    display: flex;
    align-items: center;
    gap: 6px;
    font-weight: 600;
  }
  .hint small {
    display: block;
    color: var(--text-3);
    font-size: 11.5px;
  }
  .lvl {
    font-size: 10.5px;
    text-transform: uppercase;
    font-weight: 600;
    padding: 0 5px;
    border-radius: 4px;
    margin-right: 3px;
  }
  .lvl.likely {
    background: var(--warning);
    color: #000;
  }
  .lvl.possible {
    background: var(--surface-3);
    color: var(--text-2);
  }
  .note {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 11px;
    color: var(--text-3);
  }
  .listbar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 12px;
    border-bottom: 1px solid var(--border);
  }
  .split {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .list {
    flex: 1;
    min-width: 0;
    overflow: auto;
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
    table-layout: fixed;
  }
  th {
    position: sticky;
    top: 0;
    background: var(--surface-2);
    text-align: left;
    font-weight: 500;
    color: var(--text-3);
    padding: 4px 8px;
    border-bottom: 1px solid var(--border);
  }
  th:nth-child(1) {
    width: 70px;
  }
  th:nth-child(2) {
    width: 80px;
  }
  th:nth-child(3) {
    width: 82px;
  }
  th:nth-child(4),
  th:nth-child(5) {
    width: 64px;
  }
  th:nth-child(6) {
    width: 40px;
  }
  td {
    padding: 3px 8px;
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
    cursor: pointer;
  }
  tr.on td {
    background: var(--accent-soft);
  }
  .tag {
    font-size: 10.5px;
    font-weight: 600;
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--surface-3);
  }
  .k-lora .tag {
    background: #d4f1ee;
    color: #0d5c55;
  }
  .k-ook .tag {
    background: #fbe3cc;
    color: #7a3d00;
  }
  .k-fsk .tag {
    background: #dae8f8;
    color: #1d4a7a;
  }
  .g {
    color: var(--text-2);
  }
  .empty {
    color: var(--text-3);
    padding: 16px 10px;
    white-space: normal;
    cursor: default;
  }
  .detail {
    width: 320px;
    flex: none;
    overflow: auto;
    padding: 10px 12px;
    border-left: 1px solid var(--border);
    font-size: 12.5px;
    user-select: text;
  }
  .detail h3 {
    margin: 0 0 4px;
    font-size: 14px;
  }
  .detail p {
    margin: 0 0 6px;
    line-height: 1.4;
  }
  .mod {
    font-weight: 500;
  }
  .kv {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 3px 10px;
    margin: 6px 0;
  }
  .kv span {
    color: var(--text-3);
  }
  .kv b {
    font-weight: 500;
  }
  .sub {
    margin-top: 8px;
    font-weight: 600;
    font-size: 12px;
  }
  .train {
    width: 100%;
    height: 34px;
    margin: 4px 0;
  }
  .train rect {
    fill: var(--accent);
  }
  .train line {
    stroke: var(--border-strong);
    stroke-width: 1;
  }
  .tiny {
    font-size: 11px;
    color: var(--text-2);
    word-break: break-all;
  }
  .sel {
    user-select: all;
  }
</style>
