// What Gilgamesh can do for the owner, through RAD's own features: remember facts (RAD's
// memories, shared with every chat), save notes (Inbox project), add calendar events, open views
// and web pages, search the web and control the music. Also what he knows about "now".

import { api } from '../api';
import { store } from '../state.svelte';
import { music, musicEngine } from '../media.svelte';
import { PROJECT_COLORS } from '../util';
import type * as T from '../types';
import type { ActionName } from './text';
import { monitor, siteName } from '../monitor.svelte';
import { gilgamesh } from './gilgamesh.svelte';

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
  games: { kind: 'games' }, monitoring: { kind: 'monitor' }, monitor: { kind: 'monitor' }, websites: { kind: 'monitor' }, rymnet: { kind: 'rymnet' }, rymwin: { kind: 'rymwin' }, zx: { kind: 'zx' }, spectrum: { kind: 'zx' },
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

async function calendar(): Promise<T.CalendarEvent[]> {
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
        const list = await calendar();
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
  const events = (await calendar())
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
