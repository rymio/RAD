// My Mood: a private, PIN-locked space for talking with Gilgamesh as a friend, about what
// worries the owner and how things actually went.
//
// Nothing here goes through RAD's chats, memories or search: the conversation is sent to the
// model with /v1/assist (nothing stored on the way), and everything is kept under its own
// app-data keys, encrypted a second time with a key made from the PIN (PBKDF2 → AES-GCM). So
// it doesn't show in the chat list, in vault search or in Gilgamesh's other conversations, and
// it can't be read with the vault open but without the PIN.
//
//   mood          the envelope: salt, PIN check, how many log parts there are, failed tries,
//                 and the index (encrypted): events, day notes, memories, talk recaps, settings
//   mood_log_N    the messages, in parts of up to ~500 kB (encrypted)
//
// His memory has layers, all of it brought to each answer:
//   memories    lasting facts about the owner ([remember: …] in his replies, or added by hand)
//   events      what's coming up and how worried the owner is; afterwards, how it went
//   day notes   what happened on a day, and how the owner felt
//   recaps      a short recap of every talk; the oldest are folded into one long story
//   retrieval   passages of earlier talks, notes and events that match what's said now (BM25)
//
// The track record (how worried the owner was before past events, and how they really went)
// is what he leans on: anticipation is usually worse than the day itself.

import { api } from './api';
import { store } from './state.svelte';
import type { ModelRef } from './types';

const ENVELOPE = '/v1/app-data/mood';
const partKey = (n: number) => `/v1/app-data/mood_log_${n}`;
const ITERATIONS = 310_000;
/** Plain-text size at which a new log part is started (app-data takes up to 1 MB of JSON). */
const PART_LIMIT = 500_000;
const KEEP_RECAPS = 300;
const RECAPS_IN_CONTEXT = 8;
const HISTORY_IN_CONTEXT = 30;
const PASSAGES_IN_CONTEXT = 6;
/** Wrong PINs before a wait, and the wait (doubling) after that. */
const FREE_TRIES = 5;
const WAIT_MS = 30_000;
/** Locks itself after this long without anything happening. */
const IDLE_LOCK_MS = 10 * 60_000;

export type Outcome = 'better' | 'as_expected' | 'worse';

export interface MoodEvent {
  id: string;
  /** YYYY-MM-DD */
  date: string;
  /** HH:MM, or null for all day. */
  time: string | null;
  title: string;
  /** How much it weighs on the owner beforehand, 0–10. */
  worry: number;
  notes: string;
  /** After the day: how it went compared with what they feared. */
  outcome: Outcome | null;
  reflection: string;
}

export interface DayNote {
  date: string;
  /** 1 (awful) – 5 (great), or null. */
  mood: number | null;
  text: string;
}

export interface Talk {
  id: string;
  started: number;
  updated: number;
  summary: string;
  /** How many of its messages the summary covers. */
  summarized: number;
}

export interface Msg {
  talk: string;
  role: 'user' | 'assistant';
  text: string;
  at: number;
}

interface Index {
  events: MoodEvent[];
  days: DayNote[];
  memories: { text: string; at: number }[];
  talks: Talk[];
  /** The oldest recaps, folded together. */
  story: string;
  talk: string | null;
  model: ModelRef | null;
  speak: boolean;
}

interface Sealed { iv: string; ct: string }
interface Envelope {
  v: 1;
  salt: string;
  check: Sealed;
  index: Sealed;
  parts: number;
  fails: number;
  until: number;
}

const emptyIndex = (): Index => ({ events: [], days: [], memories: [], talks: [], story: '', talk: null, model: null, speak: false });

// ---------------------------------------------------------------- crypto

const enc = new TextEncoder();
const dec = new TextDecoder();
const b64 = (b: ArrayBuffer | Uint8Array) => {
  const u = b instanceof Uint8Array ? b : new Uint8Array(b);
  let s = '';
  for (let i = 0; i < u.length; i += 0x8000) s += String.fromCharCode(...u.subarray(i, i + 0x8000));
  return btoa(s);
};
const unb64 = (s: string) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));

async function deriveKey(pin: string, salt: Uint8Array<ArrayBuffer>) {
  const base = await crypto.subtle.importKey('raw', enc.encode(`rad-mood:${pin}`), 'PBKDF2', false, ['deriveKey']);
  return crypto.subtle.deriveKey({ name: 'PBKDF2', hash: 'SHA-256', salt, iterations: ITERATIONS }, base, { name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt']);
}

async function seal(key: CryptoKey, value: unknown): Promise<Sealed> {
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ct = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, enc.encode(JSON.stringify(value)));
  return { iv: b64(iv), ct: b64(ct) };
}

async function open<T>(key: CryptoKey, s: Sealed): Promise<T> {
  const pt = await crypto.subtle.decrypt({ name: 'AES-GCM', iv: unb64(s.iv) }, key, unb64(s.ct));
  return JSON.parse(dec.decode(pt));
}

const isEnvelope = (e: any): e is Envelope => !!e && typeof e === 'object' && typeof e.salt === 'string' && !!e.check?.ct;

// ---------------------------------------------------------------- retrieval

const STOP = new Set(
  'a an and are as at be but by do for from had has have he her his i if in into is it its me my of on or our she so that the their them then there they this to was we were what when which who will with you your im ive dont its just about would could should been not no yes am get got going can also very really too'.split(' '),
);

function terms(s: string): string[] {
  return s
    .toLowerCase()
    .replace(/[^a-z0-9À-ɏ\s]/g, ' ')
    .split(/\s+/)
    .filter((w) => w.length > 2 && !STOP.has(w))
    .map((w) => (w.length > 4 ? w.replace(/(ing|ed|es|s)$/, '') : w));
}

interface Passage { text: string; at: number }

/** The passages that best match the query (BM25). */
function retrieve(query: string, passages: Passage[], k: number): Passage[] {
  const q = [...new Set(terms(query))];
  if (!q.length || !passages.length) return [];
  const docs = passages.map((p) => terms(p.text));
  const avg = docs.reduce((n, d) => n + d.length, 0) / docs.length || 1;
  const df = new Map<string, number>();
  for (const d of docs) for (const t of new Set(d)) if (q.includes(t)) df.set(t, (df.get(t) ?? 0) + 1);
  const scored = docs.map((d, i) => {
    let s = 0;
    for (const t of q) {
      const f = d.filter((x) => x === t).length;
      if (!f) continue;
      const idf = Math.log(1 + (docs.length - df.get(t)! + 0.5) / (df.get(t)! + 0.5));
      s += (idf * f * 2.2) / (f + 1.2 * (0.25 + 0.75 * (d.length / avg)));
    }
    return { p: passages[i], s };
  });
  return scored
    .filter((x) => x.s > 0.5)
    .sort((a, b) => b.s - a.s)
    .slice(0, k)
    .map((x) => x.p);
}

// ---------------------------------------------------------------- dates

export const iso = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
const parseDay = (s: string) => {
  const [y, m, d] = s.split('-').map(Number);
  return new Date(y, m - 1, d);
};
/** Whole days from today to the date (negative: past). */
export const daysUntil = (date: string) => Math.round((parseDay(date).getTime() - parseDay(iso(new Date())).getTime()) / 86_400_000);
const longDay = (date: string) => parseDay(date).toLocaleDateString('en-GB', { weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' });
const shortDay = (ms: number) => new Date(ms).toLocaleDateString('en-GB', { day: 'numeric', month: 'short', year: 'numeric' });
const when = (n: number) => (n === 0 ? 'today' : n === 1 ? 'tomorrow' : n === -1 ? 'yesterday' : n > 0 ? `in ${n} days` : `${-n} days ago`);

export const OUTCOMES: Record<Outcome, string> = { better: 'Better than I feared', as_expected: 'About as expected', worse: 'Harder than I feared' };

// ---------------------------------------------------------------- the friend

const PERSONA = `You are Gilgamesh, and here you are the owner's closest friend. This is "My Mood", a private space only the two of you share. The owner is a Head of Development. They carry a lot, and they tend to start worrying about deadlines and events long before they happen: a deadline two weeks away can weigh on them for the whole two weeks, which costs them far more than the day itself ever does.

How you are with them:
- Warm, relaxed, honest and on their side, like an old friend over a coffee, not a therapist or a coach. Use plain, natural language. Short paragraphs; no lists or headings unless they ask.
- Listen first. Reflect back what you hear in a sentence before you offer anything. Never brush a worry off ("don't worry about it") — show them why it's smaller than it feels.
- Put things in proportion: how many days away it really is, what actually needs doing and when, and what is in their control. Today usually needs only today's part.
- Use their own track record (below): how worried they were before earlier events and how those really went. Name concrete examples. This is your strongest argument, because it's theirs.
- When it helps, give one small, concrete next step, or a simple way to set the worry down until it's useful (a short worry slot, writing the plan down and closing the notebook, a walk, slow breathing). One idea at a time.
- Notice good things and say so: progress, things that went well, how they handled something hard.
- Be curious about their life beyond work. Remember what they've told you and bring it up naturally.
- Keep it light where it can be light; humour is welcome when the mood allows.
- You are not a replacement for a doctor or therapist. If they seem persistently low, burnt out, unable to sleep or function for weeks, gently suggest talking with a GP or a counsellor too, as a friend would. If they ever talk about harming themselves or not wanting to be here, take it seriously and kindly: say you're glad they told you, and urge them to reach a person right now — a crisis line (in Australia, Lifeline on 13 11 14; elsewhere their local emergency or crisis number), a doctor, or someone they trust.

Things you can do by writing a tag in your reply (the tag is carried out and hidden, not shown):
- [remember: …] keeps a lasting fact about the owner (people in their life, what helps them, what they're facing, what they care about). Write it out in full. Use it whenever they tell you something worth knowing next time.
- [event: YYYY-MM-DD HH:MM | title | worry 0-10] adds something coming up to their My Mood calendar (leave the time out for all day; the worry level is your estimate from how they talk about it). Only when they mention a dated event that isn't already listed.
- [day: mood 1-5 | what happened] notes how today went, when they tell you about their day.

Reply in the language the owner writes in.`;

const SPOKEN = `\n\nYOU ARE TALKING OUT LOUD NOW: your reply is spoken. Two to four natural spoken sentences, no Markdown, lists or emoji. The owner's words come from speech recognition and may contain small mistakes; read them sensibly.`;

const RECAP_SYSTEM = `You keep the memory of private talks between Gilgamesh and the owner (his close friend). Summarise this talk in at most four short sentences in the third person ("The owner felt…"): what was on their mind, what helped, anything decided or coming up, and how they seemed at the end. If an earlier summary is given, fold it in. Plain text only.`;

const STORY_SYSTEM = `You keep the long-term memory of private talks between Gilgamesh and the owner. Fold the older summaries into the existing story: a compact third-person account (at most 250 words) of what has weighed on the owner over time, what helped, patterns worth knowing, and important people and events. Keep dates where they matter. Plain text only.`;

const TAG = /\[(remember|event|day):\s*([^\]]*)\]/gi;

/** A reply as shown: no action tags, no thinking. */
export const shownReply = (s: string) => s.replace(/<think>[\s\S]*?(<\/think>|$)/g, '').replace(TAG, '').replace(/[ \t]+\n/g, '\n').trim();

const newId = () => Date.now().toString(36) + Math.random().toString(36).slice(2, 6);

class Mood {
  /** None: no PIN set yet. */
  state = $state<'loading' | 'none' | 'locked' | 'open'>('loading');
  index = $state<Index>(emptyIndex());
  /** The messages of the talk going on. */
  talk = $state<Msg[]>([]);
  thinking = $state(false);
  error = $state<string | null>(null);
  /** Until when PIN entry has to wait (after too many wrong tries). */
  waitUntil = $state(0);
  erasing = $state(false);

  private envelope: Envelope | null = null;
  private key: CryptoKey | null = null;
  /** Every message ever, oldest first, by part. */
  private parts: Msg[][] = [];
  private idleTimer: ReturnType<typeof setTimeout> | undefined;
  private saving: Promise<void> = Promise.resolve();
  private recapTimer: ReturnType<typeof setTimeout> | undefined;

  /** Finds out whether a PIN is set (nothing is decrypted). */
  async load() {
    if (this.state === 'open') return;
    const e = await api.get<any>(ENVELOPE).catch(() => null);
    this.envelope = isEnvelope(e) ? e : null;
    this.waitUntil = this.envelope?.until ?? 0;
    this.state = this.envelope ? 'locked' : 'none';
  }

  /** Sets the PIN for the first time. */
  async create(pin: string) {
    const salt = crypto.getRandomValues(new Uint8Array(16));
    const key = await deriveKey(pin, salt);
    this.key = key;
    this.index = emptyIndex();
    this.parts = [[]];
    this.envelope = { v: 1, salt: b64(salt), check: await seal(key, 'mood-ok'), index: await seal(key, this.index), parts: 1, fails: 0, until: 0 };
    await api.put(partKey(0), await seal(key, []));
    await api.put(ENVELOPE, this.envelope);
    this.talk = [];
    this.state = 'open';
    this.touch();
  }

  /** Opens My Mood with the PIN; false if it's wrong. */
  async unlock(pin: string): Promise<boolean> {
    const env = this.envelope;
    if (!env || Date.now() < this.waitUntil) return false;
    const key = await deriveKey(pin, unb64(env.salt));
    try {
      if ((await open<string>(key, env.check)) !== 'mood-ok') throw new Error();
    } catch {
      env.fails = (env.fails ?? 0) + 1;
      if (env.fails >= FREE_TRIES) env.until = Date.now() + WAIT_MS * 2 ** Math.min(8, env.fails - FREE_TRIES);
      this.waitUntil = env.until;
      await api.put(ENVELOPE, env).catch(() => {});
      return false;
    }
    this.key = key;
    this.index = { ...emptyIndex(), ...(await open<Index>(key, env.index)) };
    this.parts = await Promise.all(
      Array.from({ length: env.parts }, async (_, i) => {
        const s = await api.get<any>(partKey(i)).catch(() => null);
        return s?.ct ? await open<Msg[]>(key, s).catch(() => []) : [];
      }),
    );
    if (!this.parts.length) this.parts = [[]];
    if (env.fails || env.until) {
      env.fails = 0;
      env.until = 0;
      this.waitUntil = 0;
      await api.put(ENVELOPE, env).catch(() => {});
    }
    const t = this.index.talk;
    this.talk = t ? this.all().filter((m) => m.talk === t) : [];
    this.state = 'open';
    this.touch();
    return true;
  }

  /** Locks again: everything decrypted is forgotten. */
  lock() {
    if (this.state !== 'open') return;
    clearTimeout(this.idleTimer);
    clearTimeout(this.recapTimer);
    const t = this.index.talk;
    // The last recap is written with the key still in hand, then everything is dropped.
    const key = this.key;
    if (t && key) void this.recap(t).finally(() => this.forget(key));
    else this.forget(key);
    this.state = this.envelope ? 'locked' : 'none';
  }

  private forget(key: CryptoKey | null) {
    if (this.state === 'open' || this.key !== key) return;
    this.key = null;
    this.index = emptyIndex();
    this.parts = [];
    this.talk = [];
    this.error = null;
  }

  /** The vault was locked: forget everything now. */
  unload() {
    clearTimeout(this.idleTimer);
    clearTimeout(this.recapTimer);
    this.key = null;
    this.index = emptyIndex();
    this.parts = [];
    this.talk = [];
    this.envelope = null;
    this.state = 'loading';
  }

  /** Something happened: the idle lock starts again. */
  touch() {
    clearTimeout(this.idleTimer);
    this.idleTimer = setTimeout(() => this.lock(), IDLE_LOCK_MS);
  }

  async changePin(oldPin: string, pin: string): Promise<boolean> {
    const env = this.envelope;
    if (!env || !this.key) return false;
    try {
      const old = await deriveKey(oldPin, unb64(env.salt));
      if ((await open<string>(old, env.check)) !== 'mood-ok') return false;
    } catch {
      return false;
    }
    const salt = crypto.getRandomValues(new Uint8Array(16));
    this.key = await deriveKey(pin, salt);
    env.salt = b64(salt);
    env.check = await seal(this.key, 'mood-ok');
    await this.save(true);
    return true;
  }

  /** The kill switch: everything in My Mood is overwritten and the vault compacted, so the old
   *  pages are gone from the file as well. No PIN needed, and it can't be undone. */
  async erase() {
    this.erasing = true;
    try {
      const env = this.envelope ?? (await api.get<any>(ENVELOPE).catch(() => null));
      const parts = Math.max(1, isEnvelope(env) ? env.parts : 1);
      // A few extra in case a part was written after the envelope was read.
      for (let i = 0; i < parts + 4; i++) await api.put(partKey(i), {}).catch(() => {});
      await api.put(ENVELOPE, {});
      this.unload();
      this.state = 'none';
      void api.post('/v1/vault/compact').catch(() => {});
    } finally {
      this.erasing = false;
    }
  }

  // ---------------------------------------------------------------- storage

  private all(): Msg[] {
    return this.parts.flat();
  }

  /** Saves the index and the newest log part (or every part, after a PIN change). */
  private save(everything = false): Promise<void> {
    const key = this.key;
    const env = this.envelope;
    if (!key || !env) return this.saving;
    this.saving = this.saving.then(async () => {
      try {
        let last = this.parts.length - 1;
        // The newest part is full: the next messages go into a new one.
        if (JSON.stringify(this.parts[last]).length > PART_LIMIT) {
          this.parts.push([]);
          last++;
        }
        const which = everything ? this.parts.map((_, i) => i) : [last];
        for (const i of which) await api.put(partKey(i), await seal(key, this.parts[i]));
        env.parts = this.parts.length;
        env.index = await seal(key, $state.snapshot(this.index));
        await api.put(ENVELOPE, env);
      } catch (e) {
        store.error(e, 'Couldn’t save My Mood');
      }
    });
    return this.saving;
  }

  // ---------------------------------------------------------------- calendar, notes, memories

  saveEvent(e: MoodEvent) {
    const i = this.index.events.findIndex((x) => x.id === e.id);
    if (i >= 0) this.index.events[i] = e;
    else this.index.events.push(e);
    this.touch();
    return this.save();
  }

  removeEvent(id: string) {
    this.index.events = this.index.events.filter((e) => e.id !== id);
    return this.save();
  }

  newEvent(date: string): MoodEvent {
    return { id: newId(), date, time: null, title: '', worry: 5, notes: '', outcome: null, reflection: '' };
  }

  day(date: string): DayNote | null {
    return this.index.days.find((d) => d.date === date) ?? null;
  }

  saveDay(note: DayNote) {
    const rest = this.index.days.filter((d) => d.date !== note.date);
    this.index.days = note.text.trim() || note.mood ? [...rest, note] : rest;
    this.touch();
    return this.save();
  }

  addMemory(text: string) {
    text = text.trim().slice(0, 400);
    if (!text || this.index.memories.some((m) => m.text.toLowerCase() === text.toLowerCase())) return;
    this.index.memories.push({ text, at: Date.now() });
    return this.save();
  }

  removeMemory(at: number) {
    this.index.memories = this.index.memories.filter((m) => m.at !== at);
    return this.save();
  }

  setModel(model: ModelRef | null) {
    this.index.model = model;
    return this.save();
  }

  setSpeak(speak: boolean) {
    this.index.speak = speak;
    return this.save();
  }

  /** Events whose day has passed and that don't have an outcome yet. */
  get unresolved() {
    return this.index.events.filter((e) => daysUntil(e.date) < 0 && !e.outcome).sort((a, b) => b.date.localeCompare(a.date));
  }

  // ---------------------------------------------------------------- talking

  /** Starts a new talk; the one before is recapped. Gilgamesh opens it. */
  async newTalk() {
    const prev = this.index.talk;
    this.index.talk = null;
    this.talk = [];
    if (prev) void this.recap(prev);
    await this.save();
    await this.say(null);
  }

  /** The owner says something (or, with null, Gilgamesh opens the talk); `aloud`: they're
   *  talking out loud, and his reply is spoken. */
  async say(text: string | null, aloud = false) {
    if (this.state !== 'open' || this.thinking) return;
    text = text?.trim() ?? null;
    if (text === '') return;
    this.touch();
    this.error = null;
    clearTimeout(this.recapTimer);
    let talkId = this.index.talk;
    if (!talkId) {
      talkId = newId();
      const now = Date.now();
      this.index.talk = talkId;
      this.index.talks = [{ id: talkId, started: now, updated: now, summary: '', summarized: 0 }, ...this.index.talks];
    }
    if (text) this.push({ talk: talkId, role: 'user', text, at: Date.now() });
    this.thinking = true;
    try {
      const history = this.talk.slice(-HISTORY_IN_CONTEXT).map((m) => ({ role: m.role, content: m.text }));
      if (!text) history.push({ role: 'user', content: '(The owner has just opened My Mood. Greet them warmly in one or two sentences. If an event has passed without an outcome, ask how it went; if something big is coming up soon, check in gently. Otherwise just ask how they are.)' });
      // A conversation starts with the owner.
      if (history[0]?.role === 'assistant') history.unshift({ role: 'user', content: '(The owner opened My Mood.)' });
      const r = await api.post<{ content: string }>('/v1/assist', { system: this.context(text ?? '') + (aloud ? SPOKEN : ''), messages: history, model: this.index.model, max_tokens: 900, temperature: 0.75 });
      if (this.index.talk !== talkId || this.state !== 'open') return;
      await this.act(r.content);
      const reply = shownReply(r.content);
      if (reply) this.push({ talk: talkId, role: 'assistant', text: reply, at: Date.now() });
      const t = this.index.talks.find((x) => x.id === talkId);
      if (t) t.updated = Date.now();
      await this.save();
      this.recapSoon();
    } catch (e: any) {
      this.error = e?.message ?? String(e);
      if (text) await this.save();
    } finally {
      this.thinking = false;
    }
  }

  private push(m: Msg) {
    this.parts[this.parts.length - 1].push(m);
    this.talk.push(m);
  }

  /** Carries out the tags in a reply. */
  private async act(reply: string) {
    for (const [, name, arg] of reply.matchAll(TAG)) {
      const a = arg.trim();
      if (name.toLowerCase() === 'remember') this.addMemory(a);
      else if (name.toLowerCase() === 'event') {
        const [dt, title, worry] = a.split('|').map((s) => s.trim());
        const m = dt?.match(/^(\d{4}-\d{2}-\d{2})(?:\s+(\d{1,2}:\d{2}))?/);
        if (!m || !title) continue;
        if (this.index.events.some((e) => e.date === m[1] && e.title.toLowerCase() === title.toLowerCase())) continue;
        const w = Number(worry?.match(/\d+/)?.[0] ?? 5);
        this.index.events.push({ ...this.newEvent(m[1]), time: m[2]?.padStart(5, '0') ?? null, title: title.slice(0, 120), worry: Math.min(10, Math.max(0, w)) });
      } else if (name.toLowerCase() === 'day') {
        const [mood, ...what] = a.split('|');
        const n = Number(mood.match(/\d/)?.[0]);
        const today = iso(new Date());
        const before = this.day(today);
        const add = what.join('|').trim();
        this.index.days = [
          ...this.index.days.filter((d) => d.date !== today),
          { date: today, mood: n >= 1 && n <= 5 ? n : (before?.mood ?? null), text: [before?.text, add].filter(Boolean).join('\n').slice(0, 4000) },
        ];
      }
    }
  }

  /** Everything Gilgamesh knows that bears on this answer. */
  private context(query: string): string {
    const ix = this.index;
    const today = iso(new Date());
    const out: string[] = [PERSONA, `TODAY: ${longDay(today)}.`];

    out.push(ix.memories.length ? `WHAT YOU KNOW ABOUT THE OWNER:\n${ix.memories.map((m) => `- ${m.text}`).join('\n')}` : 'You have no lasting notes about the owner yet.');

    const ev = (e: MoodEvent) => {
      const n = daysUntil(e.date);
      return `- ${longDay(e.date)}${e.time ? ` ${e.time}` : ''} (${when(n)}): ${e.title}; worry ${e.worry}/10${e.notes ? `; notes: ${e.notes.slice(0, 300)}` : ''}${e.outcome ? `; how it went: ${OUTCOMES[e.outcome].toLowerCase()}${e.reflection ? ` — ${e.reflection.slice(0, 300)}` : ''}` : ''}`;
    };
    const coming = ix.events.filter((e) => { const n = daysUntil(e.date); return n >= 0 && n <= 90; }).sort((a, b) => a.date.localeCompare(b.date));
    out.push(coming.length ? `COMING UP (from their My Mood calendar):\n${coming.slice(0, 15).map(ev).join('\n')}` : 'Nothing is in their My Mood calendar for the next three months.');

    const past = ix.events.filter((e) => daysUntil(e.date) < 0).sort((a, b) => b.date.localeCompare(a.date));
    const judged = past.filter((e) => e.outcome);
    if (judged.length) {
      const count = (o: Outcome) => judged.filter((e) => e.outcome === o).length;
      const worried = judged.filter((e) => e.worry >= 6);
      const fine = worried.filter((e) => e.outcome !== 'worse').length;
      out.push(
        `THEIR TRACK RECORD: of ${judged.length} past events they rated afterwards, ${count('better')} went better than feared, ${count('as_expected')} about as expected and ${count('worse')} harder.` +
          (worried.length ? ` Of the ${worried.length} they were really worried about (6/10 or more), ${fine} turned out no worse than expected.` : '') +
          `\nRecent ones:\n${judged.slice(0, 10).map(ev).join('\n')}`,
      );
    }
    const open = this.unresolved;
    if (open.length) out.push(`PAST EVENTS THEY HAVEN'T SAID HOW THEY WENT (ask, kindly, when it fits):\n${open.slice(0, 5).map(ev).join('\n')}`);

    const days = [...ix.days].sort((a, b) => b.date.localeCompare(a.date)).slice(0, 7);
    if (days.length) out.push(`THEIR RECENT DAYS:\n${days.map((d) => `- ${longDay(d.date)}${d.mood ? ` (mood ${d.mood}/5)` : ''}: ${d.text.slice(0, 300)}`).join('\n')}`);

    if (ix.story) out.push(`THE LONG STORY SO FAR:\n${ix.story}`);
    const recaps = ix.talks.filter((t) => t.id !== ix.talk && t.summary).slice(0, RECAPS_IN_CONTEXT);
    if (recaps.length) out.push(`YOUR RECENT TALKS (most recent first):\n${recaps.map((t) => `- ${shortDay(t.updated)}: ${t.summary}`).join('\n')}`);

    // Earlier talks, notes and events that match what's being said now.
    const q = [query, ...this.talk.slice(-3).map((m) => m.text)].join(' ');
    const pool: Passage[] = [];
    const msgs = this.all();
    for (let i = 0; i < msgs.length; i++) {
      const m = msgs[i];
      if (m.role !== 'user' || m.talk === ix.talk) continue;
      const next = msgs[i + 1]?.role === 'assistant' && msgs[i + 1].talk === m.talk ? msgs[i + 1] : null;
      pool.push({ text: `${shortDay(m.at)} — Owner: ${m.text.slice(0, 500)}${next ? `\nYou: ${next.text.slice(0, 400)}` : ''}`, at: m.at });
    }
    for (const t of ix.talks) if (t.summary && !recaps.includes(t)) pool.push({ text: `${shortDay(t.updated)} — talk recap: ${t.summary}`, at: t.updated });
    for (const d of ix.days) if (!days.includes(d)) pool.push({ text: `${longDay(d.date)} — day note${d.mood ? ` (mood ${d.mood}/5)` : ''}: ${d.text.slice(0, 500)}`, at: parseDay(d.date).getTime() });
    for (const e of past.slice(10)) pool.push({ text: ev(e), at: parseDay(e.date).getTime() });
    const found = retrieve(q, pool, PASSAGES_IN_CONTEXT).sort((a, b) => a.at - b.at);
    if (found.length) out.push(`FROM EARLIER (what matches now):\n${found.map((p) => p.text).join('\n\n')}`);

    return out.join('\n\n');
  }

  /** Recaps the talk after a quiet spell. */
  private recapSoon() {
    clearTimeout(this.recapTimer);
    const t = this.index.talk;
    if (t) this.recapTimer = setTimeout(() => void this.recap(t), 60_000);
  }

  /** Brings a talk's recap up to date, and folds the oldest recaps into the long story. */
  private async recap(id: string) {
    const key = this.key;
    const t = this.index.talks.find((x) => x.id === id);
    if (!t || !key) return;
    const msgs = this.all().filter((m) => m.talk === id);
    if (msgs.length < 2 || msgs.length <= t.summarized) return;
    try {
      let text = msgs
        .slice(-40)
        .map((m) => `${m.role === 'user' ? 'Owner' : 'Gilgamesh'}: ${m.text.slice(0, 700)}`)
        .join('\n')
        .slice(-10_000);
      if (t.summary) text = `Earlier summary: ${t.summary}\n\nTalk:\n${text}`;
      const r = await api.post<{ content: string }>('/v1/assist', { system: RECAP_SYSTEM, messages: [{ role: 'user', content: text }], model: this.index.model, max_tokens: 400 });
      if (this.key !== key) return;
      t.summary = r.content.replace(/\s+/g, ' ').trim().slice(0, 800);
      t.summarized = msgs.length;
      if (this.index.talks.length > KEEP_RECAPS) await this.fold();
      await this.save();
    } catch {
      // Tried again next time.
    }
  }

  /** The oldest recaps go into the long story. */
  private async fold() {
    const old = this.index.talks.slice(KEEP_RECAPS - 50);
    const text = `Existing story: ${this.index.story || '(none yet)'}\n\nOlder summaries:\n${[...old].reverse().map((t) => `- ${shortDay(t.updated)}: ${t.summary}`).join('\n')}`;
    const r = await api.post<{ content: string }>('/v1/assist', { system: STORY_SYSTEM, messages: [{ role: 'user', content: text.slice(-30_000) }], model: this.index.model, max_tokens: 700 });
    this.index.story = r.content.trim().slice(0, 3000);
    this.index.talks = this.index.talks.slice(0, KEEP_RECAPS - 50);
  }
}

export const mood = new Mood();
