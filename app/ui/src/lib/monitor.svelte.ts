// Web Monitoring: the websites the owner keeps an eye on. Each one is checked (app/src-tauri/
// src/monitor.rs: IP addresses, whether it answers and how fast, its TLS certificate) when RAD
// starts and every three hours, and on request. Gilgamesh says out loud what has gone wrong or
// come right since he last told the owner (an offline site, an expiring or broken certificate,
// a site back online), and knows the latest results when asked.
//
// Sites, their SSH details and notes (logins included) are kept in the encrypted vault as app
// data, not as notes, so credentials never turn up in search or chat answers. Thumbnails are
// taken by a browser engine of their own and kept under a key per site.

import { invoke } from '@tauri-apps/api/core';
import { api } from './api';
import { store } from './state.svelte';
import { gilgamesh } from './gilgamesh/gilgamesh.svelte';

export interface SshDetails { host: string; user?: string; port?: number; identity_file?: string }

export interface Certificate {
  valid?: boolean;
  problem?: string | null;
  issuer?: string;
  subject?: string;
  names?: string[];
  not_before?: number;
  expires?: number;
  days_left?: number;
  protocol?: string | null;
  /** The certificate couldn't be read at all. */
  error?: string;
}

export interface Check {
  url: string;
  host: string;
  checked_at: number;
  online: boolean;
  status: number | null;
  ms: number | null;
  final_url: string | null;
  title: string | null;
  ips: string[];
  certificate: Certificate | null;
  error: string | null;
}

export interface Site {
  id: string;
  url: string;
  name: string;
  ssh: SshDetails | null;
  /** Logins and anything else worth keeping; shown hidden until asked. */
  notes: string;
  added: number;
  check: Check | null;
  /** Recent checks, for the uptime strip (oldest first). */
  history: { at: number; online: boolean; ms: number | null }[];
  thumbAt?: number;
  /** Problems Gilgamesh has already told the owner about (so he doesn't repeat himself). */
  told?: string[];
}

interface Saved { sites: Site[]; voice: boolean; lastRun: number }

const KEY = '/v1/app-data/web_monitor';
const thumbKey = (id: string) => `/v1/app-data/web_monitor_thumb_${id}`;
export const EVERY_MS = 3 * 3600_000;
const HISTORY = 48;
/** Thumbnails are taken again after a day. */
const THUMB_AGE = 24 * 3600_000;
/** Certificates closer than this to expiring are worth a word. */
export const CERT_WARN_DAYS = 14;
const PARALLEL = 4;

export const hostOf = (url: string) => {
  try {
    return new URL(url.includes('://') ? url : `https://${url}`).hostname.replace(/^www\./, '');
  } catch {
    return url;
  }
};

export const siteName = (s: Site) => s.name || hostOf(s.url);

const dateLabel = (ms: number) => new Date(ms).toLocaleDateString('en-GB', { day: 'numeric', month: 'short', year: 'numeric' });

/** What's wrong with a site now, as short spoken phrases (empty: all well). */
export function problems(s: Site): string[] {
  const c = s.check;
  if (!c) return [];
  const out: string[] = [];
  const name = siteName(s);
  if (!c.online) out.push(`${name} is offline${c.status ? ` (it answers with error ${c.status})` : ''}`);
  const cert = c.certificate;
  if (cert?.error && c.online) out.push(`${name}'s certificate couldn't be checked`);
  else if (cert && cert.days_left !== undefined) {
    if (cert.days_left < 0) out.push(`${name}'s certificate expired on ${dateLabel(cert.expires!)}`);
    else if (cert.problem) out.push(`${name}'s certificate has a problem: ${cert.problem.replace(/^the certificate /, 'it ')}`);
    else if (cert.days_left <= CERT_WARN_DAYS) out.push(`${name}'s certificate expires ${cert.days_left === 0 ? 'today' : cert.days_left === 1 ? 'tomorrow' : `in ${cert.days_left} days`}`);
  }
  return out;
}

/** 0–100: the share of recent checks it answered. */
export const uptime = (s: Site) => (s.history.length ? Math.round((s.history.filter((h) => h.online).length / s.history.length) * 100) : null);

class Monitor {
  sites = $state<Site[]>([]);
  voice = $state(true);
  lastRun = $state(0);
  loaded = $state(false);
  /** Sites being checked now. */
  checking = $state<Set<string>>(new Set());
  thumbs = $state<Record<string, string>>({});
  thumbing = $state<Set<string>>(new Set());

  private timer: ReturnType<typeof setTimeout> | undefined;
  private loading: Promise<void> | null = null;

  load() {
    if (this.loaded) return Promise.resolve();
    this.loading ??= (async () => {
      const v = await api.get<any>(KEY).catch(() => null);
      this.sites = Array.isArray(v?.sites) ? v.sites : [];
      this.voice = v?.voice ?? true;
      this.lastRun = v?.lastRun ?? 0;
      this.loaded = true;
      for (const s of this.sites) void this.loadThumb(s.id);
    })().finally(() => (this.loading = null));
    return this.loading;
  }

  /** After unlocking: a check now (RAD has just started), then every three hours. */
  async start() {
    await this.load();
    clearTimeout(this.timer);
    this.timer = setTimeout(() => void this.run('start'), 4000);
  }

  private schedule() {
    clearTimeout(this.timer);
    this.timer = setTimeout(() => void this.run('scheduled'), EVERY_MS);
  }

  /** The vault was locked: forget everything. */
  unload() {
    clearTimeout(this.timer);
    this.sites = [];
    this.thumbs = {};
    this.loaded = false;
    this.lastRun = 0;
  }

  private save() {
    const data: Saved = { sites: $state.snapshot(this.sites) as Site[], voice: this.voice, lastRun: this.lastRun };
    return api.put(KEY, data).catch((e) => store.error(e, 'Couldn’t save the monitored websites'));
  }

  get nextRun() {
    return this.lastRun ? this.lastRun + EVERY_MS : 0;
  }

  // ---------------------------------------------------------------- sites

  async add(url: string) {
    url = url.trim();
    if (!url) return;
    await this.load();
    const full = url.includes('://') ? url : `https://${url}`;
    if (this.sites.some((s) => hostOf(s.url) === hostOf(full) && new URL(s.url).pathname === new URL(full).pathname)) {
      store.toast('error', 'Already monitored', hostOf(full));
      return;
    }
    const site: Site = { id: Date.now().toString(36) + Math.random().toString(36).slice(2, 6), url: full, name: '', ssh: null, notes: '', added: Date.now(), check: null, history: [] };
    this.sites = [...this.sites, site];
    await this.save();
    await this.check(site.id);
    void this.thumb(site.id);
    return site.id;
  }

  async update(id: string, patch: Partial<Pick<Site, 'url' | 'name' | 'ssh' | 'notes'>>) {
    const s = this.sites.find((x) => x.id === id);
    if (!s) return;
    const moved = patch.url !== undefined && patch.url !== s.url;
    Object.assign(s, patch);
    if (moved) {
      s.check = null;
      s.history = [];
      s.told = [];
    }
    await this.save();
    if (moved) {
      await this.check(id);
      void this.thumb(id);
    }
  }

  async remove(id: string) {
    this.sites = this.sites.filter((s) => s.id !== id);
    delete this.thumbs[id];
    await api.put(thumbKey(id), {}).catch(() => {});
    await this.save();
  }

  setVoice(on: boolean) {
    this.voice = on;
    return this.save();
  }

  // ---------------------------------------------------------------- checking

  /** Checks one site now. */
  async check(id: string, save = true) {
    const s = this.sites.find((x) => x.id === id);
    if (!s || this.checking.has(id)) return;
    this.checking = new Set([...this.checking, id]);
    try {
      const c = await invoke<Check>('monitor_check', { url: s.url });
      const fresh = this.sites.find((x) => x.id === id);
      if (!fresh) return;
      fresh.check = c;
      fresh.history = [...fresh.history, { at: c.checked_at, online: c.online, ms: c.ms }].slice(-HISTORY);
      if (save) await this.save();
    } catch (e) {
      store.error(e, `Couldn’t check ${siteName(s)}`);
    } finally {
      const n = new Set(this.checking);
      n.delete(id);
      this.checking = n;
    }
  }

  /** Checks every site (a few at a time), then tells the owner what changed. Returns what's wrong. */
  async run(why: 'start' | 'scheduled' | 'manual' | 'asked'): Promise<string[]> {
    await this.load();
    this.schedule();
    const ids = this.sites.map((s) => s.id);
    const queue = [...ids];
    await Promise.all(
      Array.from({ length: Math.min(PARALLEL, queue.length) }, async () => {
        for (let id = queue.shift(); id; id = queue.shift()) await this.check(id, false);
      }),
    );
    this.lastRun = Date.now();
    // Thumbnails older than a day are taken again, one by one, in the background.
    const stale = this.sites.filter((s) => s.check?.online && (!s.thumbAt || Date.now() - s.thumbAt > THUMB_AGE)).map((s) => s.id);
    void (async () => {
      for (const id of stale) await this.thumb(id).catch(() => {});
    })();
    const now = this.sites.flatMap(problems);
    if (why !== 'asked') this.tell(why);
    await this.save();
    return now;
  }

  /** Says what's new: problems not mentioned before, and sites that are fine again. */
  private tell(why: 'start' | 'scheduled' | 'manual') {
    const news: string[] = [];
    const recovered: string[] = [];
    for (const s of this.sites) {
      const p = problems(s);
      const before = s.told ?? [];
      const wasOffline = before.some((t) => t.includes(' is offline'));
      news.push(...p.filter((x) => !before.includes(x)));
      if (wasOffline && s.check?.online) recovered.push(siteName(s));
      s.told = p;
    }
    // Starting up, everything that's wrong is worth a reminder.
    const say = why === 'start' ? this.sites.flatMap(problems) : news;
    if (!say.length && !recovered.length) {
      if (why === 'manual' && this.sites.length) store.toast('success', 'All websites are fine', `${this.sites.length} checked`);
      return;
    }
    const parts: string[] = [];
    if (say.length) parts.push(`${sentence(say)}.`);
    if (recovered.length) parts.push(`${sentence(recovered)} ${recovered.length === 1 ? 'is' : 'are'} back online.`);
    const text = `Website check: ${parts.join(' ')}`;
    store.toast(say.length ? 'error' : 'success', 'Web Monitoring', parts.join(' '), 9000);
    if (this.voice) void gilgamesh.announce(text, say.length ? 'concerned' : 'relieved');
  }

  // ---------------------------------------------------------------- thumbnails

  private async loadThumb(id: string) {
    const v = await api.get<any>(thumbKey(id)).catch(() => null);
    if (typeof v?.data === 'string') this.thumbs[id] = v.data;
  }

  /** Takes a new thumbnail of a site. */
  async thumb(id: string) {
    const s = this.sites.find((x) => x.id === id);
    if (!s || this.thumbing.has(id)) return;
    this.thumbing = new Set([...this.thumbing, id]);
    try {
      const jpeg = await invoke<ArrayBuffer>('monitor_thumb', { url: s.url });
      const data = await new Promise<string>((resolve, reject) => {
        const r = new FileReader();
        r.onload = () => resolve(String(r.result));
        r.onerror = () => reject(r.error);
        r.readAsDataURL(new Blob([jpeg], { type: 'image/jpeg' }));
      });
      await api.put(thumbKey(id), { data });
      this.thumbs[id] = data;
      const fresh = this.sites.find((x) => x.id === id);
      if (fresh) {
        fresh.thumbAt = Date.now();
        await this.save();
      }
    } catch (e: any) {
      // A browser engine that isn't built: no thumbnails, quietly.
      if (e?.code !== 'browser_missing') console.warn('thumbnail', e);
    } finally {
      const n = new Set(this.thumbing);
      n.delete(id);
      this.thumbing = n;
    }
  }

  // ---------------------------------------------------------------- for Gilgamesh

  /** The latest results, for his answers. */
  describe(): string {
    if (!this.sites.length) return '';
    const when = this.lastRun ? new Date(this.lastRun).toLocaleString('en-GB', { weekday: 'short', hour: '2-digit', minute: '2-digit' }) : 'not yet';
    const lines = this.sites.map((s) => {
      const c = s.check;
      if (!c) return `- ${siteName(s)} (${s.url}): not checked yet`;
      const cert = c.certificate;
      const certText = !cert
        ? 'no HTTPS'
        : cert.error
          ? `certificate not readable (${cert.error})`
          : `certificate ${cert.days_left! < 0 ? 'EXPIRED' : 'expires'} ${dateLabel(cert.expires!)} (${cert.days_left} days)${cert.problem ? `, PROBLEM: ${cert.problem}` : ''}`;
      return `- ${siteName(s)} (${s.url}): ${c.online ? 'ONLINE' : 'OFFLINE'}${c.status ? `, HTTP ${c.status}` : ''}${c.ms ? `, ${c.ms} ms` : ''}${c.ips.length ? `, IP ${c.ips.join(', ')}` : ''}, ${certText}${c.error ? `; error: ${c.error}` : ''}${s.ssh ? '; SSH set up' : ''}${uptime(s) !== null ? `; up ${uptime(s)}% of the last ${s.history.length} checks` : ''}`;
    });
    return `WEBSITES YOU MONITOR (Web Monitoring; last checked ${when}, every 3 hours):\n${lines.join('\n')}`;
  }
}

/** "a, b and c". */
function sentence(items: string[]) {
  return items.length <= 1 ? (items[0] ?? '') : `${items.slice(0, -1).join(', ')} and ${items.at(-1)}`;
}

export const monitor = new Monitor();
