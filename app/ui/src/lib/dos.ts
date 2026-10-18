/** The DOS view's helpers: the PC keyboard (KeyboardEvent.code → libretro key codes, which
 *  DOSBox turns into PC scan codes) and the PC's settings. */

/** PC keys by KeyboardEvent.code (libretro's RETROK_* values). */
const CODES: Record<string, number> = {
  Backspace: 8, Tab: 9, Enter: 13, Pause: 19, Escape: 27, Space: 32, Quote: 39, Comma: 44, Minus: 45, Period: 46, Slash: 47,
  Semicolon: 59, Equal: 61, BracketLeft: 91, Backslash: 92, BracketRight: 93, Backquote: 96, Delete: 127, IntlBackslash: 92,
  Numpad0: 256, Numpad1: 257, Numpad2: 258, Numpad3: 259, Numpad4: 260, Numpad5: 261, Numpad6: 262, Numpad7: 263, Numpad8: 264, Numpad9: 265,
  NumpadDecimal: 266, NumpadDivide: 267, NumpadMultiply: 268, NumpadSubtract: 269, NumpadAdd: 270, NumpadEnter: 271, NumpadEqual: 272,
  ArrowUp: 273, ArrowDown: 274, ArrowRight: 275, ArrowLeft: 276, Insert: 277, Home: 278, End: 279, PageUp: 280, PageDown: 281,
  NumLock: 300, CapsLock: 301, ScrollLock: 302, ShiftRight: 303, ShiftLeft: 304, ControlRight: 305, ControlLeft: 306, AltRight: 307, AltLeft: 308,
  PrintScreen: 316,
};

/** The PC key for a key on this Mac, or null. */
export function pcKey(code: string): number | null {
  if (/^Key[A-Z]$/.test(code)) return code.charCodeAt(3) + 32; // RETROK_a = 'a'
  if (/^Digit[0-9]$/.test(code)) return code.charCodeAt(5);
  const f = code.match(/^F([0-9]{1,2})$/);
  if (f && +f[1] >= 1 && +f[1] <= 12) return 281 + +f[1]; // RETROK_F1 = 282
  return CODES[code] ?? null;
}

export interface DosSettings {
  /** Memory in MB. */
  memory: '4' | '8' | '16' | '32' | '64';
  /** CPU speed: DOSBox cycles ("auto", "max" or a number). */
  speed: string;
  sound: 'sb16' | 'sbpro2' | 'sbpro1' | 'sb2' | 'sb1' | 'none';
  graphics: 'svga' | 'vga' | 'ega' | 'cga' | 'tandy' | 'hercules';
  svga: 'svga_s3' | 'vesa_nolfb' | 'vesa_oldvbe' | 'svga_et4000' | 'svga_et3000' | 'svga_paradise';
  svgamem: '1' | '2' | '4' | '8';
  gus: boolean;
  keyboard: string;
}

export const DEFAULTS: DosSettings = { memory: '8', speed: 'auto', sound: 'sb16', graphics: 'svga', svga: 'svga_s3', svgamem: '2', gus: false, keyboard: 'us' };

export const SPEEDS: [string, string][] = [
  ['auto', 'Auto (detects what a game needs)'],
  ['315', '8088, 4.77 MHz (1981)'],
  ['1320', '286, 6 MHz'],
  ['2750', '286, 12 MHz'],
  ['4720', '386, 20 MHz'],
  ['7800', '386DX, 33 MHz'],
  ['13400', '486DX, 33 MHz'],
  ['26800', '486DX2, 66 MHz (DOOM)'],
  ['77000', 'Pentium, 100 MHz'],
  ['max', 'As fast as possible'],
];

export function loadSettings(): DosSettings {
  try {
    return { ...DEFAULTS, ...JSON.parse(localStorage.getItem('rad-dos-settings') ?? '{}') };
  } catch {
    return { ...DEFAULTS };
  }
}

export function saveSettings(s: DosSettings) {
  try {
    localStorage.setItem('rad-dos-settings', JSON.stringify(s));
  } catch {
    /* private mode */
  }
}

/** Disk images recently put in the drives (paths on this Mac). */
export function recentImages(): string[] {
  try {
    return JSON.parse(localStorage.getItem('rad-dos-images') ?? '[]');
  } catch {
    return [];
  }
}

export function rememberImage(path: string) {
  try {
    localStorage.setItem('rad-dos-images', JSON.stringify([path, ...recentImages().filter((p) => p !== path)].slice(0, 12)));
  } catch {
    /* private mode */
  }
}

/** A picture from the engine: `[w][h][format]` then palette + indices, or RGBA. */
export function decodeFrame(buf: ArrayBuffer, img: ImageData | null): ImageData | null {
  const v = new DataView(buf);
  if (buf.byteLength < 12) return null;
  const w = v.getUint32(0, true);
  const h = v.getUint32(4, true);
  const fmt = v.getUint8(8);
  if (!w || !h) return null;
  if (!img || img.width !== w || img.height !== h) img = new ImageData(w, h);
  const out = new Uint32Array(img.data.buffer);
  const bytes = new Uint8Array(buf);
  if (fmt === 1) {
    const lut = new Uint32Array(256);
    for (let i = 0; i < 256; i++) lut[i] = 0xff000000 | (bytes[12 + i * 3 + 2] << 16) | (bytes[12 + i * 3 + 1] << 8) | bytes[12 + i * 3];
    const px = bytes.subarray(12 + 768, 12 + 768 + w * h);
    for (let i = 0; i < px.length; i++) out[i] = lut[px[i]];
  } else {
    img.data.set(bytes.subarray(12, 12 + w * h * 4));
  }
  return img;
}
