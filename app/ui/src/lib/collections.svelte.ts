// Collections: one folder on this computer for everything the owner drops on RAD — whole folders
// or single files (PDF, MP3, FLAC, M4A, MP4). Kept in RAD's data folder (app/src-tauri/src/
// collections.rs); dropped folders keep their shape. Files open right here (PDFs read, music
// listened to, films watched), streamed from rad://localhost/collections/…, and can be moved on
// to Media: PDFs to Books, music to the Music player, films to the Video player.
//
// Gilgamesh sees the folder that's open and what's playing (activity.seen), and "Add to RAG"
// uploads a file to the vault's "Collections" project, so every chat can search it.

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { api, rad, radUrl } from './api';
import { store } from './state.svelte';
import { activity } from './gilgamesh/observe.svelte';
import { books } from './books.svelte';
import { music, video, musicEngine, fileItem } from './media.svelte';
import type * as T from './types';

export const COLLECTION_EXTENSIONS = ['pdf', 'mp3', 'mp4', 'flac', 'm4a'];
const PROJECT = 'Collections';

export type Kind = 'pdf' | 'audio' | 'video';

export interface Entry {
  name: string;
  /** Relative to the collection, `/`-separated. */
  path: string;
  is_dir: boolean;
  size: number;
  modified: number;
  kind: Kind | null;
  /** Folders: files in it (subfolders too), and by kind. */
  count: number;
  pdf: number;
  audio: number;
  video: number;
}

export interface FileRef {
  path: string;
  name: string;
  kind: Kind;
  size?: number;
}

export const kindOf = (name: string): Kind | null => {
  const e = name.split('.').pop()?.toLowerCase() ?? '';
  return e === 'pdf' ? 'pdf' : e === 'mp4' ? 'video' : ['mp3', 'flac', 'm4a'].includes(e) ? 'audio' : null;
};

/** Where the webview streams a file from. */
export const fileUrl = (path: string) => radUrl(`/collections/${path.split('/').map(encodeURIComponent).join('/')}`);

export const stem = (name: string) => name.replace(/\.[^.]+$/, '');

class CollectionsState {
  /** The folder being shown ("" is the top). */
  dir = $state('');
  entries = $state<Entry[]>([]);
  loaded = $state(false);
  loading = $state(false);
  /** Files copied so far in the import under way, and the latest one. */
  importing = $state<{ done: number; name: string } | null>(null);
  moving = $state(false);

  /** The PDF or film open in the viewer. */
  viewing = $state<FileRef | null>(null);
  /** The music being listened to: the queue and where in it. */
  queue = $state<FileRef[]>([]);
  at = $state(-1);
  /** Vault documents of files added to RAG this session (by path). */
  inVault = $state<Record<string, number>>({});

  /** The music player's state (an <audio> outside any view, so it plays on in other views). */
  paused = $state(true);
  time = $state(0);
  duration = $state(0);
  volume = $state(0.9);
  audioError = $state('');

  private listening = false;
  private audio: HTMLAudioElement | null = null;

  get playing(): FileRef | null {
    return this.queue[this.at] ?? null;
  }

  async load(dir = this.dir): Promise<void> {
    if (!this.listening) {
      this.listening = true;
      await listen<{ name: string; done: number }>('collections-progress', (e) => {
        if (this.importing) this.importing = { done: e.payload.done, name: e.payload.name };
      });
    }
    this.loading = true;
    try {
      const r = await invoke<{ dir: string; entries: Entry[] }>('collections_list', { dir });
      this.dir = r.dir;
      this.entries = r.entries;
    } catch (e: any) {
      // The folder went (renamed or moved away): back to the top.
      if (dir) return this.load('');
      throw e;
    } finally {
      this.loading = false;
      this.loaded = true;
      this.tell();
    }
  }

  open(dir: string) {
    return this.load(dir).catch((e) => store.error(e, 'Couldn’t open the folder'));
  }

  /** Copies dropped (or picked) files and folders into `into` (the open folder by default). */
  async import(paths: string[], into = this.dir) {
    if (!paths.length || this.importing) return;
    this.importing = { done: 0, name: '' };
    try {
      const r = await invoke<{ added: number; skipped: number; errors: string[] }>('collections_import', { paths, into });
      for (const e of r.errors.slice(0, 3)) store.toast('error', 'Couldn’t add a file', e);
      if (r.added) store.toast('success', `${r.added} file${r.added > 1 ? 's' : ''} added to Collections`, r.skipped ? `${r.skipped} other file${r.skipped > 1 ? 's were' : ' was'} left out (Collections takes PDF, MP3, FLAC, M4A and MP4).` : 'Kept on this computer, in RAD’s Collections folder.');
      else if (!r.errors.length) store.toast('info', 'Nothing to add', 'Collections takes PDF, MP3, FLAC, M4A and MP4 files, and folders with them in.');
    } catch (e) {
      store.error(e, 'Couldn’t add to Collections');
    } finally {
      this.importing = null;
      await this.load().catch(() => {});
    }
  }

  async mkdir(name: string) {
    const path = await invoke<string>('collections_mkdir', { dir: this.dir, name });
    await this.load();
    return path;
  }

  async rename(e: Entry, name: string) {
    if (!name.trim() || name.trim() === e.name) return;
    await invoke<string>('collections_rename', { path: e.path, name: name.trim() });
    this.forget([e.path]);
    await this.load();
  }

  async remove(paths: string[]) {
    await invoke('collections_delete', { paths });
    this.forget(paths);
    await this.load();
  }

  /** Stops showing or playing what's gone (or moved). */
  private forget(paths: string[]) {
    const gone = (p: string) => paths.some((x) => p === x || p.startsWith(`${x}/`));
    if (this.viewing && gone(this.viewing.path)) this.viewing = null;
    if (this.playing && gone(this.playing.path)) this.stopListening();
    else if (this.queue.some((f) => gone(f.path))) {
      const cur = this.playing;
      this.queue = this.queue.filter((f) => !gone(f.path));
      this.at = cur ? this.queue.indexOf(cur) : -1;
    }
  }

  /** Moves files (and folders' contents) to Media: PDFs to Books, music to Music, films to Video. */
  async toMedia(paths: string[]) {
    if (!paths.length || this.moving) return;
    this.moving = true;
    this.forget(paths);
    try {
      const r = await invoke<{ music: string[]; video: string[]; books: number; errors: string[] }>('collections_to_media', { paths });
      // The playlists are read first, so adding to them never replaces what's saved.
      if (r.music.length) {
        await music.load();
        music.add(r.music.map(fileItem));
      }
      if (r.video.length) {
        await video.load();
        video.add(r.video.map(fileItem));
      }
      if (r.books) await books.refresh().catch(() => {});
      for (const e of r.errors.slice(0, 3)) store.toast('error', 'Couldn’t move a file to Books', e);
      const parts = [r.music.length && `${r.music.length} to Music`, r.video.length && `${r.video.length} to Video`, r.books && `${r.books} to Books`].filter(Boolean);
      if (parts.length) store.toast('success', 'Moved to Media', `${parts.join(', ')}.`);
      else if (!r.errors.length) store.toast('info', 'Nothing to move');
    } catch (e) {
      store.error(e, 'Couldn’t move to Media');
    } finally {
      this.moving = false;
      await this.load().catch(() => {});
    }
  }

  // ---------------------------------------------------------------- open, watch, listen

  /** Opens a file: PDFs and films in the viewer, music in the player (with the rest of its folder). */
  async play(f: FileRef) {
    if (f.kind === 'audio') {
      const here = this.entries.filter((e) => e.kind === 'audio').map((e) => ({ path: e.path, name: e.name, kind: 'audio' as const, size: e.size }));
      const q = here.some((x) => x.path === f.path) ? here : [f];
      this.listen(q, q.findIndex((x) => x.path === f.path));
    } else {
      this.viewing = f;
      if (f.kind === 'video') this.quietMusic();
      this.tell();
    }
  }

  /** Every song in a folder (and its subfolders), from the first. */
  async playFolder(e: Entry) {
    const files = await invoke<FileRef[]>('collections_files', { dir: e.path });
    const songs = files.filter((f) => f.kind === 'audio');
    if (songs.length) this.listen(songs, 0);
    else {
      const first = files.find((f) => f.kind === 'video') ?? files[0];
      if (first) await this.play(first);
    }
  }

  listen(queue: FileRef[], at: number) {
    this.queue = queue;
    this.at = Math.max(0, at);
    this.load_();
  }

  private player() {
    if (this.audio) return this.audio;
    const a = new Audio();
    a.preload = 'auto';
    a.volume = this.volume;
    a.addEventListener('play', () => {
      this.paused = false;
      this.quietMusic();
    });
    a.addEventListener('pause', () => (this.paused = true));
    a.addEventListener('timeupdate', () => (this.time = a.currentTime));
    a.addEventListener('durationchange', () => (this.duration = isFinite(a.duration) ? a.duration : 0));
    a.addEventListener('ended', () => {
      if (!this.step(1)) this.paused = true;
    });
    a.addEventListener('error', () => {
      if (!a.getAttribute('src')) return;
      this.audioError = 'This file can’t be played here.';
      this.paused = true;
    });
    return (this.audio = a);
  }

  /** Loads the song at `at` and plays it. */
  private load_() {
    const f = this.playing;
    const a = this.player();
    this.audioError = '';
    this.time = 0;
    this.duration = 0;
    if (!f) {
      a.pause();
      a.removeAttribute('src');
      a.load();
      this.paused = true;
    } else {
      a.src = fileUrl(f.path);
      a.play().catch(() => {});
    }
    this.tell();
  }

  step(by: 1 | -1) {
    // "Previous" a few seconds into a song starts it again.
    if (by === -1 && this.time > 4 && this.audio) {
      this.audio.currentTime = 0;
      return true;
    }
    const i = this.at + by;
    if (i < 0 || i >= this.queue.length) return false;
    this.at = i;
    this.load_();
    return true;
  }

  toggle() {
    const a = this.audio;
    if (!a || !this.playing) return;
    if (a.paused) a.play().catch(() => {});
    else a.pause();
  }

  seek(seconds: number) {
    if (this.audio) this.audio.currentTime = Math.max(0, Math.min(seconds, this.duration || seconds));
  }

  setVolume(v: number) {
    this.volume = Math.max(0, Math.min(1, v));
    if (this.audio) this.audio.volume = this.volume;
  }

  stopListening() {
    this.queue = [];
    this.at = -1;
    this.load_();
  }

  /** For a file the webview can't play: the Music or Video player (libVLC) plays anything. */
  async playInMedia(f: FileRef) {
    const abs = await invoke<string>('collections_path', { path: f.path });
    if (f.kind === 'audio') {
      this.audio?.pause();
      await music.load();
      music.add([fileItem(abs)], true);
      store.navigate({ kind: 'music' });
    } else {
      await video.load();
      video.add([fileItem(abs)], true);
      store.navigate({ kind: 'video' });
    }
  }

  closeViewer() {
    this.viewing = null;
    this.tell();
  }

  /** One thing at a time: the Music player pauses (it resumes from the Music view). */
  quietMusic() {
    if (musicEngine.status.state === 'playing') musicEngine.send({ cmd: 'pause' });
  }

  // ---------------------------------------------------------------- the vault

  private async project(): Promise<T.Project> {
    const p = store.projects.find((p) => p.name === PROJECT);
    if (p) return p;
    const created = await api.post<T.Project>('/v1/projects', { name: PROJECT, color: '#0e7490', icon: 'folder', description: 'Files from Collections, searchable from any chat' });
    store.projects = [...store.projects, created];
    return created;
  }

  /** Uploads a file to the vault's Collections project, where it is indexed for search (RAG). */
  async addToRag(path: string) {
    const [p, abs] = await Promise.all([this.project(), invoke<string>('collections_path', { path })]);
    const res = await rad.uploadPaths(p.id, [abs]);
    const r = res[0];
    if (!r || 'error' in r) throw new Error(r && 'error' in r ? r.error : 'Upload failed');
    this.inVault = { ...this.inVault, [path]: r.document.id };
    store.toast('success', 'In your vault', r.duplicate ? 'It was there already.' : 'In the Collections project; it’s being indexed, then any chat can search it.');
  }

  async reveal(path: string) {
    rad.fsReveal(await invoke<string>('collections_path', { path })).catch((e) => store.error(e));
  }

  /** What Gilgamesh sees of Collections. */
  private tell() {
    const where = this.dir ? `the folder “${this.dir}”` : 'the top of Collections';
    const files = this.entries.filter((e) => !e.is_dir).map((e) => e.name);
    const dirs = this.entries.filter((e) => e.is_dir).map((e) => e.name);
    const bits = [`${where}: ${dirs.length} folder${dirs.length === 1 ? '' : 's'}${dirs.length ? ` (${dirs.slice(0, 5).join(', ')})` : ''}, ${files.length} file${files.length === 1 ? '' : 's'}`];
    if (this.viewing) bits.push(`${this.viewing.kind === 'pdf' ? 'reading' : 'watching'} “${this.viewing.name}”`);
    if (this.playing) bits.push(`listening to “${stem(this.playing.name)}”`);
    activity.seen('collections', bits.join('; '));
  }
}

export const collections = new CollectionsState();
