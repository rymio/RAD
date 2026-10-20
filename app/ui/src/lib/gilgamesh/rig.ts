// The face rig: a triangle mesh over the photo, built from the 478 face landmarks plus rings
// around the head and the photo's border, and a set of expression shapes. Each shape moves some
// landmarks (the "controls") and spreads smoothly to their neighbours; a frame is the rest mesh,
// turned a little with the head, plus a weighted sum of shapes.

import Delaunator from 'delaunator';
import type { Avatar } from './face';

export const SHAPES = ['jaw', 'wide', 'round', 'smile', 'frown', 'browUp', 'blink', 'gazeX', 'gazeY'] as const;
export type Shape = (typeof SHAPES)[number];

export interface Rig {
  w: number;
  h: number;
  n: number;
  /** Rest positions in pixels (x, y), and with depth (x, y, z) for turning the head. */
  rest: Float32Array;
  rest3: Float32Array;
  /** How much each vertex follows the head (1 on the face, less in the hair, 0 at the border). */
  follow: Float32Array;
  uv: Float32Array;
  tris: Uint16Array;
  shapes: Record<Shape, Float32Array>;
  pivot: [number, number, number];
  /** The inner lip contours, corner to corner (the inside of the mouth is drawn between them). */
  mouthUpper: number[];
  mouthLower: number[];
  faceH: number;
  mouthW: number;
  down: [number, number];
}

// Landmark indices (MediaPipe Face Mesh).
const UO = [185, 40, 39, 37, 0, 267, 269, 270, 409];
const UI = [191, 80, 81, 82, 13, 312, 311, 310, 415];
const LO = [146, 91, 181, 84, 17, 314, 405, 321, 375];
const LI = [95, 88, 178, 87, 14, 317, 402, 318, 324];
const CO = [61, 291];
const CI = [78, 308];
/** The outer lip outline, all the way round. */
const LIP_OUTLINE = [61, 185, 40, 39, 37, 0, 267, 269, 270, 409, 291, 375, 321, 405, 314, 17, 84, 181, 91, 146];
const FACE_OVAL = [10, 338, 297, 332, 284, 251, 389, 356, 454, 323, 361, 288, 397, 365, 379, 378, 400, 377, 152, 148, 176, 149, 150, 136, 172, 58, 132, 93, 234, 127, 162, 21, 54, 103, 67, 109];
const JAW_A = [152, 148, 176, 149, 150, 136, 172, 58, 132, 93];
const JAW_B = [152, 377, 400, 378, 379, 365, 397, 288, 361, 323];
const NOSE = [1, 2, 4, 5, 6, 19, 94, 98, 327, 168, 197, 195];
const CHEEK = [50, 280, 205, 425, 36, 266, 101, 330, 117, 346, 123, 352, 147, 376];
const BROW_A = [70, 63, 105, 66, 107, 46, 53, 52, 65, 55];
const BROW_B = [300, 293, 334, 296, 336, 276, 283, 282, 295, 285];
const BROW_INNER = [107, 55, 65, 336, 285, 295];
const FOREHEAD_LOW = [151, 108, 69, 104, 337, 299, 333, 9, 67, 297, 109, 338];
const FOREHEAD_TOP = [10, 54, 284, 21, 251, 103, 332];
interface Eye { upper: number[]; lower: number[]; corners: number[]; crease: number[]; iris: number[] }
const EYES: Eye[] = [
  { upper: [246, 161, 160, 159, 158, 157, 173], lower: [7, 163, 144, 145, 153, 154, 155], corners: [33, 133], crease: [247, 30, 29, 27, 28, 56, 190], iris: [] },
  { upper: [466, 388, 387, 386, 385, 384, 398], lower: [249, 390, 373, 374, 380, 381, 382], corners: [263, 362], crease: [467, 260, 259, 257, 258, 286, 414], iris: [] },
];
const IRIS = [
  [468, 469, 470, 471, 472],
  [473, 474, 475, 476, 477],
];
export const MOUTH_UPPER = [78, 191, 80, 81, 82, 13, 312, 311, 310, 415, 308];
export const MOUTH_LOWER = [78, 95, 88, 178, 87, 14, 317, 402, 318, 324, 308];

type V2 = [number, number];
const sub = (a: V2, b: V2): V2 => [a[0] - b[0], a[1] - b[1]];
const dot = (a: V2, b: V2) => a[0] * b[0] + a[1] * b[1];
const len = (a: V2) => Math.hypot(a[0], a[1]);

export function buildRig(a: Avatar): Rig {
  const L = a.points.length / 3;
  const P = (i: number): V2 => [a.points[i * 3], a.points[i * 3 + 1]];
  const top = P(10), chin = P(152);
  const faceH = len(sub(chin, top));
  const faceW = len(sub(P(454), P(234)));
  const down: V2 = [(chin[0] - top[0]) / faceH, (chin[1] - top[1]) / faceH];
  const right: V2 = [down[1], -down[0]];
  const center: V2 = [(top[0] + chin[0]) / 2, (top[1] + chin[1]) / 2];

  // ---- vertices: landmarks, two rings around the head, the border
  const pts: number[] = [];
  const z: number[] = [];
  const follow: number[] = [];
  for (let i = 0; i < L; i++) {
    pts.push(a.points[i * 3], a.points[i * 3 + 1]);
    z.push(a.points[i * 3 + 2]);
    follow.push(1);
  }
  const inside = (x: number, y: number, m: number) => x > m && y > m && x < a.w - m && y < a.h - m;
  // Ring points get the depth of the nearest point on the face's outline, so turning the head
  // doesn't shear the face against the hair beside it.
  const ovalZ = (x: number, y: number) => {
    let best = FACE_OVAL[0], bd = Infinity;
    for (const i of FACE_OVAL) {
      const d = (P(i)[0] - x) ** 2 + (P(i)[1] - y) ** 2;
      if (d < bd) (bd = d), (best = i);
    }
    return a.points[best * 3 + 2];
  };
  for (const [sx, sy, f, count] of [[1.18, 1.14, 0.95, 20], [1.5, 1.42, 0.75, 22], [2.0, 1.85, 0.35, 26]] as const) {
    for (let k = 0; k < count; k++) {
      const t = (k / count) * Math.PI * 2;
      const lx = Math.cos(t) * faceW * 0.5 * sx;
      const ly = Math.sin(t) * faceH * 0.5 * sy;
      const x = center[0] + right[0] * lx + down[0] * ly;
      const y = center[1] + right[1] * lx + down[1] * ly;
      if (!inside(x, y, 6)) continue;
      pts.push(x, y);
      z.push(ovalZ(x, y));
      follow.push(f);
    }
  }
  const B = 6;
  for (let k = 0; k < B; k++) {
    const t = k / B;
    for (const [x, y] of [[t * a.w, 0], [a.w, t * a.h], [a.w - t * a.w, a.h], [0, a.h - t * a.h]]) {
      pts.push(x, y);
      z.push(0);
      follow.push(0);
    }
  }
  const n = pts.length / 2;

  // ---- the lips: every landmark inside the outer lip outline (there are rings of points between
  // the outer and inner contours) belongs to the upper lip (0), the lower one (1), or, near the
  // corners where they meet, a bit of both.
  const outline = LIP_OUTLINE.map(P);
  const inLips = (q: V2) => {
    let inside = false;
    for (let i = 0, j = outline.length - 1; i < outline.length; j = i++) {
      const [xi, yi] = outline[i], [xj, yj] = outline[j];
      if (yi > q[1] !== yj > q[1] && q[0] < ((xj - xi) * (q[1] - yi)) / (yj - yi) + xi) inside = !inside;
    }
    return inside;
  };
  // Where the lips meet: halfway between the inner contours (MediaPipe puts them a little inside
  // the lips). Beyond the corners it carries on straight.
  const meet: V2[] = MOUTH_UPPER.map((u, k) => {
    const a = P(u), b = P(MOUTH_LOWER[k]);
    return [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
  });
  const meetGap = MOUTH_UPPER.map((u, k) => Math.abs(dot(sub(P(MOUTH_LOWER[k]), P(u)), down)));
  /** How far below the lip line a point is (along the face's down axis), and the lips' gap there. */
  const belowMeet = (q: V2): [number, number] => {
    const x = dot(q, right);
    for (let k = 0; k < meet.length - 1; k++) {
      const x0 = dot(meet[k], right), x1 = dot(meet[k + 1], right);
      if ((x - x0) * (x - x1) <= 0 && x0 !== x1) {
        const t = (x - x0) / (x1 - x0);
        const m: V2 = [meet[k][0] + (meet[k + 1][0] - meet[k][0]) * t, meet[k][1] + (meet[k + 1][1] - meet[k][1]) * t];
        return [dot(sub(q, m), down), meetGap[k] + (meetGap[k + 1] - meetGap[k]) * t];
      }
    }
    const end = Math.abs(x - dot(meet[0], right)) < Math.abs(x - dot(meet[meet.length - 1], right)) ? meet[0] : meet[meet.length - 1];
    return [dot(sub(q, end), down), 0];
  };
  const smooth = (a: number, b: number, x: number) => {
    const t = Math.min(1, Math.max(0, (x - a) / (b - a)));
    return t * t * (3 - 2 * t);
  };
  const lowness = new Map<number, number>();
  for (const i of [...UO, ...UI]) lowness.set(i, 0);
  for (const i of [...LO, ...LI]) lowness.set(i, 1);
  for (let i = 0; i < L; i++) {
    if (lowness.has(i) || CO.includes(i) || CI.includes(i) || !inLips(P(i))) continue;
    const [b, gap] = belowMeet(P(i));
    const h = Math.max(0.004 * faceH, gap * 0.4);
    lowness.set(i, smooth(-h, h, b));
  }
  const LIPS = [...lowness.keys(), ...CO, ...CI];
  const isCorner = (i: number) => CO.includes(i) || CI.includes(i);
  /** Mixes the upper-lip and lower-lip values by how much a point belongs to each. */
  const byLip = (i: number, up: number, low: number) => up + (low - up) * (lowness.get(i) ?? 0);

  // ---- expression shapes
  const mouthC: V2 = [(P(13)[0] + P(14)[0]) / 2, (P(13)[1] + P(14)[1]) / 2];
  const halfMW = len(sub(P(291), P(61))) / 2;
  /** Across the mouth: -1 at one corner, +1 at the other. */
  const xr = (i: number) => Math.max(-1, Math.min(1, dot(sub(P(i), mouthC), right) / halfMW));
  /** Across the face: negative on one side of the nose, positive on the other. */
  const side = (i: number) => Math.sign(dot(sub(P(i), center), right)) || 1;
  /** A displacement in face units (fractions of the face height) along the face's axes. */
  const vec = (dx: number, dy: number): V2 => [(right[0] * dx + down[0] * dy) * faceH, (right[1] * dx + down[1] * dy) * faceH];

  function field(controls: Map<number, V2>, radius: number): Float32Array {
    const out = new Float32Array(n * 2);
    const r2 = (radius * faceH) ** 2;
    const cs = [...controls.entries()].map(([i, d]) => ({ x: pts[i * 2], y: pts[i * 2 + 1], d }));
    for (let v = 0; v < n; v++) {
      if (follow[v] === 0) continue;
      const c = controls.get(v);
      if (c) {
        out[v * 2] = c[0];
        out[v * 2 + 1] = c[1];
        continue;
      }
      let sw = 0, sx = 0, sy = 0;
      for (const k of cs) {
        const g = Math.exp(-((pts[v * 2] - k.x) ** 2 + (pts[v * 2 + 1] - k.y) ** 2) / r2);
        sw += g;
        sx += g * k.d[0];
        sy += g * k.d[1];
      }
      const norm = Math.max(1, sw);
      out[v * 2] = sx / norm;
      out[v * 2 + 1] = sy / norm;
    }
    return out;
  }
  const zero: V2 = [0, 0];
  const anchor = (m: Map<number, V2>, ...sets: number[][]) => {
    for (const s of sets) for (const i of s) if (!m.has(i)) m.set(i, zero);
    return m;
  };
  const eyeAll = EYES.flatMap((e) => [...e.upper, ...e.lower, ...e.corners]);

  // Which iris belongs to which eye: the nearer one.
  for (const e of EYES) {
    const ec = P(e.lower[3]);
    e.iris = IRIS.reduce((best, ir) => (len(sub(P(ir[0]), ec)) < len(sub(P(best[0]), ec)) ? ir : best));
  }
  const hasIris = L >= 478;

  // The jaw hinges near the ears: everything below the lips moves down together, less toward the
  // jaw's angles; beside the mouth corners the cheeks follow part way.
  const jaw = new Map<number, V2>();
  const JAW_OPEN = 0.12;
  const across = (i: number) => Math.abs(dot(sub(P(i), mouthC), right)) / faceW;
  const jawWeight = (i: number) => 1 - 0.85 * smooth(0.12, 0.5, across(i));
  for (let i = 0; i < L; i++) {
    let d: number;
    if (isCorner(i)) d = 0.45 * JAW_OPEN * jawWeight(i);
    else if (lowness.has(i)) d = byLip(i, -0.006, JAW_OPEN * jawWeight(i));
    else {
      const [b] = belowMeet(P(i));
      d = JAW_OPEN * jawWeight(i) * smooth(-0.015, 0.05, b / faceH);
    }
    jaw.set(i, vec(0, d));
  }

  const wide = new Map<number, V2>();
  for (const i of LIPS) {
    wide.set(i, vec(0.03 * xr(i), isCorner(i) ? -0.004 : byLip(i, 0.004, -0.006)));
  }
  anchor(wide, NOSE, JAW_A, JAW_B, CHEEK);

  const round = new Map<number, V2>();
  for (const i of LIPS) {
    const bulge = 1 - xr(i) ** 2;
    round.set(i, vec(-0.035 * xr(i), isCorner(i) ? 0 : byLip(i, -0.01 * bulge, 0.012 * bulge)));
  }
  anchor(round, NOSE, JAW_A, JAW_B, CHEEK);

  const smile = new Map<number, V2>();
  for (const i of LIPS) {
    const curve = Math.abs(xr(i)) ** 1.5;
    smile.set(i, vec(0.028 * xr(i), isCorner(i) ? -0.036 : byLip(i, -0.004 - 0.014 * curve, -0.02 * curve)));
  }
  for (const i of CHEEK) smile.set(i, vec(0.008 * side(i), [123, 352, 147, 376].includes(i) ? -0.012 : -0.026));
  for (const e of EYES) for (const i of e.lower) smile.set(i, vec(0, -0.007));
  anchor(smile, NOSE, JAW_A, JAW_B, BROW_A, BROW_B, FOREHEAD_LOW, EYES.flatMap((e) => [...e.upper, ...e.corners]));

  const frown = new Map<number, V2>();
  for (const i of [...CO, ...CI]) frown.set(i, vec(0, 0.022));
  for (const i of LIPS) if (!isCorner(i)) frown.set(i, vec(0, byLip(i, 0, 0.004 * Math.abs(xr(i)))));
  for (const i of BROW_INNER) frown.set(i, vec(-0.014 * side(i), 0.022));
  for (const i of [70, 46, 300, 276]) frown.set(i, vec(0, -0.004));
  anchor(frown, NOSE, eyeAll, FOREHEAD_TOP);

  const browUp = new Map<number, V2>();
  for (const i of [...BROW_A, ...BROW_B]) browUp.set(i, vec(0, -0.032));
  for (const i of FOREHEAD_LOW) browUp.set(i, vec(0, -0.012));
  for (const e of EYES) for (const i of e.upper) browUp.set(i, vec(0, -0.004));
  anchor(browUp, FOREHEAD_TOP, NOSE, EYES.flatMap((e) => [...e.lower, ...e.corners]));

  // Closing an eye: the upper lid, the crease above it (part way) and the iris come down to the
  // lower lid, each straight down to where the lower lid is at its own position across.
  const blink = new Map<number, V2>();
  for (const e of EYES) {
    const lid = [e.corners[0], ...e.lower, e.corners[1]].map(P).sort((a, b) => dot(a, right) - dot(b, right));
    const dropTo = (i: number, frac: number, gap: number): V2 => {
      const q = P(i), x = dot(q, right);
      let k = 0;
      while (k < lid.length - 2 && dot(lid[k + 1], right) < x) k++;
      const x0 = dot(lid[k], right), x1 = dot(lid[k + 1], right);
      const t = Math.min(1, Math.max(0, (x - x0) / (x1 - x0 || 1)));
      const target: V2 = [lid[k][0] + (lid[k + 1][0] - lid[k][0]) * t, lid[k][1] + (lid[k + 1][1] - lid[k][1]) * t];
      const d = Math.max(0, dot(sub(target, q), down) - gap) * frac;
      return [down[0] * d, down[1] * d];
    };
    const g = faceH * 0.004;
    for (const u of e.upper) blink.set(u, dropTo(u, 1, g));
    for (const c of e.crease) blink.set(c, dropTo(c, 0.42, g));
    if (hasIris) for (const i of e.iris) blink.set(i, dropTo(i, 1, g * 0.5));
  }
  anchor(blink, EYES.flatMap((e) => [...e.lower, ...e.corners]), BROW_A, BROW_B);

  const gazeX = new Map<number, V2>();
  const gazeY = new Map<number, V2>();
  if (hasIris) {
    for (const e of EYES) {
      for (const i of e.iris) {
        gazeX.set(i, vec(0.014, 0));
        gazeY.set(i, vec(0, 0.008));
      }
      for (const i of e.upper) gazeY.set(i, vec(0, 0.004));
    }
  }
  anchor(gazeX, eyeAll);
  anchor(gazeY, eyeAll);

  const shapes: Record<Shape, Float32Array> = {
    jaw: field(jaw, 0.07),
    wide: field(wide, 0.06),
    round: field(round, 0.06),
    smile: field(smile, 0.09),
    frown: field(frown, 0.06),
    browUp: field(browUp, 0.07),
    blink: field(blink, 0.025),
    gazeX: field(gazeX, 0.008),
    gazeY: field(gazeY, 0.008),
  };

  // ---- triangles: made with the lower lip moved down a little (only the lips move), so no
  // triangle bridges the lips; the slivers between them, made only of inner-lip points, are
  // dropped: the inside of the mouth is drawn there.
  const open = Float64Array.from(pts);
  const part = 0.035 * faceH;
  for (const i of LIPS) {
    const t = isCorner(i) ? 0.5 : (lowness.get(i) ?? 0);
    open[i * 2] += down[0] * part * t;
    open[i * 2 + 1] += down[1] * part * t;
  }
  const del = new Delaunator(open);
  const hole = [...MOUTH_UPPER, ...[...MOUTH_LOWER].reverse()].map((i): V2 => [open[i * 2], open[i * 2 + 1]]);
  const inHole = (x: number, y: number) => {
    let inside = false;
    for (let i = 0, j = hole.length - 1; i < hole.length; j = i++) {
      const [xi, yi] = hole[i], [xj, yj] = hole[j];
      if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) inside = !inside;
    }
    return inside;
  };
  const innerSet = new Set([...MOUTH_UPPER, ...MOUTH_LOWER]);
  const tris: number[] = [];
  for (let t = 0; t < del.triangles.length; t += 3) {
    const v = [del.triangles[t], del.triangles[t + 1], del.triangles[t + 2]];
    const cx = (open[v[0] * 2] + open[v[1] * 2] + open[v[2] * 2]) / 3;
    const cy = (open[v[0] * 2 + 1] + open[v[1] * 2 + 1] + open[v[2] * 2 + 1]) / 3;
    if (!(v.every((i) => innerSet.has(i)) && inHole(cx, cy))) tris.push(...v);
  }

  const rest = Float32Array.from(pts);
  const rest3 = new Float32Array(n * 3);
  const uv = new Float32Array(n * 2);
  for (let v = 0; v < n; v++) {
    rest3[v * 3] = pts[v * 2];
    rest3[v * 3 + 1] = pts[v * 2 + 1];
    rest3[v * 3 + 2] = z[v];
    uv[v * 2] = pts[v * 2] / a.w;
    uv[v * 2 + 1] = pts[v * 2 + 1] / a.h;
  }
  let zMean = 0;
  for (let i = 0; i < L; i++) zMean += z[i] / L;
  const neck = [center[0] + down[0] * faceH * 0.55, center[1] + down[1] * faceH * 0.55];

  return {
    w: a.w,
    h: a.h,
    n,
    rest,
    rest3,
    follow: Float32Array.from(follow),
    uv,
    tris: Uint16Array.from(tris),
    shapes,
    pivot: [neck[0], neck[1], zMean],
    mouthUpper: MOUTH_UPPER,
    mouthLower: MOUTH_LOWER,
    faceH,
    mouthW: halfMW * 2,
    down,
  };
}

export interface Pose {
  /** Head turn in radians: left/right, up/down, tilt. */
  yaw: number;
  pitch: number;
  roll: number;
  /** Small up/down movement (breathing, nodding along), in face heights. */
  lift: number;
  shapes: Record<Shape, number>;
}

/** Vertex positions (pixels) for a pose, written into `out`. */
export function posed(rig: Rig, pose: Pose, out: Float32Array) {
  const { yaw, pitch, roll } = pose;
  const cy = Math.cos(yaw), sy = Math.sin(yaw);
  const cp = Math.cos(pitch), sp = Math.sin(pitch);
  const cr = Math.cos(roll), sr = Math.sin(roll);
  const [px, py, pz] = rig.pivot;
  const lift = pose.lift * rig.faceH;
  const active = SHAPES.filter((s) => Math.abs(pose.shapes[s]) > 1e-4);
  for (let v = 0; v < rig.n; v++) {
    const rx = rig.rest[v * 2], ry = rig.rest[v * 2 + 1];
    let x = rx, y = ry;
    const f = rig.follow[v];
    if (f > 0) {
      // Yaw about the vertical axis, pitch about the horizontal one, then roll; orthographic.
      let X = rig.rest3[v * 3] - px, Y = rig.rest3[v * 3 + 1] - py, Z = rig.rest3[v * 3 + 2] - pz;
      const x1 = cy * X + sy * Z;
      const z1 = -sy * X + cy * Z;
      const y1 = cp * Y - sp * z1;
      X = cr * x1 - sr * y1;
      Y = sr * x1 + cr * y1;
      x = rx + f * (X + px - rx);
      y = ry + f * (Y + py - ry) + f * lift;
      for (const s of active) {
        const w = pose.shapes[s];
        const d = rig.shapes[s];
        x += w * d[v * 2];
        y += w * d[v * 2 + 1];
      }
    }
    out[v * 2] = x;
    out[v * 2 + 1] = y;
  }
}
