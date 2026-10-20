// Draws the talking head with WebGL2 and keeps it alive: blinking, glancing, small head
// movements and breathing, a mood on the face, and lips that follow the speech track.

import { buildRig, posed, SHAPES, type Pose, type Rig, type Shape } from './rig';
import { sample, type Track } from './lipsync';
import type { Avatar } from './face';
import type { Mood } from './text';

export type Attention = 'idle' | 'listening' | 'thinking' | 'speaking';

const MOOD_SHAPES: Record<Mood, Partial<Record<Shape, number>>> = {
  neutral: {},
  happy: { smile: 0.75, browUp: 0.12 },
  amused: { smile: 1, browUp: 0.3 },
  concerned: { frown: 0.75 },
  surprised: { browUp: 1, jaw: 0.08 },
  thinking: { frown: 0.25, browUp: 0.15 },
};

const FACE_VS = `#version 300 es
in vec2 pos; in vec2 uv;
uniform vec2 size;
out vec2 vUv;
void main() { vUv = uv; gl_Position = vec4(pos.x / size.x * 2.0 - 1.0, 1.0 - pos.y / size.y * 2.0, 0.0, 1.0); }`;

const FACE_FS = `#version 300 es
precision mediump float;
in vec2 vUv; uniform sampler2D img; out vec4 o;
void main() { o = texture(img, vUv); }`;

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
  private nextBlink = performance.now() + 1500;
  private blinkStart = -1;
  private gaze = { x: 0, y: 0, tx: 0, ty: 0, next: 0 };
  private nod = 0;
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

  private pose(now: number): Pose {
    const t = (now - this.start) / 1000;
    const speaking = this.attention === 'speaking' && this.speech;
    const st = speaking ? this.speech!.time() : 0;
    const tr = speaking ? this.speech!.track : null;
    const lips = (a: Float32Array | undefined) => (tr && a ? sample(a, tr.fps, st) : 0);
    const energy = lips(tr?.energy);

    // ---- target shapes: mood + attention + speech
    const target: Record<Shape, number> = Object.fromEntries(SHAPES.map((s) => [s, 0])) as Record<Shape, number>;
    const mood = this.attention === 'thinking' ? 'thinking' : this.mood;
    for (const [s, v] of Object.entries(MOOD_SHAPES[mood])) target[s as Shape] += v;
    if (this.attention === 'listening') {
      target.browUp += 0.12;
      target.smile += 0.12;
    }
    // Talking: less smile, so the lips can move.
    if (speaking) target.smile *= 0.6;
    const jaw = lips(tr?.jaw);
    target.jaw += jaw;
    target.wide += lips(tr?.wide) * 0.9;
    target.round += lips(tr?.round) * 0.9;
    target.browUp += energy * 0.12;

    // Ease toward the targets: the mouth follows the track closely, the rest more slowly.
    for (const s of SHAPES) {
      if (s === 'blink' || s === 'gazeX' || s === 'gazeY') continue;
      const k = s === 'jaw' || s === 'wide' || s === 'round' ? 0.6 : 0.08;
      this.cur[s] += (target[s] - this.cur[s]) * k;
    }

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
    this.cur.blink = blink;

    // ---- where the eyes look: mostly at the user, with small glances
    const g = this.gaze;
    if (now >= g.next) {
      if (this.attention === 'thinking') {
        g.tx = rand(-0.9, -0.4) * (Math.random() < 0.5 ? 1 : -1);
        g.ty = rand(-0.9, -0.5);
        g.next = now + rand(900, 2000);
      } else {
        const wander = this.attention === 'listening' || speaking ? 0.25 : 0.6;
        g.tx = Math.random() < 0.6 ? 0 : rand(-wander, wander);
        g.ty = Math.random() < 0.7 ? 0 : rand(-wander, wander) * 0.6;
        g.next = now + rand(700, 3000);
      }
    }
    // Saccades are quick.
    g.x += (g.tx - g.x) * 0.35;
    g.y += (g.ty - g.y) * 0.35;
    this.cur.gazeX = g.x;
    this.cur.gazeY = g.y + blink * 0.3;

    // ---- head: slow sway, breathing, nodding along with speech
    const calm = this.attention === 'listening' ? 0.6 : 1;
    this.nod += ((speaking ? energy : 0) - this.nod) * 0.06;
    const deg = Math.PI / 180;
    return {
      yaw: wiggle(t, 1.3) * 2.2 * deg * calm + g.x * 1.0 * deg,
      pitch: wiggle(t, 4.1) * 1.5 * deg * calm + this.nod * 2.2 * deg + (this.attention === 'thinking' ? -1.5 * deg : 0),
      roll: wiggle(t, 7.7) * 1.2 * deg * calm + (this.attention === 'listening' ? 1.2 * deg : 0),
      lift: Math.sin(t * ((2 * Math.PI) / 4.6)) * 0.0025,
      shapes: { ...this.cur },
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
