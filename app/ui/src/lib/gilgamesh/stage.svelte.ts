// Gilgamesh in a window of his own (e.g. full screen on a second monitor). The window only shows
// his picture (components/GilgameshStage.svelte); everything else stays here in the main window,
// which sends it his face and everything that moves it: mood, attention, gestures, the speech's
// mouth track and its clock, and where the owner's face is.

import { invoke } from '@tauri-apps/api/core';
import { emitTo, listen } from '@tauri-apps/api/event';
import type { Track } from './lipsync';
import type { Attention, Head, HeadLike } from './head';
import type { Gesture, Mood } from './text';
import { gilgamesh } from './gilgamesh.svelte';

export const STAGE = 'gilgamesh';
export const STAGE_EVENT = 'gil-stage';

/** A message to the stage window. */
export type StageMsg =
  | { t: 'face'; avatar: unknown }
  | { t: 'mood'; v: Mood }
  | { t: 'attention'; v: Attention }
  | { t: 'speech'; track: { fps: number; jaw: number[]; wide: number[]; round: number[]; energy: number[] } | null; time?: number }
  | { t: 'sync'; time: number }
  | { t: 'blink' }
  | { t: 'gesture'; v: Gesture }
  | { t: 'ack' }
  | { t: 'see'; x: number; y: number };

const send = (m: StageMsg) => void emitTo(STAGE, STAGE_EVENT, m).catch(() => {});

/** Gilgamesh's state, passed on to the stage window. */
class RemoteHead implements HeadLike {
  private m: Mood = 'neutral';
  private a: Attention = 'idle';
  private s: Head['speech'] = null;
  private clock: ReturnType<typeof setInterval> | undefined;
  private lastSee = 0;

  get mood() { return this.m; }
  set mood(v: Mood) { this.m = v; send({ t: 'mood', v }); }
  get attention() { return this.a; }
  set attention(v: Attention) { this.a = v; send({ t: 'attention', v }); }
  get speech() { return this.s; }
  set speech(v: Head['speech']) {
    this.s = v;
    clearInterval(this.clock);
    if (!v) {
      send({ t: 'speech', track: null });
      return;
    }
    const tr: Track = v.track;
    send({ t: 'speech', track: { fps: tr.fps, jaw: [...tr.jaw], wide: [...tr.wide], round: [...tr.round], energy: [...tr.energy] }, time: v.time() });
    // The audio plays here: its clock is sent now and then, so the lips stay with it.
    this.clock = setInterval(() => this.s === v && send({ t: 'sync', time: v.time() }), 400);
  }

  blink() { send({ t: 'blink' }); }
  gesture(v: Gesture) { send({ t: 'gesture', v }); }
  acknowledge() { send({ t: 'ack' }); }
  see(x: number, y: number) {
    // The camera runs up to ~8 frames a second; that's plenty for his eyes.
    const now = performance.now();
    if (now - this.lastSee < 110) return;
    this.lastSee = now;
    send({ t: 'see', x, y });
  }

  stop() {
    clearInterval(this.clock);
  }

  /** Everything the window needs to start from. */
  replay() {
    send({ t: 'mood', v: this.m });
    send({ t: 'attention', v: this.a });
  }
}

class Stage {
  /** The window is open. */
  open = $state(false);

  private remote: RemoteHead | null = null;
  private off: (() => void) | null = null;
  private unlisten: (() => void)[] = [];
  private stopEffect: (() => void) | null = null;

  /** Listens for the window (it can be open from before a reload of this one). */
  async start() {
    this.unload(false);
    this.unlisten.push(
      await listen(`${STAGE_EVENT}-ready`, () => this.attach()),
      await listen(`${STAGE_EVENT}-closed`, () => this.detach()),
    );
    // A new face goes to the window too.
    this.stopEffect = $effect.root(() => {
      $effect(() => {
        const a = gilgamesh.avatar;
        if (this.open && a) send({ t: 'face', avatar: $state.snapshot(a) });
      });
    });
    if (await invoke<boolean>('gilgamesh_window', { open: null }).catch(() => false)) this.attach();
  }

  /** The window has (re)loaded: it gets his face and current state, then follows him. */
  private attach() {
    // Joining his pictures sends his mood and attention.
    if (!this.remote) {
      this.remote = new RemoteHead();
      this.off = gilgamesh.head.add(this.remote);
    } else this.remote.replay();
    // Opening sends the face (the effect above); a window that reloaded needs it again.
    const was = this.open;
    this.open = true;
    if (was && gilgamesh.avatar) send({ t: 'face', avatar: $state.snapshot(gilgamesh.avatar) });
  }

  private detach() {
    this.off?.();
    this.off = null;
    this.remote?.stop();
    this.remote = null;
    this.open = false;
  }

  async toggle() {
    if (this.open) {
      await invoke('gilgamesh_window', { open: false });
      this.detach();
    } else {
      await invoke('gilgamesh_window', { open: true });
    }
  }

  /** The vault locked (or the app is going): the window closes with it. */
  unload(close = true) {
    this.stopEffect?.();
    this.stopEffect = null;
    for (const u of this.unlisten) u();
    this.unlisten = [];
    if (close && this.open) void invoke('gilgamesh_window', { open: false }).catch(() => {});
    this.detach();
  }
}

export const stage = new Stage();
