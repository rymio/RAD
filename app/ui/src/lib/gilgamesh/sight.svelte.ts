// Gilgamesh's sight: the camera, when the owner turns it on. Everything runs on this computer.
//
//   camera (getUserMedia) → MediaPipe Face Landmarker (the same model that animates his photo)
//   → where the owner's face is (his eyes follow it), whether they look at him, whether they
//     smile → presence: here, gone, back → a greeting when they come back
//   → now and then a face crop → faceid.rs (SFace) → 128 numbers → is it the owner?
//
// The owner teaches him their face once (a few seconds in front of the camera); only those
// numbers are kept, in the vault. No picture is kept, except one frame sent to the chat model
// when the owner asks him to look at something ([look: …]).
//
// Modes: off; while Gilgamesh is on (the default); or watching for the owner all the time (at
// a low frame rate). Hands-free, he can answer only when he's looked at, so talking to someone
// else doesn't set him off. Someone he doesn't know alone at the computer can make him go quiet,
// or lock RAD. Face recognition is for being friendly, not security: a photo can fool it.

import { untrack } from 'svelte';
import { invoke } from '@tauri-apps/api/core';
import { FaceLandmarker, type FaceLandmarkerResult, type NormalizedLandmark } from '@mediapipe/tasks-vision';
import wasmLoaderPath from '@mediapipe/tasks-vision/vision_wasm_internal.js?url';
import wasmBinaryPath from '@mediapipe/tasks-vision/vision_wasm_internal.wasm?url';
import modelAssetPath from '../../assets/gilgamesh/face_landmarker.task?url';
import { api, rad } from '../api';
import { store } from '../state.svelte';
import { bots } from '../bots.svelte';
import { gilgamesh } from './gilgamesh.svelte';
import { calendarEvents } from './tools';

export type SightMode = 'off' | 'with_gilgamesh' | 'watch';
export type StrangerAction = 'off' | 'quiet' | 'lock';

export interface SightSettings {
  mode: SightMode;
  /** He says hello when the owner comes back after a while. */
  greet: boolean;
  /** Hands-free: he only answers when the owner looked at him while talking. */
  lookToTalk: boolean;
  /** He smiles back. */
  mirror: boolean;
  /** Someone he doesn't know alone at the computer. */
  stranger: StrangerAction;
  /** What he calls the owner when he greets them. */
  name: string;
}

export type Who = 'owner' | 'stranger' | 'unsure';

const KEY = '/v1/app-data/gilgamesh_sight';
const FACE_KEY = '/v1/app-data/gilgamesh_faceid';
const DEFAULTS: SightSettings = { mode: 'off', greet: true, lookToTalk: false, mirror: true, stranger: 'quiet', name: '' };

/** Cosine similarity of SFace numbers: the same person above MATCH (OpenCV suggests 0.363; a
 *  little stricter here), someone else below NO_MATCH, unsure in between. */
const MATCH = 0.42;
const NO_MATCH = 0.3;
/** Frames: quick while he's on (his eyes follow), slow while only watching. */
const FAST_MS = 120;
const SLOW_MS = 600;
const RECOGNISE_MS = 1500;
/** Out of sight this long: gone. Back after this long: worth a hello. */
const GONE_MS = 4000;
const GREET_AFTER_MS = 10 * 60_000;
/** Looking at him counts for a moment after the eyes move away. */
const LOOK_GRACE_MS = 1500;
const ENROL_SAMPLES = 10;

const pad = (n: number) => String(n).padStart(2, '0');
const cos = (a: number[], b: number[]) => a.reduce((s, x, i) => s + x * b[i], 0);

let landmarker: Promise<FaceLandmarker> | null = null;

/** A landmarker of its own, for video (the one in face.ts works on still photos). */
function getLandmarker() {
  landmarker ??= FaceLandmarker.createFromOptions(
    { wasmLoaderPath, wasmBinaryPath },
    { baseOptions: { modelAssetPath, delegate: 'CPU' }, runningMode: 'VIDEO', numFaces: 3, outputFaceBlendshapes: true, outputFacialTransformationMatrixes: true },
  ).catch((e) => {
    landmarker = null;
    throw e;
  });
  return landmarker;
}

let lastTs = 0;

/** Faces in the video's current frame (timestamps must only ever go up, whoever asks). */
async function detect(v: HTMLVideoElement) {
  const lm = await getLandmarker();
  lastTs = Math.max(lastTs + 1, performance.now());
  return lm.detectForVideo(v, lastTs);
}

/** The five points SFace aligns on, in the picture's pixels: eye centres, nose tip, mouth
 *  corners; each pair ordered left to right as seen in the picture. */
function fivePoints(f: NormalizedLandmark[], w: number, h: number): [number, number][] {
  const p = (i: number): [number, number] => [f[i].x * w, f[i].y * h];
  const pair = (a: number, b: number) => (p(a)[0] <= p(b)[0] ? [p(a), p(b)] : [p(b), p(a)]);
  const [e1, e2] = pair(468, 473);
  const [m1, m2] = pair(61, 291);
  return [e1, e2, p(1), m1, m2];
}

/** Head turn from the face's transformation matrix (degrees): 0, 0 is facing the camera. */
function headAngles(r: FaceLandmarkerResult, i: number): { yaw: number; pitch: number } | null {
  const m = r.facialTransformationMatrixes?.[i]?.data;
  if (!m || m.length < 16) return null;
  const deg = 180 / Math.PI;
  return { yaw: Math.atan2(m[8], m[10]) * deg, pitch: Math.asin(Math.max(-1, Math.min(1, -m[9]))) * deg };
}

class Sight {
  settings = $state<SightSettings>({ ...DEFAULTS });
  loaded = $state(false);
  /** The camera is on. */
  on = $state(false);
  starting = $state(false);
  error = $state<string | null>(null);
  stream = $state<MediaStream | null>(null);
  // What he sees now.
  present = $state(false);
  faces = $state(0);
  who = $state<Who | null>(null);
  looking = $state(false);
  smiling = $state(false);
  // Knowing the owner's face.
  enrolled = $state(false);
  enrolling = $state<{ done: number; total: number } | null>(null);
  model = $state<{ installed: boolean; bytes: number; downloading: { done: number; total: number } | null } | null>(null);

  private video: HTMLVideoElement | null = null;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private stopEffect: (() => void) | null = null;
  private embeddings: number[][] = [];
  private lastSeen = 0;
  private lastLook = 0;
  private lastRecognise = 0;
  private recognising = false;
  private ownerAt = 0;
  private strangerRun = 0;
  private strangerHandled = false;
  /** When the owner was last seen before they came back (for the greeting). */
  private awayFrom = 0;
  private greetPending = false;
  private mirrorUntil = 0;
  private lastFrameAt = -1;
  private wanted = false;

  async load() {
    if (this.loaded) return;
    const [s, f] = await Promise.all([api.get<Partial<SightSettings> | null>(KEY).catch(() => null), api.get<{ embeddings: number[][] } | null>(FACE_KEY).catch(() => null)]);
    this.settings = { ...DEFAULTS, ...(s ?? {}) };
    this.embeddings = Array.isArray(f?.embeddings) ? f!.embeddings : [];
    this.enrolled = this.embeddings.length > 0;
    this.loaded = true;
    void this.refreshModel();
  }

  /** Loads, then keeps the camera on whenever the settings and Gilgamesh want it. */
  async start() {
    await this.load();
    this.stopEffect?.();
    this.stopEffect = $effect.root(() => {
      $effect(() => {
        const want = this.settings.mode === 'watch' || (this.settings.mode === 'with_gilgamesh' && gilgamesh.phase !== 'off');
        // Only the settings and his phase decide (a camera that fails isn't retried in a loop).
        untrack(() => void this.want(want));
      });
    });
  }

  unload() {
    this.stopEffect?.();
    this.stopEffect = null;
    this.camera(false);
    this.loaded = false;
  }

  async saveSettings(patch: Partial<SightSettings>) {
    this.settings = { ...this.settings, ...patch };
    await api.put(KEY, this.settings);
  }

  async refreshModel() {
    this.model = await invoke<any>('faceid_status').catch(() => null);
  }

  private async want(on: boolean) {
    this.wanted = on;
    if (on && !this.on && !this.starting) await this.camera(true);
    else if (!on && this.on && !this.enrolling) this.camera(false);
  }

  // ---------------------------------------------------------------- the camera

  private async camera(on: boolean) {
    if (!on) {
      clearTimeout(this.timer);
      for (const t of this.stream?.getTracks() ?? []) t.stop();
      this.stream = null;
      this.video?.remove();
      this.video = null;
      this.on = false;
      this.present = false;
      this.faces = 0;
      this.who = null;
      this.looking = false;
      this.smiling = false;
      return;
    }
    this.starting = true;
    this.error = null;
    try {
      if (!navigator.mediaDevices?.getUserMedia) throw new Error('This window can’t use a camera.');
      const [stream] = await Promise.all([navigator.mediaDevices.getUserMedia({ video: { width: { ideal: 640 }, height: { ideal: 480 }, facingMode: 'user' }, audio: false }), getLandmarker()]);
      const v = document.createElement('video');
      v.muted = true;
      v.playsInline = true;
      v.srcObject = stream;
      // Kept in the page (hidden): some engines don't decode a video that isn't.
      v.style.cssText = 'position:fixed;left:0;top:0;width:2px;height:2px;opacity:0;pointer-events:none';
      document.body.appendChild(v);
      await v.play();
      this.video = v;
      this.stream = stream;
      this.on = true;
      this.lastFrameAt = -1;
      this.loop();
    } catch (e: any) {
      this.error = e?.name === 'NotAllowedError' ? 'RAD isn’t allowed to use the camera: allow it in System Settings → Privacy & Security → Camera.' : (e?.message ?? String(e));
      this.camera(false);
    } finally {
      this.starting = false;
    }
  }

  private loop() {
    clearTimeout(this.timer);
    if (!this.on) return;
    const busy = gilgamesh.phase !== 'off' || !!this.enrolling;
    this.timer = setTimeout(() => {
      void this.frame().finally(() => this.loop());
    }, busy ? FAST_MS : SLOW_MS);
  }

  private async frame() {
    const v = this.video;
    if (!v || v.readyState < 2 || v.currentTime === this.lastFrameAt) return;
    this.lastFrameAt = v.currentTime;
    const r = await detect(v);
    const now = Date.now();
    this.faces = r.faceLandmarks.length;
    if (!this.faces) {
      if (this.present && now - this.lastSeen > GONE_MS) {
        this.present = false;
        this.looking = false;
        this.smiling = false;
      }
      return;
    }
    // The nearest face (the biggest) is the one he looks at.
    const span = (f: NormalizedLandmark[]) => Math.abs(f[468].x - f[473].x);
    let main = 0;
    r.faceLandmarks.forEach((f, i) => span(f) > span(r.faceLandmarks[main]) && (main = i));
    const f = r.faceLandmarks[main];

    if (!this.present) {
      this.present = true;
      // Back after a while (not just RAD starting): a hello, once he knows who it is.
      this.awayFrom = this.lastSeen;
      this.greetPending = this.lastSeen > 0 && now - this.lastSeen > GREET_AFTER_MS;
    }
    this.lastSeen = now;

    // His eyes follow the face (mirrored: the owner moving to their right is to his left).
    const ex = (f[468].x + f[473].x) / 2;
    const ey = (f[468].y + f[473].y) / 2;
    gilgamesh.head.see((0.5 - ex) * 2.4, (ey - 0.42) * 2.4);

    const a = headAngles(r, main);
    this.looking = a ? Math.abs(a.yaw) < 25 && Math.abs(a.pitch) < 22 : true;
    if (this.looking) this.lastLook = now;
    const bs = r.faceBlendshapes?.[main]?.categories ?? [];
    const score = (n: string) => bs.find((c) => c.categoryName === n)?.score ?? 0;
    this.smiling = (score('mouthSmileLeft') + score('mouthSmileRight')) / 2 > 0.55;
    this.mirror(now);

    if (this.enrolled && this.model?.installed && !this.recognising && now - this.lastRecognise > RECOGNISE_MS) {
      this.lastRecognise = now;
      void this.recognise(r.faceLandmarks);
    } else if (!this.enrolled && this.greetPending) {
      this.greetPending = false;
      void this.greet();
    }
  }

  /** He smiles back at a smile, when he's not busy. */
  private mirror(now: number) {
    if (!this.settings.mirror || !this.smiling || now < this.mirrorUntil) return;
    if (!['off', 'listening'].includes(gilgamesh.phase) || gilgamesh.head.mood !== 'neutral') return;
    this.mirrorUntil = now + 20_000;
    gilgamesh.head.mood = 'happy';
    setTimeout(() => {
      if (gilgamesh.head.mood === 'happy' && ['off', 'listening'].includes(gilgamesh.phase)) gilgamesh.head.mood = 'neutral';
    }, 4000);
  }

  // ---------------------------------------------------------------- whose face

  /** A face's numbers (faceid.rs): a small crop around it and its five points. */
  private async embed(f: NormalizedLandmark[]): Promise<number[] | null> {
    const v = this.video;
    if (!v) return null;
    const W = v.videoWidth;
    const H = v.videoHeight;
    const pts = fivePoints(f, W, H);
    const eye = Math.hypot(pts[1][0] - pts[0][0], pts[1][1] - pts[0][1]);
    // Too small to tell (far away).
    if (eye < 22) return null;
    const cx = pts.reduce((s, p) => s + p[0], 0) / 5;
    const cy = pts.reduce((s, p) => s + p[1], 0) / 5;
    const side = eye * 3.4;
    const x0 = Math.max(0, cx - side / 2);
    const y0 = Math.max(0, cy - side / 2);
    const sw = Math.min(W - x0, side);
    const sh = Math.min(H - y0, side);
    const k = Math.min(1, 224 / Math.max(sw, sh));
    const cw = Math.max(1, Math.round(sw * k));
    const ch = Math.max(1, Math.round(sh * k));
    const c = document.createElement('canvas');
    c.width = cw;
    c.height = ch;
    const ctx = c.getContext('2d', { willReadFrequently: true })!;
    ctx.drawImage(v, x0, y0, sw, sh, 0, 0, cw, ch);
    const px = ctx.getImageData(0, 0, cw, ch).data;
    const body = new ArrayBuffer(48 + px.length);
    const dv = new DataView(body);
    dv.setUint32(0, cw, true);
    dv.setUint32(4, ch, true);
    pts.forEach(([x, y], i) => {
      dv.setFloat32(8 + i * 8, (x - x0) * k, true);
      dv.setFloat32(12 + i * 8, (y - y0) * k, true);
    });
    new Uint8Array(body, 48).set(px);
    return invoke<number[]>('faceid_embed', new Uint8Array(body));
  }

  private best(e: number[]) {
    return this.embeddings.reduce((m, x) => Math.max(m, cos(e, x)), -1);
  }

  private async recognise(faces: NormalizedLandmark[][]) {
    this.recognising = true;
    try {
      const scores: number[] = [];
      for (const f of faces) {
        const e = await this.embed(f);
        if (e) scores.push(this.best(e));
      }
      if (!scores.length) return;
      const now = Date.now();
      if (scores.some((s) => s >= MATCH)) {
        this.who = 'owner';
        this.ownerAt = now;
        this.strangerRun = 0;
        this.strangerHandled = false;
        if (this.greetPending) {
          this.greetPending = false;
          void this.greet();
        }
      } else if (scores.every((s) => s < NO_MATCH)) {
        // Three looks in a row before he decides it's someone else.
        if (++this.strangerRun >= 3) {
          this.who = 'stranger';
          this.greetPending = false;
          this.onStranger();
        }
      } else if (this.who !== 'owner' || now - this.ownerAt > 10_000) {
        this.who = 'unsure';
      }
    } catch (e: any) {
      if (e?.code === 'faceid_missing') void this.refreshModel();
      else console.warn('face recognition', e);
    } finally {
      this.recognising = false;
    }
  }

  private onStranger() {
    if (this.strangerHandled || this.settings.stranger === 'off') return;
    this.strangerHandled = true;
    if (this.settings.stranger === 'lock') {
      store.toast('info', 'RAD locked', 'Gilgamesh saw someone he doesn’t know at the computer.', 8000);
      void store.lock();
    } else if (gilgamesh.phase !== 'off') {
      gilgamesh.stop();
      store.toast('info', 'Gilgamesh went quiet', 'He saw someone he doesn’t know at the computer.', 8000);
    }
  }

  // ---------------------------------------------------------------- saying hello

  private async greet() {
    if (!this.settings.greet) return;
    const h = new Date().getHours();
    const hello = h < 5 ? 'Hello' : h < 12 ? 'Good morning' : h < 18 ? 'Good afternoon' : 'Good evening';
    const parts = [`${hello}${this.settings.name ? `, ${this.settings.name}` : ''}, welcome back.`];
    // What happened while they were away: bots that finished, the next thing on the calendar.
    const done = bots.tasks.filter((t) => t.ended && t.ended > this.awayFrom && t.from !== 'bot' && t.status === 'done');
    if (done.length === 1) parts.push(`While you were away, ${bots.bot(done[0].bot)?.name ?? 'a bot'} finished “${done[0].title}”.`);
    else if (done.length > 1) parts.push(`While you were away, your bots finished ${done.length} tasks.`);
    const now = new Date();
    const today = `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`;
    const hm = `${pad(now.getHours())}:${pad(now.getMinutes())}`;
    const soon = `${pad(Math.min(23, now.getHours() + 3))}:${pad(now.getMinutes())}`;
    const next = (await calendarEvents().catch(() => []))
      .filter((e) => e.date === today && e.start && e.start >= hm && e.start <= soon)
      .sort((a, b) => a.start!.localeCompare(b.start!))[0];
    if (next) parts.push(`${next.title} is at ${next.start}.`);
    void gilgamesh.announce(parts.join(' '), 'happy');
  }

  // ---------------------------------------------------------------- for Gilgamesh

  /** Whether the owner was looking at him at some point since `since` (ms): with "only when I
   *  look at him" on, what's said otherwise isn't for him. */
  addressed(since: number) {
    if (!this.on || !this.settings.lookToTalk) return true;
    return this.lastLook >= since - LOOK_GRACE_MS;
  }

  /** What he sees, for his answers. */
  describe(): string {
    if (!this.on) return '';
    if (!this.present) {
      const ago = this.lastSeen ? Math.round((Date.now() - this.lastSeen) / 60_000) : null;
      return `WHAT YOU SEE (your camera): nobody at the computer${ago !== null ? ` (the owner left ${ago < 1 ? 'just now' : `${ago} minute${ago === 1 ? '' : 's'} ago`})` : ''}.`;
    }
    const who = this.who === 'owner' ? 'the owner' : this.who === 'stranger' ? 'someone you don’t recognise (not the owner)' : this.enrolled ? 'a person (you aren’t sure it’s the owner)' : 'the owner';
    const bits = [`${who} is at the computer`, this.looking ? 'looking at you' : 'looking away', this.smiling ? 'smiling' : ''].filter(Boolean);
    const others = this.faces > 1 ? ` ${this.faces - 1 === 1 ? 'Someone else is' : `${this.faces - 1} other people are`} there too.` : '';
    return `WHAT YOU SEE (your camera, now): ${bits.join(', ')}.${others} You can't see details; for that, use [look: …].`;
  }

  /** One picture from the camera (turned on for a moment if it's off), as a JPEG data URL. */
  async snapshot(): Promise<string> {
    const temporary = !this.on;
    if (temporary) await this.camera(true);
    try {
      const v = this.video;
      if (!v) throw new Error(this.error ?? 'The camera didn’t start');
      // A camera just turned on needs a moment for its exposure.
      if (temporary) await new Promise((r) => setTimeout(r, 900));
      const k = Math.min(1, 1024 / v.videoWidth);
      const c = document.createElement('canvas');
      c.width = Math.round(v.videoWidth * k);
      c.height = Math.round(v.videoHeight * k);
      c.getContext('2d')!.drawImage(v, 0, 0, c.width, c.height);
      return c.toDataURL('image/jpeg', 0.85);
    } finally {
      if (temporary && !this.wanted) this.camera(false);
    }
  }

  /** [look: question]: a picture from the camera, answered by the chat model, in his words. */
  async look(question: string): Promise<string> {
    const image = await this.snapshot();
    const r = await rad.assist(
      'You are the eyes of Gilgamesh, a talking assistant, looking through the camera on his owner’s computer. Answer the owner’s question about the picture in one to three short spoken sentences, as Gilgamesh, addressing the owner as "you". Plain text: no Markdown, lists or emoji. If you can’t tell, say so.',
      [{ role: 'user', content: question || 'What do you see?', images: [image] }],
      300,
    );
    return r.content.replace(/<think>[\s\S]*?(<\/think>|$)/g, '').trim();
  }

  // ---------------------------------------------------------------- teaching him the owner's face

  async downloadModel() {
    await invoke('faceid_download');
    await this.refreshModel();
  }

  /** A few seconds in front of the camera: ten looks, kept as numbers in the vault. */
  async enrol(): Promise<void> {
    if (!this.model?.installed) await this.downloadModel();
    const temporary = !this.on;
    if (temporary) await this.camera(true);
    if (!this.on) throw new Error(this.error ?? 'The camera didn’t start');
    this.enrolling = { done: 0, total: ENROL_SAMPLES };
    this.loop();
    const got: number[][] = [];
    try {
      const until = Date.now() + 25_000;
      while (got.length < ENROL_SAMPLES && Date.now() < until && this.enrolling) {
        await new Promise((r) => setTimeout(r, 350));
        const v = this.video;
        if (!v || v.readyState < 2) continue;
        const r = await detect(v);
        if (r.faceLandmarks.length !== 1) continue;
        const e = await this.embed(r.faceLandmarks[0]);
        if (!e) continue;
        got.push(e);
        this.enrolling = { done: got.length, total: ENROL_SAMPLES };
      }
      // Cancelled.
      if (!this.enrolling) return;
      if (got.length < 5) throw new Error('Gilgamesh couldn’t get a good look: sit in front of the camera, alone, in good light, and try again.');
      // The looks should agree with each other (one person, steady light).
      let sum = 0;
      let n = 0;
      for (let i = 0; i < got.length; i++) for (let j = i + 1; j < got.length; j++) (sum += cos(got[i], got[j])), n++;
      if (sum / n < 0.5) throw new Error('The looks didn’t agree (moving too much, or someone else in view?). Try again in steady light.');
      this.embeddings = got;
      await api.put(FACE_KEY, { embeddings: got, at: Date.now() });
      this.enrolled = true;
      this.who = 'owner';
      this.ownerAt = Date.now();
    } finally {
      this.enrolling = null;
      if (temporary && !this.wanted) this.camera(false);
    }
  }

  cancelEnrol() {
    this.enrolling = null;
  }

  async forget() {
    this.embeddings = [];
    this.enrolled = false;
    this.who = null;
    await api.put(FACE_KEY, { embeddings: [], at: Date.now() });
  }
}

export const sight = new Sight();
