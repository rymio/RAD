// Finds the face in a photo (MediaPipe Face Landmarker, bundled and run on this computer) and
// prepares the avatar: a portrait crop around the face plus its 478 landmarks.

import { FaceLandmarker } from '@mediapipe/tasks-vision';
import wasmLoaderPath from '@mediapipe/tasks-vision/vision_wasm_internal.js?url';
import wasmBinaryPath from '@mediapipe/tasks-vision/vision_wasm_internal.wasm?url';
import modelAssetPath from '../../assets/gilgamesh/face_landmarker.task?url';

/** A prepared avatar, stored (encrypted) in the vault. */
export interface Avatar {
  /** JPEG data URL of the portrait crop. */
  image: string;
  w: number;
  h: number;
  /** 478 landmarks in pixels of the crop: x, y, z (z in the same scale as x). */
  points: number[];
  /** Mean colour of the lips (0–1), to tint the inside of the mouth. */
  lip: [number, number, number];
}

let landmarker: Promise<FaceLandmarker> | null = null;

function getLandmarker() {
  landmarker ??= FaceLandmarker.createFromOptions(
    { wasmLoaderPath, wasmBinaryPath },
    { baseOptions: { modelAssetPath, delegate: 'CPU' }, runningMode: 'IMAGE', numFaces: 1 },
  ).catch((e) => {
    landmarker = null;
    throw e;
  });
  return landmarker;
}

function loadImage(src: string) {
  return new Promise<HTMLImageElement>((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error('That file isn’t an image RAD can read'));
    img.src = src;
  });
}

async function detect(src: CanvasImageSource & { width: number; height: number }) {
  const lm = await getLandmarker();
  const r = lm.detect(src as any);
  return r.faceLandmarks[0] ?? null;
}

const OUT_W = 720;
const OUT_H = 900;

/** Makes an avatar from a photo file: finds the face, crops a portrait around it, finds it again. */
export async function prepareAvatar(file: Blob): Promise<{ avatar: Avatar; warnings: string[] }> {
  const url = URL.createObjectURL(file);
  try {
    const img = await loadImage(url);
    // Big photos are scaled down first: detection doesn't need the detail.
    const s0 = Math.min(1, 1600 / Math.max(img.naturalWidth, img.naturalHeight));
    const c0 = document.createElement('canvas');
    c0.width = Math.round(img.naturalWidth * s0);
    c0.height = Math.round(img.naturalHeight * s0);
    c0.getContext('2d')!.drawImage(img, 0, 0, c0.width, c0.height);
    const first = await detect(c0);
    if (!first) throw new Error('No face found in that photo. Use a clear photo with the face looking at the camera.');

    // Portrait crop (4:5) around the face: the face fills about 45% of the height.
    let x0 = 1, y0 = 1, x1 = 0, y1 = 0;
    for (const p of first) {
      x0 = Math.min(x0, p.x); x1 = Math.max(x1, p.x);
      y0 = Math.min(y0, p.y); y1 = Math.max(y1, p.y);
    }
    const fw = (x1 - x0) * c0.width;
    const fh = (y1 - y0) * c0.height;
    const cx = ((x0 + x1) / 2) * c0.width;
    const cy = ((y0 + y1) / 2) * c0.height;
    let ch = Math.max(fh / 0.45, (fw / 0.5) * (OUT_H / OUT_W));
    let cw = ch * (OUT_W / OUT_H);
    // Keep inside the photo (shrinking the crop if the photo is tight around the face).
    const k = Math.min(1, c0.width / cw, c0.height / ch);
    cw *= k;
    ch *= k;
    const left = Math.min(Math.max(cx - cw / 2, 0), c0.width - cw);
    const top = Math.min(Math.max(cy - ch * 0.46, 0), c0.height - ch);

    const out = document.createElement('canvas');
    out.width = OUT_W;
    out.height = OUT_H;
    const g = out.getContext('2d')!;
    g.imageSmoothingQuality = 'high';
    g.drawImage(c0, left, top, cw, ch, 0, 0, OUT_W, OUT_H);
    const face = await detect(out);
    if (!face) throw new Error('The face couldn’t be found again after cropping. Try another photo.');

    const points: number[] = [];
    for (const p of face) points.push(p.x * OUT_W, p.y * OUT_H, p.z * OUT_W);

    const warnings: string[] = [];
    const P = (i: number) => [points[i * 3], points[i * 3 + 1]];
    const dist = (a: number, b: number) => Math.hypot(P(a)[0] - P(b)[0], P(a)[1] - P(b)[1]);
    const faceH = dist(10, 152);
    if (dist(13, 14) > faceH * 0.03) warnings.push('The mouth is open in this photo; a photo with closed lips looks more natural.');
    const yaw = Math.abs(dist(1, 234) - dist(1, 454)) / dist(234, 454);
    if (yaw > 0.25) warnings.push('The face is turned to the side; a photo looking at the camera works best.');
    if (faceH < 160) warnings.push('The face is small in this photo; a closer or sharper photo gives a better result.');

    return { avatar: { image: out.toDataURL('image/jpeg', 0.9), w: OUT_W, h: OUT_H, points, lip: lipColour(g, points) }, warnings };
  } finally {
    URL.revokeObjectURL(url);
  }
}

/** Mean colour around the middle of both lips. */
function lipColour(g: CanvasRenderingContext2D, pts: number[]): [number, number, number] {
  let r = 0, gr = 0, b = 0, n = 0;
  for (const i of [0, 17, 37, 267, 84, 314]) {
    const d = g.getImageData(Math.round(pts[i * 3]) - 2, Math.round(pts[i * 3 + 1]) - 2, 5, 5).data;
    for (let j = 0; j < d.length; j += 4) {
      r += d[j]; gr += d[j + 1]; b += d[j + 2]; n++;
    }
  }
  return n ? [r / n / 255, gr / n / 255, b / n / 255] : [0.6, 0.3, 0.3];
}

export function avatarImage(a: Avatar) {
  return loadImage(a.image);
}
