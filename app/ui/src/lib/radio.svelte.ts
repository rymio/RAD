// Radio (Media → Radio): the owner's RTL-SDR dongle. A waterfall to look at and listen in
// (FM broadcast, airband, two-way radio), a scanner that sweeps a band and lists what's
// transmitting, a local aircraft radar from ADS-B, and a packet analyser (Wireshark-style) for
// the Mode S frames behind it. The dongle runs in RAD's backend (radio.rs, rad-sdr); this keeps
// what it reports, and Gilgamesh sees it (describe()) and can drive it ([radio: …]).

import { invoke, Channel } from '@tauri-apps/api/core';
import { store } from './state.svelte';

export type Demod = 'wfm' | 'nfm' | 'am';
export type Tab = 'waterfall' | 'scanner' | 'subghz' | 'aircraft' | 'packets';
export type Mode = 'spectrum' | 'scan' | 'adsb' | 'bursts';

export interface Band {
  id: string;
  name: string;
  from: number;
  to: number;
  /** How it's listened to, when it's voice or music. */
  demod: Demod | null;
  /** Offered a Listen button in the scanner's list. */
  listen?: boolean;
  /** Channel spacing, for rounding what the scanner finds. */
  step: number;
  /** For Gilgamesh and the tooltip. */
  what: string;
}

/** What lives where (the first match wins, so narrow ones come first). */
export const BANDS: Band[] = [
  { id: 'adsb', name: 'ADS-B / Mode S', from: 1089e6, to: 1091e6, demod: null, step: 1e6, what: 'aircraft transponders (the radar tab decodes them)' },
  { id: 'ais', name: 'AIS (ships)', from: 161.95e6, to: 162.05e6, demod: null, step: 25e3, what: 'ships’ automatic identification (digital)' },
  { id: 'noaa', name: 'NOAA weather radio', from: 162.39e6, to: 162.56e6, demod: 'nfm', step: 25e3, what: 'US weather broadcasts' },
  { id: 'pmr', name: 'PMR446', from: 446.0e6, to: 446.2e6, demod: 'nfm', step: 6.25e3, what: 'licence-free walkie-talkies' },
  { id: 'mw', name: 'AM broadcast (medium wave)', from: 0.52e6, to: 1.71e6, demod: 'am', step: 9e3, what: 'AM radio stations (needs a direct-sampling dongle)' },
  { id: 'sw', name: 'Shortwave', from: 2.3e6, to: 26.1e6, demod: 'am', step: 5e3, what: 'shortwave broadcasts and HF (needs a direct-sampling dongle)' },
  { id: 'cb', name: 'CB radio', from: 26.96e6, to: 27.41e6, demod: 'am', step: 10e3, what: 'citizens band' },
  { id: 'fm', name: 'FM broadcast', from: 87.5e6, to: 108e6, demod: 'wfm', listen: true, step: 100e3, what: 'FM radio stations' },
  { id: 'nav', name: 'Air navigation (VOR / ILS)', from: 108e6, to: 117.975e6, demod: 'am', step: 50e3, what: 'radio beacons for aircraft navigation (Morse idents)' },
  { id: 'air', name: 'Airband', from: 117.975e6, to: 137e6, demod: 'am', listen: true, step: 5e3, what: 'pilots and air traffic control (AM voice)' },
  { id: 'wxsat', name: 'Weather satellites', from: 137e6, to: 138e6, demod: 'nfm', step: 5e3, what: 'NOAA / Meteor weather-satellite downlinks' },
  { id: '2m', name: '2 m amateur', from: 144e6, to: 148e6, demod: 'nfm', step: 12.5e3, what: 'radio amateurs (hams), repeaters, APRS on 144.39/144.8' },
  { id: 'marine', name: 'Marine VHF', from: 156e6, to: 162.025e6, demod: 'nfm', step: 25e3, what: 'ships and harbours (channel 16 = 156.8 MHz, distress)' },
  { id: 'dab', name: 'DAB digital radio', from: 174e6, to: 240e6, demod: null, step: 1e6, what: 'digital radio multiplexes' },
  { id: '70cm', name: '70 cm amateur', from: 430e6, to: 440e6, demod: 'nfm', step: 12.5e3, what: 'radio amateurs, repeaters' },
  { id: 'ism433', name: 'ISM 433 MHz', from: 433.05e6, to: 434.79e6, demod: null, step: 5e3, what: 'remote controls, weather stations, car keys, sensors (short digital bursts)' },
  { id: 'tv', name: 'TV (DVB-T)', from: 470e6, to: 694e6, demod: null, step: 8e6, what: 'digital television' },
  { id: 'ism868', name: 'ISM 868 MHz', from: 863e6, to: 870e6, demod: null, step: 25e3, what: 'smart meters, LoRa, alarms (Europe)' },
  { id: 'ism915', name: 'ISM 915 MHz', from: 902e6, to: 928e6, demod: null, step: 25e3, what: 'LoRa, meters, sensors (Americas, Australia)' },
  { id: 'gsm', name: 'Mobile phones', from: 925e6, to: 960e6, demod: null, step: 200e3, what: 'mobile network downlinks (digital)' },
  { id: 'fpv', name: '1.2/1.3 GHz (FPV video, radar, 23 cm ham)', from: 1080e6, to: 1360e6, demod: null, step: 1e6, what: 'analog FPV video from drones, air-traffic radar, radio amateurs' },
  { id: 'l', name: 'Satellite L-band', from: 1525e6, to: 1660e6, demod: null, step: 5e3, what: 'Inmarsat, Iridium, GPS (digital)' },
];

/** Scanner presets: bands worth sweeping. */
export const PRESETS = [
  { id: 'fm', name: 'FM broadcast', from: 87.5e6, to: 108e6 },
  { id: 'air', name: 'Airband', from: 118e6, to: 137e6 },
  { id: '2m', name: '2 m amateur', from: 144e6, to: 148e6 },
  { id: 'marine', name: 'Marine VHF', from: 156e6, to: 162.1e6 },
  { id: '70cm', name: '70 cm amateur', from: 430e6, to: 440e6 },
  { id: 'pmr', name: 'PMR446 + ISM 433', from: 433e6, to: 447e6 },
  { id: 'vhf', name: 'All VHF (30–300 MHz)', from: 30e6, to: 300e6 },
  { id: 'all', name: 'Everything (24–1766 MHz, slow)', from: 24e6, to: 1766e6 },
] as const;

/** Where the sub-GHz watch can sit (it sees ~2 MHz around the centre). */
export const WATCH = [
  { id: '433', name: '433.92 MHz: remotes, sensors, weather stations', freq: 433.92e6 },
  { id: '315', name: '315 MHz: remotes (US)', freq: 315e6 },
  { id: '868', name: '868.3 MHz: EU sensors, LoRaWAN, ELRS / Crossfire', freq: 868.3e6 },
  { id: 'mesh-eu', name: '869.525 MHz: Meshtastic EU (LongFast)', freq: 869.525e6 },
  { id: 'mesh-us', name: '906.875 MHz: Meshtastic US (LongFast)', freq: 906.875e6 },
  { id: '915', name: '915 MHz: US/AU ISM (LoRa, ELRS, Crossfire, SiK)', freq: 915e6 },
  { id: 'mesh-anz', name: '919.875 MHz: Meshtastic ANZ (LongFast)', freq: 919.875e6 },
  { id: 'as923', name: '923.2 MHz: AS923 (Asia LoRaWAN / Meshtastic)', freq: 923.2e6 },
  { id: 'fpv', name: '1280 MHz: 1.2/1.3 GHz FPV video (drones)', freq: 1280e6 },
] as const;

export interface Burst {
  n: number;
  t: number;
  freq: number;
  bw: number;
  dur_us: number;
  db: number;
  snr: number;
  kind: 'ook' | 'fsk' | 'lora' | 'carrier' | 'wide' | 'continuous' | 'unknown';
  modulation: string;
  sf: number | null;
  lora_bw: number | null;
  baud: number | null;
  deviation: number | null;
  pulses: number[];
  bits: string | null;
  guess: string;
  sig: string;
}

export interface DroneHint {
  level: 'likely' | 'possible';
  what: string;
  evidence: string;
  last: number;
}

export interface LoraNet {
  sf: number;
  bw: number;
  freqs: number[];
  count: number;
  last: number;
  guess: string;
}

export function bandOf(f: number): Band | null {
  return BANDS.find((b) => f >= b.from && f <= b.to) ?? null;
}

/** "98.0 MHz", "121.500 MHz", "1090 MHz". */
export function mhz(f: number, band = bandOf(f)): string {
  const d = band ? (band.step >= 100e3 ? 1 : band.step >= 1e6 ? 0 : 3) : 3;
  return `${(f / 1e6).toFixed(d)} MHz`;
}

/** The demodulation a frequency is usually heard with. */
export function demodFor(f: number): Demod {
  return bandOf(f)?.demod ?? (f < 30e6 ? 'am' : 'nfm');
}

export interface Signal {
  freq: number;
  bw: number;
  db: number;
  snr: number;
  hits: number;
  first: number;
  last: number;
  active: boolean;
  band: Band | null;
}

export interface Aircraft {
  icao: string;
  country: string;
  callsign: string | null;
  category: string | null;
  squawk: string | null;
  emergency: string | null;
  alt: number | null;
  alt_gnss: number | null;
  on_ground: boolean;
  gs: number | null;
  track: number | null;
  vr: number | null;
  ias: number | null;
  tas: number | null;
  heading: number | null;
  lat: number | null;
  lon: number | null;
  km: number | null;
  bearing: number | null;
  msgs: number;
  signal: number;
  first_seen: number;
  last_seen: number;
  pos_seen: number | null;
}

export interface Field {
  group: string;
  name: string;
  value: string;
  start: number;
  len: number;
}

export interface Packet {
  n: number;
  t: number;
  df: number;
  icao: string;
  hex: string;
  signal: number;
  crc: 'ok' | 'corrected' | 'address';
  kind: string;
  info: string;
  fields: Field[];
}

export interface Bookmark {
  freq: number;
  demod: Demod;
  name: string;
}

interface Settings {
  freq: number;
  demod: Demod;
  gain: number | null;
  ppm: number;
  squelch: number | null;
  volume: number;
  lat: number | null;
  lon: number | null;
  bookmarks: Bookmark[];
  scanFrom: number;
  scanTo: number;
  /** Where the sub-GHz watch sits. */
  watch: number;
  threshold: number;
  floor: number;
  ceiling: number;
}

const DEFAULTS: Settings = { freq: 98.0e6, demod: 'wfm', gain: null, ppm: 0, squelch: null, volume: 0.8, lat: null, lon: null, bookmarks: [], scanFrom: 87.5e6, scanTo: 108e6, watch: 433.92e6, threshold: 10, floor: -110, ceiling: -30 };

const MAX_PACKETS = 5000;
const MAX_BURSTS = 2000;
const TRAIL = 240;

type RowListener = (row: Uint8Array, center: number, rate: number) => void;
type SweepListener = (row: Uint8Array, from: number, to: number) => void;

function b64(s: string): Uint8Array {
  const bin = atob(s);
  const a = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) a[i] = bin.charCodeAt(i);
  return a;
}

class Radio {
  tab = $state<Tab>('waterfall');
  /** rtl_tcp's path, or null when rtl-sdr isn't installed; undefined until checked. */
  tool = $state<string | null | undefined>(undefined);
  on = $state(false);
  starting = $state(false);
  error = $state<string | null>(null);
  tuner = $state('');
  gains = $state<number[]>([]);
  settings = $state<Settings>({ ...DEFAULTS });

  mode = $state<Mode>('spectrum');
  center = $state(0);
  rate = $state(2.048e6);
  listening = $state(false);
  level = $state<number | null>(null);
  open = $state(false);
  msps = $state(0);
  scanStep = $state<{ step: number; steps: number } | null>(null);

  signals = $state<Signal[]>([]);
  sweeps = $state(0);
  aircraft = $state<Aircraft[]>([]);
  trails = $state<Record<string, [number, number][]>>({});
  frames = $state(0);
  bad = $state(0);
  frameRate = $state(0);
  packets = $state.raw<Packet[]>([]);
  paused = $state(false);
  /** Selected in the radar / table, and in the packet list (Gilgamesh sees them). */
  selectedAircraft = $state<string | null>(null);
  selectedPacket = $state<number | null>(null);
  emergencies = $state<Aircraft[]>([]);
  bursts = $state.raw<Burst[]>([]);
  selectedBurst = $state<number | null>(null);

  private rowListeners = new Set<RowListener>();
  private sweepListeners = new Set<SweepListener>();
  private loaded = false;
  private saveTimer: ReturnType<typeof setTimeout> | undefined;
  private wanted: Mode = 'spectrum';

  // ---------------------------------------------------------------- setup

  async load() {
    if (this.loaded) return;
    this.loaded = true;
    try {
      const s = await invoke<Partial<Settings>>('radio_settings_get');
      this.settings = { ...DEFAULTS, ...s };
    } catch {
      // Defaults.
    }
    await this.check();
  }

  async check() {
    try {
      const st = await invoke<{ rtl_tcp: string | null; running: boolean }>('radio_status');
      this.tool = st.rtl_tcp;
    } catch {
      this.tool = null;
    }
  }

  save(patch: Partial<Settings>) {
    this.settings = { ...this.settings, ...patch };
    clearTimeout(this.saveTimer);
    this.saveTimer = setTimeout(() => void invoke('radio_settings_set', { settings: $state.snapshot(this.settings) }).catch(() => {}), 600);
  }

  onRow(f: RowListener) {
    this.rowListeners.add(f);
    return () => this.rowListeners.delete(f);
  }

  onSweep(f: SweepListener) {
    this.sweepListeners.add(f);
    return () => this.sweepListeners.delete(f);
  }

  // ---------------------------------------------------------------- on and off

  /** Turns the dongle on (and goes to `mode`). */
  async start(mode: Mode = this.wanted) {
    await this.load();
    this.wanted = mode;
    if (this.on || this.starting) return this.go(mode);
    this.starting = true;
    this.error = null;
    const ch = new Channel<any>();
    ch.onmessage = (m) => this.onEvent(m);
    try {
      await invoke('radio_start', { onEvent: ch });
      this.on = true;
    } catch (e: any) {
      this.error = e?.message ?? String(e);
      await this.check();
    } finally {
      this.starting = false;
    }
  }

  async stop() {
    await invoke('radio_stop').catch(() => {});
    this.on = false;
    this.listening = false;
    this.level = null;
    this.scanStep = null;
  }

  private cmd(c: Record<string, unknown>) {
    if (!this.on) return Promise.resolve();
    return invoke('radio_command', { cmd: c }).catch((e) => store.error(e, 'Radio'));
  }

  private onEvent(m: any) {
    switch (m.type) {
      case 'ready': {
        this.tuner = m.tuner;
        this.gains = m.gains;
        const s = this.settings;
        void this.cmd({ cmd: 'gain', db: s.gain });
        void this.cmd({ cmd: 'ppm', ppm: s.ppm });
        void this.cmd({ cmd: 'squelch', db: s.squelch });
        void this.cmd({ cmd: 'volume', v: s.volume });
        void this.cmd({ cmd: 'location', lat: s.lat, lon: s.lon });
        void this.go(this.wanted);
        break;
      }
      case 'row': {
        const row = b64(m.bins);
        this.center = m.center;
        this.rate = m.rate;
        for (const f of this.rowListeners) f(row, m.center, m.rate);
        break;
      }
      case 'sweep': {
        const row = b64(m.bins);
        for (const f of this.sweepListeners) f(row, m.from, m.to);
        break;
      }
      case 'scan':
        this.scanStep = { step: m.step, steps: m.steps };
        break;
      case 'signals':
        this.sweeps = m.sweeps;
        this.signals = tidy(m.list);
        break;
      case 'status':
        this.mode = m.mode;
        this.center = m.center;
        this.rate = m.rate;
        this.listening = m.listening;
        this.level = m.level;
        this.open = m.open;
        this.msps = m.msps;
        if (m.mode !== 'scan') this.scanStep = null;
        break;
      case 'aircraft': {
        this.aircraft = m.list;
        this.frames = m.frames;
        this.bad = m.bad;
        this.frameRate = m.rate;
        const t = { ...this.trails };
        const alive = new Set<string>();
        for (const a of m.list as Aircraft[]) {
          alive.add(a.icao);
          if (a.lat == null || a.lon == null) continue;
          const tr = t[a.icao] ?? [];
          const last = tr.at(-1);
          if (!last || last[0] !== a.lat || last[1] !== a.lon) t[a.icao] = [...tr.slice(-TRAIL + 1), [a.lat, a.lon]];
        }
        for (const k of Object.keys(t)) if (!alive.has(k)) delete t[k];
        this.trails = t;
        break;
      }
      case 'packets':
        if (!this.paused) {
          const next = this.packets.concat(m.list);
          this.packets = next.length > MAX_PACKETS ? next.slice(next.length - MAX_PACKETS) : next;
        }
        break;
      case 'bursts': {
        const next = this.bursts.concat(m.list);
        this.bursts = next.length > MAX_BURSTS ? next.slice(next.length - MAX_BURSTS) : next;
        break;
      }
      case 'emergency': {
        const a = m.aircraft as Aircraft;
        this.emergencies = [a, ...this.emergencies.filter((x) => x.icao !== a.icao)].slice(0, 10);
        store.toast('error', `${a.callsign ?? a.icao}: squawk ${a.squawk ?? ''}`, a.emergency ?? 'Emergency code');
        void import('./gilgamesh/gilgamesh.svelte').then(({ gilgamesh }) => gilgamesh.announce(`On the radar: ${a.callsign ?? `aircraft ${a.icao}`} is squawking ${a.squawk ?? 'an emergency'}${a.emergency ? `, ${a.emergency}` : ''}.`, 'concerned'));
        break;
      }
      case 'error':
        this.error = m.message;
        break;
      case 'stopped':
        this.on = false;
        this.listening = false;
        this.level = null;
        this.scanStep = null;
        break;
    }
  }

  // ---------------------------------------------------------------- what it does

  private go(mode: Mode) {
    const s = this.settings;
    if (mode === 'adsb') return this.cmd({ cmd: 'adsb' });
    if (mode === 'bursts') return this.cmd({ cmd: 'bursts', center: s.watch });
    if (mode === 'scan') return this.cmd({ cmd: 'scan', from: s.scanFrom, to: s.scanTo, threshold: s.threshold });
    return this.cmd({ cmd: 'tune', freq: s.freq, demod: s.demod }).then(() => this.cmd({ cmd: 'listen', on: this.listening }));
  }

  /** Shows (and, with `listen`, plays) a frequency. */
  async tune(freq: number, demod: Demod = demodFor(freq), listen?: boolean) {
    this.save({ freq: Math.round(freq), demod });
    this.tab = 'waterfall';
    this.wanted = 'spectrum';
    if (!this.on) {
      // Starting sends the frequency and this once the dongle is ready.
      if (listen !== undefined) this.listening = listen;
      return this.start('spectrum');
    }
    await this.cmd({ cmd: 'tune', freq: Math.round(freq), demod });
    if (listen !== undefined) await this.listen(listen);
  }

  async listen(on: boolean) {
    this.listening = on;
    await this.cmd({ cmd: 'listen', on });
  }

  async scan(from = this.settings.scanFrom, to = this.settings.scanTo) {
    this.save({ scanFrom: from, scanTo: to });
    this.tab = 'scanner';
    this.wanted = 'scan';
    if (!this.on) return this.start('scan');
    await this.cmd({ cmd: 'scan', from, to, threshold: this.settings.threshold });
  }

  /** Watches ~2 MHz around `center` for short transmissions and identifies them. */
  async watch(center = this.settings.watch) {
    this.save({ watch: Math.round(center) });
    this.tab = 'subghz';
    this.wanted = 'bursts';
    if (!this.on) return this.start('bursts');
    await this.cmd({ cmd: 'bursts', center: Math.round(center) });
  }

  clearBursts() {
    this.bursts = [];
    this.selectedBurst = null;
  }

  /** LoRa heard lately, by spreading factor and bandwidth: mesh networks, LoRaWAN, RC links. */
  get lora(): LoraNet[] {
    const since = Date.now() - 10 * 60_000;
    const by = new Map<string, LoraNet>();
    for (const b of this.bursts) {
      if (b.kind !== 'lora' || b.t < since || !b.sf || !b.lora_bw) continue;
      const k = `${b.sf}/${b.lora_bw}`;
      const n = by.get(k) ?? { sf: b.sf, bw: b.lora_bw, freqs: [], count: 0, last: 0, guess: b.guess };
      n.count++;
      n.last = Math.max(n.last, b.t);
      const f = Math.round(b.freq / 25e3) * 25e3;
      if (!n.freqs.includes(f)) n.freqs.push(f);
      by.set(k, n);
    }
    return [...by.values()].sort((a, b) => b.last - a.last);
  }

  /** Signs of drones (and RC models): frequency-hopping control links, ExpressLRS-like LoRa,
   *  1.2/1.3 GHz FPV video. Most consumer drones (DJI…) are on 2.4/5.8 GHz, out of the dongle's
   *  reach, so this is what *can* be heard. */
  get drones(): DroneHint[] {
    const now = Date.now();
    const recent = this.bursts.filter((b) => now - b.t < 20_000);
    const out: DroneHint[] = [];
    const groups = new Map<string, Burst[]>();
    for (const b of recent) if ((b.kind === 'lora' || b.kind === 'fsk') && b.sig) groups.set(b.sig, [...(groups.get(b.sig) ?? []), b]);
    for (const [sig, list] of groups) {
      const freqs = new Set(list.map((b) => Math.round(b.freq / 50e3)));
      const span = (Math.max(...list.map((b) => b.t)) - Math.min(...list.map((b) => b.t))) / 1000 || 1;
      const rate = list.length / Math.max(1, span);
      const last = Math.max(...list.map((b) => b.t));
      const f = list[0].freq;
      const ism = (f > 863e6 && f < 870e6) || (f > 902e6 && f < 928e6) || (f > 433e6 && f < 435e6);
      const elrs = list[0].kind === 'lora' && list[0].lora_bw === 500e3 && (list[0].sf ?? 99) <= 9;
      if (elrs && list.length >= 3) {
        out.push({ level: 'likely', what: 'RC control link: ExpressLRS-style LoRa (drone or RC model)', evidence: `${list.length} short LoRa SF${list[0].sf}/500 kHz packets in 20 s on ${freqs.size} frequencies`, last });
      } else if (ism && freqs.size >= 4 && rate >= 2) {
        out.push({ level: 'likely', what: `Frequency-hopping ${list[0].kind === 'lora' ? 'LoRa' : 'FSK'} link: an RC control or telemetry link (ExpressLRS, TBS Crossfire, FrSky R9, SiK telemetry), typical of drones`, evidence: `the same ${sig} signal on ${freqs.size} different frequencies, ${rate.toFixed(1)} bursts/s (only part of the hop set is in view)`, last });
      } else if (ism && list[0].kind === 'fsk' && rate >= 8) {
        out.push({ level: 'possible', what: 'A fast, regular FSK link: telemetry or RC control (drone?), or a busy sensor network', evidence: `${list.length} FSK bursts in 20 s (${rate.toFixed(1)}/s)`, last });
      }
    }
    for (const b of recent) {
      if ((b.kind === 'wide' || b.kind === 'continuous') && b.freq > 1080e6 && b.freq < 1360e6) {
        out.push({ level: 'possible', what: '1.2/1.3 GHz wideband transmission: analog FPV video from a drone, or a radar', evidence: `${b.modulation} at ${mhz(b.freq)}`, last: b.t });
        break;
      }
    }
    if (this.sweeps >= 3) {
      for (const sg of this.signals) {
        if (sg.freq > 1080e6 && sg.freq < 1360e6 && sg.bw >= 1e6 && sg.hits / this.sweeps >= 0.7) {
          out.push({ level: 'possible', what: 'A steady wideband signal at 1.2/1.3 GHz: analog FPV video (drone), or ground radar', evidence: `${mhz(sg.freq)}, ${(sg.bw / 1e6).toFixed(1)} MHz wide, in ${sg.hits} of ${this.sweeps} sweeps`, last: sg.last });
        }
      }
    }
    return out;
  }

  async radar() {
    if (this.tab !== 'packets') this.tab = 'aircraft';
    this.wanted = 'adsb';
    if (!this.on) return this.start('adsb');
    await this.cmd({ cmd: 'adsb' });
  }

  setGain(db: number | null) {
    this.save({ gain: db });
    void this.cmd({ cmd: 'gain', db });
  }

  setPpm(ppm: number) {
    this.save({ ppm });
    void this.cmd({ cmd: 'ppm', ppm });
  }

  setSquelch(db: number | null) {
    this.save({ squelch: db });
    void this.cmd({ cmd: 'squelch', db });
  }

  setVolume(v: number) {
    this.save({ volume: v });
    void this.cmd({ cmd: 'volume', v });
  }

  setLocation(lat: number | null, lon: number | null) {
    this.save({ lat, lon });
    void this.cmd({ cmd: 'location', lat, lon });
  }

  clearSignals() {
    this.signals = [];
    this.sweeps = 0;
    void this.cmd({ cmd: 'clear_signals' });
  }

  clearPackets() {
    this.packets = [];
    this.selectedPacket = null;
  }

  bookmark(name: string) {
    const s = this.settings;
    const b: Bookmark = { freq: s.freq, demod: s.demod, name: name.trim() || mhz(s.freq) };
    this.save({ bookmarks: [...s.bookmarks.filter((x) => Math.abs(x.freq - b.freq) > 1e3), b].sort((a, b) => a.freq - b.freq) });
  }

  unbookmark(freq: number) {
    this.save({ bookmarks: this.settings.bookmarks.filter((x) => x.freq !== freq) });
  }

  /** Saves the packets as AVR (one frame per line, `*HEX;`, what dump1090 / Virtual Radar read) or JSON. */
  async export(kind: 'avr' | 'json' | 'aircraft' | 'signals' | 'bursts') {
    const stamp = new Date().toISOString().slice(0, 19).replace(/[:T]/g, '-');
    let name: string;
    let text: string;
    if (kind === 'avr') {
      name = `packets-${stamp}.avr`;
      text = this.packets.map((p) => `*${p.hex};`).join('\n') + '\n';
    } else if (kind === 'json') {
      name = `packets-${stamp}.json`;
      text = JSON.stringify(this.packets, null, 1);
    } else if (kind === 'aircraft') {
      name = `aircraft-${stamp}.csv`;
      text = 'icao,callsign,country,squawk,alt_ft,gs_kt,track,vr_fpm,lat,lon,km,msgs\n' + this.aircraft.map((a) => [a.icao, a.callsign ?? '', `"${a.country}"`, a.squawk ?? '', a.alt ?? '', a.gs?.toFixed(0) ?? '', a.track?.toFixed(0) ?? '', a.vr ?? '', a.lat?.toFixed(5) ?? '', a.lon?.toFixed(5) ?? '', a.km?.toFixed(1) ?? '', a.msgs].join(',')).join('\n') + '\n';
    } else if (kind === 'bursts') {
      name = `subghz-${stamp}.csv`;
      text = 'time,mhz,kind,bandwidth_khz,duration_us,snr_db,sf,lora_bw_khz,baud,deviation_hz,bits,modulation,guess\n' + this.bursts.map((b) => [new Date(b.t).toISOString(), (b.freq / 1e6).toFixed(4), b.kind, (b.bw / 1e3).toFixed(0), b.dur_us, b.snr.toFixed(1), b.sf ?? '', b.lora_bw ? b.lora_bw / 1e3 : '', b.baud ?? '', b.deviation ?? '', `"${(b.bits ?? '').split(' ')[0]}"`, `"${b.modulation}"`, `"${b.guess}"`].join(',')).join('\n') + '\n';
    } else {
      name = `signals-${stamp}.csv`;
      text = 'mhz,band,bandwidth_khz,peak_dbfs,snr_db,hits,active\n' + this.signals.map((s) => [(s.freq / 1e6).toFixed(4), `"${s.band?.name ?? ''}"`, (s.bw / 1e3).toFixed(1), s.db.toFixed(1), s.snr.toFixed(1), s.hits, s.active].join(',')).join('\n') + '\n';
    }
    try {
      const path = await invoke<string>('radio_export', { name, text });
      store.toast('success', 'Saved', path);
    } catch (e) {
      store.error(e, 'Couldn’t save');
    }
  }

  // ---------------------------------------------------------------- Gilgamesh

  /** What Gilgamesh knows about the radio: what's on, what was found, the aircraft and packets. */
  describe(): string {
    if (!this.on && store.route.kind !== 'radio') return '';
    const s = this.settings;
    const lines: string[] = [];
    if (!this.on) {
      lines.push(`THE OWNER'S RADIO (an RTL-SDR dongle, Media → Radio) is open but off.${this.error ? ` Last problem: ${this.error}` : ''}${this.tool === null ? ' rtl-sdr (rtl_tcp) isn’t installed on this computer.' : ''}`);
    } else {
      const band = bandOf(s.freq);
      const what =
        this.mode === 'adsb'
          ? 'on 1090 MHz, decoding aircraft (ADS-B)'
          : this.mode === 'bursts'
          ? `watching ${mhz(s.watch)} ±1 MHz for short transmissions (sub-GHz identification, LoRa, drone links)`
          : this.mode === 'scan'
            ? `scanning ${mhz(s.scanFrom)}–${mhz(s.scanTo)}${this.scanStep ? ` (step ${this.scanStep.step + 1} of ${this.scanStep.steps})` : ''}`
            : `${this.listening ? 'listening to' : 'showing'} ${mhz(s.freq)}${band ? ` (${band.name}: ${band.what})` : ''}, ${s.demod.toUpperCase()}${this.listening && this.level != null ? `, signal ${this.level.toFixed(0)} dBFS${this.open ? '' : ' (squelched: nothing heard)'}` : ''}`;
      lines.push(`THE OWNER'S RADIO (an RTL-SDR dongle, ${this.tuner} tuner; Media → Radio) is on: ${what}. Gain ${s.gain == null ? 'automatic' : `${s.gain} dB`}.`);
    }
    if (this.signals.length) {
      const list = [...this.signals].sort((a, b) => b.snr - a.snr).slice(0, 40);
      lines.push(
        `FREQUENCIES THE SCANNER FOUND over ${mhz(s.scanFrom)}–${mhz(s.scanTo)} (${this.sweeps} sweeps; strongest first): ` +
          list.map((x) => `${mhz(x.freq, x.band)}${x.band ? ` ${x.band.name}` : ''} (${x.snr.toFixed(0)} dB over the noise, ${Math.round(x.bw / 1e3)} kHz wide, in ${x.hits}/${this.sweeps} sweeps${x.active ? ', on now' : ''})`).join('; ') +
          '.',
      );
    }
    if (this.aircraft.length || this.mode === 'adsb') {
      const here = s.lat != null && s.lon != null ? `receiver at ${s.lat.toFixed(3)}, ${s.lon.toFixed(3)}` : 'the receiver’s place isn’t set, so no distances';
      const ac = this.aircraft.slice(0, 30).map((a) => {
        const bits = [
          `${a.callsign ?? 'no callsign yet'} (ICAO ${a.icao}, ${a.country}${a.category ? `, ${a.category}` : ''})`,
          a.on_ground ? 'on the ground' : a.alt != null ? `${a.alt.toLocaleString('en')} ft${a.vr ? (a.vr > 0 ? ` climbing ${a.vr} ft/min` : ` descending ${-a.vr} ft/min`) : ''}` : '',
          a.gs != null ? `${a.gs.toFixed(0)} kt` : '',
          a.track != null ? `heading ${a.track.toFixed(0)}° (${compass(a.track)})` : '',
          a.km != null && a.bearing != null ? `${a.km.toFixed(0)} km ${compass(a.bearing)} of the receiver` : a.lat != null ? `at ${a.lat.toFixed(3)}, ${a.lon!.toFixed(3)}` : 'position not decoded yet',
          a.squawk ? `squawk ${a.squawk}` : '',
          a.emergency ? `EMERGENCY: ${a.emergency}` : '',
        ].filter(Boolean);
        return bits.join(', ');
      });
      lines.push(`AIRCRAFT HEARD (ADS-B, ${here}; ${this.frames} frames, ${this.frameRate}/s): ${ac.length ? ac.join('; ') : 'none yet (the antenna needs a clear view of the sky; planes are heard up to ~300 km)'}.`);
    }
    if (this.bursts.length || this.mode === 'bursts') {
      // Grouped: the same kind of transmitter, with how often and where it was heard.
      const groups = new Map<string, { b: Burst; n: number; freqs: Set<number>; last: number }>();
      for (const x of this.bursts.slice(-400)) {
        const key = `${x.kind}|${x.sig}|${x.guess}`;
        const g = groups.get(key) ?? { b: x, n: 0, freqs: new Set<number>(), last: 0 };
        g.n++;
        g.freqs.add(Math.round(x.freq / 5e3) * 5e3);
        if (x.t >= g.last) (g.last = x.t), (g.b = x);
        groups.set(key, g);
      }
      const by: Record<string, number> = {};
      for (const x of this.bursts) by[x.kind] = (by[x.kind] ?? 0) + 1;
      const top = [...groups.values()].sort((x, y) => y.last - x.last).slice(0, 12);
      lines.push(
        `SUB-GHZ WATCH around ${mhz(s.watch)} (short transmissions identified by modulation; ${this.bursts.length} so far: ${Object.entries(by).map(([k, n]) => `${n} ${k}`).join(', ') || 'none yet'}). By kind, latest first: ` +
          (top.length
            ? top
                .map(({ b: x, n, freqs, last }) => {
                  const fs = [...freqs].sort((p, q) => p - q);
                  return `${n}× ${x.modulation} on ${fs.slice(0, 5).map((f) => mhz(f)).join(', ')}${fs.length > 5 ? ` (+${fs.length - 5} more)` : ''}, ${x.dur_us >= 1000 ? `${(x.dur_us / 1000).toFixed(1)} ms` : `${x.dur_us} µs`} each, up to ${x.snr.toFixed(0)} dB SNR${x.bits ? `, last bits ${x.bits.split(' ')[0]}` : ''}, last at ${new Date(last).toLocaleTimeString()}: ${x.guess}`;
                })
                .join('; ')
            : 'nothing heard yet') +
          '.',
      );
      const nets = this.lora;
      if (nets.length) lines.push(`LORA HEARD (last 10 min): ${nets.map((n) => `SF${n.sf}/${n.bw / 1e3} kHz, ${n.count} packets on ${n.freqs.map((f) => mhz(f)).join(', ')}: ${n.guess}`).join('; ')}. (Detected and measured, not decoded: reading Meshtastic messages needs a full LoRa decoder, which the radio doesn't have.)`);
    }
    const dr = this.drones;
    if (dr.length) lines.push(`DRONE / RC SIGNS: ${dr.map((d) => `${d.level.toUpperCase()}: ${d.what} (${d.evidence})`).join('; ')}.`);
    if (this.mode === 'bursts' || dr.length) lines.push('About drones: this dongle hears 24–1766 MHz, so it can catch sub-GHz RC and telemetry links (433/868/915 MHz) and 1.2/1.3 GHz analog FPV video, but not DJI and most consumer drones, nor Remote ID (2.4/5.8 GHz Wi-Fi and Bluetooth). Say so when it matters.');
    const bsel = this.bursts.find((b) => b.n === this.selectedBurst);
    if (bsel) lines.push(`THE BURST THE OWNER SELECTED: ${mhz(bsel.freq)}, ${bsel.modulation}, ${(bsel.bw / 1e3).toFixed(0)} kHz wide, ${bsel.dur_us} µs, ${bsel.snr.toFixed(0)} dB SNR${bsel.bits ? `, bits ${bsel.bits}` : ''}${bsel.pulses.length ? `, pulses/gaps (µs) ${bsel.pulses.slice(0, 40).join(' ')}` : ''}: ${bsel.guess}.`);
    const sel = this.aircraft.find((a) => a.icao === this.selectedAircraft);
    if (sel) lines.push(`The owner has selected ${sel.callsign ?? sel.icao} on the radar.`);
    const pk = this.packets.find((p) => p.n === this.selectedPacket);
    if (pk) lines.push(`THE PACKET THE OWNER SELECTED (#${pk.n}, DF${pk.df}, ${pk.hex}): ${pk.fields.map((f) => `${f.name}: ${f.value}`).join('; ')}.`);
    else if (this.packets.length) {
      const by: Record<string, number> = {};
      for (const p of this.packets) by[p.kind] = (by[p.kind] ?? 0) + 1;
      lines.push(`PACKETS captured: ${this.packets.length} Mode S frames (${Object.entries(by).map(([k, n]) => `${n} ${k}`).join(', ')}).`);
    }
    lines.push(
      'You can work the radio: [radio: listen | 98.0] plays a frequency in MHz (the band decides FM or AM; add "| am", "| fm" or "| nfm" to choose), [radio: tune | 121.5] shows it without sound, [radio: mute], [radio: scan | fm] sweeps a band (fm, airband, marine, 2m, 70cm, vhf, or "118-137") and lists what transmits, [radio: aircraft] starts the aircraft radar, [radio: watch | 433.92] watches a frequency for short transmissions and identifies them (remotes, sensors, LoRa / Meshtastic, drone RC links; also "| meshtastic", "| 868", "| 915", "| fpv"), [radio: packets] shows the packet analyser, [radio: off]. Explain what the owner hears and sees: what a band is for, what a signal is, how ADS-B and Mode S work, what an aircraft is doing.',
    );
    return lines.join('\n');
  }
}

function compass(deg: number) {
  return ['N', 'NE', 'E', 'SE', 'S', 'SW', 'W', 'NW'][Math.round(deg / 45) % 8];
}

/** The scanner's list as the owner sees it: rounded to each band's channels, one per channel. */
function tidy(list: Omit<Signal, 'band'>[]): Signal[] {
  const out = new Map<number, Signal>();
  for (const s of list) {
    const band = bandOf(s.freq);
    const step = band?.step ?? 5e3;
    const freq = Math.round(s.freq / step) * step;
    const prev = out.get(freq);
    if (!prev) out.set(freq, { ...s, freq, band });
    else
      out.set(freq, {
        ...prev,
        db: Math.max(prev.db, s.db),
        snr: Math.max(prev.snr, s.snr),
        bw: Math.max(prev.bw, s.bw),
        hits: Math.max(prev.hits, s.hits),
        first: Math.min(prev.first, s.first),
        last: Math.max(prev.last, s.last),
        active: prev.active || s.active,
      });
  }
  return [...out.values()].sort((a, b) => a.freq - b.freq);
}

export const radio = new Radio();
