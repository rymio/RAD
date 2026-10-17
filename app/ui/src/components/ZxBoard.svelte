<script lang="ts">
  /** The ZX Spectrum 48K's main board, with the data flowing between its parts as the game runs
   *  (from the emulator's per-frame counts: instruction fetches, memory writes, port I/O). */
  import type { FrameStats } from '../lib/types';

  let { stats, fn = null, live = false }: { stats: FrameStats | null; fn?: string | null; live?: boolean } = $props();

  // Smoothed per-second rates.
  let rate = $state<Record<string, number>>({});
  let last = 0;
  $effect(() => {
    const s = stats;
    if (!s) return;
    const now = performance.now();
    const dt = last ? Math.min(0.2, (now - last) / 1000) : 0.02;
    last = now;
    const per = (v: number) => v * 50;
    const target: Record<string, number> = {
      rom: per(s.fetch_rom),
      ramhi: per(s.fetch_ram_high + s.write_ram_high * 4),
      ramlo: per(s.fetch_ram_low + s.write_ram_low * 4 + s.write_screen * 4),
      screen: per(s.write_screen),
      key: per(s.in_fe),
      spk: per(s.out_fe),
      edge: per(s.in_joy + s.other_io),
      instr: per(s.instructions),
      halts: per(s.halts),
    };
    const k = Math.min(1, dt * 6);
    const next = { ...rate };
    for (const [key, v] of Object.entries(target)) next[key] = (next[key] ?? 0) + (v - (next[key] ?? 0)) * k;
    rate = next;
  });

  /** 0 (idle) … 4 (very busy), from a rate. */
  function level(v: number | undefined, busy: number) {
    if (!v || v < 1) return 0;
    return Math.min(4, Math.ceil((Math.log10(v + 1) / Math.log10(busy + 1)) * 4));
  }
  const fmt = (v: number | undefined) => (!v ? '0' : v >= 1e6 ? `${(v / 1e6).toFixed(1)}M` : v >= 1e3 ? `${(v / 1e3).toFixed(1)}k` : v.toFixed(0));

  // About 17,500 instructions fit in a frame when the CPU never rests.
  const busy = $derived(stats ? Math.min(100, Math.round((stats.instructions / 17500) * 100)) : 0);
  const BORDER = ['#000', '#00d', '#d00', '#d0d', '#0d0', '#0dd', '#dd0', '#ddd'];

  type Link = { id: string; d: string; key: string; busy: number; label: string; always?: boolean };
  const links: Link[] = [
    { id: 'rom', d: 'M 330 245 C 260 260 230 300 205 330', key: 'rom', busy: 1e6, label: 'instructions from ROM' },
    { id: 'ramhi', d: 'M 380 270 C 380 300 380 320 380 352', key: 'ramhi', busy: 1e6, label: 'upper RAM (code & data)' },
    { id: 'cpuula', d: 'M 460 228 L 520 228', key: 'ramlo', busy: 5e5, label: 'CPU ↔ lower RAM, through the ULA' },
    { id: 'ularam', d: 'M 595 270 L 595 342', key: 'video', busy: 1, label: 'screen fetch (always)', always: true },
    { id: 'tv', d: 'M 670 228 C 710 228 720 228 760 228', key: 'video', busy: 1, label: 'video to the TV', always: true },
    { id: 'key', d: 'M 600 186 C 600 130 300 120 200 80', key: 'key', busy: 3000, label: 'keyboard reads (IN $FE)' },
    { id: 'spk', d: 'M 640 186 C 660 120 720 100 760 92', key: 'spk', busy: 3000, label: 'border & speaker (OUT $FE)' },
    { id: 'edge', d: 'M 380 270 C 330 330 300 420 300 470', key: 'edge', busy: 3000, label: 'expansion port (joystick, sound chip)' },
    { id: 'int', d: 'M 520 250 C 490 262 480 262 460 250', key: 'int', busy: 1, label: '50 Hz interrupt', always: true },
  ];
  const lv = (l: Link) => (l.always ? (live ? 2 : 0) : live ? level(rate[l.key], l.busy) : 0);
</script>

<div class="board">
  <svg viewBox="0 0 900 540" role="img" aria-label="The ZX Spectrum 48K's main board, with data flowing between its chips">
    <defs>
      <pattern id="pcb" width="18" height="18" patternUnits="userSpaceOnUse">
        <path d="M 18 0 L 0 0 0 18" fill="none" class="grid" />
      </pattern>
    </defs>
    <rect x="10" y="10" width="880" height="520" rx="18" class="pcb" />
    <rect x="10" y="10" width="880" height="520" rx="18" fill="url(#pcb)" />

    {#each links as l (l.id)}
      <path d={l.d} class="trace" />
      <path d={l.d} class="flow l{lv(l)}" class:out={l.id === 'tv' || l.id === 'spk'}>
        <title>{l.label}: {fmt(l.always ? 50 : rate[l.key])}/s</title>
      </path>
    {/each}

    <!-- Keyboard ribbon -->
    <g transform="translate(60 40)">
      <rect width="190" height="46" rx="6" class="part key" />
      <text x="95" y="20" class="name">Keyboard</text>
      <text x="95" y="36" class="sub">40 keys · 8 × 5 matrix</text>
    </g>
    <!-- Speaker / EAR / MIC -->
    <g transform="translate(760 60)">
      <rect width="110" height="64" rx="6" class="part audio" />
      <text x="55" y="22" class="name">Speaker</text>
      <text x="55" y="38" class="sub">EAR · MIC (tape)</text>
      <text x="55" y="54" class="val">{fmt(rate.spk)}/s</text>
    </g>
    <!-- CPU -->
    <g transform="translate(300 186)">
      <rect width="160" height="84" rx="4" class="chip cpu" />
      {#each Array(10) as _, i}<rect x={10 + i * 15} y="-6" width="6" height="6" class="pin" /><rect x={10 + i * 15} y="84" width="6" height="6" class="pin" />{/each}
      <text x="80" y="28" class="name light">Z80A CPU</text>
      <text x="80" y="46" class="sub light">3.5 MHz</text>
      <text x="80" y="66" class="val light">{live ? `${busy}% busy · ${fmt(rate.instr)} instr/s` : 'idle'}</text>
    </g>
    <!-- ULA -->
    <g transform="translate(520 186)">
      <rect width="150" height="84" rx="4" class="chip ula" />
      {#each Array(9) as _, i}<rect x={10 + i * 15} y="-6" width="6" height="6" class="pin" /><rect x={10 + i * 15} y="84" width="6" height="6" class="pin" />{/each}
      <text x="75" y="28" class="name light">ULA</text>
      <text x="75" y="46" class="sub light">Ferranti · video, keys, sound</text>
      <text x="75" y="66" class="val light">{stats ? 'border' : ''}</text>
      {#if stats}<rect x="105" y="57" width="16" height="12" rx="2" fill={BORDER[stats.border & 7]} stroke="#fff" stroke-width="1" />{/if}
    </g>
    <!-- ROM -->
    <g transform="translate(120 330)">
      <rect width="150" height="60" rx="4" class="chip rom" />
      <text x="75" y="24" class="name light">ROM 16K</text>
      <text x="75" y="42" class="sub light">BASIC · $0000–$3FFF</text>
      <text x="75" y="56" class="val light">{fmt(rate.rom)}/s</text>
    </g>
    <!-- Upper RAM -->
    <g transform="translate(300 352)">
      {#each Array(8) as _, i}<rect x={i * 20} width="16" height="54" rx="2" class="chip ram" />{/each}
      <text x="78" y="72" class="name">RAM 32K</text>
      <text x="78" y="88" class="sub">$8000–$FFFF · the CPU's own</text>
      <text x="78" y="104" class="val">{fmt(rate.ramhi)}/s</text>
    </g>
    <!-- Lower RAM (screen) -->
    <g transform="translate(520 342)">
      {#each Array(8) as _, i}<rect x={i * 19} width="15" height="54" rx="2" class="chip ram low" />{/each}
      <text x="72" y="72" class="name">RAM 16K</text>
      <text x="72" y="88" class="sub">$4000–$7FFF · screen, shared</text>
      <text x="72" y="104" class="val">{fmt(rate.screen)} screen bytes/s</text>
    </g>
    <!-- TV -->
    <g transform="translate(760 196)">
      <rect width="110" height="64" rx="6" class="part tv" />
      <text x="55" y="24" class="name">TV out</text>
      <text x="55" y="42" class="sub">RF modulator</text>
      <text x="55" y="56" class="val">50 frames/s</text>
    </g>
    <!-- Expansion port -->
    <g transform="translate(200 470)">
      <rect width="200" height="34" rx="4" class="part edge" />
      {#each Array(14) as _, i}<rect x={10 + i * 13} y="26" width="8" height="8" class="gold" />{/each}
      <text x="100" y="20" class="name">Expansion port</text>
    </g>
    <text x="490" y="276" class="tag">INT 50 Hz</text>
    <text x="700" y="500" class="tag">{fn ? `running: ${fn}` : live ? '' : 'Play a game to see it work'}</text>
  </svg>
</div>

<style>
  .board {
    width: 100%;
    height: 100%;
    display: grid;
    place-items: center;
  }
  svg {
    width: 100%;
    max-height: 100%;
  }
  .pcb {
    fill: #0f3b26;
  }
  .grid {
    stroke: rgba(255, 255, 255, 0.04);
    stroke-width: 1;
  }
  .trace {
    fill: none;
    stroke: #1f6b45;
    stroke-width: 6;
    stroke-linecap: round;
  }
  .flow {
    fill: none;
    stroke: #ffd75e;
    stroke-linecap: round;
    stroke-dasharray: 6 14;
    opacity: 0;
    transition: opacity 0.3s, stroke-width 0.3s;
  }
  .flow.l1 {
    opacity: 0.35;
    stroke-width: 2;
    animation: flow 2.4s linear infinite;
  }
  .flow.l2 {
    opacity: 0.6;
    stroke-width: 3;
    animation: flow 1.4s linear infinite;
  }
  .flow.l3 {
    opacity: 0.85;
    stroke-width: 4;
    animation: flow 0.8s linear infinite;
  }
  .flow.l4 {
    opacity: 1;
    stroke-width: 5;
    animation: flow 0.45s linear infinite;
    filter: drop-shadow(0 0 4px #ffd75e);
  }
  .flow.out {
    stroke: #7ee2ff;
  }
  @keyframes flow {
    to {
      stroke-dashoffset: -40;
    }
  }
  .chip {
    fill: #17171c;
    stroke: #3c3c48;
  }
  .chip.cpu {
    fill: #1e1e26;
  }
  .chip.ula {
    fill: #242018;
  }
  .chip.rom {
    fill: #1c1c24;
  }
  .chip.ram.low {
    fill: #221a1a;
  }
  .pin {
    fill: #b8b8c0;
  }
  .gold {
    fill: #d9a93a;
  }
  .part {
    fill: #233;
    stroke: #4a6;
  }
  .part.key {
    fill: #222;
  }
  text {
    text-anchor: middle;
    font-family: var(--font, system-ui);
  }
  .name {
    font-size: 15px;
    font-weight: 700;
    fill: #e8f5ec;
  }
  .sub {
    font-size: 11px;
    fill: #9cc7ae;
  }
  .val {
    font-size: 11px;
    fill: #ffd75e;
    font-family: var(--mono);
  }
  .light {
    fill: #f0f0f4;
  }
  .sub.light {
    fill: #a8a8b8;
  }
  .val.light {
    fill: #ffd75e;
  }
  .tag {
    font-size: 11px;
    fill: #9cc7ae;
    font-family: var(--mono);
  }
</style>
