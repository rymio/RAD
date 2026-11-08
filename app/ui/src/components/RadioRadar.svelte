<script lang="ts">
  // The aircraft radar: what ADS-B says about the planes around, drawn around the receiver
  // (north up, range rings), with their trails; the table below has the numbers.
  import { radio, type Aircraft } from '../lib/radio.svelte';
  import { fit, themeColors } from '../lib/radio/draw';
  import { MapPin, Download, Plane, TriangleAlert } from '@lucide/svelte';

  let canvas = $state<HTMLCanvasElement | null>(null);
  let range = $state(200);
  let latText = $state(radio.settings.lat?.toString() ?? '');
  let lonText = $state(radio.settings.lon?.toString() ?? '');
  let tick = $state(0);
  let sort = $state<'km' | 'alt' | 'callsign' | 'last'>('km');

  const s = $derived(radio.settings);
  const placed = $derived(radio.aircraft.filter((a) => a.lat != null && a.lon != null));
  // Around the receiver, or around the planes when its place isn't known.
  const center = $derived.by((): [number, number] | null => {
    if (s.lat != null && s.lon != null) return [s.lat, s.lon];
    if (!placed.length) return null;
    return [placed.reduce((t, a) => t + a.lat!, 0) / placed.length, placed.reduce((t, a) => t + a.lon!, 0) / placed.length];
  });
  const sel = $derived(radio.aircraft.find((a) => a.icao === radio.selectedAircraft) ?? null);
  const rows = $derived(
    [...radio.aircraft].sort((a, b) =>
      sort === 'alt' ? (b.alt ?? -1e9) - (a.alt ?? -1e9) : sort === 'callsign' ? (a.callsign ?? a.icao).localeCompare(b.callsign ?? b.icao) : sort === 'last' ? b.last_seen - a.last_seen : (a.km ?? 1e9) - (b.km ?? 1e9),
    ),
  );

  $effect(() => {
    const t = setInterval(() => (tick = (tick + 1) % 1000), 500);
    return () => clearInterval(t);
  });

  $effect(() => {
    void radio.aircraft;
    void radio.trails;
    void sel;
    void range;
    void center;
    void tick;
    draw();
  });

  function project(lat: number, lon: number, c: [number, number]): [number, number] {
    return [(lon - c[1]) * Math.cos((c[0] * Math.PI) / 180) * 111.32, (lat - c[0]) * 110.57];
  }

  function altColor(a: Aircraft) {
    if (a.on_ground) return 'hsl(0 0% 55%)';
    const alt = Math.max(0, Math.min(40000, a.alt ?? 0));
    return `hsl(${30 + (alt / 40000) * 240} 85% 52%)`;
  }

  let hits: { icao: string; x: number; y: number }[] = [];

  function draw() {
    const cv = canvas;
    if (!cv) return;
    const { w, h, dpr } = fit(cv);
    const ctx = cv.getContext('2d')!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    const col = themeColors(cv);
    const cx = w / 2;
    const cy = h / 2;
    const R = Math.min(w, h) / 2 - 18;
    const k = R / range;
    // Rings and compass.
    ctx.strokeStyle = col.grid;
    ctx.fillStyle = col.text;
    ctx.font = '10px ui-sans-serif, system-ui';
    ctx.lineWidth = 1;
    for (let i = 1; i <= 4; i++) {
      const r = (R * i) / 4;
      ctx.beginPath();
      ctx.arc(cx, cy, r, 0, Math.PI * 2);
      ctx.stroke();
      ctx.fillText(`${Math.round((range * i) / 4)} km`, cx + 3, cy - r - 3);
    }
    ctx.beginPath();
    ctx.moveTo(cx - R, cy);
    ctx.lineTo(cx + R, cy);
    ctx.moveTo(cx, cy - R);
    ctx.lineTo(cx, cy + R);
    ctx.stroke();
    for (const [t, x, y] of [['N', cx - 3, cy - R - 5], ['S', cx - 3, cy + R + 13], ['E', cx + R + 5, cy + 4], ['W', cx - R - 14, cy + 4]] as const) ctx.fillText(t, x, y);
    // The receiver.
    if (s.lat != null) {
      ctx.fillStyle = col.accent;
      ctx.beginPath();
      ctx.arc(cx, cy, 4, 0, Math.PI * 2);
      ctx.fill();
    }
    hits = [];
    const c = center;
    if (!c) return;
    // Trails.
    for (const a of placed) {
      const tr = radio.trails[a.icao];
      if (!tr || tr.length < 2) continue;
      ctx.beginPath();
      tr.forEach(([la, lo], i) => {
        const [x, y] = project(la, lo, c);
        const px = cx + x * k;
        const py = cy - y * k;
        if (i === 0) ctx.moveTo(px, py);
        else ctx.lineTo(px, py);
      });
      ctx.strokeStyle = altColor(a);
      ctx.globalAlpha = 0.45;
      ctx.lineWidth = 1.5;
      ctx.stroke();
      ctx.globalAlpha = 1;
    }
    // The aircraft.
    for (const a of placed) {
      const [x, y] = project(a.lat!, a.lon!, c);
      const px = cx + x * k;
      const py = cy - y * k;
      if (px < -20 || py < -20 || px > w + 20 || py > h + 20) continue;
      hits.push({ icao: a.icao, x: px, y: py });
      const emergency = !!a.emergency || ['7500', '7600', '7700'].includes(a.squawk ?? '');
      const stale = Date.now() - (a.pos_seen ?? 0) > 30_000;
      ctx.save();
      ctx.translate(px, py);
      ctx.rotate((((a.track ?? 0) as number) * Math.PI) / 180);
      ctx.beginPath();
      // A small aeroplane, nose up (north) before turning.
      ctx.moveTo(0, -8);
      ctx.lineTo(1.6, -2);
      ctx.lineTo(8, 2);
      ctx.lineTo(8, 3.5);
      ctx.lineTo(1.6, 1.5);
      ctx.lineTo(1.2, 6);
      ctx.lineTo(3.5, 8);
      ctx.lineTo(-3.5, 8);
      ctx.lineTo(-1.2, 6);
      ctx.lineTo(-1.6, 1.5);
      ctx.lineTo(-8, 3.5);
      ctx.lineTo(-8, 2);
      ctx.lineTo(-1.6, -2);
      ctx.closePath();
      ctx.fillStyle = emergency && tick % 2 ? col.danger : altColor(a);
      ctx.globalAlpha = stale ? 0.4 : 1;
      ctx.fill();
      if (a.icao === radio.selectedAircraft) {
        ctx.strokeStyle = col.strong;
        ctx.lineWidth = 1.5;
        ctx.stroke();
      }
      ctx.restore();
      ctx.globalAlpha = stale ? 0.5 : 1;
      ctx.fillStyle = emergency ? col.danger : col.strong;
      ctx.font = '600 11px ui-sans-serif, system-ui';
      ctx.fillText(a.callsign ?? a.icao, px + 11, py - 2);
      ctx.font = '10px ui-sans-serif, system-ui';
      ctx.fillStyle = col.text;
      const fl = a.on_ground ? 'GND' : a.alt != null ? `FL${String(Math.round(a.alt / 100)).padStart(3, '0')}` : '';
      ctx.fillText(`${fl}${a.vr ? (a.vr > 256 ? '↑' : a.vr < -256 ? '↓' : '') : ''} ${a.gs != null ? Math.round(a.gs) + 'kt' : ''}`, px + 11, py + 10);
      ctx.globalAlpha = 1;
    }
  }

  function click(e: MouseEvent) {
    const r = canvas!.getBoundingClientRect();
    const x = e.clientX - r.left;
    const y = e.clientY - r.top;
    let best: string | null = null;
    let bd = 22 * 22;
    for (const h of hits) {
      const d = (h.x - x) ** 2 + (h.y - y) ** 2;
      if (d < bd) {
        bd = d;
        best = h.icao;
      }
    }
    radio.selectedAircraft = best;
  }

  function wheel(e: WheelEvent) {
    e.preventDefault();
    const steps = [25, 50, 100, 200, 300, 400, 600];
    const i = steps.indexOf(range);
    range = steps[Math.max(0, Math.min(steps.length - 1, (i < 0 ? 3 : i) + (e.deltaY > 0 ? 1 : -1)))];
  }

  function place(e: Event) {
    e.preventDefault();
    const la = parseFloat(latText);
    const lo = parseFloat(lonText);
    if (latText.trim() === '' && lonText.trim() === '') return radio.setLocation(null, null);
    if (!(Math.abs(la) <= 90 && Math.abs(lo) <= 180)) return;
    radio.setLocation(la, lo);
  }

  const ago = (t: number) => `${Math.max(0, Math.round((Date.now() - t) / 1000))}s`;
</script>

<div class="rd">
  <div class="controls">
    <span class="stat"><Plane size={13} /> {radio.aircraft.length} aircraft · {placed.length} with position</span>
    <span class="muted small">{radio.frames.toLocaleString()} frames · {radio.frameRate}/s</span>
    <span class="spacer"></span>
    <form class="loc" onsubmit={place} title="Where the antenna is: for distances, range rings, and positions from a single message">
      <MapPin size={13} />
      <input class="input sm mono" placeholder="Latitude" bind:value={latText} />
      <input class="input sm mono" placeholder="Longitude" bind:value={lonText} />
      <button class="btn sm">Set</button>
    </form>
    <select class="input sm" bind:value={range} title="Range (scroll on the radar to zoom)">
      {#each [25, 50, 100, 200, 300, 400, 600] as r (r)}<option value={r}>{r} km</option>{/each}
    </select>
    <button class="icon-btn sm" title="Save the aircraft list (CSV)" disabled={!radio.aircraft.length} onclick={() => radio.export('aircraft')}><Download size={13} /></button>
  </div>
  {#if radio.emergencies.length}
    <div class="emerg"><TriangleAlert size={14} /> {radio.emergencies.map((a) => `${a.callsign ?? a.icao}: squawk ${a.squawk ?? '—'}${a.emergency ? ` (${a.emergency})` : ''}`).join(' · ')}</div>
  {/if}
  <div class="split">
    <div class="scope">
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
      <canvas bind:this={canvas} onclick={click} onwheel={wheel}></canvas>
      {#if !radio.aircraft.length}
        <div class="empty">
          <p><strong>Listening on 1090 MHz…</strong></p>
          <p>Aircraft appear as their transponders are heard, up to ~300 km away with a clear view of the sky. Indoors, or with the small stock antenna, there may be none: put the antenna by a window, as high as you can (a 1090 MHz antenna is ~6.9 cm per element).</p>
        </div>
      {:else if !center}
        <div class="empty"><p>Heard {radio.aircraft.length} aircraft but no positions yet. Positions need two messages (or the receiver's place, above).</p></div>
      {/if}
      {#if sel}
        <div class="card">
          <div class="ct"><strong>{sel.callsign ?? '—'}</strong> <span class="mono muted">{sel.icao}</span></div>
          <div class="cg">
            <span>Country</span><b>{sel.country}</b>
            {#if sel.category}<span>Type</span><b>{sel.category}</b>{/if}
            <span>Altitude</span><b>{sel.on_ground ? 'on the ground' : sel.alt != null ? `${sel.alt.toLocaleString()} ft` : '—'}</b>
            <span>Speed</span><b>{sel.gs != null ? `${Math.round(sel.gs)} kt` : '—'}{sel.ias ? ` (IAS ${sel.ias})` : ''}</b>
            <span>Track</span><b>{sel.track != null ? `${Math.round(sel.track)}°` : '—'}</b>
            <span>Climb</span><b>{sel.vr != null ? `${sel.vr > 0 ? '+' : ''}${sel.vr} ft/min` : '—'}</b>
            <span>Squawk</span><b class:red={['7500', '7600', '7700'].includes(sel.squawk ?? '')}>{sel.squawk ?? '—'}</b>
            {#if sel.km != null}<span>Distance</span><b>{sel.km.toFixed(1)} km at {Math.round(sel.bearing ?? 0)}°</b>{/if}
            {#if sel.lat != null}<span>Position</span><b class="mono">{sel.lat.toFixed(4)}, {sel.lon?.toFixed(4)}</b>{/if}
            <span>Heard</span><b>{sel.msgs} msgs · {sel.signal.toFixed(1)} dBFS · {ago(sel.last_seen)} ago</b>
          </div>
        </div>
      {/if}
    </div>
    <div class="table">
      <table>
        <thead>
          <tr>
            <th><button onclick={() => (sort = 'callsign')}>Callsign</button></th>
            <th>ICAO</th>
            <th>Country</th>
            <th><button onclick={() => (sort = 'alt')}>Alt (ft)</button></th>
            <th>Speed</th>
            <th>Trk</th>
            <th>V/S</th>
            <th><button onclick={() => (sort = 'km')}>Dist</button></th>
            <th>Sqk</th>
            <th><button onclick={() => (sort = 'last')}>Seen</button></th>
          </tr>
        </thead>
        <tbody>
          {#each rows as a (a.icao)}
            <tr class:on={a.icao === radio.selectedAircraft} class:red={!!a.emergency || ['7500', '7600', '7700'].includes(a.squawk ?? '')} onclick={() => (radio.selectedAircraft = a.icao === radio.selectedAircraft ? null : a.icao)}>
              <td class="b">{a.callsign ?? ''}</td>
              <td class="mono">{a.icao}</td>
              <td class="truncate cn">{a.country}</td>
              <td class="mono">{a.on_ground ? 'GND' : (a.alt?.toLocaleString() ?? '')}</td>
              <td class="mono">{a.gs != null ? Math.round(a.gs) : ''}</td>
              <td class="mono">{a.track != null ? Math.round(a.track) : ''}</td>
              <td class="mono">{a.vr ?? ''}</td>
              <td class="mono">{a.km != null ? a.km.toFixed(0) : ''}</td>
              <td class="mono">{a.squawk ?? ''}</td>
              <td class="mono small">{ago(a.last_seen)}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  </div>
</div>

<style>
  .controls :global(select.input) {
    width: auto;
  }
  .rd {
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
    font-size: 12.5px;
  }
  .stat {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-weight: 500;
  }
  .spacer {
    flex: 1;
  }
  .loc {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    color: var(--text-3);
  }
  .loc input {
    width: 92px;
  }
  .emerg {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 12px;
    background: var(--danger-soft);
    color: var(--danger);
    font-size: 12.5px;
    font-weight: 500;
  }
  .split {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .scope {
    flex: 1.4;
    min-height: 200px;
    position: relative;
    background: var(--surface);
  }
  .scope canvas {
    width: 100%;
    height: 100%;
    display: block;
    cursor: pointer;
  }
  .empty {
    position: absolute;
    left: 50%;
    top: 50%;
    transform: translate(-50%, -50%);
    max-width: 380px;
    text-align: center;
    font-size: 12.5px;
    color: var(--text-2);
    background: var(--glass);
    padding: 12px 16px;
    border-radius: var(--r-sm);
    pointer-events: none;
  }
  .empty p {
    margin: 0 0 6px;
    line-height: 1.45;
  }
  .card {
    position: absolute;
    right: 10px;
    top: 10px;
    width: 240px;
    padding: 10px 12px;
    border-radius: var(--r-sm);
    border: 1px solid var(--border);
    background: var(--glass);
    backdrop-filter: blur(8px);
    box-shadow: var(--shadow);
    font-size: 12px;
  }
  .ct {
    font-size: 14px;
    margin-bottom: 6px;
  }
  .cg {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 3px 10px;
  }
  .cg span {
    color: var(--text-3);
  }
  .cg b {
    font-weight: 500;
  }
  .red {
    color: var(--danger) !important;
  }
  .table {
    flex: 1;
    min-height: 100px;
    overflow: auto;
    border-top: 1px solid var(--border);
    background: var(--surface);
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 12px;
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
  th button {
    font: inherit;
    color: inherit;
    border: 0;
    background: none;
    padding: 0;
    cursor: pointer;
  }
  td {
    padding: 3px 8px;
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
    cursor: pointer;
  }
  .cn {
    max-width: 120px;
  }
  tr.on td {
    background: var(--accent-soft);
  }
  .b {
    font-weight: 600;
  }
</style>
