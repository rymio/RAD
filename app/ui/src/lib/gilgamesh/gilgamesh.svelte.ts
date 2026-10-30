// Gilgamesh: a spoken conversation with RAD, face to face.
//
//   microphone → speech to text (stt.rs) → RAD chat → sentences → text to speech (tts.rs)
//   → speaker + the talking head's lips, mood and gestures
//
// The RAD chat brings the vault (retrieval, with sources), long-term memories, excerpts of
// earlier conversations and the action planner (SSH, web pages, terminals). Gilgamesh adds his
// own memory: every conversation is kept and can be resumed, and when one ends he writes himself
// a short recap, which later conversations start from. His answers carry tags for his face and
// for actions ([remember: …], [note: …], [event: …], [open: …], [search: …], [music: …]).
//
// He can also just listen and keep a transcript (transcripts.svelte.ts); a conversation can be
// about one of them, which then goes into his context.
//
// Gilgamesh goes everywhere in RAD: his face sits in the sidebar, the microphone stays on from
// view to view, and he sees which part of RAD is open (observe.svelte.ts).
//
// The answer is spoken sentence by sentence while it is still being written. Talking over
// Gilgamesh interrupts him (with headphones, or while holding the push-to-talk key); through
// the speakers the microphone is ignored while he speaks, so he doesn't hear himself. Actions
// that leave this view (opening the browser, music…) wait until he has finished speaking.

import { invoke, Channel } from '@tauri-apps/api/core';
import { api, rad } from '../api';
import { store } from '../state.svelte';
import { speakable } from '../speech.svelte';
import type { AgentAction, Chat, ChatStreamEvent, ChatWithMessages, Citation, Message } from '../types';
import { analyse, decodeWav, type Track } from './lipsync';
import { Heads } from './head';
import { activity } from './observe.svelte';
import { citedNumbers, parseTags, stripCitations, stripTags, takeSentences, GESTURES, MOODS, type Gesture, type Mood } from './text';
import { runAction, situation } from './tools';
import { thumbnail, type Avatar } from './face';
import { transcripts } from './transcripts.svelte';
import { monitor } from '../monitor.svelte';
import { bots } from '../bots.svelte';

export type Mode = 'hands_free' | 'push_to_talk';

/** One conversation with Gilgamesh (a RAD chat) and his own recap of it. */
export interface Conversation {
  id: number;
  title: string;
  started: number;
  updated: number;
  summary: string;
  /** How many messages the recap covers. */
  summarized: number;
  /** The face Gilgamesh wore in it. */
  face?: string;
  /** The transcript it is about (its note's id). */
  transcript?: number;
}

/** One of Gilgamesh's faces: a prepared photo, kept under its own key in the vault. */
export interface Face {
  id: string;
  name: string;
  /** Small JPEG data URL, for picking between faces. */
  thumb: string;
  added: number;
}

export interface Settings {
  mode: Mode;
  /** With headphones the microphone stays on while Gilgamesh talks, so you can interrupt him. */
  headphones: boolean;
  /** Gilgamesh's voice (a MioTTS voice key); empty: a male preset. */
  voice: string;
  /** The conversation going on now. */
  chatId: number | null;
  conversations: Conversation[];
  faces: Face[];
  /** The face he wears now. */
  face: string | null;
  /** A new conversation brings the next face. */
  rotate: boolean;
  /** His face shows in the sidebar (else only the controls). */
  dock: boolean;
}
export type Phase = 'off' | 'starting' | 'listening' | 'hearing' | 'thinking' | 'speaking';
export interface Turn {
  who: 'you' | 'gilgamesh';
  text: string;
  sources?: Citation[];
  actions?: { label: string; ok: boolean }[];
}

const SETTINGS_KEY = '/v1/app-data/gilgamesh';
/** The single face kept before Gilgamesh had several (moved into the list on load). */
const AVATAR_KEY = '/v1/app-data/gilgamesh_avatar';
const faceKey = (id: string) => `/v1/app-data/gilgamesh_face_${id}`;
export const MAX_FACES = 24;
const KEEP_CONVERSATIONS = 60;
const RECAPS_IN_CONTEXT = 6;

const tags = (names: readonly string[]) => names.map((n) => `[${n}]`).join(' ');

const PERSONA = `YOU ARE SPEAKING ALOUD. You are Gilgamesh, the voice and face of RAD, talking with its owner face to face. Your words are spoken by a talking head, so:
- answer in one to three short, natural spoken sentences, unless asked for more detail;
- no Markdown, lists, tables, code, links or emoji. You may cite sources like [1]; citations are shown, not read aloud;
- begin your reply with one mood tag that fits what you say: ${tags(MOODS)}. Put another one before a later sentence only if your mood changes;
- you may add a gesture where it fits naturally, but not in every sentence: ${tags(GESTURES)}.

You remember the owner: the facts they asked you to keep (in your instructions), your own recaps of earlier conversations (below), and excerpts of earlier conversations among the sources.

You can act for the owner by writing an action tag in your reply. It is carried out, not read aloud:
- [remember: …] keeps a lasting fact about the owner (their name, people, preferences, plans), written out in full. Use it when they tell you something worth knowing next time, or ask you to remember something.
- [note: title | text] saves a note in their vault.
- [event: YYYY-MM-DD HH:MM | title] adds an event to their calendar (24-hour time; leave it out for an all-day event). Take the date from DATES below.
- [open: name] opens part of RAD (browser, terminal, files, calendar, email, music, video, settings, memories), or [open: https://…] opens a web page.
- [search: words] searches the web in RAD's browser.
- [music: play | pause | next | previous | song or artist] controls their music.
- [check: websites] checks the websites the owner monitors again now; you tell them the result once it's done (say you're checking).
- [open: monitoring] opens Web Monitoring.
- [bot: Name | task] gives one of the owner's bots a task (YOUR BOTS, below). A bot works in the background on a computer of its own: it browses, fills in forms, logs in with the owner's saved logins, downloads and checks things, and can follow the skills it has learned. Write the task out in full. Leave "Name |" out to give it to whichever bot is free. Say you've passed it on; you'll be told when it's done.
- [bot: approve] or [bot: deny] answers a bot that waits for the owner's OK, only when the owner has said yes or no to it. [bot: answer | text] passes on the owner's answer to a bot's question. [bot: stop | Name] stops a bot.
Only act when the owner asks or clearly wants it, and always say a few words about what you're doing. You go with the owner everywhere in RAD (your face is in the sidebar), so opening something or searching doesn't end the conversation; it happens once you've finished speaking.

You can also listen without answering and write down what is said (the owner starts it with the Transcripts button). Those transcripts are notes in the owner's vault: passages from them appear among the sources, and the recent ones are listed below.

You watch the owner's websites (Web Monitoring, below): when RAD starts and every three hours you check whether each answers and when its certificate expires, and tell the owner about problems. When asked how the websites are, answer from those results: name what is offline or has a certificate problem or expiring soon, and say briefly that the rest are fine.

You can see which part of RAD the owner has open and where they were before (WHAT THE OWNER IS DOING, below). Use it to help with what they're doing ("this page", "this email", "what I'm looking at"), but don't comment on it unless it helps.

Examples:
Owner: My sister is called Kate. → [happy] Kate, lovely name. I'll remember that. [remember: The owner's sister is called Kate.]
Owner: Put the dentist in for Friday at half past two. → [nod] Done, the dentist is in your calendar for Friday at half past two. [event: {FRIDAY} 14:30 | Dentist]
Owner: Open my music. → [happy] Sure, opening your music now, so I'll be quiet. [open: music]
Owner: Write down that the boiler code is 4471. → [nod] Noted. [note: Boiler code | The boiler code is 4471.]
Owner: Get Ada to find the cheapest flight to Perth next Friday. → [nod] I've asked Ada to look; I'll tell you what it finds. [bot: Ada | Find the cheapest flight from here to Perth next Friday and list the three best options with times and prices.]
The owner's words come from speech recognition and may contain small mistakes; read them sensibly.`;

const RECAP_SYSTEM = `You keep Gilgamesh's memory of his spoken conversations with his owner. Summarise the conversation in at most three short sentences, in the third person ("The owner asked…"): what was talked about, anything decided, promised or still open, and anything personal worth recalling. If an earlier recap is given, fold it in. Plain text only.`;

/** A message as shown in the transcript. */
const shown = (s: string) => stripCitations(stripTags(s.replace(/<think>[\s\S]*?(<\/think>|$)/g, ''))).trim();

/** The sources an answer actually cites, once each. */
function citedSources(text: string, citations: Citation[]): Citation[] {
  const n = citedNumbers(text);
  const seen = new Set<string>();
  return citations.filter((c) => {
    const key = c.chat_id ? `c${c.chat_id}` : `d${c.document_id}`;
    if (!n.has(c.n) || seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

function serverActionLabel(a: AgentAction) {
  if (a.kind === 'ssh') return `Opening SSH to ${a.user ? a.user + '@' : ''}${a.host}`;
  if (a.kind === 'open_url') return `Opening ${a.url}`;
  return 'Opening a terminal';
}

/** The date of the coming Friday, for the calendar example in the instructions. */
function nextFriday() {
  const d = new Date();
  d.setDate(d.getDate() + ((12 - d.getDay()) % 7 || 7));
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

const isAvatar = (a: any): a is Avatar => !!a && typeof a === 'object' && typeof a.image === 'string' && Array.isArray(a.points);

const dayLabel = (ms: number) => new Date(ms).toLocaleDateString('en-GB', { weekday: 'short', day: 'numeric', month: 'short' });

interface Line { text: string; mood: Mood | null; gestures: Gesture[]; wav: Promise<ArrayBuffer | null> }

class Gilgamesh {
  phase = $state<Phase>('off');
  /** What the microphone is hearing right now (live caption). */
  heard = $state('');
  /** The sentence Gilgamesh is saying. */
  saying = $state('');
  /** What RAD is doing before the answer starts ("Searching your vault…"). */
  status = $state('');
  level = $state(0);
  turns = $state<Turn[]>([]);
  error = $state<string | null>(null);
  settings = $state<Settings>({ mode: 'hands_free', headphones: false, voice: '', chatId: null, conversations: [], faces: [], face: null, rotate: false, dock: true });
  avatar = $state<Avatar | null>(null);
  loaded = $state(false);
  holding = $state(false);
  /** The transcript the conversation going on is about. */
  about = $state<{ id: number; title: string } | null>(null);

  /** Every picture of him on screen. */
  head = new Heads();
  private run = 0;
  private lines: Line[] = [];
  private wake: (() => void) | null = null;
  private streamDone = true;
  private streaming = false;
  private lastSynth: Promise<unknown> = Promise.resolve();
  private audio: HTMLAudioElement | null = null;
  private micOn = false;
  private micPaused = false;
  /** Actions to carry out once the answer has been spoken. */
  private later: (() => void)[] = [];
  private lastAck = 0;
  private recapTimer: ReturnType<typeof setTimeout> | undefined;
  private recapping: Promise<void> = Promise.resolve();

  private loading: Promise<void> | null = null;

  /** Loads his settings, faces and the conversation going on (once, however many views ask). */
  load() {
    if (this.loaded) return Promise.resolve();
    this.loading ??= this.loadNow().finally(() => (this.loading = null));
    return this.loading;
  }

  private async loadNow() {
    const s = await api.get<any>(SETTINGS_KEY).catch(() => null);
    if (s && typeof s === 'object') {
      this.settings = { ...this.settings, ...s, conversations: Array.isArray(s.conversations) ? s.conversations : [], faces: Array.isArray(s.faces) ? s.faces : [] };
    }
    // The one face from before Gilgamesh had several.
    if (!this.settings.faces.length) {
      const a = await api.get<any>(AVATAR_KEY).catch(() => null);
      if (isAvatar(a)) {
        await this.addFaces([{ avatar: a, name: 'Face' }]);
        await api.put(AVATAR_KEY, {}).catch(() => {});
      }
    }
    const face = this.settings.faces.find((f) => f.id === this.settings.face) ?? this.settings.faces[0];
    if (face) await this.useFace(face.id).catch(() => {});
    // A conversation from before Gilgamesh kept a list of them.
    const id = this.settings.chatId;
    if (id && !this.settings.conversations.some((c) => c.id === id)) {
      this.settings.conversations = [{ id, title: 'Gilgamesh', started: Date.now(), updated: Date.now(), summary: '', summarized: 0 }, ...this.settings.conversations];
    }
    await transcripts.load().catch(() => {});
    this.loaded = true;
    this.about = this.aboutOf(id);
    if (id) await this.loadTurns(id);
  }

  /** Forgets everything loaded (the vault was locked); the microphone goes off. */
  unload() {
    this.stop();
    transcripts.unload();
    this.about = null;
    this.loaded = false;
    this.avatar = null;
    this.turns = [];
    this.faceCache.clear();
    this.loading = null;
    this.settings = { mode: 'hands_free', headphones: false, voice: '', chatId: null, conversations: [], faces: [], face: null, rotate: false, dock: true };
  }

  saveSettings(patch: Partial<Settings>) {
    this.settings = { ...this.settings, ...patch };
    return api.put(SETTINGS_KEY, $state.snapshot(this.settings)).catch((e) => store.error(e, 'Couldn’t save Gilgamesh’s settings'));
  }

  // ---------------------------------------------------------------- faces

  private faceCache = new Map<string, Avatar>();

  private async faceAvatar(id: string): Promise<Avatar> {
    const cached = this.faceCache.get(id);
    if (cached) return cached;
    const a = await api.get<any>(faceKey(id));
    if (!isAvatar(a)) throw new Error('That face is missing from the vault');
    this.faceCache.set(id, a);
    return a;
  }

  /** Adds prepared photos to Gilgamesh's faces and puts the last one on. */
  async addFaces(list: { avatar: Avatar; name: string }[]) {
    let last: string | null = null;
    for (const { avatar, name } of list) {
      if (this.settings.faces.length >= MAX_FACES) throw new Error(`Gilgamesh can keep up to ${MAX_FACES} faces; remove one first`);
      const id = Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
      await api.put(faceKey(id), avatar);
      this.faceCache.set(id, avatar);
      const face: Face = { id, name: name.slice(0, 60) || 'Face', thumb: await thumbnail(avatar).catch(() => ''), added: Date.now() };
      await this.saveSettings({ faces: [...this.settings.faces, face] });
      last = id;
    }
    if (last) await this.useFace(last);
  }

  /** Puts a face on (the conversation going on now remembers it). */
  async useFace(id: string) {
    const a = await this.faceAvatar(id);
    this.avatar = a;
    const c = this.conversation(this.settings.chatId);
    const changed = this.settings.face !== id || (c && c.face !== id);
    if (c) c.face = id;
    if (changed) await this.saveSettings({ face: id });
  }

  async removeFace(id: string) {
    const faces = this.settings.faces.filter((f) => f.id !== id);
    await api.put(faceKey(id), {});
    this.faceCache.delete(id);
    const wasOn = this.settings.face === id;
    await this.saveSettings({ faces, face: wasOn ? null : this.settings.face });
    if (!wasOn) return;
    if (faces.length) await this.useFace(faces[0].id);
    else {
      this.stop();
      this.avatar = null;
    }
  }

  renameFace(id: string, name: string) {
    name = name.trim().slice(0, 60);
    if (!name) return;
    return this.saveSettings({ faces: this.settings.faces.map((f) => (f.id === id ? { ...f, name } : f)) });
  }

  /** The face after the one on now, round the list. */
  private nextFace() {
    const f = this.settings.faces;
    const i = f.findIndex((x) => x.id === this.settings.face);
    return f[(i + 1) % f.length]?.id ?? null;
  }

  // ---------------------------------------------------------------- conversations

  /** The conversations, most recent first. */
  get conversations() {
    return [...this.settings.conversations].sort((a, b) => b.updated - a.updated);
  }

  private conversation(id: number | null) {
    return this.settings.conversations.find((c) => c.id === id) ?? null;
  }

  private aboutOf(id: number | null) {
    const t = transcripts.find(this.conversation(id)?.transcript);
    return t ? { id: t.id, title: t.title } : null;
  }

  /** Starts a new conversation about a transcript. */
  async discuss(id: number) {
    const t = transcripts.find(id);
    if (!t) return;
    await this.newConversation();
    this.about = { id: t.id, title: t.title };
  }

  /** Shows a conversation's messages in the transcript. */
  private async loadTurns(id: number) {
    try {
      const r = await api.get<ChatWithMessages>(`/v1/chats/${id}`);
      if (this.settings.chatId !== id) return;
      this.turns = r.messages
        .filter((m) => m.role === 'user' || m.role === 'assistant')
        .map((m: Message): Turn => (m.role === 'user' ? { who: 'you', text: m.content } : { who: 'gilgamesh', text: shown(m.content), sources: citedSources(m.content, m.citations) }));
    } catch {
      // The chat was deleted elsewhere: forget it.
      await this.saveSettings({ chatId: null, conversations: this.settings.conversations.filter((c) => c.id !== id) });
      this.turns = [];
    }
  }

  /** Starts a new conversation; the one before is kept (and recapped). */
  async newConversation() {
    this.interrupt();
    const prev = this.settings.chatId;
    this.turns = [];
    this.about = null;
    await this.saveSettings({ chatId: null });
    if (prev) void this.recap(prev);
    const next = this.settings.rotate && this.settings.faces.length > 1 ? this.nextFace() : null;
    if (next) await this.useFace(next).catch((e) => store.error(e, 'Couldn’t change Gilgamesh’s face'));
  }

  /** Continues an earlier conversation. */
  async resume(id: number) {
    if (id === this.settings.chatId) return;
    this.interrupt();
    const prev = this.settings.chatId;
    this.turns = [];
    await this.saveSettings({ chatId: id });
    this.about = this.aboutOf(id);
    // He wears the face he had in that conversation, if it's still there.
    const face = this.conversation(id)?.face;
    if (face && face !== this.settings.face && this.settings.faces.some((f) => f.id === face)) await this.useFace(face).catch(() => {});
    await this.loadTurns(id);
    if (prev) void this.recap(prev);
  }

  /** Deletes a conversation (the chat and Gilgamesh's recap of it). */
  async forget(id: number) {
    if (id === this.settings.chatId) {
      this.interrupt();
      this.turns = [];
      this.about = null;
    }
    await api.del(`/v1/chats/${id}`).catch(() => {});
    await this.saveSettings({ chatId: this.settings.chatId === id ? null : this.settings.chatId, conversations: this.settings.conversations.filter((c) => c.id !== id) });
    store.reloadChats();
  }

  private async chatId(firstWords: string): Promise<number> {
    const id = this.settings.chatId;
    if (id && this.conversation(id)) return id;
    const title = this.about ? `Gilgamesh: about ${this.about.title.slice(0, 44)}` : `Gilgamesh: ${firstWords.replace(/\s+/g, ' ').slice(0, 50)}`;
    const c = await api.post<Chat>('/v1/chats', { title });
    store.reloadChats();
    const now = Date.now();
    const list = [{ id: c.id, title, started: now, updated: now, summary: '', summarized: 0, face: this.settings.face ?? undefined, transcript: this.about?.id }, ...this.settings.conversations].slice(0, KEEP_CONVERSATIONS);
    await this.saveSettings({ chatId: c.id, conversations: list });
    return c.id;
  }

  /** Gilgamesh's recaps of earlier conversations, for the next answer. */
  private recaps(): string {
    const lines = this.conversations
      .filter((c) => c.id !== this.settings.chatId && c.summary)
      .slice(0, RECAPS_IN_CONTEXT)
      .map((c) => `- ${dayLabel(c.updated)}: ${c.summary}`);
    return lines.length ? `YOUR RECAPS OF EARLIER CONVERSATIONS WITH THE OWNER (most recent first):\n${lines.join('\n')}` : 'You have no recaps of earlier conversations yet.';
  }

  /** Brings Gilgamesh's recap of a conversation up to date (one at a time). */
  recap(id: number): Promise<void> {
    this.recapping = this.recapping.then(async () => {
      const c = this.conversation(id);
      if (!c) return;
      try {
        const r = await api.get<ChatWithMessages>(`/v1/chats/${id}`);
        const msgs = r.messages.filter((m) => m.role === 'user' || m.role === 'assistant');
        if (msgs.length <= c.summarized || msgs.length < 2) return;
        let text = msgs
          .slice(-40)
          .map((m) => `${m.role === 'user' ? 'Owner' : 'Gilgamesh'}: ${(m.role === 'user' ? m.content : shown(m.content)).slice(0, 600)}`)
          .join('\n')
          .slice(-8000);
        if (c.summary) text = `Earlier recap: ${c.summary}\n\nConversation:\n${text}`;
        const out = await rad.assist(RECAP_SYSTEM, [{ role: 'user', content: text }]);
        const fresh = this.conversation(id);
        if (!fresh) return;
        fresh.summary = out.content.replace(/\s+/g, ' ').trim().slice(0, 700);
        fresh.summarized = msgs.length;
        await this.saveSettings({});
      } catch {
        // Tried again next time.
      }
    });
    return this.recapping;
  }

  /** Recaps the conversation once it has been quiet for a while. */
  private recapSoon() {
    clearTimeout(this.recapTimer);
    const id = this.settings.chatId;
    if (!id) return;
    this.recapTimer = setTimeout(() => {
      if (this.phase === 'thinking' || this.phase === 'speaking' || this.phase === 'hearing') return this.recapSoon();
      void this.recap(id);
    }, 45_000);
  }

  // ---------------------------------------------------------------- listening

  async start() {
    if (this.phase !== 'off') return;
    // One microphone: a transcript being made stops (and is saved).
    if (transcripts.recording) await transcripts.stop();
    this.error = null;
    this.phase = 'starting';
    const ch = new Channel<any>();
    ch.onmessage = (ev) => this.onStt(ev);
    const ptt = this.settings.mode === 'push_to_talk';
    try {
      await invoke('stt_listen', { onEvent: ch, paused: ptt });
      this.micOn = true;
      this.micPaused = ptt;
      this.phase = 'listening';
      if (this.head) this.head.attention = 'listening';
    } catch (e: any) {
      this.phase = 'off';
      this.error = e?.message ?? String(e);
    }
  }

  stop() {
    this.interrupt();
    if (this.micOn) invoke('stt_stop').catch(() => {});
    this.micOn = false;
    this.phase = 'off';
    this.heard = '';
    this.level = 0;
    this.holding = false;
    if (this.head) this.head.attention = 'idle';
    clearTimeout(this.recapTimer);
    if (this.settings.chatId) void this.recap(this.settings.chatId);
  }

  private pauseMic(paused: boolean) {
    if (this.micPaused === paused || !this.micOn) return;
    this.micPaused = paused;
    invoke('stt_pause', { paused }).catch(() => {});
  }

  /** The microphone should hear the user now: always in hands-free mode, or while the key is held. */
  private wantMic() {
    return this.settings.mode === 'hands_free' ? true : this.holding;
  }

  /** Where to go once an answer is over: listening if the microphone is on, else off. */
  private get idlePhase(): Phase {
    return this.micOn ? 'listening' : 'off';
  }

  private onStt(ev: any) {
    switch (ev.type) {
      case 'level':
        this.level = ev.rms;
        break;
      case 'speech_start':
        // Talking over Gilgamesh stops him.
        if (this.phase === 'speaking' || this.phase === 'thinking') this.interrupt();
        this.phase = 'hearing';
        this.heard = '';
        if (this.head) {
          this.head.attention = 'listening';
          this.head.mood = 'neutral';
        }
        break;
      case 'partial':
        this.heard = ev.text;
        // Now and then a small nod: he's following.
        if (this.head && Date.now() - this.lastAck > 2500 && Math.random() < 0.5) {
          this.lastAck = Date.now();
          this.head.acknowledge();
        }
        break;
      case 'final':
        this.heard = '';
        this.head?.acknowledge();
        this.ask(ev.text);
        break;
      case 'cancelled':
        if (this.phase === 'hearing') {
          this.phase = this.idlePhase;
          this.heard = '';
        }
        break;
      case 'error':
        this.error = ev.message;
        this.stop();
        break;
    }
  }

  /** Push to talk: the key went down / up. */
  press(down: boolean) {
    if (this.settings.mode !== 'push_to_talk' || this.phase === 'off' || this.holding === down) return;
    this.holding = down;
    if (down) {
      if (this.phase === 'speaking' || this.phase === 'thinking') this.interrupt();
      this.pauseMic(false);
    } else {
      invoke('stt_finish').catch(() => {});
      this.pauseMic(true);
    }
  }

  // ---------------------------------------------------------------- answering

  /** Sends what the user said (or typed) and speaks the answer. */
  async ask(text: string) {
    text = text.trim();
    if (!text) return;
    this.interrupt();
    clearTimeout(this.recapTimer);
    const run = ++this.run;
    this.error = null;
    this.status = '';
    this.later = [];
    this.turns.push({ who: 'you', text });
    this.phase = 'thinking';
    if (this.head) {
      this.head.attention = 'thinking';
      this.head.blink();
    }
    this.streamDone = false;
    this.lines = [];
    // The reply's place in the transcript, once it starts (the stored item is a reactive copy).
    let replyAt = -1;
    const reply = () => {
      if (replyAt < 0) replyAt = this.turns.push({ who: 'gilgamesh', text: '', actions: [] }) - 1;
      return this.turns[replyAt];
    };
    let full = '';
    let buf = '';
    let citations: Citation[] = [];
    // Mood and gestures carry on to the next spoken sentence (a tag may stand on its own).
    let mood: Mood | null = null;
    let gestures: Gesture[] = [];
    // Actions run as soon as they're written; their outcomes are awaited before the turn ends.
    const pending: Promise<void>[] = [];
    const act = (name: Parameters<typeof runAction>[0], arg: string) => {
      pending.push(
        runAction(name, arg).then((o) => {
          if (run !== this.run) return;
          reply().actions!.push({ label: o.label, ok: o.ok });
          if (o.later) this.later.push(o.later);
        }),
      );
    };
    const push = (final: boolean) => {
      const r = takeSentences(buf, final);
      buf = r.rest;
      for (const s of r.sentences) {
        const { text: plain, tags } = parseTags(s);
        for (const t of tags) {
          if (t.kind === 'mood') mood = t.name;
          else if (t.kind === 'gesture') gestures.push(t.name);
          else act(t.name, t.arg);
        }
        const spoken = speakable(plain);
        if (spoken) {
          this.say(spoken, mood, gestures);
          gestures = [];
        }
      }
      if (final && this.head && (mood || gestures.length)) {
        if (mood) this.head.mood = mood;
        for (const g of gestures) this.head.gesture(g);
      }
    };
    this.actionsDone = Promise.resolve();
    void this.player(run);
    let chatId: number;
    let context: string;
    let topic: string;
    try {
      [chatId, context, topic] = await Promise.all([this.chatId(text), situation().catch(() => ''), this.about ? transcripts.context(this.about.id).catch(() => '') : '']);
    } catch (e: any) {
      this.fail(run, e);
      return;
    }
    const made = [transcripts.describe(), monitor.describe(), bots.describe()].filter(Boolean).join('\n\n');
    // The transcript under discussion goes last, so a long one is what gets cut at the limit.
    context = `${PERSONA.replace('{FRIDAY}', nextFriday())}\n\n${context}\n${activity.describe()}\n\n${this.recaps()}${made ? `\n\n${made}` : ''}${topic ? `\n\n${topic}` : ''}`;
    this.streaming = true;
    try {
      await rad.chatSend(chatId, { content: text, context, attachments: [], model: null }, (ev: ChatStreamEvent) => {
        if (run !== this.run) return;
        switch (ev.type) {
          case 'status':
            this.status = ev.text;
            break;
          case 'sources':
            citations = ev.citations;
            break;
          case 'delta':
            this.status = '';
            full += ev.text;
            buf += ev.text;
            reply().text = shown(full);
            push(false);
            break;
          case 'action': {
            // The action planner (SSH, web pages, terminals): carried out once he has spoken.
            const a = ev.action;
            reply().actions!.push({ label: serverActionLabel(a), ok: true });
            this.later.push(() => store.runAction(a));
            break;
          }
          case 'error':
            this.error = ev.message;
            break;
        }
      });
      if (run === this.run) {
        push(true);
        if (replyAt >= 0) this.turns[replyAt].sources = citedSources(full, citations);
      }
    } catch (e: any) {
      if (run === this.run) this.error = e?.message ?? String(e);
    } finally {
      this.streaming = false;
      this.status = '';
      this.actionsDone = Promise.all(pending).then(() => {});
      if (run === this.run) {
        this.streamDone = true;
        this.wake?.();
        const c = this.conversation(chatId!);
        if (c) {
          c.updated = Date.now();
          void this.saveSettings({});
        }
        this.recapSoon();
      }
      store.reloadChats();
    }
  }

  /** The current answer's actions, carried out (or failed). */
  private actionsDone: Promise<void> = Promise.resolve();

  private fail(run: number, e: any) {
    if (run !== this.run) return;
    this.error = e?.message ?? String(e);
    this.streamDone = true;
    this.wake?.();
  }

  /** Queues a sentence; synthesis runs one at a time, ahead of playback. */
  private say(text: string, mood: Mood | null, gestures: Gesture[]) {
    const cfg = store.config;
    const run = this.run;
    const wav = this.lastSynth.then(() =>
      run !== this.run
        ? null
        : invoke<ArrayBuffer>('tts_speak', {
            text,
            voice: this.settings.voice || 'en_male',
            temperature: cfg?.tts_temperature,
            topP: cfg?.tts_top_p,
            topK: cfg?.tts_top_k,
            repetitionPenalty: cfg?.tts_repetition_penalty,
          }),
    );
    this.lastSynth = wav.catch(() => null);
    this.lines.push({ text, mood, gestures, wav });
    this.wake?.();
  }

  /** Plays the queued sentences in order until the answer is finished (or interrupted). */
  private async player(run: number) {
    try {
      for (;;) {
        if (run !== this.run) return;
        const line = this.lines.shift();
        if (!line) {
          if (this.streamDone) break;
          await new Promise<void>((r) => (this.wake = r));
          if (run !== this.run) return;
          this.wake = null;
          continue;
        }
        const wav = await line.wav;
        if (run !== this.run) return;
        if (!wav) continue;
        let track: Track;
        try {
          const { rate, samples } = decodeWav(wav);
          track = analyse(rate, samples);
        } catch {
          continue;
        }
        if (!this.settings.headphones && this.settings.mode === 'hands_free') this.pauseMic(true);
        this.phase = 'speaking';
        this.saying = line.text;
        if (this.head) {
          if (line.mood) this.head.mood = line.mood;
          for (const g of line.gestures) this.head.gesture(g);
          this.head.attention = 'speaking';
        }
        await this.play(wav, track, run);
      }
    } catch (e: any) {
      if (run === this.run) this.error = e?.message ?? String(e);
    }
    if (run !== this.run) return;
    await this.finishTurn(run);
  }

  private play(wav: ArrayBuffer, track: Track, run: number) {
    const cfg = store.config;
    return new Promise<void>((resolve, reject) => {
      const url = URL.createObjectURL(new Blob([wav], { type: 'audio/wav' }));
      const a = new Audio(url);
      a.preservesPitch = true;
      a.playbackRate = Math.min(2, Math.max(0.5, cfg?.tts_speed || 1));
      a.volume = Math.min(1, Math.max(0, cfg?.tts_volume ?? 1));
      this.audio = a;
      // The mouth track is in seconds of audio, which the element's clock keeps even at other speeds.
      if (this.head) this.head.speech = { track, time: () => a.currentTime };
      const done = () => {
        URL.revokeObjectURL(url);
        if (this.head?.speech?.track === track) this.head.speech = null;
        resolve();
      };
      a.onended = done;
      a.onpause = () => run !== this.run && done();
      a.onerror = () => {
        URL.revokeObjectURL(url);
        reject(new Error('Couldn’t play Gilgamesh’s voice'));
      };
      a.play().catch((e) => {
        URL.revokeObjectURL(url);
        reject(e);
      });
    });
  }

  private async finishTurn(run: number) {
    this.saying = '';
    if (this.phase !== 'off') this.phase = this.idlePhase;
    if (this.head) {
      this.head.attention = this.micOn ? 'listening' : 'idle';
      this.head.speech = null;
    }
    this.pauseMic(!this.wantMic());
    // Then what had to wait until he'd spoken: opening a view, a page, the music.
    await this.actionsDone;
    if (run !== this.run) return;
    const later = this.later;
    this.later = [];
    for (const f of later) f();
  }

  /** Says something of his own accord (a website check…), once he's free: spoken if his
   *  voice is set up, and shown in the transcript. */
  async announce(text: string, mood: Mood | null = null) {
    // Not over the owner, nor over himself: wait (up to two minutes) until he's free.
    for (let i = 0; i < 60 && ['starting', 'hearing', 'thinking', 'speaking'].includes(this.phase); i++) await new Promise((r) => setTimeout(r, 2000));
    this.turns.push({ who: 'gilgamesh', text });
    const tts = await invoke<{ installed: boolean; engine: boolean }>('tts_status').catch(() => null);
    if (!tts?.installed || !tts.engine) return;
    const run = ++this.run;
    this.lines = [];
    this.later = [];
    this.actionsDone = Promise.resolve();
    this.streamDone = false;
    void this.player(run);
    for (const s of takeSentences(text, true).sentences) {
      const spoken = speakable(s);
      if (spoken) this.say(spoken, mood, []);
      mood = null;
    }
    this.streamDone = true;
    this.wake?.();
  }

  /** Stops the answer now: the speech, the sentences still waiting and the writing. */
  interrupt() {
    const was = this.phase === 'thinking' || this.phase === 'speaking';
    this.run++;
    this.lines = [];
    this.later = [];
    this.wake?.();
    if (this.audio) {
      this.audio.pause();
      this.audio = null;
    }
    if (this.streaming && this.settings.chatId) rad.chatStop(this.settings.chatId).catch(() => {});
    if (this.head) this.head.speech = null;
    this.saying = '';
    this.status = '';
    if (was) {
      this.phase = this.idlePhase;
      if (this.head) this.head.attention = this.micOn ? 'listening' : 'idle';
      this.pauseMic(!this.wantMic());
    }
  }
}

export const gilgamesh = new Gilgamesh();
