// Browsers → Scraper: saves whole websites for offline reading (app/src-tauri/src/scraper.rs does
// the crawling and keeps the copies on this computer), and "Add to RAG" puts the text of every
// page, and the documents it linked to, into the vault, one project per site, so every chat and
// Gilgamesh can search and cite it.
//
// Offline pages are shown from rad://localhost/scraped/… in a sandboxed frame: no scripts,
// nothing from the internet. A link to a page that wasn't saved opens in RAD's web browser.

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { api, rad, radUrl } from './api';
import { store } from './state.svelte';
import type * as T from './types';

export interface SiteOptions {
  /** "site" (this host), "section" (under the start page's folder) or "subdomains". */
  scope: 'site' | 'section' | 'subdomains';
  max_pages: number;
  max_depth: number;
  assets: boolean;
  documents: boolean;
  robots: boolean;
  delay_ms: number;
  concurrency: number;
}

export interface Site {
  id: string;
  title: string;
  url: string;
  host: string;
  created: number;
  updated: number;
  status: 'running' | 'done' | 'stopped' | 'failed';
  pages: number;
  assets: number;
  documents: number;
  bytes: number;
  errors: number;
  blocked: number;
  queued: number;
  current: string | null;
  message: string | null;
  start: string | null;
  options: SiteOptions;
  rag: { project: number; docs: number[]; added: number } | null;
  rag_stale: boolean;
}

export interface Page {
  url: string;
  file: string;
  title: string;
  depth: number;
  bytes: number;
  words: number;
  kind: 'page' | 'document';
  rag: string | null;
}

export const DEFAULT_OPTIONS: SiteOptions = { scope: 'site', max_pages: 500, max_depth: 12, assets: true, documents: true, robots: true, delay_ms: 250, concurrency: 4 };
const PAGE_CHARS = 6000;

class Scraper {
  list = $state<Site[]>([]);
  loaded = $state(false);
  /** The site open in the viewer. */
  openId = $state<string | null>(null);
  pages = $state.raw<Page[]>([]);
  /** The page shown (its file in the copy), and the one the frame was told to load (a click
   *  inside the page changes the first only). */
  pageFile = $state<string | null>(null);
  frameFile = $state<string | null>(null);
  /** Adding to (or taking out of) the vault: the site, and how far it is. */
  ragBusy = $state<string | null>(null);
  ragProgress = $state('');
  options = $state<SiteOptions>(this.loadOptions());

  private loadingList: Promise<void> | null = null;
  private listening = false;
  /** Sites to add to RAG as soon as they've been saved (asked for with the scrape). */
  private ragWhenDone = new Set<string>();
  private pageText = new Map<string, string>();

  get site() {
    return this.find(this.openId);
  }

  get page() {
    return this.pages.find((p) => p.file === this.pageFile) ?? null;
  }

  find(id: string | null | undefined) {
    return id ? (this.list.find((s) => s.id === id) ?? null) : null;
  }

  get running() {
    return this.list.filter((s) => s.status === 'running');
  }

  private loadOptions(): SiteOptions {
    try {
      return { ...DEFAULT_OPTIONS, ...JSON.parse(localStorage.getItem('rad-scraper') ?? '{}') };
    } catch {
      return { ...DEFAULT_OPTIONS };
    }
  }

  saveOptions(patch: Partial<SiteOptions>) {
    this.options = { ...this.options, ...patch };
    try {
      localStorage.setItem('rad-scraper', JSON.stringify(this.options));
    } catch {
      // Not remembered.
    }
  }

  load(): Promise<void> {
    void this.listen();
    if (this.loaded) return Promise.resolve();
    return (this.loadingList ??= this.refresh().finally(() => (this.loadingList = null)));
  }

  async refresh() {
    this.list = await invoke<Site[]>('scraper_list');
    this.loaded = true;
  }

  private async listen() {
    if (this.listening) return;
    this.listening = true;
    await listen<Site>('scraper-progress', (e) => {
      const s = e.payload;
      const i = this.list.findIndex((x) => x.id === s.id);
      if (i >= 0) this.list[i] = s;
      else this.list = [s, ...this.list];
      if (s.status !== 'running') this.finished(s);
    });
    await listen<{ id: string; file: string }>('scraper-viewing', (e) => {
      if (e.payload.id === this.openId && this.pages.some((p) => p.file === e.payload.file)) this.pageFile = e.payload.file;
    });
    // A link in an offline page to something that wasn't saved: RAD's browser opens it.
    await listen<{ url: string }>('scraper-open-external', (e) => store.runAction({ kind: 'open_url', url: e.payload.url }));
  }

  private finished(s: Site) {
    if (this.openId === s.id) void this.loadPages(s.id);
    const ok = s.status === 'done' || (s.status === 'stopped' && s.pages > 0);
    if (s.status === 'failed') store.toast('error', `Couldn’t save ${s.host}`, s.message ?? '');
    else if (ok) store.toast('success', `${s.title} is saved`, `${s.pages} pages${s.documents ? `, ${s.documents} documents` : ''} (${fmtBytes(s.bytes)}), ready offline.`);
    if (ok && this.ragWhenDone.delete(s.id)) void this.addToRag(s.id);
  }

  /** Starts saving a site (or `id` again from the start). `rag`: add it to the vault when done. */
  async start(url: string, options: SiteOptions = this.options, id?: string, rag = false) {
    await this.load();
    const s = await invoke<Site>('scraper_start', { url, options, id: id ?? null });
    const i = this.list.findIndex((x) => x.id === s.id);
    if (i >= 0) this.list[i] = s;
    else this.list = [s, ...this.list];
    if (rag) this.ragWhenDone.add(s.id);
    return s;
  }

  stop(id: string) {
    return invoke('scraper_stop', { id });
  }

  async remove(id: string, alsoVault: boolean) {
    if (alsoVault) await this.removeFromRag(id).catch(() => {});
    if (this.openId === id) this.close();
    await invoke('scraper_delete', { id });
    this.list = this.list.filter((s) => s.id !== id);
  }

  async open(id: string, file?: string) {
    this.openId = id;
    store.navigate({ kind: 'scraper' });
    await this.loadPages(id);
    this.show(file ?? this.site?.start ?? this.pages[0]?.file ?? null);
  }

  show(file: string | null) {
    this.pageFile = file;
    this.frameFile = file;
  }

  close() {
    this.openId = null;
    this.pages = [];
    this.pageFile = null;
    this.frameFile = null;
  }

  private async loadPages(id: string) {
    const list = await invoke<Page[]>('scraper_pages', { id }).catch(() => []);
    if (this.openId === id) this.pages = list;
  }

  /** The offline address of a file in a site's copy. */
  fileUrl(id: string, file: string) {
    return radUrl(`/scraped/${id}/${file.split('/').map(encodeURIComponent).join('/')}`);
  }

  async folder(id: string) {
    return invoke<string>('scraper_folder', { id });
  }

  // ---------------------------------------------------------------- the vault

  private async project(s: Site): Promise<T.Project> {
    const have = s.rag && store.projects.find((p) => p.id === s.rag!.project);
    if (have) return have;
    const name = `Web: ${s.host.replace(/^www\./, '')}`;
    const named = store.projects.find((p) => p.name === name);
    if (named) return named;
    const created = await api.post<T.Project>('/v1/projects', { name, color: '#0e7490', icon: 'globe', description: `Pages saved from ${s.url} with the Scraper` });
    store.projects = [...store.projects, created];
    return created;
  }

  /** Uploads the text of every page (and the documents) to the site's project in the vault. An
   *  earlier copy there is replaced. */
  async addToRag(id: string) {
    const s = this.find(id);
    if (!s || this.ragBusy) return;
    this.ragBusy = id;
    try {
      if (s.rag?.docs.length) {
        this.ragProgress = 'Taking out the old copy…';
        for (const d of s.rag.docs) await api.del(`/v1/documents/${d}`).catch(() => {});
      }
      const p = await this.project(s);
      const files = await invoke<string[]>('scraper_rag_files', { id });
      if (!files.length) {
        store.toast('info', 'Nothing to add', 'None of the saved pages has enough text.');
        return;
      }
      const docs: number[] = [];
      let failed = 0;
      for (let i = 0; i < files.length; i += 20) {
        this.ragProgress = `Adding ${Math.min(i + 20, files.length)} of ${files.length}…`;
        const res = await rad.uploadPaths(p.id, files.slice(i, i + 20));
        for (const r of res) {
          if ('error' in r) failed++;
          else docs.push(r.document.id);
        }
      }
      const updated = await invoke<Site>('scraper_set_rag', { id, project: p.id, docs });
      this.list = this.list.map((x) => (x.id === id ? updated : x));
      store.toast('success', `${s.title} is in your vault`, `${docs.length} pages and documents in the “${p.name}” project${failed ? ` (${failed} couldn’t be added)` : ''}. They’re being indexed; then every chat and Gilgamesh can use them.`);
    } catch (e) {
      store.error(e, 'Couldn’t add it to the vault');
    } finally {
      this.ragBusy = null;
      this.ragProgress = '';
    }
  }

  /** Takes the site's pages out of the vault (the offline copy stays). */
  async removeFromRag(id: string) {
    const s = this.find(id);
    if (!s?.rag) return;
    this.ragBusy = id;
    try {
      for (const d of s.rag.docs) await api.del(`/v1/documents/${d}`).catch(() => {});
      const updated = await invoke<Site>('scraper_set_rag', { id, project: null, docs: [] });
      this.list = this.list.map((x) => (x.id === id ? updated : x));
    } finally {
      this.ragBusy = null;
    }
  }

  // ---------------------------------------------------------------- for Gilgamesh

  /** The page open in the viewer, as text (read once). */
  async loadPageText() {
    const s = this.site;
    const p = this.page;
    if (!s || !p?.rag || this.pageText.has(p.file)) return;
    const t = await invoke<string>('scraper_page_text', { id: s.id, rag: p.rag }).catch(() => '');
    this.pageText.set(p.file, t);
  }

  describe(): string {
    const parts: string[] = [];
    const s = this.site;
    const p = this.page;
    if (s && p && store.route.kind === 'scraper') {
      let text = (this.pageText.get(p.file) ?? '').trim();
      if (text.length > PAGE_CHARS) text = text.slice(0, PAGE_CHARS) + ' …';
      parts.push(`THE OWNER IS READING A SAVED PAGE in RAD's Scraper (offline copy of ${s.host}): “${p.title}”, ${p.url}.${text ? `\nTHE PAGE:\n"""\n${text}\n"""` : ''}`);
    }
    if (this.list.length) {
      const lines = this.list.slice(0, 12).map((x) => `- ${x.title} (${x.url}): ${x.status === 'running' ? `being saved now, ${x.pages} pages so far` : `${x.pages} pages${x.documents ? `, ${x.documents} documents` : ''}, ${x.status}`}; ${x.rag ? `in the vault (project “Web: ${x.host.replace(/^www\./, '')}”${x.rag_stale ? ', older than the latest copy' : ''})` : 'not in the vault'}`);
      parts.push(`WEBSITES THE OWNER HAS SAVED with the Scraper (offline copies; the ones in the vault are among your sources):\n${lines.join('\n')}`);
    }
    return parts.join('\n\n');
  }
}

export const scraper = new Scraper();

export function fmtBytes(n: number) {
  return n >= 1e9 ? `${(n / 1e9).toFixed(1)} GB` : n >= 1e6 ? `${(n / 1e6).toFixed(1)} MB` : n >= 1e3 ? `${Math.round(n / 1e3)} KB` : `${n} B`;
}
