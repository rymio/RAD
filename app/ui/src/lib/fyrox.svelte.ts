// Fyrox (Development → Fyrox): the Rust game engine and its editor, and the docs Gilgamesh uses
// to help with them: the Fyrox book and the engine's notes, downloaded onto this computer
// (app/src-tauri/src/fyrox.rs) and added to the vault (project "Fyrox") for RAG, so any chat
// can answer from them. In the Fyrox view, the passages that fit a question go with it too.

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { api, rad } from './api';
import { store } from './state.svelte';
import type * as T from './types';

const PROJECT = 'Fyrox';
const SETTINGS = '/v1/app-data/fyrox_settings';

export interface FyroxDocs {
  engine: string;
  fetched: number;
  files: string[];
  pages: number;
  missing: string[];
  rag: string[];
}

interface Passage {
  file: string;
  heading: string;
  text: string;
}

class FyroxStore {
  docs = $state<FyroxDocs | null>(null);
  loaded = $state(false);
  downloading = $state<string | null>(null);
  ragAt = $state<number | null>(null);
  adding = $state(false);
  private listening = false;

  async load() {
    if (!this.listening) {
      this.listening = true;
      await listen<{ step: string }>('fyrox-docs', (e) => {
        if (this.downloading) this.downloading = e.payload.step === 'done' ? null : e.payload.step;
      });
    }
    const [docs, set] = await Promise.all([invoke<FyroxDocs | null>('fyrox_docs_status').catch(() => null), api.get<any>(SETTINGS).catch(() => null)]);
    this.docs = docs;
    this.ragAt = typeof set?.ragAt === 'number' ? set.ragAt : null;
    this.loaded = true;
  }

  get inRag() {
    return !!this.docs && this.ragAt === this.docs.fetched;
  }

  /** Downloads the docs, then adds them to the vault. */
  async download() {
    if (this.downloading) return;
    this.downloading = 'Starting';
    try {
      this.docs = await invoke<FyroxDocs>('fyrox_docs_download');
      store.toast('success', 'Fyrox docs downloaded', `The Fyrox book (${this.docs.pages} pages) and the engine’s notes, on this computer.`);
    } catch (e) {
      store.error(e, 'Couldn’t download the Fyrox docs');
      return;
    } finally {
      this.downloading = null;
    }
    await this.addToRag().catch((e) => store.error(e, 'Couldn’t add the Fyrox docs to the vault'));
  }

  private async project(): Promise<T.Project> {
    const p = store.projects.find((p) => p.name === PROJECT);
    if (p) return p;
    const created = await api.post<T.Project>('/v1/projects', { name: PROJECT, color: '#c2410c', icon: 'gamepad', description: 'The Fyrox game engine’s book and notes, for Gilgamesh and every chat' });
    store.projects = [...store.projects, created];
    return created;
  }

  async addToRag() {
    const d = this.docs;
    if (!d || this.adding) return;
    this.adding = true;
    try {
      const p = await this.project();
      // A new download replaces the files from before.
      if (this.ragAt) {
        const docs = await api.get<T.Document[]>(`/v1/projects/${p.id}/documents`).catch(() => [] as T.Document[]);
        for (const x of docs) if (/^Fyrox (book|\d)/.test(x.original_name)) await api.del(`/v1/documents/${x.id}`).catch(() => {});
      }
      const res = await rad.uploadPaths(p.id, d.rag);
      const failed = res.filter((r) => 'error' in r);
      if (failed.length === res.length) throw new Error((failed[0] as any)?.error ?? 'Upload failed');
      this.ragAt = d.fetched;
      await api.put(SETTINGS, { ragAt: d.fetched }).catch(() => {});
      store.toast('success', 'Fyrox docs are in your vault', 'In the Fyrox project; once they’re indexed, ask about Fyrox in any chat.');
    } finally {
      this.adding = false;
    }
  }

  /** The book's passages for a question asked in the Fyrox view. */
  async context(question: string): Promise<string> {
    if (store.route.kind !== 'fyrox' || !this.docs) return '';
    const hits = await invoke<Passage[]>('fyrox_docs_search', { query: question, limit: 5 }).catch(() => [] as Passage[]);
    if (!hits.length) return '';
    let budget = 9000;
    const parts: string[] = [];
    for (const p of hits) {
      const s = `### ${p.heading || p.file} (${p.file})\n${p.text}`;
      if (s.length > budget && parts.length) continue;
      parts.push(s.slice(0, budget));
      budget -= s.length;
    }
    return `THE OWNER HAS THE FYROX VIEW OPEN (Development → Fyrox: the Fyrox ${this.docs.engine} game engine, its editor, and your world, which is a Fyrox project).
FROM THE FYROX BOOK (use these for Fyrox questions; never invent APIs):
${parts.join('\n\n')}
When you give Fyrox code, put it in a fenced rust block, for Fyrox ${this.docs.engine}.`;
  }
}

export const fyrox = new FyroxStore();
