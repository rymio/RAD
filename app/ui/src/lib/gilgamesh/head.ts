// Draws the talking head with WebGL2 and keeps it alive: blinking, glancing (and following the
// pointer), small head movements, breathing and shifts of posture, a mood on the face, the lids,
// the cheeks and in the way the head is held, gestures (nod, shake, wink, laugh, eye-roll, sigh,
// yawn, shrug…), small nods while listening, brows and head beats on stressed words, and lips
// that follow the speech track.

import { buildRig, posed, SHAPES, type Pose, type Rig, type Shape } from './rig';
import { sample, type Track } from './lipsync';
import type { Avatar } from './face';
import type { Gesture, Mood } from './text';

export type Attention = 'idle' | 'listening' | 'thinking' | 'speaking';

/** Shapes, plus how far the upper lids droop (0–1) and how much the cheeks blush. */
type Shapes = Partial<Record<Shape | 'lids' | 'blush', number>>;

/** Each mood: the face, and how the head is held (degrees: down, tilt; gaze down, gaze to the
 *  side). A mood may start with a gesture, and repeat one now and then while it lasts. */
interface MoodLook { face: Shapes; pitch?: number; roll?: number; look?: number; lookX?: number; enter?: Gesture; repeat?: { g: Gesture; every: [number, number] } }
const MOODS: Record<Mood, MoodLook> = {
  neutral: { face: {} },
  happy: { face: { smile: 0.85, squint: 0.35, browUp: 0.12 } },
  amused: { face: { smile: 1, squint: 0.55, browUp: 0.25, smileL: 0.15 } },
  excited: { face: { smile: 0.9, eyesWide: 0.7, browUp: 0.6 }, pitch: -1, enter: 'eyebrows' },
  concerned: { face: { frown: 0.6, sad: 0.35 }, pitch: 1 },
  sad: { face: { sad: 1, press: 0.15, lids: 0.15 }, pitch: 2.5, look: 0.35 },
  surprised: { face: { browUp: 1, eyesWide: 0.9, jaw: 0.12 }, pitch: -1.5, enter: 'gasp' },
  curious: { face: { browL: 0.9, smileR: 0.2, eyesWide: 0.3 }, roll: 4 },
  skeptical: { face: { browR: 0.9, squint: 0.5, smileL: 0.45, press: 0.25, mouthL: 0.2 }, roll: -2, pitch: 1 },
  thinking: { face: { press: 0.35, frown: 0.2, browL: 0.2, mouthR: 0.15 } },
  playful: { face: { smileL: 0.7, smileR: 0.3, squint: 0.3, browR: 0.25 }, roll: 2, enter: 'wink' },
  sympathetic: { face: { sad: 0.35, smile: 0.3 }, roll: 3, pitch: 1 },
  serious: { face: { frown: 0.3, press: 0.25, browDown: 0.25 } },
  proud: { face: { smile: 0.55, squint: 0.2, press: 0.15 }, pitch: -2.5 },
  angry: { face: { browDown: 1, press: 0.45, sneer: 0.2, squint: 0.3, frown: 0.2 }, pitch: 1.5 },
  annoyed: { face: { browDown: 0.5, press: 0.4, mouthR: 0.3, squint: 0.25, lids: 0.15 }, roll: -1.5, enter: 'eyeroll' },
  disgusted: { face: { sneer: 1, browDown: 0.4, squint: 0.4, frown: 0.3 }, pitch: -1, roll: -1.5 },
  worried: { face: { sad: 0.6, stretch: 0.35, eyesWide: 0.25, press: 0.1 }, pitch: 1 },
  scared: { face: { eyesWide: 1, browUp: 0.6, sad: 0.5, stretch: 0.6, jaw: 0.08 }, pitch: -2, enter: 'gasp' },
  confused: { face: { browL: 0.8, browDown: 0.25, frown: 0.2, mouthR: 0.35, press: 0.2 }, roll: -4 },
  shy: { face: { smile: 0.45, press: 0.2, blush: 0.6, lids: 0.15 }, pitch: 2.5, roll: 3, look: 0.4, lookX: 0.35, enter: 'lookaway' },
  smug: { face: { smileR: 0.7, mouthR: 0.25, lids: 0.3, browL: 0.25 }, pitch: -2, enter: 'smirk' },
  bored: { face: { lids: 0.4, press: 0.2, frown: 0.1, mouthL: 0.15 }, roll: -3, lookX: -0.4, repeat: { g: 'sigh', every: [9000, 16000] } },
  sleepy: { face: { lids: 0.55, sad: 0.2, jaw: 0.03 }, pitch: 3, look: 0.2, enter: 'yawn', repeat: { g: 'yawn', every: [12000, 22000] } },
  relieved: { face: { smile: 0.5, sad: 0.2, lids: 0.2 }, pitch: -1, enter: 'sigh' },
  determined: { face: { browDown: 0.6, press: 0.5, squint: 0.2 }, pitch: 1.5, enter: 'nod' },
  tender: { face: { smile: 0.55, squint: 0.35, sad: 0.15, blush: 0.25 }, roll: 4 },
  mischievous: { face: { smileL: 0.75, mouthL: 0.2, squint: 0.45, browDown: 0.25, browR: 0.3 }, roll: 3, enter: 'eyebrows' },
  disappointed: { face: { sad: 0.5, press: 0.4, frown: 0.3, lids: 0.2 }, pitch: 2, look: 0.4, enter: 'sigh' },
};

/** How long each gesture takes, in ms. */
const GESTURE_MS: Record<Gesture | 'ack', number> = {
  nod: 900, shake: 1100, tilt: 2600, wink: 450, laugh: 1400, ack: 700, eyeroll: 1300, sigh: 2200, gasp: 900,
  smirk: 1500, eyebrows: 700, yawn: 3200, blush: 2800, shrug: 1400, lookaway: 2000, bow: 1800,
};

/** Small expressions now and then while idle or listening. */
const IDLE: Shapes[] = [
  { press: 0.25 }, { smile: 0.2 }, { browUp: 0.2 }, { smileL: 0.25 }, { squint: 0.2 },
  { mouthL: 0.25, press: 0.15 }, { mouthR: 0.2 }, { smileR: 0.2, browL: 0.15 }, { lids: 0.25 }, { browDown: 0.15, press: 0.1 },
];

const deg = Math.PI / 180;
/** 0 → 1 → 0 over u in [0, 1]: up in the first `a`, held, down in the last `b`. */
const hold = (u: number, a: number, b: number) => Math.max(0, Math.min(1, u / a, (1 - u) / b));

const FACE_VS = `#version 300 es
in vec2 pos; in vec2 uv; in float cheek;
uniform vec2 size;
out vec2 vUv; out float vCheek;
void main() { vUv = uv; vCheek = cheek; gl_Position = vec4(pos.x / size.x * 2.0 - 1.0, 1.0 - pos.y / size.y * 2.0, 0.0, 1.0); }`;

// A blush reddens the skin over the cheeks (it keeps the skin's own shading).
const FACE_FS = `#version 300 es
precision mediump float;
in vec2 vUv; in float vCheek; uniform sampler2D img; uniform float blush; out vec4 o;
void main() {
  vec4 c = texture(img, vUv);
  vec3 red = c.rgb * vec3(1.0, 0.72, 0.74) + vec3(0.09, 0.0, 0.01);
  o = vec4(mix(c.rgb, red, clamp(blush * vCheek, 0.0, 1.0) * 0.55), c.a);
}`;

// The inside of the mouth, drawn between the inner lip contours (MediaPipe puts them a little
// inside the lips, so at rest there is lip between them). u runs corner to corner, t from the
// upper contour (0) to the lower (1); gap is the distance between them now and at rest, in
// pixels. The lip between them at rest is split: its top half stays with the upper lip, its
// bottom half with the lower lip, and the mouth opens in between.
const MOUTH_VS = `#version 300 es
in vec2 pos; in vec2 ut; in vec2 gap; in vec2 uvU; in vec2 uvL;
uniform vec2 size;
out vec2 vUt; out vec2 vGaps; out vec2 vUvU; out vec2 vUvL;
void main() {
  vUt = ut; vGaps = gap; vUvU = uvU; vUvL = uvL;
  gl_Position = vec4(pos.x / size.x * 2.0 - 1.0, 1.0 - pos.y / size.y * 2.0, 0.0, 1.0);
}`;

const MOUTH_FS = `#version 300 es
precision mediump float;
in vec2 vUt; in vec2 vGaps; in vec2 vUvU; in vec2 vUvL;
uniform float mouthW; uniform vec3 lip; uniform sampler2D img;
out vec4 o;
void main() {
  float u = vUt.x, t = vUt.y;
  float g = vGaps.x, rest = min(vGaps.y, g);
  float fromTop = t * g, fromBottom = (1.0 - t) * g;
  float half_ = rest * 0.5;
  // The lip between the contours, moved with its own half.
  if (fromTop <= half_ + 0.5 || fromBottom <= half_ + 0.5) {
    float tr = fromTop <= half_ + 0.5 ? fromTop / max(rest, 1e-3) : 1.0 - fromBottom / max(rest, 1e-3);
    o = vec4(texture(img, mix(vUvU, vUvL, clamp(tr, 0.0, 1.0))).rgb, 1.0);
    return;
  }
  // The opening itself.
  float open = g - rest;
  float top = fromTop - half_, bottom = fromBottom - half_;
  float centre = 1.0 - abs(u - 0.5) * 2.0;
  float edge = min(top, bottom) / max(open * 0.5, 1.0);
  vec3 c = mix(lip * 0.26, vec3(0.03, 0.008, 0.012), smoothstep(0.0, 0.6, edge));
  float tongue = smoothstep(open * 0.5, open * 0.15, bottom) * smoothstep(0.15, 0.55, centre) * smoothstep(mouthW * 0.12, mouthW * 0.3, open);
  c = mix(c, lip * 0.42, tongue * 0.45);
  // Upper teeth just inside the upper lip: ivory, in the lip's shadow at the top, darker toward
  // the corners.
  float th = min(mouthW * 0.085, open * 0.55);
  float upper = smoothstep(th, th * 0.6, top) * smoothstep(0.12, 0.55, centre);
  float shade = (0.45 + 0.55 * smoothstep(0.15, 0.85, centre)) * (0.6 + 0.4 * smoothstep(0.0, th * 0.6, top));
  vec3 teeth = vec3(0.74, 0.7, 0.63) * shade;
  c = mix(c, teeth, upper * 0.9);
  // A hint of the lower teeth when the mouth is wide open.
  float lower = smoothstep(th * 0.45, th * 0.2, bottom) * smoothstep(0.3, 0.65, centre) * smoothstep(mouthW * 0.2, mouthW * 0.38, open);
  c = mix(c, teeth * 0.65, lower * 0.7);
  // Soften the first pixel of the opening into the lips (both halves meet at the middle at rest).
  float soft = smoothstep(0.0, 1.5, min(top, bottom));
  vec3 lipEdge = texture(img, mix(vUvU, vUvL, 0.5)).rgb * 0.6;
  o = vec4(mix(lipEdge, c, soft), 1.0);
}`;

function compile(gl: WebGL2RenderingContext, vs: string, fs: string) {
  const p = gl.createProgram()!;
  for (const [type, src] of [[gl.VERTEX_SHADER, vs], [gl.FRAGMENT_SHADER, fs]] as const) {
    const s = gl.createShader(type)!;
    gl.shaderSource(s, src);
    gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) ?? 'shader');
    gl.attachShader(p, s);
  }
  gl.linkProgram(p);
  if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(p) ?? 'program');
  return p;
}

const rand = (a: number, b: number) => a + Math.random() * (b - a);

/** Smooth random wiggle: a few sines at unrelated speeds. */
function wiggle(t: number, seed: number) {
  return (Math.sin(t * 0.31 + seed) * 0.5 + Math.sin(t * 0.73 + seed * 2.1) * 0.3 + Math.sin(t * 1.37 + seed * 3.7) * 0.2);
}

export class Head {
  private gl: WebGL2RenderingContext;
  private rig: Rig;
  private face: WebGLProgram;
  private mouth: WebGLProgram;
  private faceVao: WebGLVertexArrayObject;
  private mouthVao: WebGLVertexArrayObject;
  private posBuf: WebGLBuffer;
  private mouthBuf: WebGLBuffer;
  private pos: Float32Array;
  private mouthData: Float32Array;
  private tex: WebGLTexture;
  private raf = 0;
  private lip: [number, number, number];
  private restGap: number[];
  private ro: ResizeObserver;

  // What the head is doing.
  mood: Mood = 'neutral';
  attention: Attention = 'idle';
  /** The speech now playing: its mouth track and a clock (seconds into it). */
  speech: { track: Track; time: () => number } | null = null;

  private cur: Record<Shape, number> = Object.fromEntries(SHAPES.map((s) => [s, 0])) as Record<Shape, number>;
  private shownMood: Mood = 'neutral';
  private gestures: { name: Gesture | 'ack'; start: number }[] = [];
  private headBias = { pitch: 0, roll: 0, look: 0 };
  private lastEnergy = 0;
  private flashAt = -1e9;
  private idle: { shapes: Shapes; start: number; ms: number } | null = null;
  private nextIdle = performance.now() + 6000;
  private nextBlink = performance.now() + 1500;
  private blinkStart = -1;
  private gaze = { x: 0, y: 0, tx: 0, ty: 0, next: 0 };
  private nod = 0;
  private lids = 0;
  private blush = 0;
  /** Small nods and turns on stressed words. */
  private beat = { at: -1e9, dir: 0 };
  /** How the head is held at rest: it shifts now and then, as people do. */
  private posture = { yaw: 0, roll: 0, tyaw: 0, troll: 0, next: performance.now() + 5000 };
  /** Tiny eye movements while looking at one place. */
  private micro = { x: 0, y: 0, next: 0 };
  /** The pointer, as seen from the face (-1..1 across the picture), and when it last moved. */
  private pointer = { x: 0, y: 0, at: -1e9 };
  /** Where the owner's face is, from the camera (sight.svelte.ts), and when it was last seen. */
  private viewer = { x: 0, y: 0, at: -1e9 };
  private follow = { yaw: 0, pitch: 0 };
  private nextRepeat = 0;
  /** The blush drawn this frame. */
  private blushNow = 0;
  private start = performance.now();

  constructor(private canvas: HTMLCanvasElement, avatar: Avatar, img: HTMLImageElement) {
    const gl = canvas.getContext('webgl2', { antialias: true, premultipliedAlpha: false, preserveDrawingBuffer: false });
    if (!gl) throw new Error('This computer’s graphics can’t draw the talking head (WebGL 2 is needed)');
    this.gl = gl;
    this.rig = buildRig(avatar);
    this.lip = avatar.lip;
    const r = this.rig;
    this.pos = new Float32Array(r.n * 2);

    this.face = compile(gl, FACE_VS, FACE_FS);
    this.mouth = compile(gl, MOUTH_VS, MOUTH_FS);

    // Face mesh: dynamic positions, fixed texture coordinates and triangles.
    this.faceVao = gl.createVertexArray()!;
    gl.bindVertexArray(this.faceVao);
    this.posBuf = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.posBuf);
    gl.bufferData(gl.ARRAY_BUFFER, this.pos.byteLength, gl.DYNAMIC_DRAW);
    const lp = gl.getAttribLocation(this.face, 'pos');
    gl.enableVertexAttribArray(lp);
    gl.vertexAttribPointer(lp, 2, gl.FLOAT, false, 0, 0);
    const uvBuf = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, uvBuf);
    gl.bufferData(gl.ARRAY_BUFFER, r.uv, gl.STATIC_DRAW);
    const lu = gl.getAttribLocation(this.face, 'uv');
    gl.enableVertexAttribArray(lu);
    gl.vertexAttribPointer(lu, 2, gl.FLOAT, false, 0, 0);
    const cheekBuf = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, cheekBuf);
    gl.bufferData(gl.ARRAY_BUFFER, r.cheek, gl.STATIC_DRAW);
    const lc = gl.getAttribLocation(this.face, 'cheek');
    gl.enableVertexAttribArray(lc);
    gl.vertexAttribPointer(lc, 1, gl.FLOAT, false, 0, 0);
    const ib = gl.createBuffer()!;
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, ib);
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, r.tris, gl.STATIC_DRAW);

    // Mouth strip: per vertex x, y, u, t, gap, rest gap, and the rest texture coordinates of the
    // upper and lower contour points of its column.
    const m = r.mouthUpper.length;
    this.mouthData = new Float32Array(m * 2 * 10);
    this.restGap = r.mouthUpper.map((u, k) => this.gapAt(r.rest, u, r.mouthLower[k]));
    this.mouthVao = gl.createVertexArray()!;
    gl.bindVertexArray(this.mouthVao);
    this.mouthBuf = gl.createBuffer()!;
    gl.bindBuffer(gl.ARRAY_BUFFER, this.mouthBuf);
    gl.bufferData(gl.ARRAY_BUFFER, this.mouthData.byteLength, gl.DYNAMIC_DRAW);
    const stride = 10 * 4;
    for (const [name, size, off] of [['pos', 2, 0], ['ut', 2, 8], ['gap', 2, 16], ['uvU', 2, 24], ['uvL', 2, 32]] as const) {
      const l = gl.getAttribLocation(this.mouth, name);
      gl.enableVertexAttribArray(l);
      gl.vertexAttribPointer(l, size, gl.FLOAT, false, stride, off);
    }
    const mi: number[] = [];
    for (let k = 0; k < m - 1; k++) {
      const a = k * 2, b = a + 1, c = a + 2, d = a + 3;
      mi.push(a, b, c, b, d, c);
    }
    const mib = gl.createBuffer()!;
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, mib);
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, Uint16Array.from(mi), gl.STATIC_DRAW);
    gl.bindVertexArray(null);

    this.tex = gl.createTexture()!;
    gl.bindTexture(gl.TEXTURE_2D, this.tex);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, img);
    gl.generateMipmap(gl.TEXTURE_2D);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.LINEAR_MIPMAP_LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.LINEAR);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);

    window.addEventListener('pointermove', this.onPointer);
    this.ro = new ResizeObserver(() => this.resize());
    this.ro.observe(canvas);
    this.resize();
    const loop = () => {
      this.frame();
      this.raf = requestAnimationFrame(loop);
    };
    this.raf = requestAnimationFrame(loop);
  }

  destroy() {
    cancelAnimationFrame(this.raf);
    this.ro.disconnect();
    window.removeEventListener('pointermove', this.onPointer);
    this.gl.getExtension('WEBGL_lose_context')?.loseContext();
  }

  private resize() {
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    const w = Math.max(1, Math.round(this.canvas.clientWidth * dpr));
    const h = Math.max(1, Math.round(this.canvas.clientHeight * dpr));
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w;
      this.canvas.height = h;
    }
  }

  /** The opening between an upper and a lower inner-lip point, in pixels. */
  private gapAt(pos: Float32Array, u: number, l: number) {
    const d = this.rig.down;
    return Math.max(0, (pos[l * 2] - pos[u * 2]) * d[0] + (pos[l * 2 + 1] - pos[u * 2 + 1]) * d[1]);
  }

  /** Blinks now (e.g. when a turn starts). */
  blink() {
    this.blinkStart = performance.now();
  }

  gesture(name: Gesture) {
    const now = performance.now();
    this.gestures = this.gestures.filter((g) => g.name !== name && now - g.start < GESTURE_MS[g.name]);
    this.gestures.push({ name, start: now });
  }

  /** A small nod: "I'm following". */
  acknowledge() {
    const now = performance.now();
    if (this.gestures.some((g) => now - g.start < GESTURE_MS[g.name])) return;
    this.gestures.push({ name: 'ack', start: now });
  }

  /** The eyes follow the pointer for a while after it moves (x, y: -1..1 across the picture). */
  lookAt(x: number, y: number) {
    this.pointer = { x: Math.max(-1.5, Math.min(1.5, x)), y: Math.max(-1.5, Math.min(1.5, y)), at: performance.now() };
  }

  /** The owner's face, seen by the camera (x, y: -1..1, as the owner sees the screen): while
   *  it's seen, he looks at them rather than straight ahead. */
  see(x: number, y: number) {
    this.viewer = { x: Math.max(-1.2, Math.min(1.2, x)), y: Math.max(-1, Math.min(1, y)), at: performance.now() };
  }

  /** Where the pointer is, seen from the middle of the face (a little above the picture's middle). */
  private onPointer = (e: PointerEvent) => {
    const r = this.canvas.getBoundingClientRect();
    if (!r.width || !r.height) return;
    this.lookAt((e.clientX - (r.left + r.width / 2)) / (r.width / 2), (e.clientY - (r.top + r.height * 0.42)) / (r.height / 2));
  };

  /** Shapes and head movement from the gestures under way. */
  private gestureOffsets(now: number) {
    const add: Shapes = {};
    const plus = (s: keyof Shapes, v: number) => (add[s] = (add[s] ?? 0) + v);
    let pitch = 0, yaw = 0, roll = 0, lift = 0;
    this.gestures = this.gestures.filter((g) => now - g.start < GESTURE_MS[g.name]);
    for (const g of this.gestures) {
      const u = (now - g.start) / GESTURE_MS[g.name];
      const env = Math.sin(Math.PI * Math.min(1, u * 1.2));
      switch (g.name) {
        case 'nod':
          pitch += 4 * deg * Math.sin(2 * Math.PI * 2 * u) * (1 - u);
          break;
        case 'ack':
          pitch += 1.6 * deg * Math.sin(2 * Math.PI * u) * (1 - u);
          plus('smile', 0.1 * env);
          break;
        case 'shake':
          yaw += 6 * deg * Math.sin(2 * Math.PI * 1.5 * u) * (1 - u) ** 0.7;
          break;
        case 'tilt':
          roll += 5 * deg * Math.min(1, u / 0.12, (1 - u) / 0.2);
          plus('browL', 0.3 * env);
          break;
        case 'wink': {
          const t = u * GESTURE_MS.wink;
          plus('blinkL', t < 90 ? t / 90 : t < 210 ? 1 : Math.max(0, 1 - (t - 210) / 240));
          plus('smileL', 0.45 * env);
          plus('squint', 0.15 * env);
          break;
        }
        case 'laugh':
          plus('smile', 0.55 * env);
          plus('squint', 0.55 * env);
          plus('browUp', 0.2 * env);
          plus('jaw', 0.2 * Math.abs(Math.sin(2 * Math.PI * 3.5 * u)) * (1 - u));
          pitch += -1.5 * deg * Math.abs(Math.sin(2 * Math.PI * 3.5 * u)) * (1 - u);
          break;
        case 'eyeroll': {
          // Up, across from one side to the other, and back down.
          const k = hold(u, 0.2, 0.25);
          plus('gazeY', -1.1 * k);
          plus('gazeX', -0.8 * Math.cos(Math.PI * Math.min(1, u * 1.3)) * k);
          plus('browUp', 0.3 * k);
          plus('press', 0.25 * k);
          roll += 2 * deg * k;
          pitch += -1.5 * deg * k;
          break;
        }
        case 'sigh': {
          // A breath in (the head and brows lift), then out: the head sinks and the lids lower.
          const inn = u < 0.35 ? Math.sin((Math.PI / 2) * (u / 0.35)) : 0;
          const out = u >= 0.35 ? Math.sin((Math.PI * (u - 0.35)) / 0.65) : 0;
          lift += -0.005 * inn;
          plus('browUp', 0.25 * inn);
          pitch += -1.5 * deg * inn + 3 * deg * out;
          plus('lids', 0.45 * out);
          plus('round', 0.18 * out);
          plus('sad', 0.15 * out);
          break;
        }
        case 'gasp': {
          const k = u < 0.15 ? u / 0.15 : Math.max(0, 1 - (u - 0.15) / 0.85);
          plus('jaw', 0.3 * k);
          plus('round', 0.2 * k);
          plus('eyesWide', 0.5 * k);
          plus('browUp', 0.5 * k);
          pitch += -3 * deg * k;
          lift += -0.004 * k;
          break;
        }
        case 'smirk':
          plus('smileR', 0.6 * env);
          plus('mouthR', 0.25 * env);
          plus('browL', 0.35 * env);
          plus('squint', 0.15 * env);
          roll += -1.5 * deg * env;
          break;
        case 'eyebrows':
          plus('browUp', 0.6 * Math.abs(Math.sin(2 * Math.PI * u)));
          plus('smile', 0.15 * env);
          break;
        case 'yawn': {
          const k = hold(u, 0.4, 0.35);
          plus('jaw', 0.75 * k);
          plus('round', 0.25 * k);
          plus('wide', 0.1 * k);
          plus('lids', 0.75 * k);
          plus('browUp', 0.35 * k);
          plus('sad', 0.15 * k);
          pitch += -5 * deg * k;
          lift += -0.004 * k;
          break;
        }
        case 'blush': {
          const k = hold(u, 0.15, 0.5);
          plus('blush', 0.7 * k);
          plus('smile', 0.25 * k);
          plus('press', 0.15 * k);
          plus('gazeY', 0.4 * k);
          plus('gazeX', -0.35 * k);
          pitch += 2 * deg * k;
          roll += 2 * deg * k;
          break;
        }
        case 'shrug':
          // "Who knows?": brows up, corners down, the head tilts and lifts with the shoulders.
          plus('browUp', 0.6 * env);
          plus('press', 0.35 * env);
          plus('frown', 0.35 * env);
          roll += 4 * deg * env;
          pitch += -1 * deg * env;
          lift += -0.007 * env;
          break;
        case 'lookaway': {
          const k = hold(u, 0.12, 0.15);
          plus('gazeX', 0.8 * k);
          plus('gazeY', 0.2 * k);
          yaw += -3 * deg * k;
          break;
        }
        case 'bow': {
          const k = Math.sin(Math.PI * u) ** 2;
          pitch += 8 * deg * k;
          plus('gazeY', 0.3 * k);
          plus('lids', 0.2 * k);
          break;
        }
      }
    }
    return { add, pitch, yaw, roll, lift };
  }

  private pose(now: number): Pose {
    const t = (now - this.start) / 1000;
    const speaking = this.attention === 'speaking' && this.speech;
    const st = speaking ? this.speech!.time() : 0;
    const tr = speaking ? this.speech!.track : null;
    const lips = (a: Float32Array | undefined) => (tr && a ? sample(a, tr.fps, st) : 0);
    const energy = lips(tr?.energy);

    // ---- target shapes: mood + attention + speech
    const target: Record<Shape, number> = Object.fromEntries(SHAPES.map((s) => [s, 0])) as Record<Shape, number>;
    let lids = 0, blush = 0;
    const put = (s: string, v: number) => {
      if (s === 'lids') lids += v;
      else if (s === 'blush') blush += v;
      else target[s as Shape] += v;
    };
    const mood: Mood = this.attention === 'thinking' ? 'thinking' : this.mood;
    const m = MOODS[mood] ?? MOODS.neutral;
    if (mood !== this.shownMood) {
      this.shownMood = mood;
      if (m.enter) this.gesture(m.enter);
      if (m.repeat) this.nextRepeat = now + rand(...m.repeat.every);
    }
    // Some moods come back to a gesture now and then (a bored sigh, a sleepy yawn).
    if (m.repeat && !speaking && now >= this.nextRepeat) {
      this.gesture(m.repeat.g);
      this.nextRepeat = now + rand(...m.repeat.every);
    }
    for (const [s, v] of Object.entries(m.face)) put(s, v);
    if (this.attention === 'listening') {
      target.browUp += 0.12;
      target.smile += 0.12;
    }
    // Small expressions now and then, when nothing else is going on.
    if (!speaking && this.attention !== 'thinking' && mood === 'neutral' && !this.gestures.length) {
      if (!this.idle && now >= this.nextIdle) {
        // Sometimes a whole gesture instead: a glance away, a small sigh.
        const r = Math.random();
        if (this.attention === 'idle' && r < 0.12) {
          this.gesture(r < 0.07 ? 'lookaway' : 'sigh');
          this.nextIdle = now + rand(9000, 16000);
        } else this.idle = { shapes: IDLE[Math.floor(Math.random() * IDLE.length)], start: now, ms: rand(1200, 2200) };
      }
    }
    if (this.idle) {
      const u = (now - this.idle.start) / this.idle.ms;
      if (u >= 1) {
        this.idle = null;
        this.nextIdle = now + rand(7000, 14000);
      } else for (const [s, v] of Object.entries(this.idle.shapes)) put(s, v * Math.sin(Math.PI * u));
    }
    // Talking: less smile, so the lips can move.
    if (speaking) {
      target.smile *= 0.6;
      target.smileL *= 0.6;
      target.smileR *= 0.6;
      target.press *= 0.3;
      target.sneer *= 0.6;
      target.stretch *= 0.6;
    }
    const jaw = lips(tr?.jaw);
    target.jaw += jaw;
    target.wide += lips(tr?.wide) * 0.9;
    target.round += lips(tr?.round) * 0.9;
    target.browUp += energy * 0.12;
    // On some stressed words (a sudden rise in loudness) the brows lift, or the head gives a
    // small nod or turn.
    const rise = speaking ? energy - this.lastEnergy : 0;
    if (rise > 0.18 && now - this.flashAt > 1800 && Math.random() < 0.5) this.flashAt = now;
    else if (rise > 0.14 && now - this.beat.at > 700 && Math.random() < 0.4) this.beat = { at: now, dir: rand(-1, 1) };
    this.lastEnergy += (energy - this.lastEnergy) * 0.25;
    const flash = now - this.flashAt;
    if (flash < 450) target.browUp += 0.35 * Math.sin((Math.PI * flash) / 450);
    const bu = (now - this.beat.at) / 380;
    const beat = bu < 1 ? Math.sin(Math.PI * bu) : 0;

    // Ease toward the targets: the mouth follows the track closely, the rest more slowly.
    for (const s of SHAPES) {
      if (s === 'blink' || s === 'blinkL' || s === 'blinkR' || s === 'gazeX' || s === 'gazeY') continue;
      const k = s === 'jaw' || s === 'wide' || s === 'round' ? 0.6 : 0.08;
      this.cur[s] += (target[s] - this.cur[s]) * k;
    }
    this.lids += (lids - this.lids) * 0.06;
    this.blush += (blush - this.blush) * 0.03;
    // The head is held differently in some moods.
    const hb = this.headBias;
    hb.pitch += ((m.pitch ?? 0) - hb.pitch) * 0.04;
    hb.roll += ((m.roll ?? 0) - hb.roll) * 0.04;
    hb.look += ((m.look ?? 0) - hb.look) * 0.06;
    const gst = this.gestureOffsets(now);

    // ---- blinking: every 2–6 s, sometimes twice
    if (now >= this.nextBlink) {
      this.blinkStart = now;
      this.nextBlink = now + (Math.random() < 0.15 ? 280 : rand(2000, 6000));
    }
    let blink = 0;
    if (this.blinkStart >= 0) {
      const b = now - this.blinkStart;
      blink = b < 70 ? b / 70 : b < 170 ? 1 - (b - 70) / 100 : 0;
      if (b >= 170) this.blinkStart = -1;
    }

    // ---- where the eyes look: at the pointer when it has just moved, else mostly at the user,
    // with small glances
    const g = this.gaze;
    const pointing = now - this.pointer.at < 2500 && this.attention !== 'thinking' && !speaking && !this.gestures.length;
    // The owner, where the camera sees them: "straight ahead" is wherever they are.
    const seen = now - this.viewer.at < 1500;
    const vx = seen ? this.viewer.x : 0;
    const vy = seen ? this.viewer.y : 0;
    if (pointing) {
      g.tx = this.pointer.x;
      g.ty = this.pointer.y * 0.8;
      g.next = now + rand(500, 1500);
    } else if (now >= g.next) {
      if (this.attention === 'thinking') {
        g.tx = rand(-0.9, -0.4) * (Math.random() < 0.5 ? 1 : -1);
        g.ty = rand(-0.9, -0.5);
        g.next = now + rand(900, 2000);
      } else {
        const wander = this.attention === 'listening' || speaking ? 0.25 : 0.6;
        g.tx = (m.lookX ?? 0) + vx + (Math.random() < 0.6 ? 0 : rand(-wander, wander));
        g.ty = vy + (Math.random() < 0.7 ? 0 : rand(-wander, wander) * 0.6);
        g.next = now + rand(700, 3000);
      }
    }
    // Saccades are quick; between them the eyes are never quite still.
    g.x += (g.tx - g.x) * 0.35;
    g.y += (g.ty - g.y) * 0.35;
    const mc = this.micro;
    if (now >= mc.next) {
      mc.x = rand(-0.04, 0.04);
      mc.y = rand(-0.03, 0.03);
      mc.next = now + rand(250, 700);
    }
    this.cur.gazeX = g.x + mc.x;
    this.cur.gazeY = g.y + mc.y + hb.look;
    // The head turns a little toward the pointer, or the owner, too.
    const fw = this.follow;
    fw.yaw += ((pointing ? -this.pointer.x * 4 : -vx * 3) - fw.yaw) * 0.05;
    fw.pitch += ((pointing ? this.pointer.y * 2.5 : vy * 2) - fw.pitch) * 0.05;

    // ---- how the head is held: it settles somewhere new now and then
    const ps = this.posture;
    if (now >= ps.next) {
      const range = this.attention === 'listening' ? 1.5 : speaking ? 2 : 3;
      ps.tyaw = rand(-range, range);
      ps.troll = rand(-range, range) * 0.8;
      ps.next = now + rand(5000, 12000);
    }
    ps.yaw += (ps.tyaw - ps.yaw) * 0.01;
    ps.roll += (ps.troll - ps.roll) * 0.01;

    // ---- head: slow sway, breathing, nodding along with speech
    const calm = this.attention === 'listening' ? 0.6 : 1;
    this.nod += ((speaking ? energy : 0) - this.nod) * 0.06;
    const shapes = { ...this.cur };
    let gLids = 0;
    let gBlush = 0;
    for (const [s, v] of Object.entries(gst.add)) {
      if (s === 'lids') gLids += v;
      else if (s === 'blush') gBlush += v;
      else shapes[s as Shape] += v;
    }
    // Drooping lids (sleepy, bored, smug) take a blink the rest of the way; the eyes look a
    // little lower under them.
    const droop = Math.min(0.85, Math.max(0, this.lids + gLids));
    shapes.blinkL = Math.min(1, droop + (1 - droop) * blink + shapes.blinkL);
    shapes.blinkR = Math.min(1, droop + (1 - droop) * blink + shapes.blinkR);
    shapes.blink = 0;
    shapes.gazeY += blink * 0.3 + droop * 0.25;
    // A closing eye takes the squint with it (the lids would cross).
    shapes.squint *= 1 - Math.max(shapes.blinkL, shapes.blinkR);
    this.blushNow = Math.min(1, this.blush + gBlush);
    return {
      yaw: wiggle(t, 1.3) * 2.2 * deg * calm - g.x * 1.0 * deg + ps.yaw * deg + fw.yaw * deg + beat * this.beat.dir * 1.2 * deg + gst.yaw,
      pitch: wiggle(t, 4.1) * 1.5 * deg * calm + this.nod * 2.2 * deg + (this.attention === 'thinking' ? -1.5 * deg : 0) + hb.pitch * deg + fw.pitch * deg + beat * 1.4 * deg + gst.pitch,
      roll: wiggle(t, 7.7) * 1.2 * deg * calm + (this.attention === 'listening' ? 1.2 * deg : 0) + hb.roll * deg + ps.roll * deg + gst.roll,
      lift: Math.sin(t * ((2 * Math.PI) / 4.6)) * 0.0025 + gst.lift,
      shapes,
    };
  }

  private frame() {
    const gl = this.gl;
    const r = this.rig;
    const now = performance.now();
    posed(r, this.pose(now), this.pos);

    gl.viewport(0, 0, this.canvas.width, this.canvas.height);
    gl.clearColor(0, 0, 0, 0);
    gl.clear(gl.COLOR_BUFFER_BIT);

    // The inside of the mouth first; the face is drawn over it (the mesh has a hole there).
    const m = r.mouthUpper.length;
    for (let k = 0; k < m; k++) {
      const u = r.mouthUpper[k], l = r.mouthLower[k];
      const gap = this.gapAt(this.pos, u, l), rg = this.restGap[k], s = k / (m - 1);
      const tex = [r.uv[u * 2], r.uv[u * 2 + 1], r.uv[l * 2], r.uv[l * 2 + 1]];
      this.mouthData.set([this.pos[u * 2], this.pos[u * 2 + 1], s, 0, gap, rg, ...tex, this.pos[l * 2], this.pos[l * 2 + 1], s, 1, gap, rg, ...tex], k * 20);
    }
    gl.useProgram(this.mouth);
    gl.uniform2f(gl.getUniformLocation(this.mouth, 'size'), r.w, r.h);
    gl.uniform1f(gl.getUniformLocation(this.mouth, 'mouthW'), r.mouthW);
    gl.uniform3f(gl.getUniformLocation(this.mouth, 'lip'), ...this.lip);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.tex);
    gl.uniform1i(gl.getUniformLocation(this.mouth, 'img'), 0);
    gl.bindVertexArray(this.mouthVao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.mouthBuf);
    gl.bufferSubData(gl.ARRAY_BUFFER, 0, this.mouthData);
    gl.drawElements(gl.TRIANGLES, (m - 1) * 6, gl.UNSIGNED_SHORT, 0);

    gl.useProgram(this.face);
    gl.uniform2f(gl.getUniformLocation(this.face, 'size'), r.w, r.h);
    gl.uniform1f(gl.getUniformLocation(this.face, 'blush'), this.blushNow);
    gl.activeTexture(gl.TEXTURE0);
    gl.bindTexture(gl.TEXTURE_2D, this.tex);
    gl.uniform1i(gl.getUniformLocation(this.face, 'img'), 0);
    gl.bindVertexArray(this.faceVao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.posBuf);
    gl.bufferSubData(gl.ARRAY_BUFFER, 0, this.pos);
    gl.drawElements(gl.TRIANGLES, r.tris.length, gl.UNSIGNED_SHORT, 0);
    gl.bindVertexArray(null);
  }
}

/** What follows Gilgamesh's state: a picture of him, or his picture in another window (stage.svelte.ts). */
export interface HeadLike {
  mood: Mood;
  attention: Attention;
  speech: Head['speech'];
  blink(): void;
  gesture(g: Gesture): void;
  acknowledge(): void;
  see(x: number, y: number): void;
}

/** All the pictures of Gilgamesh on screen (the sidebar, his own view, his own window): they show
 *  the same mood, attention, speech and gestures. A picture added later starts from where the
 *  others are. */
export class Heads {
  private all = new Set<HeadLike>();
  private state: { mood: Mood; attention: Attention; speech: Head['speech'] } = { mood: 'neutral', attention: 'idle', speech: null };

  add(h: HeadLike) {
    h.mood = this.state.mood;
    h.attention = this.state.attention;
    h.speech = this.state.speech;
    this.all.add(h);
    return () => void this.all.delete(h);
  }

  get mood() { return this.state.mood; }
  set mood(v: Mood) { this.state.mood = v; for (const h of this.all) h.mood = v; }
  get attention() { return this.state.attention; }
  set attention(v: Attention) { this.state.attention = v; for (const h of this.all) h.attention = v; }
  get speech() { return this.state.speech; }
  set speech(v: Head['speech']) { this.state.speech = v; for (const h of this.all) h.speech = v; }

  blink() { for (const h of this.all) h.blink(); }
  gesture(g: Gesture) { for (const h of this.all) h.gesture(g); }
  see(x: number, y: number) { for (const h of this.all) h.see(x, y); }
  acknowledge() { for (const h of this.all) h.acknowledge(); }
}
