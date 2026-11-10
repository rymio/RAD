// Products (sidebar → Products): the owner's products, usually a SaaS platform with an iOS and
// an Android app, and everything it takes to get them to production:
//
//   the product (what it is, in detail)  →  store listings drafted to the stores' limits
//     →  a website on gnu.au (its REST API) that gives the stores their support / privacy URLs
//     →  bots fill in App Store Connect and Google Play Console with the owner's saved logins
//
// Everything is kept on this computer (app-data products/, app/src-tauri/src/products.rs).
// "Add to RAG" puts each product's brief in the vault ("Products" project) and a one-line fact in
// RAD's memories, so Gilgamesh knows the products in every conversation; the stores' own guides
// are fetched ahead (so they work offline) and can go into RAG too ("Store Guides").
//
// Safety: bots never submit for review, publish or roll out: they stop and report. RAD's bot loop
// also waits for the owner's OK on any click that submits, sends or publishes.

import { invoke } from '@tauri-apps/api/core';
import { api, rad } from './api';
import { store } from './state.svelte';
import { bots, onDomain } from './bots.svelte';
import type * as T from './types';

export type Platform = 'saas' | 'ios' | 'android';
export type AssetKind = 'icon-ios' | 'icon-play' | 'iphone' | 'ipad' | 'phone' | 'tablet' | 'feature' | 'other';

export interface Asset {
  name: string;
  kind: AssetKind;
  width: number;
  height: number;
  size: number;
  alpha: boolean;
  added: number;
}

export interface IosListing {
  bundleId: string;
  sku: string;
  primaryCategory: string;
  secondaryCategory: string;
  name: string;
  subtitle: string;
  promo: string;
  description: string;
  keywords: string;
  whatsNew: string;
  version: string;
  copyright: string;
  ageRating: string;
  /** What the App Privacy section should say (data types, linked to the user, tracking). */
  privacy: string;
  reviewNotes: string;
  /** The app needs a sign-in: App Review needs a demo account. */
  signIn: boolean;
  /** The name of a saved bot login holding the demo account (never the password itself). */
  demoLogin: string;
  iPad: boolean;
  price: string;
}

export interface AndroidListing {
  packageName: string;
  category: string;
  tags: string;
  name: string;
  short: string;
  full: string;
  releaseNotes: string;
  contentRating: string;
  targetAge: string;
  ads: boolean;
  /** What the Data safety form should say. */
  dataSafety: string;
  appAccess: string;
  demoLogin: string;
  free: boolean;
}

export interface GnuSite {
  id: number;
  subdomain: string;
  url: string;
  /** slug → page id. */
  pages: Record<string, number>;
  /** Asset name → its gnu.au URL (so pictures aren't uploaded twice). */
  assets: Record<string, string>;
  built: number;
  published: number | null;
}

export interface LogEntry {
  at: number;
  text: string;
  ok?: boolean;
}

export interface Product {
  id: string;
  name: string;
  tagline: string;
  /** The detailed description, in the owner's words (Markdown). Everything is drafted from it. */
  description: string;
  audience: string;
  features: string;
  pricing: string;
  /** What user data it collects and why: feeds Apple's App Privacy and Play's Data safety. */
  data: string;
  company: string;
  contactName: string;
  contactEmail: string;
  contactPhone: string;
  address: string;
  urls: { website: string; app: string; support: string; privacy: string; terms: string; marketing: string };
  platforms: Record<Platform, boolean>;
  ios: IosListing;
  android: AndroidListing;
  gnu: GnuSite | null;
  assets: Asset[];
  log: LogEntry[];
  rag: { project: number; docs: number[]; added: number; memory: number | null } | null;
  created: number;
  updated: number;
}

export interface Guide {
  url: string;
  title: string;
  store: 'apple' | 'google';
  path: string;
  words: number;
  fetched: number;
}

export interface Check {
  ok: boolean;
  /** 'must': the store won't take it without; 'should': worth fixing; 'manual': done in the store. */
  level: 'must' | 'should' | 'manual';
  label: string;
  detail?: string;
  /** The tab it's fixed on. */
  tab: Tab;
}

export type Tab = 'overview' | 'ios' | 'android' | 'assets' | 'website' | 'launch';

/** The stores' limits (Apple: App Store Connect → Platform version information; Google: Play
 *  Console → Create and set up your app). Apple counts keywords in bytes. */
export const LIMITS = {
  ios: { name: 30, subtitle: 30, promo: 170, description: 4000, keywords: 100, whatsNew: 4000, reviewNotes: 4000 },
  android: { name: 30, short: 80, full: 4000, releaseNotes: 500 },
} as const;

export const ASSET_KINDS: { kind: AssetKind; label: string; hint: string; platform: Platform | null }[] = [
  { kind: 'icon-ios', label: 'App Store icon', hint: '1024×1024 PNG, no transparency', platform: 'ios' },
  { kind: 'iphone', label: 'iPhone screenshots', hint: '1–10, 6.9″ or 6.5″ display size (see the screenshot specifications guide)', platform: 'ios' },
  { kind: 'ipad', label: 'iPad screenshots', hint: '1–10, 13″ display size; needed when the app runs on iPad', platform: 'ios' },
  { kind: 'icon-play', label: 'Google Play icon', hint: '512×512 PNG, up to 1 MB', platform: 'android' },
  { kind: 'feature', label: 'Feature graphic', hint: '1024×500 JPEG or PNG, no transparency', platform: 'android' },
  { kind: 'phone', label: 'Phone screenshots', hint: '2–8, 320–3840 px a side, long side ≤ 2× the short one; 4+ at 1080 px or more to be featured', platform: 'android' },
  { kind: 'tablet', label: 'Tablet screenshots', hint: 'Optional, up to 8 for 7″ and 10″ tablets', platform: 'android' },
  { kind: 'other', label: 'Other pictures', hint: 'Logo, photos for the website', platform: null },
];

/** Official guides fetched for offline reading and RAG (one page each). */
export const GUIDE_SOURCES: { url: string; store: 'apple' | 'google'; label: string }[] = [
  { store: 'apple', label: 'App Review Guidelines', url: 'https://developer.apple.com/app-store/review/guidelines/' },
  { store: 'apple', label: 'Platform version information (field limits)', url: 'https://developer.apple.com/help/app-store-connect/reference/app-information/platform-version-information/' },
  { store: 'apple', label: 'Screenshot specifications', url: 'https://developer.apple.com/help/app-store-connect/reference/app-information/screenshot-specifications/' },
  { store: 'apple', label: 'App preview specifications', url: 'https://developer.apple.com/help/app-store-connect/reference/app-information/app-preview-specifications/' },
  { store: 'apple', label: 'Creating your product page', url: 'https://developer.apple.com/app-store/product-page/' },
  { store: 'apple', label: 'App Store search', url: 'https://developer.apple.com/app-store/search/' },
  { store: 'apple', label: 'App privacy details', url: 'https://developer.apple.com/app-store/app-privacy-details/' },
  { store: 'google', label: 'Create and set up your app', url: 'https://support.google.com/googleplay/android-developer/answer/9859152?hl=en' },
  { store: 'google', label: 'Preview assets (icon, feature graphic, screenshots)', url: 'https://support.google.com/googleplay/android-developer/answer/9866151?hl=en' },
  { store: 'google', label: 'Best practices for your store listing', url: 'https://support.google.com/googleplay/android-developer/answer/13393723?hl=en' },
  { store: 'google', label: 'Metadata policy', url: 'https://support.google.com/googleplay/android-developer/answer/9898842?hl=en' },
  { store: 'google', label: 'Data safety section', url: 'https://support.google.com/googleplay/android-developer/answer/10787469?hl=en' },
  { store: 'google', label: 'Get discovered on Google Play search', url: 'https://support.google.com/googleplay/android-developer/answer/4448378?hl=en' },
];

/** Where a bot logs in for each store (saved logins are bound to a domain and its subdomains). */
export const STORE_LOGIN_DOMAIN = { ios: 'apple.com', android: 'google.com' } as const;

const uid = () => crypto.randomUUID().slice(0, 8);
const clip = (s: string, n: number) => (s.length > n ? `${s.slice(0, n)}…` : s);
export const bytes = (s: string) => new TextEncoder().encode(s).length;
const isUrl = (s: string) => /^https:\/\/[^\s/]+\.[^\s]+/i.test(s.trim());

function blank(name: string): Product {
  const now = Date.now();
  return {
    id: uid(),
    name: name.trim().slice(0, 60) || 'New product',
    tagline: '',
    description: '',
    audience: '',
    features: '',
    pricing: '',
    data: '',
    company: '',
    contactName: '',
    contactEmail: '',
    contactPhone: '',
    address: '',
    urls: { website: '', app: '', support: '', privacy: '', terms: '', marketing: '' },
    platforms: { saas: true, ios: true, android: true },
    ios: { bundleId: '', sku: '', primaryCategory: '', secondaryCategory: '', name: '', subtitle: '', promo: '', description: '', keywords: '', whatsNew: '', version: '1.0', copyright: '', ageRating: '', privacy: '', reviewNotes: '', signIn: false, demoLogin: '', iPad: false, price: 'Free' },
    android: { packageName: '', category: '', tags: '', name: '', short: '', full: '', releaseNotes: '', contentRating: '', targetAge: '', ads: false, dataSafety: '', appAccess: '', demoLogin: '', free: true },
    gnu: null,
    assets: [],
    log: [],
    rag: null,
    created: now,
    updated: now,
  };
}

/** Fills in fields added since a product was saved. */
function upgrade(p: any): Product {
  const b = blank(p?.name ?? '');
  return { ...b, ...p, urls: { ...b.urls, ...p.urls }, platforms: { ...b.platforms, ...p.platforms }, ios: { ...b.ios, ...p.ios }, android: { ...b.android, ...p.android }, assets: p.assets ?? [], log: p.log ?? [] };
}

/** The first JSON object in a model's reply. */
function jsonIn(text: string): any {
  const t = text.replace(/<think>[\s\S]*?(<\/think>|$)/g, '');
  const fence = t.match(/```(?:json)?\s*\n?([\s\S]*?)```/);
  const raw = fence?.[1] ?? t.slice(t.indexOf('{'), t.lastIndexOf('}') + 1);
  return JSON.parse(raw);
}

/** A gnu.au API answer that isn't 2xx, as a sentence (DRF: detail, or field → messages). */
export function gnuError(r: { status: number; body: any; retry_after?: string | null }): string {
  const b = r.body;
  if (r.status === 429) return `gnu.au says to slow down: try again in ${r.retry_after ?? 'a few'} seconds`;
  if (b && typeof b === 'object') {
    if (typeof b.detail === 'string') return b.detail;
    return Object.entries(b)
      .map(([k, v]) => `${k}: ${Array.isArray(v) ? v.join(' ') : typeof v === 'string' ? v : JSON.stringify(v)}`)
      .join('; ');
  }
  return `HTTP ${r.status}${typeof b === 'string' && b ? `: ${clip(b, 200)}` : ''}`;
}

export interface GnuAnswer {
  status: number;
  body: any;
  retry_after?: string | null;
}

export const gnu = {
  status: () => invoke<{ base: string; has_token: boolean }>('gnu_status'),
  configure: (base: string, token: string | null) => invoke<{ base: string; has_token: boolean }>('gnu_configure', { base, token }),
  raw: (method: string, path: string, body?: unknown) => invoke<GnuAnswer>('gnu_request', { method, path, body: body ?? null }),
  /** A call that must succeed: the body, else an Error with gnu.au's message. */
  async call<R = any>(method: string, path: string, body?: unknown): Promise<R> {
    const r = await gnu.raw(method, path, body);
    if (r.status >= 400) throw Object.assign(new Error(gnuError(r)), { answer: r });
    return r.body as R;
  },
  upload: (site: number, id: string, names: string[]) => invoke<GnuAnswer>('gnu_upload', { site, id, names }),
};

class Products {
  list = $state<Product[]>([]);
  loaded = $state(false);
  /** The product open in the view. */
  openId = $state<string | null>(null);
  tab = $state<Tab>('overview');
  /** What's being done for a product ("Drafting the listings…"), by id. */
  busy = $state<Record<string, string>>({});
  guides = $state<Guide[]>([]);
  gnuInfo = $state<{ base: string; has_token: boolean } | null>(null);
  /** Pictures as blob URLs, by "id/name". */
  thumbs = $state<Record<string, string>>({});

  private loading: Promise<void> | null = null;
  private saveTimers = new Map<string, ReturnType<typeof setTimeout>>();
  private catalog: any[] | null = null;

  get current(): Product | null {
    return this.list.find((p) => p.id === this.openId) ?? null;
  }

  load() {
    if (this.loaded) return Promise.resolve();
    this.loading ??= (async () => {
      const [list, guides, info] = await Promise.all([invoke<any[]>('products_list').catch(() => []), invoke<Guide[]>('products_guides').catch(() => []), gnu.status().catch(() => null)]);
      this.list = list.map(upgrade);
      this.guides = guides;
      this.gnuInfo = info;
      this.loaded = true;
    })().finally(() => (this.loading = null));
    return this.loading;
  }

  find(idOrName: string | undefined | null): Product | undefined {
    if (!idOrName) return undefined;
    const k = idOrName.trim().toLowerCase();
    return this.list.find((p) => p.id === idOrName) ?? this.list.find((p) => p.name.toLowerCase() === k) ?? this.list.find((p) => p.name.toLowerCase().includes(k));
  }

  async create(name: string) {
    await this.load();
    const p = blank(name);
    this.list = [p, ...this.list];
    await invoke('products_save', { product: $state.snapshot(p) });
    this.openId = p.id;
    this.tab = 'overview';
    return this.list[0];
  }

  /** Saves now (`soon`: after a pause in typing). */
  save(p: Product, soon = false) {
    p.updated = Date.now();
    clearTimeout(this.saveTimers.get(p.id));
    const go = () => {
      this.saveTimers.delete(p.id);
      return invoke('products_save', { product: $state.snapshot(p) }).catch((e) => store.error(e, 'Couldn’t save the product'));
    };
    if (soon) {
      this.saveTimers.set(p.id, setTimeout(go, 700));
      return Promise.resolve();
    }
    return go();
  }

  async remove(id: string) {
    const p = this.find(id);
    if (p?.rag) await this.removeFromRag(p).catch(() => {});
    await invoke('products_delete', { id });
    this.list = this.list.filter((x) => x.id !== id);
    if (this.openId === id) this.openId = null;
  }

  log(p: Product, text: string, ok?: boolean) {
    p.log = [{ at: Date.now(), text, ok }, ...p.log].slice(0, 100);
    void this.save(p);
  }

  private async task<R>(p: Product, what: string, f: () => Promise<R>): Promise<R> {
    if (this.busy[p.id]) throw new Error(`Still busy: ${this.busy[p.id]}`);
    this.busy[p.id] = what;
    try {
      return await f();
    } finally {
      delete this.busy[p.id];
    }
  }

  // ---------------------------------------------------------------- pictures

  async addAssets(p: Product, paths: string[], kind: AssetKind) {
    const res = await invoke<any[]>('products_add_assets', { id: p.id, paths, kind });
    const added = res.filter((r) => !r.error) as Asset[];
    const failed = res.filter((r) => r.error);
    p.assets = [...p.assets, ...added];
    await this.save(p);
    if (failed.length) store.toast('error', 'Some pictures weren’t added', failed.map((f) => `${f.name}: ${f.error}`).join('\n'), 9000);
    return added;
  }

  async removeAsset(p: Product, name: string) {
    await invoke('products_remove_asset', { id: p.id, name });
    p.assets = p.assets.filter((a) => a.name !== name);
    const k = `${p.id}/${name}`;
    if (this.thumbs[k]) URL.revokeObjectURL(this.thumbs[k]);
    delete this.thumbs[k];
    await this.save(p);
  }

  async thumb(p: Product, name: string): Promise<string> {
    const k = `${p.id}/${name}`;
    if (this.thumbs[k]) return this.thumbs[k];
    const buf = await invoke<ArrayBuffer>('products_asset', { id: p.id, name });
    const url = URL.createObjectURL(new Blob([buf], { type: /\.png$/i.test(name) ? 'image/png' : 'image/jpeg' }));
    this.thumbs[k] = url;
    return url;
  }

  assetsOf(p: Product, kind: AssetKind) {
    return p.assets.filter((a) => a.kind === kind);
  }

  // ---------------------------------------------------------------- readiness

  checks(p: Product): Record<Platform, Check[]> {
    const out: Record<Platform, Check[]> = { saas: [], ios: [], android: [] };
    const len = (label: string, v: string, max: number, tab: Tab, required = true, count: (s: string) => number = (s) => [...s].length): Check => {
      const n = count(v.trim());
      return { ok: (!required || n > 0) && n <= max, level: required ? 'must' : 'should', label, detail: n > max ? `${n} of ${max}: too long` : n ? `${n} of ${max}` : 'empty', tab };
    };
    const has = (label: string, ok: boolean, tab: Tab, level: Check['level'] = 'must', detail?: string): Check => ({ ok, level, label, detail, tab });
    const url = (label: string, v: string, tab: Tab, level: Check['level'] = 'must') => has(label, isUrl(v), tab, level, v ? (isUrl(v) ? v : 'must be an https:// address') : 'missing');
    const shots = (kind: AssetKind) => this.assetsOf(p, kind);

    if (p.platforms.ios) {
      const i = p.ios;
      const c = out.ios;
      c.push(has('Bundle ID', /^[A-Za-z0-9.-]+\.[A-Za-z0-9.-]+$/.test(i.bundleId), 'ios', 'must', i.bundleId || 'e.g. au.com.company.app'));
      c.push(len('App name', i.name, LIMITS.ios.name, 'ios'));
      c.push(len('Subtitle', i.subtitle, LIMITS.ios.subtitle, 'ios', false));
      c.push(len('Description', i.description, LIMITS.ios.description, 'ios'));
      c.push(len('Keywords (bytes)', i.keywords, LIMITS.ios.keywords, 'ios', true, bytes));
      c.push(len('Promotional text', i.promo, LIMITS.ios.promo, 'ios', false));
      c.push(has('Primary category', !!i.primaryCategory.trim(), 'ios'));
      c.push(has('Copyright', !!i.copyright.trim(), 'ios', 'must', i.copyright || 'e.g. 2026 Company Pty Ltd'));
      c.push(url('Support URL', p.urls.support, 'website'));
      c.push(url('Privacy policy URL', p.urls.privacy, 'website'));
      c.push(has('App Privacy answers', !!(i.privacy.trim() || p.data.trim()), 'ios', 'must', 'what data the app collects (Overview → Data, or App Privacy here)'));
      c.push(has('Review contact', !!(p.contactName && p.contactEmail && p.contactPhone), 'overview', 'must', 'name, email and phone for App Review'));
      if (i.signIn) c.push(has('Demo account for App Review', !!i.demoLogin, 'ios', 'must', 'a saved login with the demo account'));
      const icon = shots('icon-ios')[0];
      c.push(has('App icon 1024×1024', !!icon && icon.width === 1024 && icon.height === 1024 && !icon.alpha, 'assets', 'should', icon ? `${icon.width}×${icon.height}${icon.alpha ? ', has transparency' : ''}` : 'Xcode builds carry the icon; handy for the website'));
      const ip = shots('iphone');
      c.push(has('iPhone screenshots', ip.length >= 1 && ip.length <= 10, 'assets', 'must', `${ip.length} (1–10)`));
      if (i.iPad) {
        const pad = shots('ipad');
        c.push(has('iPad screenshots', pad.length >= 1 && pad.length <= 10, 'assets', 'must', `${pad.length} (1–10)`));
      }
      c.push(has('Age rating questionnaire', !!i.ageRating.trim(), 'ios', 'manual', 'answers written down for the bot'));
      c.push(has('Build uploaded (Xcode / Transporter)', false, 'launch', 'manual', 'not something a bot does'));
    }
    if (p.platforms.android) {
      const a = p.android;
      const c = out.android;
      c.push(has('Package name', /^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$/.test(a.packageName), 'android', 'must', a.packageName || 'e.g. au.com.company.app'));
      c.push(len('App name', a.name, LIMITS.android.name, 'android'));
      c.push(len('Short description', a.short, LIMITS.android.short, 'android'));
      c.push(len('Full description', a.full, LIMITS.android.full, 'android'));
      c.push(len('Release notes', a.releaseNotes, LIMITS.android.releaseNotes, 'android', false));
      c.push(has('Category', !!a.category.trim(), 'android'));
      c.push(has('Contact email', /.+@.+\..+/.test(p.contactEmail), 'overview'));
      c.push(url('Privacy policy URL', p.urls.privacy, 'website'));
      c.push(has('Data safety answers', !!(a.dataSafety.trim() || p.data.trim()), 'android'));
      const icon = shots('icon-play')[0];
      c.push(has('Icon 512×512 PNG', !!icon && icon.width === 512 && icon.height === 512 && /\.png$/i.test(icon.name) && icon.size <= 1024 * 1024, 'assets', 'must', icon ? `${icon.width}×${icon.height}, ${Math.round(icon.size / 1024)} KB` : 'missing'));
      const fg = shots('feature')[0];
      c.push(has('Feature graphic 1024×500', !!fg && fg.width === 1024 && fg.height === 500 && !fg.alpha, 'assets', 'must', fg ? `${fg.width}×${fg.height}${fg.alpha ? ', has transparency' : ''}` : 'missing'));
      const ph = shots('phone');
      const bad = ph.filter((s) => Math.min(s.width, s.height) < 320 || Math.max(s.width, s.height) > 3840 || Math.max(s.width, s.height) > 2 * Math.min(s.width, s.height) || s.alpha);
      c.push(has('Phone screenshots', ph.length >= 2 && ph.length <= 8 && !bad.length, 'assets', 'must', `${ph.length} (2–8)${bad.length ? `; ${bad.length} the wrong size or with transparency` : ''}`));
      c.push(has('4+ screenshots at 1080 px (to be featured)', ph.filter((s) => Math.min(s.width, s.height) >= 1080).length >= 4, 'assets', 'should'));
      c.push(has('Content rating questionnaire', !!a.contentRating.trim(), 'android', 'manual', 'answers written down for the bot'));
      c.push(has('Target audience', !!a.targetAge.trim(), 'android', 'manual'));
      c.push(has('App bundle uploaded (.aab)', false, 'launch', 'manual', 'not something a bot does'));
    }
    if (p.platforms.saas) {
      const c = out.saas;
      c.push(url('Website', p.urls.website, 'website'));
      c.push(url('Web app address', p.urls.app, 'overview', 'should'));
      c.push(url('Support page', p.urls.support, 'website'));
      c.push(url('Privacy policy', p.urls.privacy, 'website'));
      c.push(url('Terms of service', p.urls.terms, 'website', 'should'));
      c.push(has('Website published on gnu.au', !!p.gnu?.published, 'website', 'should', p.gnu ? (p.gnu.published ? new Date(p.gnu.published).toLocaleString('en-GB') : 'built, not published') : 'not built'));
    }
    return out;
  }

  /** Share of the must-haves done, per platform that's on. */
  readiness(p: Product): { platform: Platform; done: number; total: number }[] {
    const c = this.checks(p);
    return (Object.keys(c) as Platform[]).filter((k) => p.platforms[k]).map((k) => ({ platform: k, done: c[k].filter((x) => x.level === 'must' && x.ok).length, total: c[k].filter((x) => x.level === 'must').length }));
  }

  // ---------------------------------------------------------------- what everything is drafted from

  /** The product in full, as Markdown: the RAG copy, the model's brief and the bots' notes. */
  brief(p: Product, opts: { listings?: boolean } = {}): string {
    const L = (k: string, v: string | boolean | undefined) => (v === undefined || v === '' ? '' : `- **${k}:** ${typeof v === 'boolean' ? (v ? 'yes' : 'no') : v}\n`);
    const plats = (Object.keys(p.platforms) as Platform[]).filter((k) => p.platforms[k]).map((k) => ({ saas: 'SaaS web platform', ios: 'iOS app', android: 'Android app' })[k]);
    let s = `# ${p.name}\n\n${p.tagline ? `_${p.tagline}_\n\n` : ''}A product managed in RAD Products. Platforms: ${plats.join(', ') || 'none chosen'}.\n\n`;
    s += `## Description\n\n${p.description.trim() || '(not written yet)'}\n\n`;
    if (p.features.trim()) s += `## Key features\n\n${p.features.trim()}\n\n`;
    if (p.audience.trim()) s += `## Who it's for\n\n${p.audience.trim()}\n\n`;
    if (p.pricing.trim()) s += `## Pricing\n\n${p.pricing.trim()}\n\n`;
    if (p.data.trim()) s += `## User data it collects\n\n${p.data.trim()}\n\n`;
    s += `## Company and contact\n\n${L('Company', p.company)}${L('Contact', p.contactName)}${L('Email', p.contactEmail)}${L('Phone', p.contactPhone)}${L('Address', p.address)}\n`;
    s += `## Addresses\n\n${L('Website', p.urls.website)}${L('Web app', p.urls.app)}${L('Support', p.urls.support)}${L('Privacy policy', p.urls.privacy)}${L('Terms', p.urls.terms)}${L('Marketing', p.urls.marketing)}${p.gnu ? L('gnu.au site', `${p.gnu.url} (site ${p.gnu.id}${p.gnu.published ? `, published ${new Date(p.gnu.published).toLocaleDateString('en-GB')}` : ', not published'})`) : ''}\n`;
    if (opts.listings !== false) {
      if (p.platforms.ios) {
        const i = p.ios;
        s += `## App Store (iOS)\n\n${L('Bundle ID', i.bundleId)}${L('SKU', i.sku)}${L('Version', i.version)}${L('Price', i.price)}${L('Primary category', i.primaryCategory)}${L('Secondary category', i.secondaryCategory)}${L('Name', i.name)}${L('Subtitle', i.subtitle)}${L('Promotional text', i.promo)}${L('Keywords', i.keywords)}${L('Copyright', i.copyright)}${L('Runs on iPad', i.iPad)}${L('Needs sign-in', i.signIn)}\n`;
        if (i.description) s += `### Description\n\n${i.description}\n\n`;
        if (i.whatsNew) s += `### What's new\n\n${i.whatsNew}\n\n`;
        if (i.ageRating) s += `### Age rating answers\n\n${i.ageRating}\n\n`;
        if (i.privacy) s += `### App Privacy answers\n\n${i.privacy}\n\n`;
        if (i.reviewNotes) s += `### Notes for App Review\n\n${i.reviewNotes}\n\n`;
      }
      if (p.platforms.android) {
        const a = p.android;
        s += `## Google Play (Android)\n\n${L('Package name', a.packageName)}${L('Free', a.free)}${L('Category', a.category)}${L('Tags', a.tags)}${L('Name', a.name)}${L('Short description', a.short)}${L('Contains ads', a.ads)}${L('Target age groups', a.targetAge)}\n`;
        if (a.full) s += `### Full description\n\n${a.full}\n\n`;
        if (a.releaseNotes) s += `### Release notes\n\n${a.releaseNotes}\n\n`;
        if (a.contentRating) s += `### Content rating answers\n\n${a.contentRating}\n\n`;
        if (a.dataSafety) s += `### Data safety answers\n\n${a.dataSafety}\n\n`;
        if (a.appAccess) s += `### App access (for review)\n\n${a.appAccess}\n\n`;
      }
    }
    return s.trim() + '\n';
  }

  /** Excerpts of the fetched guides for a store, for the model (they win over what it remembers). */
  private async guideText(store: 'apple' | 'google', max = 6000): Promise<string> {
    const gs = this.guides.filter((g) => g.store === store && /version information|preview assets|create and set up|metadata|store listing/i.test(g.title + g.url));
    let out = '';
    for (const g of gs) {
      const t = await invoke<string>('products_guide_text', { url: g.url }).catch(() => '');
      out += `\n--- ${g.title} (${g.url}) ---\n${t.slice(0, Math.max(0, max - out.length))}`;
      if (out.length >= max) break;
    }
    return out.trim();
  }

  // ---------------------------------------------------------------- drafting with the model

  /** Writes the store listings (and the privacy / rating answers) from the product's
   *  description, within the stores' limits; anything too long is sent back to be shortened. */
  async draftListings(p: Product, which: 'ios' | 'android' | 'both' = 'both') {
    return this.task(p, 'Drafting the store listings…', async () => {
      if (p.description.trim().length < 40) throw new Error('Write the product’s description first (Overview): the listings are drafted from it.');
      const ios = which !== 'android' && p.platforms.ios;
      const android = which !== 'ios' && p.platforms.android;
      if (!ios && !android) throw new Error('Turn on the iOS or Android app in Overview first.');
      const guides = [ios ? await this.guideText('apple', 4000) : '', android ? await this.guideText('google', 4000) : ''].filter(Boolean).join('\n\n');
      const shape = {
        ...(ios ? { ios: { name: `≤${LIMITS.ios.name} chars`, subtitle: `≤${LIMITS.ios.subtitle} chars`, promo: `≤${LIMITS.ios.promo} chars`, description: `plain text, ≤${LIMITS.ios.description} chars`, keywords: `comma-separated, no spaces after commas, ≤${LIMITS.ios.keywords} bytes, not repeating words in the name`, whatsNew: 'short, for version ' + (p.ios.version || '1.0'), primaryCategory: 'an App Store category', secondaryCategory: 'another, or ""', ageRating: 'likely answers to the age rating questionnaire, one per line', privacy: "App Privacy answers: each data type collected, its purposes, whether it's linked to the user, whether it's used for tracking", reviewNotes: 'notes for App Review' } } : {}),
        ...(android ? { android: { name: `≤${LIMITS.android.name} chars`, short: `≤${LIMITS.android.short} chars`, full: `plain text, ≤${LIMITS.android.full} chars`, releaseNotes: `≤${LIMITS.android.releaseNotes} chars`, category: 'a Google Play category', tags: 'up to 5 Play tags, comma-separated', contentRating: 'likely answers to the IARC content rating questionnaire, one per line', targetAge: 'target age groups', dataSafety: 'Data safety answers: data collected and shared, purposes, encrypted in transit, can users ask for deletion' } } : {}),
      };
      const system = `You write app store listings for a product manager. Use only facts from the product brief; never invent features, awards, user numbers or prices. Follow the stores' rules: no "#1", "best", "free" claims or promotions in names; no competitor or trademark names; no keyword stuffing; plain text (no Markdown) in descriptions, short paragraphs and simple "• " bullet lines are fine; lead the descriptions with what the product does for the user. Respect every character limit exactly. Where the brief doesn't say (e.g. data collection), write what it most likely is and add "(check)" at the end of that line.
Reply with JSON only, in this shape: ${JSON.stringify(shape)}${guides ? `\n\nFROM THE STORES' OWN GUIDES (they override anything you remember):\n${guides}` : ''}`;
      const user = this.brief(p, { listings: false });
      let r = await rad.assist(system, [{ role: 'user', content: user }], 4000);
      let v = jsonIn(r.content);
      const over = (v: any) => {
        const o: string[] = [];
        if (v.ios) for (const k of ['name', 'subtitle', 'promo', 'description', 'whatsNew'] as const) if (typeof v.ios[k] === 'string' && [...v.ios[k]].length > LIMITS.ios[k]) o.push(`ios.${k} is ${[...v.ios[k]].length} characters (limit ${LIMITS.ios[k]})`);
        if (v.ios?.keywords && bytes(v.ios.keywords) > LIMITS.ios.keywords) o.push(`ios.keywords is ${bytes(v.ios.keywords)} bytes (limit ${LIMITS.ios.keywords})`);
        if (v.android) for (const k of ['name', 'short', 'full', 'releaseNotes'] as const) if (typeof v.android[k] === 'string' && [...v.android[k]].length > LIMITS.android[k]) o.push(`android.${k} is ${[...v.android[k]].length} characters (limit ${LIMITS.android[k]})`);
        return o;
      };
      const tooLong = over(v);
      if (tooLong.length) {
        r = await rad.assist(system, [{ role: 'user', content: user }, { role: 'assistant', content: r.content }, { role: 'user', content: `Too long: ${tooLong.join('; ')}. Shorten those fields to fit and reply with the whole JSON again.` }], 4000);
        v = jsonIn(r.content);
      }
      const str = (x: any) => (typeof x === 'string' ? x.trim() : '');
      if (v.ios && ios) for (const k of ['name', 'subtitle', 'promo', 'description', 'keywords', 'whatsNew', 'primaryCategory', 'secondaryCategory', 'ageRating', 'privacy', 'reviewNotes'] as const) if (str(v.ios[k])) (p.ios as any)[k] = str(v.ios[k]).replace(/,\s+/g, k === 'keywords' ? ',' : ', ');
      if (v.android && android) for (const k of ['name', 'short', 'full', 'releaseNotes', 'category', 'tags', 'contentRating', 'targetAge', 'dataSafety'] as const) if (str(v.android[k])) (p.android as any)[k] = str(v.android[k]);
      if (!p.ios.copyright && p.company) p.ios.copyright = `${new Date().getFullYear()} ${p.company}`;
      const left = over({ ios: ios ? p.ios : null, android: android ? p.android : null });
      this.log(p, `Drafted the ${[ios && 'App Store', android && 'Google Play'].filter(Boolean).join(' and ')} listing${ios && android ? 's' : ''}${left.length ? ` (still too long: ${left.join('; ')})` : ''}`, !left.length);
      return left;
    });
  }

  // ---------------------------------------------------------------- the website on gnu.au

  private async blockCatalog() {
    if (this.catalog) return this.catalog;
    const c = await gnu.call<{ results: any[] }>('GET', '/blocks/');
    this.catalog = c.results.map((b) => ({
      type: b.type,
      description: b.description,
      fields: (b.fields ?? []).map((f: any) => `${f.key}:${f.type}${f.options ? `(${f.options.map((o: any) => o.value).join('|')})` : ''}${f.item_fields ? `[${f.item_fields.map((i: any) => `${i.key}:${i.type}`).join(',')}]` : ''}`).join(', '),
    }));
    return this.catalog;
  }

  /** Builds (or rebuilds) the product's website on gnu.au: home, features, pricing, support,
   *  privacy and terms pages, written by the model from the product and the block catalogue.
   *  The site's addresses become the product's support / privacy / terms URLs. Not published. */
  async buildSite(p: Product, subdomain?: string) {
    return this.task(p, 'Building the website on gnu.au…', async () => {
      const set = (s: string) => (this.busy[p.id] = s);
      if (p.description.trim().length < 40) throw new Error('Write the product’s description first (Overview): the site is written from it.');
      const info = (this.gnuInfo = await gnu.status());
      if (!info.has_token) throw new Error('Add your gnu.au API token first (Products → gnu.au settings).');
      const catalog = await this.blockCatalog();
      let site: any;
      if (p.gnu) {
        site = await gnu.call('GET', `/sites/${p.gnu.id}/`).catch((e: any) => {
          if (e?.answer?.status === 404) return null;
          throw e;
        });
      }
      if (!site) {
        const sub = (subdomain ?? p.gnu?.subdomain ?? p.name).toLowerCase().replace(/[^a-z0-9-]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 32);
        set(`Creating ${sub}.gnu.au…`);
        site = await gnu.call('POST', '/sites/', { subdomain: sub, title: p.name.slice(0, 120), description: p.tagline.slice(0, 300), template: 'blank' });
        p.gnu = { id: site.id, subdomain: site.subdomain, url: site.url, pages: {}, assets: {}, built: 0, published: null };
        this.log(p, `Created the site ${site.url} on gnu.au`, true);
        await this.save(p);
      }
      const g = p.gnu!;
      // Pictures first, so the pages can use them.
      const wanted = [...this.assetsOf(p, 'icon-ios'), ...this.assetsOf(p, 'icon-play'), ...this.assetsOf(p, 'feature'), ...this.assetsOf(p, 'other'), ...this.assetsOf(p, 'iphone').slice(0, 4), ...this.assetsOf(p, 'phone').slice(0, 4)].filter((a) => !g.assets[a.name]);
      if (wanted.length) {
        set(`Uploading ${wanted.length} picture${wanted.length === 1 ? '' : 's'}…`);
        const r = await gnu.upload(g.id, p.id, wanted.map((a) => a.name));
        if (r.status >= 400 && !r.body?.results?.length) throw new Error(gnuError(r));
        for (const a of r.body?.results ?? []) {
          const mine = wanted.find((w) => w.name === a.name);
          if (mine) g.assets[mine.name] = a.url;
        }
      }
      const pictures = Object.entries(g.assets).map(([name, url]) => `- ${url}: ${p.assets.find((a) => a.name === name)?.kind ?? 'picture'} (${name})`).join('\n');
      const pages = ['index (home: navbar with autoPages, hero, features, stats or testimonials only if the brief has them, call to action, footer)', 'features', ...(p.pricing.trim() ? ['pricing'] : []), 'support (how to get help, contact block with the contact email, FAQ)', 'privacy (privacy policy built from the user data section; plain and complete: what is collected, why, sharing, retention, user rights, contact; end with the date)', 'terms (terms of service)'];
      const system = `You build websites on gnu.au from blocks. A page is {"slug": "...", "title": "...", "seo_description": "≤160 chars", "show_in_nav": true, "blocks": [{"type": "...", "props": {...}, "style": {...}}]}. Use only these block types and their props:
${catalog.map((b) => `- ${b.type}: ${b.description} Props: ${b.fields}`).join('\n')}
Style keys (all optional): tone (default|surface|primary|dark), padding (none|s|m|l|xl), align (left|center), anchor. Rich text props take <p>, <ul><li>, <strong>, <a href>, <h3>. Links between pages are "/slug" ("/" for home). Every page starts with a navbar (autoPages true, brand = product name) and ends with a footer (tone dark, "© year company"). Use the pictures given (their URLs as given) where they fit: the icon as navbar logo, screenshots in a hero image or gallery. Use only facts from the brief; never invent customers, numbers, quotes or prices. Write for the product's audience, clear and specific.
Reply with JSON only: {"theme": {"primary": "#hex", "accent": "#hex", "headingFont": "Inter|Poppins|Space Grotesk|DM Sans|Manrope|Playfair Display|Lora|Merriweather|Fraunces", "bodyFont": "…"}, "pages": [page, …]}`;
      const user = `${this.brief(p, { listings: false })}\nTHE PAGES TO MAKE (slugs): ${pages.join('; ')}.\nApp Store / Google Play links: ${p.platforms.ios ? 'iOS app coming to the App Store' : ''}${p.platforms.android ? '; Android app coming to Google Play' : ''}${p.urls.app ? `; web app at ${p.urls.app}` : ''}.\nPICTURES:\n${pictures || '(none)'}`;
      set('Writing the pages…');
      const r = await rad.assist(system, [{ role: 'user', content: user }], 12000);
      const plan = jsonIn(r.content);
      const out: any[] = Array.isArray(plan.pages) ? plan.pages : [];
      if (!out.length) throw new Error('The model didn’t write any pages: try again, or a stronger model.');
      if (plan.theme && typeof plan.theme === 'object') {
        const theme = Object.fromEntries(Object.entries(plan.theme).filter(([k, v]) => ['primary', 'accent', 'headingFont', 'bodyFont'].includes(k) && typeof v === 'string'));
        await gnu.call('PATCH', `/sites/${g.id}/`, { theme }).catch(() => gnu.call('PATCH', `/sites/${g.id}/`, { theme: { primary: theme.primary } }).catch(() => {}));
      }
      const existing = await gnu.call<{ results: any[] }>('GET', `/sites/${g.id}/pages/?limit=200`);
      const bySlug = new Map(existing.results.map((x) => [x.slug, x.id]));
      const failed: string[] = [];
      for (const page of out) {
        const slug = String(page.slug ?? '').toLowerCase().replace(/^\/+/, '').replace(/[^a-z0-9-]/g, '-') || 'index';
        const body = { title: String(page.title ?? slug).slice(0, 120), seo_description: String(page.seo_description ?? '').slice(0, 300), show_in_nav: page.show_in_nav !== false, blocks: page.blocks ?? [] };
        set(`Saving the ${body.title} page…`);
        const send = (b: typeof body) => {
          const id = bySlug.get(slug === 'index' ? 'index' : slug);
          return id ? gnu.raw('PUT', `/sites/${g.id}/pages/${id}/`, b) : gnu.raw('POST', `/sites/${g.id}/pages/`, { ...b, slug });
        };
        let res = await send(body);
        if (res.status === 400) {
          // The error names the block at fault (blocks[3].style): the model fixes that page once.
          set(`Fixing the ${body.title} page…`);
          const fix = await rad.assist(system, [{ role: 'user', content: user }, { role: 'assistant', content: JSON.stringify({ pages: [page] }) }, { role: 'user', content: `gnu.au refused the "${slug}" page: ${gnuError(res)}. Reply with that one page fixed, as {"pages": [page]}.` }], 6000).catch(() => null);
          const fixed = fix ? (() => { try { return jsonIn(fix.content).pages?.[0]; } catch { return null; } })() : null;
          if (fixed?.blocks) res = await send({ ...body, blocks: fixed.blocks });
        }
        if (res.status >= 400) failed.push(`${slug}: ${gnuError(res)}`);
        else {
          g.pages[slug] = res.body.id;
          bySlug.set(slug, res.body.id);
        }
      }
      g.built = Date.now();
      g.url = site.url ?? g.url;
      const at = (slug: string) => (g.pages[slug] ? `${g.url.replace(/\/$/, '')}/${slug}` : '');
      // The site's pages become the product's addresses, unless the owner set others.
      const mine = (u: string) => !u || u.startsWith(g.url.replace(/\/$/, ''));
      if (mine(p.urls.website)) p.urls.website = g.url;
      if (mine(p.urls.support) && at('support')) p.urls.support = at('support');
      if (mine(p.urls.privacy) && at('privacy')) p.urls.privacy = at('privacy');
      if (mine(p.urls.terms) && at('terms')) p.urls.terms = at('terms');
      if (mine(p.urls.marketing)) p.urls.marketing = g.url;
      this.log(p, `Built ${Object.keys(g.pages).length} page${Object.keys(g.pages).length === 1 ? '' : 's'} on ${g.url}${failed.length ? `; couldn’t save ${failed.join(' · ')}` : ''}. Not published yet.`, !failed.length);
      await this.save(p);
      return failed;
    });
  }

  /** The draft of the site's home page, as a full HTML document (gnu.au renders it). */
  async previewSite(p: Product, slug = 'index'): Promise<string> {
    if (!p.gnu?.pages[slug]) throw new Error('Build the site first');
    const r = await gnu.call<{ document: string }>('GET', `/sites/${p.gnu.id}/pages/${p.gnu.pages[slug]}/render/?document=1`);
    return r.document;
  }

  async publishSite(p: Product) {
    return this.task(p, 'Publishing the website…', async () => {
      if (!p.gnu) throw new Error('Build the site first');
      const r = await gnu.call<{ url: string; published_at: string }>('POST', `/sites/${p.gnu.id}/publish/`);
      p.gnu.published = Date.parse(r.published_at) || Date.now();
      this.log(p, `Published ${r.url}`, true);
      await this.save(p);
      return r.url;
    });
  }

  // ---------------------------------------------------------------- the stores, by bots

  /** The saved logins a bot could use for a store. */
  storeLogins(botId: string, platform: 'ios' | 'android') {
    const d = STORE_LOGIN_DOMAIN[platform];
    return bots.secrets.filter((s) => s.bots.includes(botId) && (s.domain === d || s.domain.endsWith(`.${d}`) || onDomain(`https://${s.domain}`, d)));
  }

  /** What the bot types in, field by field, and which file goes where. */
  listingFile(p: Product, platform: 'ios' | 'android'): string {
    const files = (kind: AssetKind) => this.assetsOf(p, kind).map((a) => `/root/shared/products/${p.id}/${a.name} (${a.width}×${a.height})`);
    const list = (k: AssetKind, title: string) => (files(k).length ? `\n### ${title}\n${files(k).map((f) => `- ${f}`).join('\n')}\n` : '');
    const head = `# ${platform === 'ios' ? 'App Store Connect' : 'Google Play Console'} listing for ${p.name}\n\nEverything below goes into the store as written. Pictures are in this folder.\n\n`;
    const pics = platform === 'ios' ? list('icon-ios', 'App icon') + list('iphone', 'iPhone screenshots (in this order)') + list('ipad', 'iPad screenshots (in this order)') : list('icon-play', 'App icon (512×512)') + list('feature', 'Feature graphic (1024×500)') + list('phone', 'Phone screenshots (in this order)') + list('tablet', 'Tablet screenshots');
    return `${head}${this.brief({ ...p, platforms: { saas: false, ios: platform === 'ios', android: platform === 'android' } })}\n## Pictures\n${pics || '\n(none)\n'}`;
  }

  /** Gives a bot the job of filling in a store for the product. Resolves once it has started. */
  async launch(p: Product, platform: 'ios' | 'android', botId: string, extra = '') {
    await bots.load();
    const bot = bots.bot(botId);
    if (!bot) throw new Error('There’s no such bot: make one in Bots');
    if (bots.busy(bot.id)) throw new Error(`${bot.name} is busy with another task`);
    const logins = this.storeLogins(bot.id, platform);
    if (!logins.length) throw new Error(`${bot.name} has no saved login for ${STORE_LOGIN_DOMAIN[platform]}. Add one in Bots → Logins (domain ${STORE_LOGIN_DOMAIN[platform]}) and give it to ${bot.name}.`);
    this.busy[p.id] = `Preparing ${bot.name}’s computer…`;
    let dir: string;
    try {
      const container = await bots.computer(bot);
      dir = await invoke<string>('products_to_container', { id: p.id, container, files: { 'LISTING.md': this.listingFile(p, platform) } });
    } finally {
      delete this.busy[p.id];
    }
    const login = logins[0].name;
    const demo = platform === 'ios' ? p.ios.demoLogin : p.android.demoLogin;
    const task =
      platform === 'ios'
        ? `Fill in the App Store Connect listing of the iOS app "${p.ios.name || p.name}" (bundle ID ${p.ios.bundleId || 'see LISTING.md'}).
Everything to enter is in ${dir}/LISTING.md: read it first (shell: cat ${dir}/LISTING.md). The pictures are in ${dir}/.
1. Open https://appstoreconnect.apple.com/apps and log in with the saved login "${login}" (ask the owner for the two-factor code when it's asked for).
2. Find the app. If it isn't there, use confirm before creating it as a New App (iOS, name, primary language English (Australia) unless LISTING.md says otherwise, the bundle ID, SKU ${p.ios.sku || p.id}).
3. App Information: subtitle, primary and secondary category, content rights, age rating (answer the questionnaire from LISTING.md), privacy policy URL ${p.urls.privacy || '(missing: ask)'}.
4. Pricing and Availability: ${p.ios.price || 'Free'}, all countries unless LISTING.md says otherwise.
5. App Privacy: answer from the App Privacy section of LISTING.md (data types, purposes, linked to the user, tracking). Ask the owner if something isn't covered.
6. The version page (iOS App, version ${p.ios.version || '1.0'}): upload the screenshots with "upload" in the order listed (iPhone, then iPad), promotional text, description, keywords, support URL ${p.urls.support || '(missing)'}, marketing URL ${p.urls.marketing || p.urls.website || '(none)'}, copyright, and the App Review information: contact ${p.contactName}, ${p.contactEmail}, ${p.contactPhone}${p.ios.signIn ? `; sign-in required: the demo account is the saved login "${demo || '(none: ask the owner)'}" (type it with login)` : '; no sign-in required'}; notes from LISTING.md. Save after each section.
7. Never press "Add for Review" or "Submit for Review", and don't pick a build. Stop there.
Finish with done: every field you filled in, every picture uploaded, and what's still missing or needs the owner (e.g. no build uploaded yet).`
        : `Fill in the Google Play Console listing of the Android app "${p.android.name || p.name}" (package ${p.android.packageName || 'see LISTING.md'}).
Everything to enter is in ${dir}/LISTING.md: read it first (shell: cat ${dir}/LISTING.md). The pictures are in ${dir}/.
1. Open https://play.google.com/console and log in with the saved login "${login}" (ask the owner for any verification code).
2. Find the app in All apps. If it isn't there, use confirm before creating it (app name, default language English (Australia) unless LISTING.md says otherwise, App, ${p.android.free ? 'Free' : 'Paid'}, and the declarations).
3. Grow users → Store presence → Main store listing: app name, short description, full description; upload the app icon, the feature graphic and the phone (and tablet) screenshots with "upload", in the order listed. Save.
4. Store settings: category ${p.android.category || '(from LISTING.md)'}, tags, contact email ${p.contactEmail}${p.contactPhone ? `, phone ${p.contactPhone}` : ''}, website ${p.urls.website || '(none)'}.
5. Policy → App content: privacy policy ${p.urls.privacy || '(missing: ask)'}; ads (${p.android.ads ? 'contains ads' : 'no ads'}); app access${demo ? ` (the demo account is the saved login "${demo}")` : ' (from LISTING.md)'}; content rating questionnaire, target audience and Data safety from LISTING.md. Ask the owner when LISTING.md doesn't answer a question.
6. Never send changes for review, start a rollout or publish. Stop there.
Finish with done: every field you filled in, every picture uploaded, and what's still missing or needs the owner (e.g. no app bundle uploaded yet).`;
    const full = extra.trim() ? `${task}\n\nThe owner adds: ${extra.trim()}` : task;
    this.log(p, `${bot.name} started on ${platform === 'ios' ? 'App Store Connect' : 'Google Play Console'}`);
    void bots
      .run(bot.id, full, { from: 'owner' })
      .then((t) => this.log(p, `${bot.name}, ${platform === 'ios' ? 'App Store Connect' : 'Google Play Console'}: ${t.status} — ${clip(t.result, 400)}`, t.status === 'done'))
      .catch((e) => this.log(p, `${bot.name} couldn’t start: ${e?.message ?? e}`, false));
    return bot;
  }

  // ---------------------------------------------------------------- the store guides

  async fetchGuides() {
    const res = await invoke<any[]>('products_fetch_guides', { sources: GUIDE_SOURCES.map(({ url, store }) => ({ url, store })) });
    this.guides = await invoke<Guide[]>('products_guides');
    return { ok: res.filter((r) => !r.error).length, failed: res.filter((r) => r.error).map((r) => `${r.url}: ${r.error}`) };
  }

  private async project(name: string, description: string): Promise<T.Project> {
    const have = store.projects.find((p) => p.name === name);
    if (have) return have;
    const created = await api.post<T.Project>('/v1/projects', { name, color: '#7c3aed', icon: 'package', description });
    store.projects = [...store.projects, created];
    return created;
  }

  /** Puts the fetched guides in the vault ("Store Guides"), replacing an earlier copy. */
  async guidesToRag() {
    if (!this.guides.length) throw new Error('Fetch the guides first');
    const proj = await this.project('Store Guides', 'App Store and Google Play guides, fetched by Products');
    // Only the earlier copies of these guides go; anything else the owner put there stays.
    const names = new Set(this.guides.map((g) => g.path.split(/[\\/]/).pop()));
    const old = await api.get<T.Document[]>(`/v1/projects/${proj.id}/documents`).catch(() => [] as T.Document[]);
    for (const d of old) if (d.source_kind === 'products' || names.has(d.original_name)) await api.del(`/v1/documents/${d.id}`).catch(() => {});
    const res = await rad.uploadPaths(proj.id, this.guides.map((g) => g.path), 'products');
    const ok = res.filter((r) => !('error' in r)).length;
    store.toast('success', 'Store guides are in your vault', `${ok} guide${ok === 1 ? '' : 's'} in “${proj.name}”. Once indexed, Gilgamesh and every chat can use them.`);
  }

  // ---------------------------------------------------------------- what Gilgamesh knows

  /** The product's brief into the vault ("Products") and a one-line fact into RAD's memories;
   *  run again after changes (the old copy is replaced). */
  async addToRag(p: Product) {
    return this.task(p, 'Adding to RAG…', async () => {
      const proj = await this.project('Products', 'The owner’s products (SaaS platforms and apps), from Products');
      if (p.rag?.docs.length) for (const d of p.rag.docs) await api.del(`/v1/documents/${d}`).catch(() => {});
      const file = await invoke<string>('products_write_text', { id: p.id, name: `${p.name.replace(/[^\w.-]+/g, '-').slice(0, 60) || 'product'}.md`, content: this.brief(p) });
      const res = await rad.uploadPaths(proj.id, [file], 'products');
      const docs = res.filter((r): r is T.UploadResponse => !('error' in r)).map((r) => r.document.id);
      const failed = res.find((r) => 'error' in r) as { error: string } | undefined;
      if (failed && !docs.length) throw new Error(failed.error);
      const fact = clip(`The owner's product "${p.name}"${p.tagline ? `: ${p.tagline}` : ''}. ${[p.platforms.saas && 'SaaS platform', p.platforms.ios && 'iOS app', p.platforms.android && 'Android app'].filter(Boolean).join(', ')}${p.urls.website ? `; website ${p.urls.website}` : ''}. Full details in the vault's Products project and in RAD Products.`, 480);
      let memory = p.rag?.memory ?? null;
      if (memory) await api.patch(`/v1/memories/${memory}`, { text: fact }).catch(() => (memory = null));
      if (!memory) memory = (await api.post<T.Memory>('/v1/memories', { text: fact }).catch(() => null))?.id ?? null;
      p.rag = { project: proj.id, docs, added: Date.now(), memory };
      this.log(p, `Added to RAG (“${proj.name}”) and to Gilgamesh’s memories`, true);
      await this.save(p);
    });
  }

  async removeFromRag(p: Product) {
    if (!p.rag) return;
    for (const d of p.rag.docs) await api.del(`/v1/documents/${d}`).catch(() => {});
    if (p.rag.memory) await api.del(`/v1/memories/${p.rag.memory}`).catch(() => {});
    p.rag = null;
    await this.save(p);
  }

  /** For his answers: every product in a line; the one open (or talked about) in full. */
  describe(talking: boolean): string {
    if (!this.list.length) return '';
    const lines = this.list.map((p) => {
      const r = this.readiness(p).map((x) => `${x.platform} ${x.done}/${x.total}`).join(', ');
      return `- "${p.name}"${p.tagline ? `: ${clip(p.tagline, 80)}` : ''} (${r || 'no platforms'})${p.urls.website ? `, ${p.urls.website}` : ''}${this.busy[p.id] ? ` — now: ${this.busy[p.id]}` : ''}`;
    });
    let s = `THE OWNER'S PRODUCTS (Products, in the sidebar; readiness = store must-haves done):\n${lines.join('\n')}\nYou can work Products for them: [product: open | name], [product: draft | name | ios, android or both] writes the store listings from the description, [product: site | name] builds its website on gnu.au, [product: publish | name] publishes that website (only when the owner says to), [product: launch | name | ios or android | bot name] gives a bot the job of filling in App Store Connect or Google Play Console (it never submits for review), [product: rag | name] puts it in the vault and your memories.`;
    const p = this.current;
    if (p && (talking || store.route.kind === 'products')) {
      const c = this.checks(p);
      const todo = (Object.keys(c) as Platform[]).filter((k) => p.platforms[k]).flatMap((k) => c[k].filter((x) => !x.ok && x.level !== 'manual').map((x) => `${k}: ${x.label}${x.detail ? ` (${x.detail})` : ''}`));
      s += `\n\nTHE PRODUCT OPEN NOW ("${p.name}", ${this.tab} tab):\n${clip(this.brief(p), 7000)}\nSTILL TO DO: ${todo.length ? todo.join('; ') : 'nothing the stores require'}.\nRecent: ${p.log.slice(0, 5).map((l) => l.text).join(' · ') || 'nothing yet'}.\nHelp like an experienced product manager: sharpen the description and positioning, suggest App Store keywords and what to show in screenshots, explain store rules (cite the store guides in the vault when they're among your sources), and say plainly what's left before launch.`;
    }
    return s;
  }

  /** [product: …] from Gilgamesh. */
  async fromGilgamesh(arg: string): Promise<{ label: string; ok: boolean; later?: () => void }> {
    await this.load();
    const [cmd, ...rest] = arg.split('|').map((x) => x.trim());
    const c = cmd.toLowerCase();
    const p = this.find(rest[0]) ?? (rest[0] ? undefined : this.current ?? undefined);
    if (c === 'open') {
      if (!p) return { label: `No product “${rest[0] ?? ''}”`, ok: false };
      return { label: `Opening ${p.name}`, ok: true, later: () => { this.openId = p.id; store.navigate({ kind: 'products' }); } };
    }
    if (c === 'new' || c === 'create') {
      const name = rest[0] || 'New product';
      return { label: `New product: ${name}`, ok: true, later: () => void this.create(name).then(() => store.navigate({ kind: 'products' })) };
    }
    if (!p) return { label: rest[0] ? `No product “${rest[0]}”` : 'Which product?', ok: false };
    const report = (what: string, pr: Promise<unknown>) => {
      void pr.then(() => store.toast('success', p.name, `${what}: done`)).catch((e) => store.toast('error', `${p.name}: ${what}`, e?.message ?? String(e), 9000));
    };
    switch (c) {
      case 'draft': {
        const w = /ios|apple|iphone/i.test(rest[1] ?? '') ? 'ios' : /android|google|play/i.test(rest[1] ?? '') ? 'android' : 'both';
        report('Store listings drafted', this.draftListings(p, w));
        return { label: `Drafting the ${w === 'both' ? 'store listings' : w === 'ios' ? 'App Store listing' : 'Google Play listing'} of ${p.name}`, ok: true };
      }
      case 'site':
      case 'website':
        report('Website built on gnu.au', this.buildSite(p));
        return { label: `Building ${p.name}’s website on gnu.au`, ok: true };
      case 'publish':
        report('Website published', this.publishSite(p));
        return { label: `Publishing ${p.name}’s website`, ok: true };
      case 'rag':
      case 'remember':
        report('Added to RAG', this.addToRag(p));
        return { label: `Adding ${p.name} to the vault`, ok: true };
      case 'launch':
      case 'fill': {
        const platform = /android|google|play/i.test(rest[1] ?? '') ? 'android' : 'ios';
        await bots.load();
        const bot = (rest[2] && bots.bot(rest[2])) || bots.bots.find((b) => !bots.busy(b.id) && this.storeLogins(b.id, platform).length);
        if (!bot) return { label: `No free bot with a ${STORE_LOGIN_DOMAIN[platform]} login`, ok: false };
        try {
          await this.launch(p, platform, bot.id);
          return { label: `${bot.name} is filling in ${platform === 'ios' ? 'App Store Connect' : 'Google Play Console'} for ${p.name}`, ok: true };
        } catch (e: any) {
          return { label: e?.message ?? String(e), ok: false };
        }
      }
    }
    return { label: `Don’t know the product action “${cmd}”`, ok: false };
  }
}

export const products = new Products();
