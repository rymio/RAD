// Gilgamesh: a spoken conversation with RAD, face to face.
//
//   microphone → speech to text (stt.rs) → RAD chat (vault, memories, actions) → sentences
//   → text to speech (tts.rs) → speaker + the talking head's lips
//
// The answer is spoken sentence by sentence while it is still being written. Talking over
// Gilgamesh interrupts him (with headphones, or while holding the push-to-talk key); through
// the speakers the microphone is ignored while he speaks, so he doesn't hear himself.

import { invoke, Channel } from '@tauri-apps/api/core';
import { api, rad } from '../api';
import { store } from '../state.svelte';
import { speakable } from '../speech.svelte';
import type { Chat, ChatStreamEvent } from '../types';
import { analyse, decodeWav, type Track } from './lipsync';
import type { Head } from './head';
import { moodOf, stripMoods, takeSentences, type Mood } from './text';
import type { Avatar } from './face';

export type Mode = 'hands_free' | 'push_to_talk';
export interface Settings {
  mode: Mode;
  /** With headphones the microphone stays on while Gilgamesh talks, so you can interrupt him. */
  headphones: boolean;
  /** Gilgamesh's voice (a MioTTS voice key); empty: a male preset. */
  voice: string;
  chatId: number | null;
}
export type Phase = 'off' | 'starting' | 'listening' | 'hearing' | 'thinking' | 'speaking';
export interface Turn { who: 'you' | 'gilgamesh'; text: string }

const SETTINGS_KEY = '/v1/app-data/gilgamesh';
const AVATAR_KEY = '/v1/app-data/gilgamesh_avatar';

const PERSONA = `YOU ARE SPEAKING ALOUD. You are Gilgamesh, the voice and face of RAD, talking with its owner face to face. Your words are spoken by a talking head, so:
- answer in one to three short, natural spoken sentences, unless asked for more detail;
- no Markdown, lists, tables, code, links, emoji or citation numbers;
- begin your reply with exactly one mood tag that fits what you say: [neutral], [happy], [amused], [concerned], [surprised] or [thinking]. Put another tag before a later sentence only if the mood changes.
The owner's words come from speech recognition and may contain small mistakes; read them sensibly.`;

interface Line { text: string; mood: Mood | null; wav: Promise<ArrayBuffer | null> }

class Gilgamesh {
  phase = $state<Phase>('off');
  /** What the microphone is hearing right now (live caption). */
  heard = $state('');
  /** The sentence Gilgamesh is saying. */
  saying = $state('');
  level = $state(0);
  turns = $state<Turn[]>([]);
  error = $state<string | null>(null);
  settings = $state<Settings>({ mode: 'hands_free', headphones: false, voice: '', chatId: null });
  avatar = $state<Avatar | null>(null);
  loaded = $state(false);
  holding = $state(false);

  head: Head | null = null;
  private run = 0;
  private lines: Line[] = [];
  private wake: (() => void) | null = null;
  private streamDone = true;
  private streaming = false;
  private lastSynth: Promise<unknown> = Promise.resolve();
  private audio: HTMLAudioElement | null = null;
  private micPaused = false;

  async load() {
    if (this.loaded) return;
    const [s, a] = await Promise.all([api.get<any>(SETTINGS_KEY).catch(() => null), api.get<any>(AVATAR_KEY).catch(() => null)]);
    if (s && typeof s === 'object') this.settings = { ...this.settings, ...s };
    if (a && typeof a === 'object' && a.image && Array.isArray(a.points)) this.avatar = a;
    this.loaded = true;
  }

  saveSettings(patch: Partial<Settings>) {
    this.settings = { ...this.settings, ...patch };
    return api.put(SETTINGS_KEY, $state.snapshot(this.settings)).catch((e) => store.error(e, 'Couldn’t save Gilgamesh’s settings'));
  }

  async setAvatar(a: Avatar | null) {
    await api.put(AVATAR_KEY, a ?? {});
    this.avatar = a;
  }

  // ---------------------------------------------------------------- listening

  async start() {
    if (this.phase !== 'off') return;
    this.error = null;
    this.phase = 'starting';
    const ch = new Channel<any>();
    ch.onmessage = (ev) => this.onStt(ev);
    const ptt = this.settings.mode === 'push_to_talk';
    try {
      await invoke('stt_listen', { onEvent: ch, paused: ptt });
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
    invoke('stt_stop').catch(() => {});
    this.phase = 'off';
    this.heard = '';
    this.level = 0;
    this.holding = false;
    if (this.head) this.head.attention = 'idle';
  }

  private pauseMic(paused: boolean) {
    if (this.micPaused === paused || this.phase === 'off') return;
    this.micPaused = paused;
    invoke('stt_pause', { paused }).catch(() => {});
  }

  /** The microphone should hear the user now: always in hands-free mode, or while the key is held. */
  private wantMic() {
    return this.settings.mode === 'hands_free' ? true : this.holding;
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
        break;
      case 'final':
        this.heard = '';
        this.ask(ev.text);
        break;
      case 'cancelled':
        if (this.phase === 'hearing') {
          this.phase = 'listening';
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

  private async chatId(): Promise<number> {
    const id = this.settings.chatId;
    if (id && store.chats.some((c) => c.id === id)) return id;
    const c = await api.post<Chat>('/v1/chats', { title: 'Gilgamesh' });
    store.reloadChats();
    await this.saveSettings({ chatId: c.id });
    return c.id;
  }

  /** New conversation: Gilgamesh forgets this chat (it stays in the sidebar). */
  async newConversation() {
    this.interrupt();
    this.turns = [];
    await this.saveSettings({ chatId: null });
  }

  /** Sends what the user said (or typed) and speaks the answer. */
  async ask(text: string) {
    text = text.trim();
    if (!text) return;
    this.interrupt();
    const run = ++this.run;
    this.error = null;
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
    let full = '';
    let buf = '';
    let mood: Mood | null = null;
    const push = (final: boolean) => {
      const r = takeSentences(buf, final);
      buf = r.rest;
      for (const s of r.sentences) {
        const m = moodOf(s);
        mood = m.mood ?? mood;
        const t = speakable(m.text);
        if (t) this.say(t, mood);
      }
    };
    void this.player(run);
    let chatId: number;
    try {
      chatId = await this.chatId();
    } catch (e: any) {
      this.fail(run, e);
      return;
    }
    this.streaming = true;
    try {
      await rad.chatSend(chatId, { content: text, context: PERSONA, attachments: [], model: null }, (ev: ChatStreamEvent) => {
        if (run !== this.run) return;
        switch (ev.type) {
          case 'delta':
            full += ev.text;
            buf += ev.text;
            if (replyAt < 0) replyAt = this.turns.push({ who: 'gilgamesh', text: '' }) - 1;
            this.turns[replyAt].text = stripMoods(full).replace(/<think>[\s\S]*?(<\/think>|$)/g, '').trim();
            push(false);
            break;
          case 'action':
            store.runAction(ev.action);
            break;
          case 'error':
            this.error = ev.message;
            break;
        }
      });
      if (run === this.run) push(true);
    } catch (e: any) {
      if (run === this.run) this.error = e?.message ?? String(e);
    } finally {
      this.streaming = false;
      if (run === this.run) {
        this.streamDone = true;
        this.wake?.();
      }
      store.reloadChats();
    }
  }

  private fail(run: number, e: any) {
    if (run !== this.run) return;
    this.error = e?.message ?? String(e);
    this.streamDone = true;
    this.wake?.();
  }

  /** Queues a sentence; synthesis runs one at a time, ahead of playback. */
  private say(text: string, mood: Mood | null) {
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
    this.lines.push({ text, mood, wav });
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
          this.head.attention = 'speaking';
        }
        await this.play(wav, track, run);
      }
    } catch (e: any) {
      if (run === this.run) this.error = e?.message ?? String(e);
    }
    if (run !== this.run) return;
    this.finishTurn();
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

  private finishTurn() {
    this.saying = '';
    if (this.phase === 'off') return;
    this.phase = 'listening';
    if (this.head) {
      this.head.attention = 'listening';
      this.head.speech = null;
    }
    this.pauseMic(!this.wantMic());
  }

  /** Stops the answer now: the speech, the sentences still waiting and the writing. */
  interrupt() {
    const was = this.phase === 'thinking' || this.phase === 'speaking';
    this.run++;
    this.lines = [];
    this.wake?.();
    if (this.audio) {
      this.audio.pause();
      this.audio = null;
    }
    if (this.streaming && this.settings.chatId) rad.chatStop(this.settings.chatId).catch(() => {});
    if (this.head) this.head.speech = null;
    this.saying = '';
    if (was) {
      this.phase = this.phase === 'off' ? 'off' : 'listening';
      if (this.head) this.head.attention = 'listening';
      this.pauseMic(!this.wantMic());
    }
  }
}

export const gilgamesh = new Gilgamesh();
