// Drawing for the radio: the waterfall's colours, scrolling it, and the spectrum trace.

/** Waterfall colours, quiet to loud: black, deep blue, cyan, yellow, red, white. */
const STOPS: [number, [number, number, number]][] = [
  [0, [0, 0, 8]],
  [0.18, [10, 20, 110]],
  [0.4, [0, 140, 200]],
  [0.62, [240, 230, 40]],
  [0.82, [235, 60, 20]],
  [1, [255, 255, 255]],
];

/** RGB for each of the 256 byte levels the backend sends (0.5 dB steps from −130 dB), with
 *  `floor`–`ceiling` dBFS spread over the colours. */
export function palette(floor: number, ceiling: number): Uint8ClampedArray {
  const p = new Uint8ClampedArray(256 * 3);
  for (let v = 0; v < 256; v++) {
    const db = v / 2 - 130;
    const t = Math.min(1, Math.max(0, (db - floor) / Math.max(1, ceiling - floor)));
    let i = 0;
    while (i < STOPS.length - 2 && t > STOPS[i + 1][0]) i++;
    const [t0, c0] = STOPS[i];
    const [t1, c1] = STOPS[i + 1];
    const f = (t - t0) / (t1 - t0 || 1);
    for (let k = 0; k < 3; k++) p[v * 3 + k] = c0[k] + (c1[k] - c0[k]) * f;
  }
  return p;
}

/** A waterfall on a canvas `bins` pixels wide: new rows come in at the top. */
export class Waterfall {
  private ctx: CanvasRenderingContext2D;
  private img: ImageData | null = null;
  constructor(private canvas: HTMLCanvasElement) {
    this.ctx = canvas.getContext('2d', { willReadFrequently: false })!;
  }

  push(row: Uint8Array, pal: Uint8ClampedArray, mark = false) {
    const c = this.canvas;
    if (c.width !== row.length) {
      c.width = row.length;
      this.img = null;
    }
    if (!this.img) this.img = this.ctx.createImageData(row.length, 1);
    const d = this.img.data;
    for (let i = 0; i < row.length; i++) {
      const v = row[i] * 3;
      d[i * 4] = mark ? 255 : pal[v];
      d[i * 4 + 1] = mark ? 255 : pal[v + 1];
      d[i * 4 + 2] = mark ? 255 : pal[v + 2];
      d[i * 4 + 3] = mark ? 90 : 255;
    }
    this.ctx.drawImage(c, 0, 0, c.width, c.height - 1, 0, 1, c.width, c.height - 1);
    this.ctx.putImageData(this.img, 0, 0);
  }

  clear() {
    this.ctx.fillStyle = '#000008';
    this.ctx.fillRect(0, 0, this.canvas.width, this.canvas.height);
  }
}

/** Sizes a canvas to its box at the screen's pixel density; returns the CSS size. */
export function fit(c: HTMLCanvasElement): { w: number; h: number; dpr: number } {
  const r = c.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  const w = Math.max(1, Math.round(r.width * dpr));
  const h = Math.max(1, Math.round(r.height * dpr));
  if (c.width !== w || c.height !== h) {
    c.width = w;
    c.height = h;
  }
  return { w: r.width, h: r.height, dpr };
}

/** Nice tick spacing (Hz) for a span shown `px` wide. */
export function tickStep(span: number, px: number): number {
  const want = span / Math.max(2, px / 90);
  const p = 10 ** Math.floor(Math.log10(want));
  for (const m of [1, 2, 2.5, 5, 10]) if (p * m >= want) return p * m;
  return p * 10;
}

/** Colours from the theme, for drawing in canvases. */
export function themeColors(el: Element) {
  const cs = getComputedStyle(el);
  const v = (n: string, d: string) => cs.getPropertyValue(n).trim() || d;
  return { text: v('--text-3', '#888'), grid: v('--border', 'rgba(128,128,128,.2)'), accent: v('--accent', '#5b6cff'), bg: v('--surface', '#fff'), strong: v('--text', '#111'), danger: v('--danger', '#e5484d') };
}
