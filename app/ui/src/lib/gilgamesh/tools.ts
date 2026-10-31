// What Gilgamesh can do for the owner, through RAD's own features: remember facts (RAD's
// memories, shared with every chat), save notes (Inbox project), add calendar events, open views
// and web pages, search the web, control the music, give the owner's bots tasks and look through
// the camera, edit pictures in PhotoCraft, read the owner's books aloud, play chess and build
// Z80 computers in Z80 Lab. Also what
// he knows about "now".

import { api } from '../api';
import { store } from '../state.svelte';
import { music, musicEngine } from '../media.svelte';
import { PROJECT_COLORS } from '../util';
import type * as T from '../types';
import type { ActionName } from './text';
import { monitor, siteName } from '../monitor.svelte';
import { gilgamesh } from './gilgamesh.svelte';
import { bots } from '../bots.svelte';
import { sight } from './sight.svelte';
import { photocraft } from '../photocraft.svelte';
import { books } from '../books.svelte';
import { chess, LEVELS, type Level } from '../chess.svelte';
import { lab, CLOCKS } from '../z80lab.svelte';
import { PROGRAMS } from '../z80lab/templates';

export interface Outcome {
  /** Shown in the transcript, e.g. "Remembered: …". */
  label: string;
  ok: boolean;
  /** Done after Gilgamesh has finished speaking (it leaves the view or makes sound). */
  later?: () => void;
}

const VIEWS: Record<string, T.Route> = {
  browser: { kind: 'browser' }, web: { kind: 'browser' }, chromium: { kind: 'chromium' }, tor: { kind: 'tor' },
  terminal: { kind: 'terminal' }, files: { kind: 'files' }, calendar: { kind: 'calendar' }, email: { kind: 'email' },
  mail: { kind: 'email' }, music: { kind: 'music' }, video: { kind: 'video' }, settings: { kind: 'settings' },
  memories: { kind: 'settings', tab: 'memories' }, chat: { kind: 'chat', id: null }, database: { kind: 'database' },
  games: { kind: 'games' }, chess: { kind: 'chess' }, z80lab: { kind: 'z80lab' }, z80: { kind: 'z80lab' }, lab: { kind: 'z80lab' }, bots: { kind: 'bots' }, bot: { kind: 'bots' }, books: { kind: 'books' }, book: { kind: 'books' }, library: { kind: 'books' }, photocraft: { kind: 'photocraft' }, pictures: { kind: 'photocraft' }, monitoring: { kind: 'monitor' }, monitor: { kind: 'monitor' }, websites: { kind: 'monitor' }, rymnet: { kind: 'rymnet' }, rymwin: { kind: 'rymwin' }, zx: { kind: 'zx' }, spectrum: { kind: 'zx' },
  c64: { kind: 'c64' }, commodore: { kind: 'c64' }, dos: { kind: 'dos' }, sidex: { kind: 'sidex' }, editor: { kind: 'sidex' },
};

const pad = (n: number) => String(n).padStart(2, '0');
const isoDay = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
const dayLabel = (iso: string) => new Date(`${iso}T12:00`).toLocaleDateString('en-GB', { weekday: 'short', day: 'numeric', month: 'short' });

const TIME_WORDS = /\s*(?:at\s+)?\b(\d{1,2})(?::(\d{2}))?\s*(am|pm|a\.m\.|p\.m\.)(?=\W|$)|\s*(?:at\s+)?\b(\d{1,2}):(\d{2})\b/i;

/** A time written in words: "9 AM", "2:30pm", "14:30". */
export function timeIn(s: string): readonly [number, number] | null {
  const m = TIME_WORDS.exec(s);
  if (!m) return null;
  if (m[4]) return [Number(m[4]), Number(m[5])];
  let h = Number(m[1]) % 12;
  if (/^p/i.test(m[3])) h += 12;
  return [h, Number(m[2] ?? 0)];
}

export async function calendarEvents(): Promise<T.CalendarEvent[]> {
  const v = await api.get<any>('/v1/app-data/calendar_events').catch(() => null);
  return Array.isArray(v) ? v : [];
}

export async function runAction(name: ActionName, arg: string): Promise<Outcome> {
  try {
    switch (name) {
      case 'remember': {
        const text = arg.slice(0, 500);
        // A placeholder copied from the instructions, not a fact.
        if (text.length < 8 || /^(fact|…|\.\.\.)$/i.test(text)) return { label: 'Nothing to remember', ok: false };
        const have = await api.get<T.Memory[]>('/v1/memories').catch(() => []);
        if (!have.some((m) => m.text.trim().toLowerCase() === text.toLowerCase())) await api.post('/v1/memories', { text });
        return { label: `Remembered: ${text}`, ok: true };
      }
      case 'note': {
        const [t, ...body] = arg.split('|');
        const title = t.trim().slice(0, 120) || 'Note from Gilgamesh';
        const content = (body.join('|').trim() || t.trim()) + `\n\n_Saved by Gilgamesh, ${new Date().toLocaleString('en-GB')}_`;
        const inbox = await store.inboxProject();
        await api.post(`/v1/projects/${inbox.id}/notes`, { title, content, tags: ['gilgamesh'] });
        return { label: `Note saved in ${inbox.name}: ${title}`, ok: true };
      }
      case 'event': {
        const [when, ...rest] = arg.split('|');
        let title = rest.join('|').trim();
        const m = /(\d{4}-\d{2}-\d{2})(?:[ T](\d{1,2}):(\d{2}))?/.exec(when);
        if (!m || !title || Number.isNaN(new Date(`${m[1]}T12:00`).getTime())) return { label: `Couldn’t add to the calendar: “${arg}”`, ok: false };
        let time = m[2] ? ([Number(m[2]), Number(m[3])] as const) : timeIn(title);
        // "Dentist at 9 AM": the time written in the title instead.
        if (!m[2] && time) title = title.replace(TIME_WORDS, '').replace(/\s+(at|@)\s*$/i, '').trim() || title;
        // Out of range, or midnight (what models write for "some time that day"): all day.
        if (time && (time[0] > 23 || time[1] > 59 || (time[0] === 0 && time[1] === 0))) time = null;
        const start = time ? `${pad(time[0])}:${pad(time[1])}` : null;
        const end = time ? `${pad(Math.min(23, time[0] + 1))}:${pad(time[1])}` : null;
        const list = await calendarEvents();
        list.push({ id: crypto.randomUUID(), title: title.slice(0, 200), date: m[1], start, end, notes: 'Added by Gilgamesh', color: PROJECT_COLORS[0] });
        await api.put('/v1/app-data/calendar_events', list);
        return { label: `Added to the calendar: ${dayLabel(m[1])}${start ? ' ' + start : ''} — ${title}`, ok: true };
      }
      case 'open': {
        const a = arg.trim();
        if (/^https?:\/\//i.test(a) || /^[\w-]+(\.[\w-]+)+(\/\S*)?$/.test(a)) {
          const url = /^https?:/i.test(a) ? a : `https://${a}`;
          return { label: `Opening ${url}`, ok: true, later: () => store.runAction({ kind: 'open_url', url }) };
        }
        const route = VIEWS[a.toLowerCase().replace(/^the\s+/, '').split(/\s+/)[0]];
        if (!route) return { label: `Don’t know how to open “${a}”`, ok: false };
        return { label: `Opening ${a}`, ok: true, later: () => store.navigate(route) };
      }
      case 'read': {
        // "start", "stop", "page", "next", "previous" or "open | title".
        const [cmd, ...rest] = arg.split('|').map((x) => x.trim());
        const c = cmd.toLowerCase();
        if (c === 'open' || rest.length) {
          await books.load();
          const b = books.match(rest.join(' ') || cmd.replace(/^open\s*/i, ''));
          if (!b) return { label: `No book “${rest.join(' ') || cmd}” in Books`, ok: false };
          return { label: `Opening ${b.title}`, ok: true, later: () => books.openBook(b.id) };
        }
        if (!books.reader) return { label: 'No book is open', ok: false };
        if (/^(stop|pause|quiet)/.test(c)) {
          books.pause();
          return { label: 'Stopped reading', ok: true };
        }
        if (/^(next|forward)/.test(c)) return { label: 'Next page', ok: true, later: () => books.turn(1) };
        if (/^(prev|back)/.test(c)) return { label: 'Previous page', ok: true, later: () => books.turn(-1) };
        if (/^(page|this page|the page)/.test(c)) return { label: 'Reading this page', ok: true, later: () => books.readPage() };
        return { label: `Reading ${books.reader.book.title}`, ok: true, later: () => books.play() };
      }
      case 'chess': {
        // "new | white", "undo", "show | Nf3", "hint", "level | hard" or "open".
        const [cmd, ...rest] = arg.split('|').map((x) => x.trim());
        const c = cmd.toLowerCase();
        const what = rest.join(' ');
        if (/^(new|start|play)/.test(c)) {
          const side = /black/i.test(what) ? 'b' : /random|either|any/i.test(what) ? 'random' : 'w';
          return { label: 'New game of chess', ok: true, later: () => void chess.newGame(side) };
        }
        if (/^level/.test(c)) {
          const l = Object.keys(LEVELS).find((k) => what.toLowerCase().includes(k)) as Level | undefined;
          if (!l) return { label: `No chess level “${what}”`, ok: false };
          chess.saveSettings({ level: l });
          return { label: `Chess level: ${LEVELS[l].label}`, ok: true };
        }
        if (/^open/.test(c)) return { label: 'Opening chess', ok: true, later: () => store.navigate({ kind: 'chess' }) };
        if (!chess.game) return { label: 'No chess game is open', ok: false };
        if (/^(undo|take)/.test(c)) return chess.canTakeBack ? { label: 'Took back your move', ok: true, later: () => chess.takeBack() } : { label: 'Nothing to take back', ok: false };
        if (/^hint/.test(c)) return { label: 'Showing a hint', ok: true, later: () => void chess.requestHint() };
        if (/^(show|point|arrow)/.test(c)) return chess.show(what) ? { label: `Showing ${what}`, ok: true } : { label: `Can’t show “${what}”`, ok: false };
        return { label: `Don’t know the chess action “${cmd}”`, ok: false };
      }
      case 'circuit': {
        // "add | part", "wire | U1.6 - X1 OUT", "connect | U1 - U5", "delete | U3", "program | blinky",
        // "clock | 5 Hz", "run", "stop", "step", "reset", "power | on", "new | easy" or "open".
        const [cmd, ...rest] = arg.split('|').map((x) => x.trim());
        const c = cmd.toLowerCase();
        const what = rest.join('|').trim();
        if (/^(new|template)/.test(c)) return { label: 'New Z80 computer', ok: true, later: () => void lab.create(/blank|empty|own/i.test(what) ? 'blank' : 'easy') };
        if (/^open/.test(c)) return { label: 'Opening Z80 Lab', ok: true, later: () => store.navigate({ kind: 'z80lab' }) };
        if (!lab.project) return { label: 'No circuit is open in Z80 Lab', ok: false };
        switch (c) {
          case 'add': {
            const type = lab.partType(what);
            if (!type) return { label: `No part like “${what}”`, ok: false };
            const at = lab.freeSpot();
            const p = lab.addPart(type, at.x, at.y);
            return p ? { label: `Added ${p.ref}`, ok: true } : { label: 'Couldn’t add it', ok: false };
          }
          case 'wire': {
            const [a, b] = what.split(/\s+(?:-|to|–|—)\s+|\s*[-–—]\s*(?=[A-Za-z]+\d)/).map((x) => lab.pinRef(x));
            if (!a || !b) return { label: `Can’t find the pins in “${what}”`, ok: false };
            lab.addWire(a, b);
            return { label: `Wired ${what}`, ok: true };
          }
          case 'connect': {
            const [a, b] = what.split(/\s+(?:-|to|and|–|—)\s+|\s*[-–—,]\s*/).map((x) => lab.partByRef(x));
            if (!a || !b) return { label: `Can’t find the parts in “${what}”`, ok: false };
            const n = lab.connectMatching(a.id, b.id);
            return { label: `Connected ${n} matching pins of ${a.ref} and ${b.ref}`, ok: n > 0 };
          }
          case 'delete':
          case 'remove': {
            const p = lab.partByRef(what);
            if (!p) return { label: `No part ${what}`, ok: false };
            lab.removeParts([p.id]);
            return { label: `Removed ${p.ref}`, ok: true };
          }
          case 'program': {
            const key = Object.keys(PROGRAMS).find((k) => what.toLowerCase().includes(k));
            if (!key) return { label: `No example program “${what}”`, ok: false };
            lab.setSource(PROGRAMS[key].source);
            return { label: `Loaded the ${key} program`, ok: true };
          }
          case 'clock': {
            const m = what.toLowerCase().match(/([\d.]+)\s*(mhz|khz|hz)?/);
            if (/full|fast|normal/.test(what.toLowerCase())) lab.setClock(0);
            else if (m) {
              const hz = Number(m[1]) * (m[2] === 'mhz' ? 1e6 : m[2] === 'khz' ? 1e3 : 1);
              const near = CLOCKS.filter((x) => x.hz).reduce((best, x) => (Math.abs(Math.log(x.hz / hz)) < Math.abs(Math.log(best.hz / hz)) ? x : best));
              lab.setClock(near.hz);
            } else return { label: `Can’t read the clock “${what}”`, ok: false };
            return { label: `Clock: ${CLOCKS.find((x) => x.hz === lab.clock)?.label}`, ok: true };
          }
          case 'power':
            lab.setPower(!/off/i.test(what));
            return { label: `Power ${lab.power ? 'on' : 'off'}`, ok: true };
          case 'run':
            return { label: 'Running the program', ok: true, later: () => void lab.run() };
          case 'stop':
            lab.stop();
            return { label: 'Stopped', ok: true };
          case 'step':
            return { label: 'One instruction', ok: true, later: () => void lab.step() };
          case 'reset':
            lab.reset();
            return { label: 'Reset', ok: true };
        }
        return { label: `Don’t know the circuit action “${cmd}”`, ok: false };
      }
      case 'search': {
        const url = `https://duckduckgo.com/?q=${encodeURIComponent(arg)}`;
        return { label: `Searching the web for “${arg}”`, ok: true, later: () => store.runAction({ kind: 'open_url', url }) };
      }
      case 'check': {
        await monitor.load();
        if (!monitor.sites.length) return { label: 'No websites are monitored yet', ok: false };
        const wrong = await monitor.run('asked');
        const n = monitor.sites.length;
        const fine = n - new Set(monitor.sites.filter((s) => wrong.some((w) => w.startsWith(siteName(s)))).map((s) => s.id)).size;
        const text = wrong.length
          ? `I've checked your ${n} website${n === 1 ? '' : 's'}. ${wrong.join('. ')}.${fine ? ` The other ${fine === 1 ? 'one is' : `${fine} are`} fine.` : ''}`
          : `I've checked your ${n} website${n === 1 ? '' : 's'}: all online, and no certificate is close to expiring.`;
        return { label: `Checked ${n} website${n === 1 ? '' : 's'}${wrong.length ? `: ${wrong.length} problem${wrong.length === 1 ? '' : 's'}` : ': all fine'}`, ok: true, later: () => void gilgamesh.announce(text, wrong.length ? 'concerned' : 'happy') };
      }
      case 'bot':
        return await bots.fromGilgamesh(arg);
      case 'edit': {
        // "file | request" or just the request (on whatever is open in PhotoCraft).
        await photocraft.refresh();
        if (!photocraft.status?.installed) return { label: 'PhotoCraft isn’t installed (see the PhotoCraft view)', ok: false };
        if (photocraft.busy()) return { label: 'PhotoCraft is busy with another edit', ok: false };
        const [head, ...rest] = arg.split('|').map((x) => x.trim());
        const file = rest.length ? photocraft.files.find((f) => f.name.toLowerCase() === head.toLowerCase())?.name : undefined;
        const request = file ? rest.join('|') : arg;
        void photocraft
          .edit(request, { target: 'app', from: 'gilgamesh', file })
          .then((j) => gilgamesh.announce(j.status === 'done' ? `Done in PhotoCraft: ${j.result}` : `I couldn't finish that in PhotoCraft: ${j.result}`, j.status === 'done' ? 'happy' : 'concerned'));
        return { label: `Editing in PhotoCraft: ${request}`, ok: true };
      }
      case 'look': {
        // A picture from the camera, answered once he's finished what he's saying.
        const answer = await sight.look(arg);
        return { label: `Looked: ${arg}`, ok: true, later: () => void gilgamesh.announce(answer) };
      }
      case 'music': {
        await music.load();
        // "play | Billie Holiday" is a request for Billie Holiday.
        const parts = arg.split('|').map((x) => x.trim()).filter(Boolean);
        const a = (parts.length > 1 && /^(play|resume)$/i.test(parts[0]) ? parts.slice(1).join(' ') : arg.trim()).toLowerCase();
        if (!music.items.length) return { label: 'The music playlist is empty', ok: false };
        if (a === 'pause' || a === 'stop') return { label: 'Pausing the music', ok: true, later: () => musicEngine.send({ cmd: 'pause' }) };
        if (a === 'next' || a === 'skip') return { label: 'Next song', ok: true, later: () => music.step(1) };
        if (a === 'previous' || a === 'back') return { label: 'Previous song', ok: true, later: () => music.step(-1) };
        if (a === 'play' || a === 'resume') return { label: 'Playing music', ok: true, later: () => music.play() };
        const hit = music.items.find((i) => `${i.title} ${i.artist ?? ''} ${i.album ?? ''}`.toLowerCase().includes(a));
        if (!hit) return { label: `No “${arg}” in the playlist`, ok: false };
        return { label: `Playing ${hit.title}${hit.artist ? ' by ' + hit.artist : ''}`, ok: true, later: () => music.play(hit.key) };
      }
    }
  } catch (e: any) {
    return { label: `Couldn’t ${name}: ${e?.message ?? e}`, ok: false };
  }
  return { label: `Unknown action ${name}`, ok: false };
}

/** What Gilgamesh knows about right now: the time, the next week's calendar, the music. */
export async function situation(): Promise<string> {
  const now = new Date();
  const lines = [`NOW: ${now.toLocaleDateString('en-GB', { weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' })}, ${pad(now.getHours())}:${pad(now.getMinutes())}.`];
  const today = isoDay(now);
  // Small models are poor at date arithmetic: the next two weeks, spelled out.
  const days = Array.from({ length: 14 }, (_, i) => {
    const d = new Date(now.getFullYear(), now.getMonth(), now.getDate() + i);
    return `${i === 0 ? 'today ' : i === 1 ? 'tomorrow ' : ''}${d.toLocaleDateString('en-GB', { weekday: 'short' })} ${isoDay(d)}`;
  });
  lines.push(`DATES: ${days.join(', ')}.`);
  const week = isoDay(new Date(now.getTime() + 7 * 864e5));
  const events = (await calendarEvents())
    .filter((e) => e.date >= today && e.date <= week)
    .sort((a, b) => `${a.date} ${a.start ?? ''}`.localeCompare(`${b.date} ${b.start ?? ''}`))
    .slice(0, 12);
  lines.push(
    events.length
      ? `CALENDAR (next 7 days):\n${events.map((e) => `- ${dayLabel(e.date)}${e.start ? ' ' + e.start : ''}: ${e.title}`).join('\n')}`
      : 'CALENDAR: nothing in the next 7 days.',
  );
  const playing = musicEngine.status.state === 'playing' ? music.currentItem : null;
  if (playing) lines.push(`MUSIC: playing “${playing.title}”${playing.artist ? ` by ${playing.artist}` : ''}.`);
  return lines.join('\n');
}
