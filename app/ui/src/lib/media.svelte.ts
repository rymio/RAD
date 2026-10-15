// The Music and Video players' state. Each drives its own libVLC engine process (see
// app/src-tauri/src/media.rs); both live for the whole session, so music keeps playing — and
// moves on to the next track — while other views are open.

import { listen } from '@tauri-apps/api/event';
import { api, rad, RadError } from './api';
import { store } from './state.svelte';
import type * as T from './types';

/** Music plays the sound of video files too. */
export const AUDIO_EXT = ['mp3', 'flac', 'wav', 'aac', 'm4a', 'alac', 'ogg', 'oga', 'opus', 'wma', 'aif', 'aiff', 'ape', 'wv', 'mka', 'mpc', 'ac3', 'dts', 'mid', 'midi', 'mp4', 'm4v', 'mkv', 'mov', 'wmv', 'avi', 'webm', 'mpg', 'mpeg'];
export const VIDEO_EXT = ['mp4', 'm4v', 'mkv', 'mov', 'avi', 'wmv', 'asf', 'webm', 'mpg', 'mpeg', 'mpe', 'm2v', 'm1v', 'ts', 'm2ts', 'mts', 'vob', 'flv', 'f4v', '3gp', '3g2', 'ogv', 'divx', 'xvid', 'rm', 'rmvb'];
export const SUBTITLE_EXT = ['srt', 'ass', 'ssa', 'vtt', 'sub', 'idx', 'smi'];

export const ext = (name: string) => (name.match(/\.([^./\\]+)$/)?.[1] ?? '').toLowerCase();
export const baseName = (path: string) => path.split(/[\\/]/).pop() ?? path;
const stem = (name: string) => name.replace(/\.[^./\\]+$/, '');

/** Is this vault file something the player can open? */
export function playableDoc(d: T.Document, kind: 'audio' | 'video') {
  const list = kind === 'audio' ? AUDIO_EXT : VIDEO_EXT;
  return list.includes(ext(d.original_name)) || d.mime.startsWith(kind === 'audio' ? 'audio/' : 'video/');
}

export function fmtTime(ms: number | undefined | null) {
  if (ms == null || !isFinite(ms) || ms < 0) return '--:--';
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, '0');
  return h ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`;
}

let keySeq = 0;
const newKey = () => `${Date.now().toString(36)}-${(++keySeq).toString(36)}`;

export const fileItem = (path: string): T.MediaItem => ({ key: newKey(), src: { kind: 'file', path }, title: stem(baseName(path)) });
export const vaultItem = (d: T.Document): T.MediaItem => ({ key: newKey(), src: { kind: 'vault', id: d.id }, title: d.title || stem(d.original_name) });

/** What the engine opens: a path, or the loopback address of a vault file. */
async function resolve(src: T.MediaSource) {
  return src.kind === 'file' ? src.path : rad.mediaVaultUrl(src.id);
}

/** Files on this computer, chosen with the system's open dialog. */
export async function pickFiles(kind: 'audio' | 'video' | 'subtitle', multiple = true): Promise<string[]> {
  const { open } = await import('@tauri-apps/plugin-dialog');
  const [name, extensions] = kind === 'audio' ? ['Audio', AUDIO_EXT] : kind === 'video' ? ['Video', VIDEO_EXT] : ['Subtitles', SUBTITLE_EXT];
  const r = await open({ multiple, filters: [{ name, extensions }, { name: 'All files', extensions: ['*'] }] });
  return r == null ? [] : Array.isArray(r) ? r : [r];
}

/** Every playable file in a folder (and its subfolders), in name order. */
export async function pickFolder(kind: 'audio' | 'video', limit = 5000): Promise<string[]> {
  const { open } = await import('@tauri-apps/plugin-dialog');
  const dir = await open({ directory: true });
  if (typeof dir !== 'string') return [];
  const list = kind === 'audio' ? AUDIO_EXT : VIDEO_EXT;
  const out: string[] = [];
  const walk = async (path: string, depth: number) => {
    if (out.length >= limit) return;
    const l = await rad.fsList(path).catch(() => null);
    if (!l) return;
    for (const e of l.entries) {
      if (e.hidden) continue;
      if (e.is_dir) {
        if (depth < 6) await walk(e.path, depth + 1);
      } else if (list.includes(ext(e.name)) && out.length < limit) out.push(e.path);
    }
  };
  await walk(dir, 0);
  return out;
}

const DEFAULT_STATUS: T.MediaStatus = { state: 'idle', has_media: false, time: 0, length: 0, seekable: false, rate: 1, volume: 100, muted: false, video: null, aspect: null };

/** One engine process: start/attach, commands, and the events it reports. */
class MediaController {
  engine = $state<'idle' | 'starting' | 'running' | 'missing' | 'stopped'>('idle');
  engineError = $state('');
  version = $state('');
  status = $state<T.MediaStatus>({ ...DEFAULT_STATUS });
  meta = $state<T.MediaMeta | null>(null);
  tracks = $state<T.MediaTracks | null>(null);
  eqInfo = $state<T.EqInfo | null>(null);
  /** Called once each time playback reaches the end of a file. */
  onEnded: (() => void) | null = null;
  onMeta: ((m: T.MediaMeta) => void) | null = null;

  private listening = false;
  private probes = new Map<string, (v: { meta: T.MediaMeta | null; duration: number }) => void>();
  private probeSeq = 0;

  constructor(readonly player: T.MediaPlayerKind) {}

  async start() {
    if (this.engine === 'running' || this.engine === 'starting') return;
    if (!this.listening) {
      this.listening = true;
      await listen<any>('media-event', (e) => e.payload.player === this.player && this.onEvent(e.payload));
    }
    this.engine = 'starting';
    this.engineError = '';
    try {
      const last = await rad.mediaStart(this.player);
      for (const v of Object.values(last ?? {})) this.onEvent(v);
      this.engine = 'running';
    } catch (e) {
      this.engine = e instanceof RadError && e.code === 'media_missing' ? 'missing' : 'stopped';
      this.engineError = e instanceof Error ? e.message : String(e);
    }
  }

  /** For a view being opened: starts the engine, or has it resend its state and last picture. */
  async attach() {
    if (this.engine !== 'running') return this.start();
    try {
      const last = await rad.mediaStart(this.player);
      for (const v of Object.values(last ?? {})) this.onEvent(v);
    } catch {
      this.engine = 'stopped';
      await this.start();
    }
  }

  send(cmd: Record<string, unknown>) {
    if (this.engine !== 'running') return;
    rad.mediaSend(this.player, cmd).catch(() => {});
  }

  async open(src: T.MediaSource, opts: Record<string, unknown> = {}) {
    await this.start();
    if (this.engine !== 'running') return false;
    try {
      this.meta = null;
      this.tracks = null;
      this.send({ cmd: 'open', src: await resolve(src), ...opts });
      return true;
    } catch (e) {
      store.error(e, 'Couldn’t open the file');
      return false;
    }
  }

  /** Tags and duration of a local file, read without playing it. */
  probe(path: string): Promise<{ meta: T.MediaMeta | null; duration: number }> {
    const id = `p${++this.probeSeq}`;
    return new Promise((resolve) => {
      this.probes.set(id, resolve);
      this.send({ cmd: 'probe', id, src: path });
      setTimeout(() => this.probes.delete(id) && resolve({ meta: null, duration: 0 }), 10_000);
    });
  }

  private onEvent(ev: any) {
    switch (ev.type) {
      case 'ready':
        this.version = ev.version ?? '';
        this.eqInfo = { freqs: ev.freqs ?? [], presets: ev.presets ?? [] };
        break;
      case 'status': {
        const was = this.status.state;
        const { type: _t, player: _p, ...st } = ev;
        this.status = st as T.MediaStatus;
        if (st.state === 'ended' && was !== 'ended') this.onEnded?.();
        break;
      }
      case 'meta':
        this.meta = ev;
        this.onMeta?.(ev);
        break;
      case 'tracks':
        this.tracks = ev;
        break;
      case 'probe': {
        const r = this.probes.get(ev.id);
        this.probes.delete(ev.id);
        r?.({ meta: ev.meta ?? null, duration: ev.duration ?? 0 });
        break;
      }
      case 'error':
        store.toast('error', this.player === 'music' ? 'Music' : 'Video', ev.message);
        break;
      case 'fatal':
        this.engineError = ev.message;
        this.engine = 'stopped';
        break;
      case 'exited':
        this.engine = 'stopped';
        this.status = { ...DEFAULT_STATUS };
        for (const r of this.probes.values()) r({ meta: null, duration: 0 });
        this.probes.clear();
        break;
    }
  }
}

export type Repeat = 'off' | 'all' | 'one';
export interface EqSettings { enabled: boolean; preset: string; preamp: number; bands: number[] }

function shuffled<T>(a: T[]) {
  const r = [...a];
  for (let i = r.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [r[i], r[j]] = [r[j], r[i]];
  }
  return r;
}

/** A playlist on one controller, saved in the vault (`<name>_playlist`, `<name>_settings`). */
class Playlist {
  items = $state<T.MediaItem[]>([]);
  /** Key of the entry that is loaded in the engine. */
  current = $state<string | null>(null);
  shuffle = $state(false);
  repeat = $state<Repeat>('all');
  volume = $state(80);
  muted = $state(false);
  eq = $state<EqSettings>({ enabled: false, preset: 'Flat', preamp: 0, bands: Array(10).fill(0) });

  private loaded = false;
  private order: string[] = [];
  private saveTimer: ReturnType<typeof setTimeout> | undefined;
  private probing = false;

  constructor(readonly c: MediaController, readonly name: 'music' | 'video', readonly kind: 'audio' | 'video') {
    c.onEnded = () => this.ended();
    c.onMeta = (m) => this.adoptMeta(m);
    if (name === 'video') this.repeat = 'off';
  }

  get index() {
    return this.items.findIndex((i) => i.key === this.current);
  }
  get currentItem() {
    return this.items.find((i) => i.key === this.current) ?? null;
  }

  async load() {
    if (this.loaded) return;
    this.loaded = true;
    const [list, set] = await Promise.all([
      api.get<any>(`/v1/app-data/${this.name}_playlist`).catch(() => null),
      api.get<any>(`/v1/app-data/${this.name}_settings`).catch(() => null),
    ]);
    if (Array.isArray(list?.items)) {
      this.items = list.items.filter((i: any) => i && i.src && i.key);
      this.current = this.items.some((i) => i.key === list.current) ? list.current : null;
    }
    if (set && typeof set === 'object') {
      if (typeof set.shuffle === 'boolean') this.shuffle = set.shuffle;
      if (['off', 'all', 'one'].includes(set.repeat)) this.repeat = set.repeat;
      if (typeof set.volume === 'number') this.volume = set.volume;
      if (set.eq && Array.isArray(set.eq.bands)) this.eq = { ...this.eq, ...set.eq };
    }
    this.probeMissing();
  }

  /** Saves the playlist and settings shortly after the last change. */
  save() {
    clearTimeout(this.saveTimer);
    this.saveTimer = setTimeout(() => {
      const items = this.items.map(({ key, src, title, artist, album, duration }) => ({ key, src, title, artist, album, duration }));
      api.put(`/v1/app-data/${this.name}_playlist`, { items, current: this.current }).catch((e) => store.error(e, 'Couldn’t save the playlist'));
      api.put(`/v1/app-data/${this.name}_settings`, { shuffle: this.shuffle, repeat: this.repeat, volume: this.volume, eq: this.eq }).catch(() => {});
    }, 800);
  }

  add(items: T.MediaItem[], playFirst = false) {
    if (!items.length) return;
    this.items = [...this.items, ...items];
    this.order = [];
    this.save();
    this.probeMissing();
    if (playFirst) this.play(items[0].key);
  }

  remove(keys: Set<string>) {
    if (keys.has(this.current ?? '')) {
      this.c.send({ cmd: 'close' });
      this.current = null;
    }
    this.items = this.items.filter((i) => !keys.has(i.key));
    this.order = [];
    this.save();
  }

  clear() {
    this.remove(new Set(this.items.map((i) => i.key)));
  }

  move(from: number, to: number) {
    if (from === to || from < 0 || to < 0 || from >= this.items.length || to >= this.items.length) return;
    const a = [...this.items];
    const [it] = a.splice(from, 1);
    a.splice(to, 0, it);
    this.items = a;
    this.save();
  }

  /** Applies volume, mute and the equalizer to the engine. */
  sync() {
    this.c.send({ cmd: 'volume', value: this.volume });
    this.c.send({ cmd: 'mute', value: this.muted });
    this.c.send({ cmd: 'eq', enabled: this.eq.enabled, preamp: this.eq.preamp, bands: this.eq.bands });
  }

  async play(key?: string, opts: Record<string, unknown> = {}) {
    const k = key ?? this.current ?? this.items[0]?.key;
    const it = this.items.find((i) => i.key === k);
    if (!it) return;
    if (k === this.current && !key && this.c.status.has_media) {
      this.c.send({ cmd: 'play' });
      return;
    }
    this.current = it.key;
    this.save();
    await this.c.start();
    this.sync();
    await this.c.open(it.src, opts);
  }

  toggle() {
    if (!this.c.status.has_media) this.play();
    else this.c.send({ cmd: 'toggle' });
  }

  stop() {
    this.c.send({ cmd: 'stop' });
  }

  /** The play order: the list, or a shuffled copy fixed until the list changes. */
  private sequence() {
    const keys = this.items.map((i) => i.key);
    if (!this.shuffle) return keys;
    if (this.order.length !== keys.length) this.order = shuffled(keys);
    return this.order;
  }

  step(dir: 1 | -1, auto = false) {
    const seq = this.sequence();
    if (!seq.length) return;
    // "Previous" a few seconds into a track starts it again, like most players.
    if (dir === -1 && !auto && this.c.status.time > 4000) {
      this.c.send({ cmd: 'seek', ms: 0 });
      return;
    }
    let i = seq.indexOf(this.current ?? '');
    i += dir;
    if (i >= seq.length || i < 0) {
      if (auto && this.repeat === 'off') return;
      if (this.shuffle && dir === 1) this.order = shuffled(seq);
      i = (i + seq.length) % seq.length;
    }
    this.play(this.sequence()[i]);
  }

  private ended() {
    if (this.repeat === 'one') this.play(this.current ?? undefined);
    else this.step(1, true);
  }

  setVolume(v: number) {
    this.volume = Math.max(0, Math.min(100, Math.round(v)));
    this.c.send({ cmd: 'volume', value: this.volume });
    this.save();
  }

  setMuted(m: boolean) {
    this.muted = m;
    this.c.send({ cmd: 'mute', value: m });
  }

  setEq(eq: Partial<EqSettings>) {
    this.eq = { ...this.eq, ...eq };
    this.c.send({ cmd: 'eq', enabled: this.eq.enabled, preamp: this.eq.preamp, bands: this.eq.bands });
    this.save();
  }

  /** The engine's tags for what is playing; for vault files the file's name is kept. */
  adoptMeta(m: T.MediaMeta | null) {
    const it = this.currentItem;
    if (!it || !m) return;
    const len = this.c.status.length;
    let changed = false;
    if (len > 0 && it.duration !== len) (it.duration = len), (changed = true);
    if (m.artist && it.artist !== m.artist) (it.artist = m.artist), (changed = true);
    if (m.album && it.album !== m.album) (it.album = m.album), (changed = true);
    if (m.title && m.artist && it.title !== m.title) (it.title = m.title), (changed = true);
    if (changed) this.save();
  }

  /** Reads tags and durations of local files that haven't been read yet, one at a time. */
  private async probeMissing() {
    if (this.probing || this.kind !== 'audio') return;
    this.probing = true;
    try {
      for (;;) {
        const it = this.items.find((i) => i.src.kind === 'file' && i.duration === undefined);
        if (!it || it.src.kind !== 'file') break;
        await this.c.start();
        if (this.c.engine !== 'running') break;
        const r = await this.c.probe(it.src.path);
        it.duration = r.duration || 0;
        if (r.meta?.artist) it.artist = r.meta.artist;
        if (r.meta?.album) it.album = r.meta.album;
        if (r.meta?.title && r.meta.title !== baseName(it.src.path)) it.title = r.meta.title;
      }
      this.save();
    } finally {
      this.probing = false;
    }
  }
}

export const musicEngine = new MediaController('music');
export const videoEngine = new MediaController('video');
export const music = new Playlist(musicEngine, 'music', 'audio');
export const video = new Playlist(videoEngine, 'video', 'video');
