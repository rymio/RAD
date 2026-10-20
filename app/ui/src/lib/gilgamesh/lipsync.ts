// Mouth movement from speech audio. Each sentence's WAV is analysed before it plays, so the
// mouth can move slightly ahead of the sound (as real mouths do). Per 10 ms: loudness opens
// the jaw, more for a high first formant ("ah"); much second-formant energy widens the lips
// ("ee"), little rounds them ("oo"); hiss ("s", "f") nearly closes the mouth. Vowel colour is
// judged against the sentence's own range, so it works for any voice.

export interface Track {
  fps: number;
  jaw: Float32Array;
  wide: Float32Array;
  round: Float32Array;
  /** Loudness 0–1, for nodding along. */
  energy: Float32Array;
}

const FPS = 100;
const WIN = 1024;
/** The mouth leads the sound by this much. */
export const LEAD_S = 0.05;

/** 16-bit PCM or 32-bit float WAV → mono samples. */
export function decodeWav(buf: ArrayBuffer): { rate: number; samples: Float32Array } {
  const v = new DataView(buf);
  const tag = (o: number) => String.fromCharCode(v.getUint8(o), v.getUint8(o + 1), v.getUint8(o + 2), v.getUint8(o + 3));
  if (buf.byteLength < 44 || tag(0) !== 'RIFF' || tag(8) !== 'WAVE') throw new Error('Not a WAV file');
  let o = 12, rate = 0, channels = 1, bits = 16, format = 1;
  let data: [number, number] | null = null;
  while (o + 8 <= buf.byteLength) {
    const id = tag(o), size = v.getUint32(o + 4, true);
    if (id === 'fmt ') {
      format = v.getUint16(o + 8, true);
      channels = v.getUint16(o + 10, true);
      rate = v.getUint32(o + 12, true);
      bits = v.getUint16(o + 22, true);
    } else if (id === 'data') {
      // Streamed WAVs may give a placeholder size: use what's there.
      data = [o + 8, Math.min(size, buf.byteLength - o - 8)];
      break;
    }
    o += 8 + size + (size & 1);
  }
  if (!data || !rate) throw new Error('Unreadable WAV');
  const [start, bytes] = data;
  const step = (bits / 8) * channels;
  const n = Math.floor(bytes / step);
  const out = new Float32Array(n);
  const isFloat = format === 3 || bits === 32;
  for (let i = 0; i < n; i++) {
    const p = start + i * step;
    out[i] = isFloat ? v.getFloat32(p, true) : v.getInt16(p, true) / 32768;
  }
  return { rate, samples: out };
}

/** In-place radix-2 FFT. */
function fft(re: Float32Array, im: Float32Array) {
  const n = re.length;
  for (let i = 1, j = 0; i < n; i++) {
    let bit = n >> 1;
    for (; j & bit; bit >>= 1) j ^= bit;
    j ^= bit;
    if (i < j) {
      [re[i], re[j]] = [re[j], re[i]];
      [im[i], im[j]] = [im[j], im[i]];
    }
  }
  for (let size = 2; size <= n; size <<= 1) {
    const half = size >> 1, ang = (-2 * Math.PI) / size;
    for (let i = 0; i < n; i += size) {
      for (let k = 0; k < half; k++) {
        const wr = Math.cos(ang * k), wi = Math.sin(ang * k);
        const a = i + k, b = a + half;
        const tr = re[b] * wr - im[b] * wi, ti = re[b] * wi + im[b] * wr;
        re[b] = re[a] - tr; im[b] = im[a] - ti;
        re[a] += tr; im[a] += ti;
      }
    }
  }
}

const smoothstep = (a: number, b: number, x: number) => {
  const t = Math.min(1, Math.max(0, (x - a) / (b - a)));
  return t * t * (3 - 2 * t);
};

export function analyse(rate: number, s: Float32Array): Track {
  const hop = Math.round(rate / FPS);
  const frames = Math.max(1, Math.ceil(s.length / hop));
  const hann = new Float32Array(WIN).map((_, i) => 0.5 - 0.5 * Math.cos((2 * Math.PI * i) / (WIN - 1)));
  const re = new Float32Array(WIN), im = new Float32Array(WIN);
  const bin = (hz: number) => Math.min(WIN / 2, Math.round((hz / rate) * WIN));
  const [b0, b1, b2, b3] = [bin(150), bin(900), bin(2400), bin(7000)];
  const db = new Float32Array(frames), f2 = new Float32Array(frames), hiss = new Float32Array(frames), c1 = new Float32Array(frames);
  for (let f = 0; f < frames; f++) {
    const c = f * hop - WIN / 2;
    let sum = 0;
    for (let i = 0; i < WIN; i++) {
      const x = s[c + i] ?? 0;
      sum += x * x;
      re[i] = x * hann[i];
      im[i] = 0;
    }
    db[f] = 10 * Math.log10(sum / WIN + 1e-10);
    fft(re, im);
    let e1 = 0, e2 = 0, e3 = 0, m1 = 0;
    for (let k = b0; k < b3; k++) {
      const p = re[k] * re[k] + im[k] * im[k];
      if (k < b1) {
        e1 += p;
        m1 += p * k;
      } else if (k < b2) e2 += p;
      else e3 += p;
    }
    // In decibels: the spectrum's tilt differs between voices, so only relative values count.
    f2[f] = 10 * Math.log10((e2 + 1e-12) / (e1 + 1e-12));
    hiss[f] = smoothstep(-6, 3, 10 * Math.log10((e3 + 1e-12) / (e1 + e2 + 1e-12)));
    c1[f] = e1 > 0 ? m1 / e1 : 0;
  }
  // Loudness relative to this sentence: its loud parts open fully, 30 dB below is closed.
  const pct = (a: ArrayLike<number>, p: number) => {
    const v = Float32Array.from(a).sort();
    return v[Math.min(v.length - 1, Math.floor(v.length * p))] ?? 0;
  };
  const peak = pct(db, 0.95);
  const floor = peak - 30;
  const loud = (f: number) => Math.min(1, Math.max(0, (db[f] - floor) / (peak - floor - 3)));
  // Vowel colour relative to this sentence's own voiced sounds.
  const voiced: number[] = [];
  for (let f = 0; f < frames; f++) if (loud(f) > 0.45 && hiss[f] < 0.5) voiced.push(f);
  const vf2 = voiced.map((f) => f2[f]), vc1 = voiced.map((f) => c1[f]);
  const [f2lo, f2mid, f2hi] = vf2.length ? [pct(vf2, 0.12), pct(vf2, 0.5), pct(vf2, 0.88)] : [-1, 0, 1];
  const [c1lo, c1hi] = vc1.length ? [pct(vc1, 0.15), pct(vc1, 0.85)] : [0, 1];
  const jaw = new Float32Array(frames), wide = new Float32Array(frames), round = new Float32Array(frames), energy = new Float32Array(frames);
  for (let f = 0; f < frames; f++) {
    const e = loud(f);
    energy[f] = e;
    const h = hiss[f];
    // Higher first formant ("ah") opens the jaw more than "ee" or "oo".
    let open = e ** 0.75 * (0.55 + 0.45 * smoothstep(c1lo, c1hi, c1[f]));
    open = open * (1 - h) + Math.min(open, 0.2) * h;
    jaw[f] = open * 0.7;
    wide[f] = e * Math.max(smoothstep(f2mid, f2hi, f2[f]) * (1 - h), 0.35 * h);
    round[f] = e * smoothstep(f2mid, f2lo, f2[f]) * (1 - h);
  }
  // Muscles: quick to open, slower to close.
  for (const a of [jaw, wide, round, energy]) {
    let y = 0;
    for (let f = 0; f < frames; f++) {
      const k = a[f] > y ? 0.55 : 0.28;
      y += (a[f] - y) * k;
      a[f] = y;
    }
  }
  return { fps: FPS, jaw, wide, round, energy };
}

/** The value of a track at time `t` (seconds), with the lead applied. */
export function sample(a: Float32Array, fps: number, t: number) {
  const x = (t + LEAD_S) * fps;
  const i = Math.floor(x);
  if (i < 0 || i >= a.length) return 0;
  const j = Math.min(a.length - 1, i + 1);
  return a[i] + (a[j] - a[i]) * (x - i);
}
