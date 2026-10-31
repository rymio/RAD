// What Gilgamesh sees of the owner's work: which part of RAD is open and what's in it, and where
// they have been lately. Views tell him about themselves (`activity.seen`: the page in the
// browser, the folder in Files, the email being read); the rest comes from the route. It goes
// into every answer, so he can help with whatever the owner is doing.

import { store } from '../state.svelte';
import type { Route } from '../types';

const NAMES: Record<Route['kind'], string> = {
  chat: 'a chat', project: 'a project', settings: 'Settings', browser: 'the web browser', chromium: 'the Chromium browser',
  tor: 'the TOR browser', terminal: 'the terminal', files: 'Files (the computer’s files)', calendar: 'the calendar', email: 'email',
  music: 'music', video: 'video', rymnet: 'RYMNET', rymwin: 'RYMWIN (a Windows desktop)', container: 'a development container',
  sidex: 'SideX (the code editor)', database: 'the database tool', games: 'the games', zx: 'the ZX Spectrum emulator',
  zxlab: 'the Spectrum Lab (disassembler)', c64: 'the Commodore 64 emulator', c64lab: 'the C64 Lab (disassembler)',
  dos: 'the DOS PC emulator', gilgamesh: 'Gilgamesh’s own view (talking with you)',
  monitor: 'Web Monitoring (the websites you watch for them)',
  bots: 'Bots (their assistants that work on computers of their own)',
  photocraft: 'PhotoCraft (the image editor: their pictures)',
  // Private: nothing of it reaches his other conversations.
  mood: 'Gilgamesh’s own view',
};

const KEEP = 12;

class Activity {
  /** What each view says about itself now, by route kind. Plain (not reactive): it's only read
   *  when he answers, and views write it from their effects. */
  private details: Record<string, string> = {};
  /** The views visited lately, most recent last. */
  private log: { what: string; at: number }[] = [];

  /** A view tells Gilgamesh what it shows (null: nothing worth saying). */
  seen(kind: Route['kind'], text: string | null) {
    if (text) this.details[kind] = text.slice(0, 300);
    else delete this.details[kind];
  }

  /** The open view in a few words. */
  where(r: Route = store.route): string {
    let s = NAMES[r.kind] ?? r.kind;
    if (r.kind === 'chat') {
      const c = r.id ? store.chats.find((c) => c.id === r.id) : null;
      s = c ? `the chat “${c.title}”` : 'a new chat';
    } else if (r.kind === 'project') {
      const p = store.projects.find((p) => p.id === r.id);
      s = p ? `the project “${p.name}”${r.doc ? ' (reading one of its documents)' : ''}` : s;
    } else if (r.kind === 'settings' && r.tab) s = `Settings → ${r.tab}`;
    else if (r.kind === 'container') s = `the ${r.type === 'rust' ? 'Rust' : 'Python'} container`;
    return s;
  }

  /** Called when the route changes. */
  visit(r: Route) {
    const what = this.where(r);
    if (this.log.at(-1)?.what === what) return;
    this.log.push({ what, at: Date.now() });
    if (this.log.length > KEEP) this.log.shift();
  }

  /** For Gilgamesh's instructions. */
  describe(): string {
    const r = store.route;
    const detail = this.details[r.kind];
    const lines = [`WHAT THE OWNER IS DOING: they have ${this.where(r)} open${detail ? ` — ${detail}` : ''}.`];
    const ago = (ms: number) => {
      const m = Math.round((Date.now() - ms) / 60000);
      return m < 1 ? 'just now' : m < 60 ? `${m} min ago` : `${Math.round(m / 60)} h ago`;
    };
    const before = this.log.slice(0, -1).reverse().slice(0, 6);
    if (before.length) lines.push(`Before that: ${before.map((v) => `${v.what} (${ago(v.at)})`).join('; ')}.`);
    return lines.join('\n');
  }
}

export const activity = new Activity();
