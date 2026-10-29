// Transcripts: Gilgamesh listens and writes down what is said (a meeting, a call, a lecture,
// thinking aloud) without answering.
//
//   microphone → speech to text (stt.rs, the same listener as conversations) → timed lines
//
// Each transcript is a note in the "Transcripts" project, so the vault indexes it like any
// other document: passages come up as sources in every chat, and in Gilgamesh's answers. The
// note is saved while recording (every minute) and when it stops. A transcript can be
// summarised (the summary goes into the note too) or discussed: a conversation with Gilgamesh
// about it, with the transcript in his context.
//
// The microphone is shared: starting a transcript ends a conversation, and the other way round.

import { invoke, Channel } from '@tauri-apps/api/core';
import { api, rad } from '../api';
import { store } from '../state.svelte';
import { gilgamesh } from './gilgamesh.svelte';
import type * as T from '../types';

/** A saved transcript; `id` is its note's document id. */
export interface Transcript {
  id: number;
  title: string;
  started: number;
  /** How long the recording ran, in seconds. */
  seconds: number;
  words: number;
  summary: string;
}

export interface Segment {
  /** Seconds from the start of the recording. */
  at: number;
  text: string;
}

const LIST_KEY = '/v1/app-data/gilgamesh_transcripts';
const PROJECT = 'Transcripts';
const AUTOSAVE_MS = 60_000;
/** What of a transcript goes into Gilgamesh's context when discussing it (the rest he finds through retrieval). */
const CONTEXT_CHARS = 14_000;
/** A summary is written from pieces this long, then combined. */
const SUMMARY_PIECE = 12_000;

const SUMMARY_SYSTEM = `You summarise transcripts that RAD's speech recognition made from the microphone: meetings, calls, lectures, interviews or someone thinking aloud. The words may contain small recognition mistakes and there are no speaker names; read them sensibly. Write in plain text:
- first a short paragraph saying what it was and what was covered;
- then "Key points:" followed by short lines starting with "- ";
- then, only if there are any, "Decisions and actions:" followed by lines starting with "- " (who does what, by when).
No other Markdown. Don't invent anything that isn't in the transcript.`;

const COMBINE_SYSTEM = `You are given summaries of consecutive parts of one long transcript. Combine them into a single summary of the whole, in the same plain-text format: a short paragraph, then "Key points:" with lines starting with "- ", then "Decisions and actions:" if there are any. Remove repetition.`;

const pad = (n: number) => String(n).padStart(2, '0');
export const clock = (s: number) => {
  s = Math.max(0, Math.floor(s));
  const h = Math.floor(s / 3600);
  return h ? `${h}:${pad(Math.floor(s / 60) % 60)}:${pad(s % 60)}` : `${pad(Math.floor(s / 60))}:${pad(s % 60)}`;
};
export const duration = (s: number) => (s < 60 ? `${Math.max(1, Math.round(s))} s` : s < 3600 ? `${Math.round(s / 60)} min` : `${Math.floor(s / 3600)} h ${Math.round(s / 60) % 60} min`);
const longDate = (ms: number) => new Date(ms).toLocaleString('en-GB', { weekday: 'short', day: 'numeric', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit' });
const countWords = (segs: Segment[]) => segs.reduce((n, s) => n + s.text.split(/\s+/).filter(Boolean).length, 0);

/** The note's Markdown: a header line, the summary (once written) and the timed lines. */
function noteContent(t: Pick<Transcript, 'started' | 'seconds' | 'words' | 'summary'>, lines: string) {
  const head = `_Transcribed by Gilgamesh · ${longDate(t.started)} · ${duration(t.seconds)} · ${t.words.toLocaleString('en-GB')} words_`;
  const summary = t.summary ? `\n\n## Summary\n\n${t.summary}` : '';
  return `${head}${summary}\n\n## Transcript\n\n${lines}\n`;
}

const segmentLines = (segs: Segment[]) => segs.map((s) => `[${clock(s.at)}] ${s.text}`).join('\n\n');

/** The timed lines of a transcript note (everything after its "Transcript" heading). */
function linesOf(content: string) {
  const i = content.indexOf('\n## Transcript\n');
  return (i < 0 ? content : content.slice(i + '\n## Transcript\n'.length)).trim();
}

class Transcripts {
  list = $state<Transcript[]>([]);
  loaded = $state(false);

  /** The recording under way. */
  recording = $state(false);
  paused = $state(false);
  starting = $state(false);
  /** What is being said right now (live caption). */
  heard = $state('');
  level = $state(0);
  segments = $state<Segment[]>([]);
  title = $state('');
  started = $state(0);
  /** Seconds recorded, ticking while recording. */
  elapsed = $state(0);
  error = $state<string | null>(null);
  /** The transcript being summarised. */
  summarising = $state<number | null>(null);

  /** The note of the recording under way, once it has been saved. */
  private noteId: number | null = null;
  private speechAt = 0;
  /** Someone is talking: a turn is under way in the listener. */
  private speaking = false;
  private flushed: (() => void) | null = null;
  private dirty = false;
  private saving: Promise<void> = Promise.resolve();
  private autosave: ReturnType<typeof setInterval> | undefined;
  private ticker: ReturnType<typeof setInterval> | undefined;
  private texts = new Map<number, string>();

  async load() {
    if (this.loaded) return;
    const v = await api.get<any>(LIST_KEY).catch(() => null);
    this.list = Array.isArray(v?.transcripts) ? v.transcripts : [];
    this.loaded = true;
  }

  /** Forgets everything loaded (the vault was locked); a recording stops. */
  unload() {
    if (this.recording) invoke('stt_stop').catch(() => {});
    this.reset();
    this.list = [];
    this.texts.clear();
    this.loaded = false;
  }

  private saveList() {
    return api.put(LIST_KEY, { transcripts: $state.snapshot(this.list) }).catch((e) => store.error(e, 'Couldn’t save the list of transcripts'));
  }

  /** The transcripts, most recent first. */
  get sorted() {
    return [...this.list].sort((a, b) => b.started - a.started);
  }

  find(id: number | null | undefined) {
    return this.list.find((t) => t.id === id) ?? null;
  }

  /** The project transcripts are kept in (made the first time). */
  async project(): Promise<T.Project> {
    const p = store.projects.find((p) => p.name === PROJECT);
    if (p) return p;
    const created = await api.post<T.Project>('/v1/projects', { name: PROJECT, color: '#0ea5e9', icon: 'mic', description: 'What Gilgamesh heard and wrote down' });
    store.projects = [...store.projects, created];
    return created;
  }

  // ---------------------------------------------------------------- recording

  async start(title = '') {
    if (this.recording || this.starting) return;
    await this.load();
    // One microphone: a conversation going on ends.
    if (gilgamesh.phase !== 'off') gilgamesh.stop();
    this.reset();
    this.starting = true;
    this.title = title.trim();
    const ch = new Channel<any>();
    ch.onmessage = (ev) => this.onStt(ev);
    try {
      await invoke('stt_listen', { onEvent: ch, paused: false });
      this.recording = true;
      this.started = Date.now();
      gilgamesh.head.attention = 'listening';
      this.ticker = setInterval(() => (this.elapsed = (Date.now() - this.started) / 1000), 500);
      this.autosave = setInterval(() => this.dirty && void this.save(), AUTOSAVE_MS);
    } catch (e: any) {
      this.error = e?.message ?? String(e);
    } finally {
      this.starting = false;
    }
  }

  /** Ends the turn under way now, and waits for its words (or a few seconds). */
  private flush() {
    if (!this.speaking) return Promise.resolve();
    invoke('stt_finish').catch(() => {});
    return new Promise<void>((r) => {
      const t = setTimeout(() => ((this.flushed = null), r()), 4000);
      this.flushed = () => (clearTimeout(t), (this.flushed = null), r());
    });
  }

  /** Stops listening; what is being said right now is still written down. */
  async stop() {
    if (!this.recording) return;
    await this.flush();
    invoke('stt_stop').catch(() => {});
    this.recording = false;
    if (gilgamesh.phase === 'off') gilgamesh.head.attention = 'idle';
    clearInterval(this.ticker);
    clearInterval(this.autosave);
    this.elapsed = (Date.now() - this.started) / 1000;
    const id = this.segments.length ? await this.save() : null;
    this.reset();
    if (id) store.toast('success', 'Transcript saved', 'It’s in the Transcripts project and can be found from any chat.');
    return id;
  }

  /** While paused, what the microphone hears isn't written down. */
  async pause(paused: boolean) {
    if (!this.recording || this.paused === paused) return;
    this.paused = paused;
    if (paused) await this.flush();
    invoke('stt_pause', { paused }).catch(() => {});
  }

  private reset() {
    clearInterval(this.ticker);
    clearInterval(this.autosave);
    this.recording = false;
    this.paused = false;
    this.heard = '';
    this.level = 0;
    this.segments = [];
    this.title = '';
    this.elapsed = 0;
    this.noteId = null;
    this.dirty = false;
    this.speaking = false;
    this.flushed?.();
  }

  private onStt(ev: any) {
    switch (ev.type) {
      case 'level':
        this.level = ev.rms;
        break;
      case 'speech_start':
        this.speechAt = Date.now();
        this.speaking = true;
        this.heard = '';
        break;
      case 'partial':
        this.heard = ev.text;
        break;
      case 'final':
        this.heard = '';
        this.speaking = false;
        this.flushed?.();
        this.segments.push({ at: Math.max(0, ((this.speechAt || Date.now()) - this.started) / 1000), text: ev.text.trim() });
        this.dirty = true;
        // The first words make the note, so even a crash keeps something.
        if (this.segments.length === 1) void this.save();
        break;
      case 'cancelled':
        this.heard = '';
        this.speaking = false;
        this.flushed?.();
        break;
      case 'error':
        this.error = ev.message;
        void this.stop();
        break;
    }
  }

  /** Writes the recording under way to its note (one save at a time); returns the note's id. */
  private save(): Promise<number | null> {
    const run = this.saving.then(async () => {
      if (!this.segments.length) return this.noteId;
      this.dirty = false;
      const seconds = Math.max(this.elapsed, (Date.now() - this.started) / 1000);
      const words = countWords(this.segments);
      const title = this.title || `Transcript ${longDate(this.started)}`;
      const content = noteContent({ started: this.started, seconds, words, summary: '' }, segmentLines(this.segments));
      try {
        if (this.noteId) await api.put(`/v1/notes/${this.noteId}`, { title, content, tags: ['gilgamesh', 'transcript'] });
        else {
          const p = await this.project();
          const n = await api.post<T.Note>(`/v1/projects/${p.id}/notes`, { title, content, tags: ['gilgamesh', 'transcript'] });
          this.noteId = n.document.id;
        }
        const entry: Transcript = { id: this.noteId!, title, started: this.started, seconds, words, summary: '' };
        const i = this.list.findIndex((t) => t.id === entry.id);
        if (i >= 0) this.list[i] = entry;
        else this.list = [entry, ...this.list];
        this.texts.set(entry.id, content);
        await this.saveList();
      } catch (e) {
        this.dirty = true;
        store.error(e, 'Couldn’t save the transcript');
      }
      return this.noteId;
    });
    this.saving = run.then(() => {});
    return run;
  }

  // ---------------------------------------------------------------- saved transcripts

  /** The note's Markdown (cached). */
  async content(id: number): Promise<string> {
    const cached = this.texts.get(id);
    if (cached !== undefined) return cached;
    const n = await api.get<T.Note>(`/v1/notes/${id}`);
    this.texts.set(id, n.content);
    return n.content;
  }

  /** The timed lines, for reading. */
  async text(id: number) {
    return linesOf(await this.content(id));
  }

  async rename(id: number, title: string) {
    title = title.trim().slice(0, 200);
    const t = this.find(id);
    if (!t || !title || title === t.title) return;
    const content = await this.content(id);
    await api.put(`/v1/notes/${id}`, { title, content, tags: ['gilgamesh', 'transcript'] });
    t.title = title;
    await this.saveList();
  }

  async remove(id: number) {
    await api.del(`/v1/documents/${id}`).catch(() => {});
    this.texts.delete(id);
    this.list = this.list.filter((t) => t.id !== id);
    await this.saveList();
  }

  /** Writes a summary of a transcript (pieces first, if it's long) and keeps it in the note. */
  async summarise(id: number) {
    const t = this.find(id);
    if (!t || this.summarising) return;
    this.summarising = id;
    try {
      const lines = await this.text(id);
      const pieces: string[] = [];
      for (let i = 0; i < lines.length; i += SUMMARY_PIECE) pieces.push(lines.slice(i, i + SUMMARY_PIECE));
      let summary: string;
      if (pieces.length <= 1) {
        summary = (await rad.assist(SUMMARY_SYSTEM, [{ role: 'user', content: `Transcript “${t.title}”:\n\n${lines}` }])).content;
      } else {
        const parts: string[] = [];
        for (const [i, p] of pieces.entries()) {
          parts.push((await rad.assist(SUMMARY_SYSTEM, [{ role: 'user', content: `Part ${i + 1} of ${pieces.length} of the transcript “${t.title}”:\n\n${p}` }])).content);
        }
        summary = (await rad.assist(COMBINE_SYSTEM, [{ role: 'user', content: parts.map((s, i) => `Part ${i + 1}:\n${s}`).join('\n\n') }])).content;
      }
      summary = summary.replace(/<think>[\s\S]*?(<\/think>|$)/g, '').trim();
      if (!summary) throw new Error('The model wrote nothing');
      const content = noteContent({ ...t, summary }, lines);
      await api.put(`/v1/notes/${id}`, { title: t.title, content, tags: ['gilgamesh', 'transcript'] });
      this.texts.set(id, content);
      const fresh = this.find(id);
      if (fresh) fresh.summary = summary;
      await this.saveList();
      return summary;
    } catch (e) {
      store.error(e, 'Couldn’t summarise the transcript');
    } finally {
      this.summarising = null;
    }
  }

  /** What Gilgamesh is told about a transcript the owner wants to discuss. */
  async context(id: number): Promise<string> {
    const t = this.find(id);
    if (!t) return '';
    const lines = await this.text(id).catch(() => '');
    const cut = lines.length > CONTEXT_CHARS;
    return [
      `THE OWNER WANTS TO TALK ABOUT THIS TRANSCRIPT, which you made by listening: “${t.title}”, ${longDate(t.started)}, ${duration(t.seconds)}.`,
      t.summary ? `Its summary:\n${t.summary}` : '',
      `The transcript${cut ? ' (the beginning; later passages appear among the sources when relevant)' : ''}:\n${cut ? lines.slice(0, CONTEXT_CHARS) + '\n…' : lines}`,
    ]
      .filter(Boolean)
      .join('\n\n');
  }

  /** The recent transcripts, for every answer (so "yesterday's meeting" makes sense to him). */
  describe(n = 8): string {
    const list = this.sorted.slice(0, n);
    if (!list.length) return '';
    return `TRANSCRIPTS YOU HAVE MADE (in the Transcripts project; most recent first):\n${list
      .map((t) => `- ${longDate(t.started)}, ${duration(t.seconds)}: “${t.title}”${t.summary ? ` — ${t.summary.split('\n')[0].slice(0, 240)}` : ''}`)
      .join('\n')}`;
  }
}

export const transcripts = new Transcripts();
