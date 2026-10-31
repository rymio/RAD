// Books (Media → Books): the owner's own e-books (PDF, EPUB, MOBI / AZW / AZW3, FB2, TXT, HTML),
// kept on this computer (app/src-tauri/src/books.rs), read in RAD and read aloud.
//
// The reader (components/BookReader.svelte) attaches itself here while a book is open: it says
// what is on the page and hands out the text to read, sentence by sentence. Reading aloud runs
// here: the next sentence is synthesised while one plays, each one shows where it is (the
// reader turns the page when the reading reaches the next one), and in Gilgamesh's voice his
// mouth reads along on every picture of him.
//
// Gilgamesh sees the page in front of the owner (describe(), in every answer), so he can explain
// it, sum it up or read it ([read: …] in gilgamesh/tools.ts). Talking to him pauses the reading.
// "Add to RAG" uploads a book to the vault's "Books" project, so every chat can search it.

import { invoke } from '@tauri-apps/api/core';
import { api, rad } from './api';
import { store } from './state.svelte';
import { chunks } from './speech.svelte';
import { gilgamesh } from './gilgamesh/gilgamesh.svelte';
import type * as T from './types';

export const BOOK_EXTENSIONS = ['pdf', 'epub', 'mobi', 'azw', 'azw3', 'prc', 'kf8', 'fb2', 'txt', 'html', 'htm', 'xhtml'];
const PROJECT = 'Books';

/** Where the owner is in a book. Reflowable books: a chapter and the first block on the page
 *  (pages change with the window); PDFs: the page (chapter 0). */
export interface Progress {
  chapter: number;
  block: number;
  page: number;
  /** 0–1 through the whole book. */
  percent: number;
}

export interface Book {
  id: string;
  title: string;
  author: string | null;
  format: string;
  file: string;
  size: number;
  added: number;
  opened: number | null;
  /** A JPEG data URL. */
  cover: string | null;
  progress: Progress | null;
  /** The vault document, once added to RAG. */
  vault: number | null;
}

/** Something to say, and how to show where it is (turning the page if need be). */
export interface Piece {
  text: string;
  show: () => void;
}

/** The open book, as the reader shows it. */
export interface Reader {
  book: Book;
  /** The text on the page(s) in front of the owner. */
  pageText(): string;
  /** The end of the page before (for "what just happened"). */
  before(): string;
  /** Where they are, in words: “chapter 3, “The Pool of Tears”, page 2 of 9”. */
  where(): string;
  /** The text to read: from the page's top, or from where the reading stopped if that's on this page. */
  pieces(fromTop: boolean): AsyncGenerator<Piece>;
  turn(by: 1 | -1): void;
}

export interface BookSettings {
  /** Gilgamesh reads (his voice and mouth), or the narrator voice from Settings → Voice. */
  voice: 'gilgamesh' | 'narrator';
  speed: number;
  fontSize: number;
  font: 'serif' | 'sans';
  theme: 'auto' | 'paper' | 'sepia' | 'night';
  /** Two pages side by side when the window is wide enough. */
  spread: boolean;
}

const SETTINGS_KEY = 'rad-books';
const DEFAULTS: BookSettings = { voice: 'gilgamesh', speed: 1, fontSize: 19, font: 'serif', theme: 'auto', spread: true };
const PAGE_CHARS = 7000;

class Books {
  list = $state<Book[]>([]);
  loaded = $state(false);
  importing = $state(false);
  /** The book open in the reader (kept when the owner looks at another view). */
  openId = $state<string | null>(null);
  /** Reading aloud (not paused). */
  playing = $state(false);
  /** Waiting for the voice. */
  loading = $state(false);
  /** The sentence being read. */
  sentence = $state('');
  settings = $state<BookSettings>(this.loadSettings());
  reader = $state.raw<Reader | null>(null);

  private run = 0;
  private loadingList: Promise<void> | null = null;
  private saveTimer: ReturnType<typeof setTimeout> | undefined;

  private loadSettings(): BookSettings {
    try {
      return { ...DEFAULTS, ...JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? '{}') };
    } catch {
      return { ...DEFAULTS };
    }
  }

  saveSettings(patch: Partial<BookSettings>) {
    this.settings = { ...this.settings, ...patch };
    try {
      localStorage.setItem(SETTINGS_KEY, JSON.stringify(this.settings));
    } catch {
      // Not remembered this time.
    }
  }

  load(): Promise<void> {
    if (this.loaded) return Promise.resolve();
    return (this.loadingList ??= this.refresh().finally(() => (this.loadingList = null)));
  }

  async refresh() {
    this.list = await invoke<Book[]>('books_list');
    this.loaded = true;
  }

  find(id: string | null | undefined) {
    return id ? (this.list.find((b) => b.id === id) ?? null) : null;
  }

  /** The book whose title (or author) best fits some words. */
  match(words: string): Book | null {
    const w = words.toLowerCase().replace(/[“”"']/g, '').trim();
    if (!w) return null;
    return (
      this.list.find((b) => b.title.toLowerCase() === w) ??
      this.list.find((b) => b.title.toLowerCase().includes(w)) ??
      this.list.find((b) => w.split(/\s+/).every((x) => `${b.title} ${b.author ?? ''}`.toLowerCase().includes(x))) ??
      null
    );
  }

  /** Copies files into the library. */
  async import(paths: string[]) {
    paths = paths.filter((p) => BOOK_EXTENSIONS.includes(p.split('.').pop()?.toLowerCase() ?? ''));
    if (!paths.length) {
      store.toast('error', 'Not a book', 'Books takes PDF, EPUB, MOBI, AZW, AZW3, FB2, TXT and HTML files.');
      return;
    }
    this.importing = true;
    try {
      const res = await invoke<Array<{ book: Book; duplicate: boolean } | { error: string }>>('books_import', { paths });
      const added = res.filter((r): r is { book: Book; duplicate: boolean } => 'book' in r);
      for (const r of res) if ('error' in r) store.toast('error', 'Couldn’t add a book', r.error);
      await this.refresh();
      const fresh = added.filter((r) => !r.duplicate).length;
      if (fresh) store.toast('success', `${fresh} book${fresh > 1 ? 's' : ''} added`, 'Kept on this computer, in RAD’s Books folder.');
      else if (added.length) store.toast('info', 'Already in your books');
    } catch (e) {
      store.error(e, 'Couldn’t add the books');
    } finally {
      this.importing = false;
    }
  }

  async update(id: string, patch: Record<string, unknown>) {
    const b = await invoke<Book>('books_update', { id, patch });
    this.list = this.list.map((x) => (x.id === id ? b : x));
    return b;
  }

  /** Remembers where the owner is (a moment after they stop turning pages). */
  saveProgress(id: string, progress: Progress) {
    const b = this.find(id);
    if (b) b.progress = progress;
    clearTimeout(this.saveTimer);
    this.saveTimer = setTimeout(() => invoke('books_update', { id, patch: { progress } }).catch(() => {}), 800);
  }

  async remove(id: string) {
    if (this.openId === id) this.close();
    await invoke('books_delete', { id });
    this.list = this.list.filter((b) => b.id !== id);
  }

  openBook(id: string) {
    if (this.openId !== id) this.pause();
    this.openId = id;
    store.navigate({ kind: 'books' });
    void this.update(id, { opened: Date.now() }).catch(() => {});
  }

  close() {
    this.pause();
    this.openId = null;
  }

  /** The reader is showing a book (or has gone). */
  attach(r: Reader | null) {
    if (!r) this.pause();
    this.reader = r;
  }

  // ---------------------------------------------------------------- the vault

  private async project(): Promise<T.Project> {
    const p = store.projects.find((p) => p.name === PROJECT);
    if (p) return p;
    const created = await api.post<T.Project>('/v1/projects', { name: PROJECT, color: '#a16207', icon: 'book', description: 'Books from your library, searchable from any chat' });
    store.projects = [...store.projects, created];
    return created;
  }

  /** Uploads a book to the vault's Books project, where it is indexed for search (RAG). */
  async addToVault(id: string) {
    const b = this.find(id);
    if (!b) return;
    const [p, path] = await Promise.all([this.project(), invoke<string>('books_path', { id })]);
    const res = await rad.uploadPaths(p.id, [path]);
    const r = res[0];
    if (!r || 'error' in r) throw new Error(r && 'error' in r ? r.error : 'Upload failed');
    await this.update(id, { vault: r.document.id });
    store.toast('success', `“${b.title}” is in your vault`, r.duplicate ? 'It was there already.' : 'In the Books project; it’s being indexed, then any chat can search it.');
  }

  // ---------------------------------------------------------------- reading aloud

  private synth(text: string): Promise<ArrayBuffer> {
    const cfg = store.config;
    return this.settings.voice === 'gilgamesh' ? gilgamesh.synthReading(text) : gilgamesh.synthReading(text, cfg?.tts_voice || 'en_female');
  }

  /** Reads aloud from where the owner is, page after page, until paused or the book ends. */
  async play(fromTop = false) {
    const r = this.reader;
    if (!r) return;
    this.pause();
    // He doesn't read over himself.
    gilgamesh.interrupt();
    const run = ++this.run;
    this.playing = true;
    this.loading = true;
    const it = r.pieces(fromTop);
    const lips = this.settings.voice === 'gilgamesh';
    const quiet = <V>(p: Promise<V>) => (p.catch(() => {}), p);
    try {
      let cur = await it.next();
      let wav = cur.done ? null : quiet(this.synth(cur.value.text));
      while (!cur.done && wav) {
        const w = await wav;
        if (run !== this.run) return;
        // The next one is made while this one plays.
        const next = await it.next();
        const nextWav = next.done ? null : quiet(this.synth(next.value.text));
        if (run !== this.run) return;
        cur.value.show();
        this.sentence = cur.value.text;
        this.loading = false;
        await gilgamesh.recite(w, lips);
        if (run !== this.run) return;
        cur = next;
        wav = nextWav;
        this.loading = true;
      }
    } catch (e) {
      if (run === this.run) store.error(e, 'Couldn’t read aloud');
    } finally {
      if (run === this.run) {
        this.playing = false;
        this.loading = false;
        this.sentence = '';
        gilgamesh.stopReciting();
      }
      void it.return(undefined);
    }
  }

  /** Reads the page in front of the owner from its top. */
  readPage() {
    return this.play(true);
  }

  pause() {
    const was = this.playing;
    this.run++;
    this.playing = false;
    this.loading = false;
    this.sentence = '';
    if (was) gilgamesh.stopReciting();
  }

  toggle() {
    if (this.playing) this.pause();
    else void this.play();
  }

  turn(by: 1 | -1) {
    const was = this.playing;
    this.pause();
    this.reader?.turn(by);
    if (was) void this.play(true);
  }

  // ---------------------------------------------------------------- for Gilgamesh

  /** The page in front of the owner (or their library), for his answers. */
  describe(): string {
    const r = this.reader;
    if (r && store.route.kind === 'books') {
      const b = r.book;
      let page = r.pageText().trim();
      if (page.length > PAGE_CHARS) page = page.slice(0, PAGE_CHARS) + ' …';
      const before = r.before().trim().slice(-600);
      const sel = (window.getSelection()?.toString() ?? '').trim().slice(0, 1500);
      const pct = b.progress ? ` (about ${Math.round(b.progress.percent * 100)}% through)` : '';
      return [
        `THE OWNER IS READING “${b.title}”${b.author ? ` by ${b.author}` : ''} in RAD's Books, at ${r.where()}${pct}.${b.vault ? ' The whole book is also in their vault (Books project), so passages from elsewhere in it may appear among the sources.' : ''}`,
        this.playing ? `You are reading it aloud to them now${this.settings.voice === 'gilgamesh' ? ' in your voice' : ''}; the sentence being read: “${this.sentence}”.` : '',
        sel ? `They have selected this on the page: “${sel}”` : '',
        before ? `The page before ended: “…${before}”` : '',
        `THE PAGE IN FRONT OF THEM:\n"""\n${page || '(no text on this page: a picture or a blank page)'}\n"""`,
      ]
        .filter(Boolean)
        .join('\n');
    }
    if (!this.list.length) return '';
    const recent = [...this.list].sort((a, b) => (b.opened ?? b.added) - (a.opened ?? a.added)).slice(0, 15);
    return `THE OWNER'S BOOKS (in RAD's Books; open one with [read: open | title]):\n${recent
      .map((b) => `- “${b.title}”${b.author ? ` by ${b.author}` : ''}${b.progress ? `, ${Math.round(b.progress.percent * 100)}% read` : ', not started'}`)
      .join('\n')}`;
  }
}

export const books = new Books();

/** Sentence-sized pieces of a paragraph, for reading aloud. */
export const sentences = (text: string) => chunks(text.replace(/\s+/g, ' ').trim(), 240);
