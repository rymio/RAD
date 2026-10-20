import { invoke } from '@tauri-apps/api/core';
import type { ClientConfig } from './types';
import { stripMoods } from './gilgamesh/text';

/** Readable text: no code, links, citations or Markdown marks. */
export function speakable(md: string): string {
  return stripMoods(md)
    .replace(/<think>[\s\S]*?<\/think>/g, ' ')
    .replace(/```[\s\S]*?```/g, ' ')
    .replace(/`([^`]+)`/g, '$1')
    .replace(/!\[[^\]]*\]\([^)]*\)/g, ' ')
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
    .replace(/\s*\[\d+(?:\s*,\s*\d+)*\]/g, '')
    .replace(/https?:\/\/\S+/g, 'a link')
    .replace(/^\s{0,3}#{1,6}\s+(.+?)\s*$/gm, '$1.')
    .replace(/^\s*[-*+]\s+/gm, '')
    .replace(/^\s*\d+\.\s+/gm, '')
    .replace(/^\s*>\s?/gm, '')
    .replace(/\|/g, ', ')
    .replace(/[*_~]{1,3}([^*_~]+)[*_~]{1,3}/g, '$1')
    .replace(/[ \t]+/g, ' ')
    .trim();
}

/** Sentence-sized pieces (≤ ~220 characters): the model's sweet spot, and playback can start early. */
export function chunks(text: string, max = 220): string[] {
  const sentences = text
    .split(/\n+/)
    // A sentence ends at . ! ? followed by a space (so “coursemat.au” stays whole), or at 。！？.
    .flatMap((p) => p.split(/(?<=[.!?]["')\]]*)\s+|(?<=[。！？])/))
    .map((s) => s.trim())
    .filter(Boolean);
  const out: string[] = [];
  for (const s of sentences) {
    if (s.length > max) {
      // Long sentence: break at commas, then at spaces.
      let rest = s;
      while (rest.length > max) {
        let cut = rest.lastIndexOf(', ', max);
        if (cut < max / 3) cut = rest.lastIndexOf(' ', max);
        if (cut < max / 3) cut = max;
        out.push(rest.slice(0, cut + 1).trim());
        rest = rest.slice(cut + 1).trim();
      }
      if (rest) out.push(rest);
    } else if (out.length && out[out.length - 1].length + s.length < 90) {
      out[out.length - 1] += ' ' + s; // keep very short sentences together
    } else out.push(s);
  }
  return out;
}

type Opts = Pick<ClientConfig, 'tts_voice' | 'tts_speed' | 'tts_volume' | 'tts_temperature' | 'tts_top_p' | 'tts_top_k' | 'tts_repetition_penalty'>;

class Speech {
  /** What is being read aloud (e.g. a message id), or null. */
  speaking = $state<string | null>(null);
  loading = $state(false);
  error = $state<string | null>(null);
  private run = 0;
  private audio: HTMLAudioElement | null = null;

  stop() {
    this.run++;
    this.speaking = null;
    this.loading = false;
    if (this.audio) {
      this.audio.pause();
      this.audio = null;
    }
  }

  private synth(text: string, o: Opts): Promise<ArrayBuffer> {
    return invoke<ArrayBuffer>('tts_speak', {
      text,
      voice: o.tts_voice || 'en_female',
      temperature: o.tts_temperature,
      topP: o.tts_top_p,
      topK: o.tts_top_k,
      repetitionPenalty: o.tts_repetition_penalty,
    });
  }

  /** Reads `text` aloud; the next piece is synthesised while the current one plays. */
  async speak(id: string, markdown: string, o: Opts) {
    this.stop();
    const run = this.run;
    const parts = chunks(speakable(markdown));
    if (!parts.length) return;
    this.speaking = id;
    this.error = null;
    this.loading = true;
    let next = this.synth(parts[0], o);
    try {
      for (let i = 0; i < parts.length; i++) {
        const wav = await next;
        if (run !== this.run) return;
        if (i + 1 < parts.length) next = this.synth(parts[i + 1], o);
        this.loading = false;
        await this.play(wav, o, run);
        if (run !== this.run) return;
        this.loading = i + 1 < parts.length;
      }
    } catch (e: any) {
      if (run === this.run) this.error = e?.message ?? String(e);
      throw e;
    } finally {
      if (run === this.run) {
        this.speaking = null;
        this.loading = false;
      }
    }
  }

  private play(wav: ArrayBuffer, o: Opts, run: number) {
    return new Promise<void>((resolve, reject) => {
      const url = URL.createObjectURL(new Blob([wav], { type: 'audio/wav' }));
      const a = new Audio(url);
      a.preservesPitch = true;
      a.playbackRate = Math.min(2, Math.max(0.5, o.tts_speed || 1));
      a.volume = Math.min(1, Math.max(0, o.tts_volume ?? 1));
      this.audio = a;
      const done = () => {
        URL.revokeObjectURL(url);
        resolve();
      };
      a.onended = done;
      a.onpause = () => run !== this.run && done();
      a.onerror = () => {
        URL.revokeObjectURL(url);
        reject(new Error('Couldn’t play the audio'));
      };
      a.play().catch((e) => {
        URL.revokeObjectURL(url);
        reject(e);
      });
    });
  }
}

export const speech = new Speech();
