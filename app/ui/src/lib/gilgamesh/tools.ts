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
import { products } from '../products.svelte';
import { sight } from './sight.svelte';
import { photocraft } from '../photocraft.svelte';
import { books } from '../books.svelte';
import { chess, LEVELS, type Level } from '../chess.svelte';
import { scraper } from '../scraper.svelte';
import { colab, TEMPLATES, lastCodeBlock } from '../colab.svelte';
import { radio, PRESETS, WATCH, bandOf, demodFor, mhz, type Demod } from '../radio.svelte';
import { coach, practiceText, type CoachControls } from '../coach.svelte';
import { oort, lastRustBlock, scenarioTitle } from '../oort.svelte';
import { world } from '../world.svelte';
import { lab, CLOCKS } from '../z80lab.svelte';
import { PROGRAMS } from '../z80lab/templates';

/** How many times in a row Gilgamesh has run notebook code on his own (after seeing a result). */
let colabRounds = 0;
/** His notebook actions, one after another (a new notebook is made before its cells go in). */
let colabChain: Promise<void> = Promise.resolve();
/** Cells he ran that haven't been reported back to him yet. */
let colabWatched: string[] = [];

/** Once the cells he ran are done, tells him what came out, all in one go. */
function watchColab(ids: string[]) {
  const first = !colabWatched.length;
  colabWatched.push(...ids);
  if (!first) return;
  const nb = colab.nb;
  void (async () => {
    // The other runs of the same answer join in.
    await colabChain;
    await new Promise((r) => setTimeout(r, 100));
    let ids: string[] = [];
    while (ids.length !== colabWatched.length) {
      ids = [...colabWatched];
      await colab.settled(ids);
    }
    colabWatched = [];
    if (colab.nb !== nb || colabRounds >= 3) return;
    const r = colab.report(ids);
    if (!r) return;
    const failed = /FAILED/.test(r);
    // Not over the owner, nor over himself.
    for (let i = 0; i < 120 && ['hearing', 'thinking', 'speaking'].includes(gilgamesh.phase); i++) await new Promise((r) => setTimeout(r, 500));
    void gilgamesh.ask(
      `(The notebook, not the owner speaking: what you ran came back.\n${r}\nTell the owner what came out and what it means, the way a tutor would.${failed ? ' Something failed: explain why plainly and fix it with [colab: fix | cell number] after a corrected code block.' : ''})`,
      { quiet: true },
    );
  })();
}

/** The nth code block of an answer (counting from 1), else its last one for the language. */
function codeBlock(md: string, lang: 'python' | 'rust', nth: number): string | null {
  const all = [...md.matchAll(/```([^\n]*)\n([\s\S]*?)```/g)];
  const hit = nth > 0 ? all[nth - 1] : null;
  if (hit && (!hit[1].trim() || /^(python|py|python3|rust|rs)$/i.test(hit[1].trim()))) return hit[2];
  return lastCodeBlock(md, lang);
}

/** A title for a notebook he starts, from the first heading or line of his answer. */
function notebookTitle(md: string): string | undefined {
  const h = md.match(/^#+\s+(.+)$/m)?.[1] ?? md.replace(/```[\s\S]*?```/g, '').split(/[.!?\n]/).map((x) => x.replace(/\[[^\]]*\]/g, '').trim()).find((x) => x.length > 3);
  return h ? h.replace(/[*_`#]/g, '').slice(0, 60) : undefined;
}

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
  games: { kind: 'games' }, chess: { kind: 'chess' }, scraper: { kind: 'scraper' }, scrapper: { kind: 'scraper' }, colab: { kind: 'colab', lang: 'python' }, notebook: { kind: 'colab', lang: 'python' }, notebooks: { kind: 'colab', lang: 'python' }, jupyter: { kind: 'colab', lang: 'python' }, coach: { kind: 'coach' }, accent: { kind: 'coach' }, pronunciation: { kind: 'coach' }, radio: { kind: 'radio' }, sdr: { kind: 'radio' }, scanner: { kind: 'radio' }, radar: { kind: 'radio' }, adsb: { kind: 'radio' }, planes: { kind: 'radio' }, aircraft: { kind: 'radio' }, oort: { kind: 'oort' }, fyrox: { kind: 'fyrox' }, world: { kind: 'fyrox' }, z80lab: { kind: 'z80lab' }, z80: { kind: 'z80lab' }, lab: { kind: 'z80lab' }, bots: { kind: 'bots' }, bot: { kind: 'bots' }, products: { kind: 'products' }, product: { kind: 'products' }, books: { kind: 'books' }, book: { kind: 'books' }, library: { kind: 'books' }, photocraft: { kind: 'photocraft' }, pictures: { kind: 'photocraft' }, monitoring: { kind: 'monitor' }, monitor: { kind: 'monitor' }, websites: { kind: 'monitor' }, rymnet: { kind: 'rymnet' }, rymwin: { kind: 'rymwin' }, zx: { kind: 'zx' }, spectrum: { kind: 'zx' },
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
      case 'build': {
        // "what", "what | near what", "what | near what | name".
        const [what, near, name] = arg.split('|').map((x) => x.trim());
        if (!what) return { label: 'Build what?', ok: false };
        if (!world.running) return { label: 'Your world isn’t open', ok: false };
        world.send({ cmd: 'build', what, near: near || null, name: name || null, by: 'gilgamesh' });
        return { label: `Building ${name || what}`, ok: true };
      }
      case 'world': {
        // "go | place", "remove | name", "tablet | task; task".
        const [what, ...rest] = arg.split('|').map((x) => x.trim());
        const opt = rest.join(' | ').trim();
        const cmd = what.toLowerCase();
        if (cmd === 'open') return { label: 'Opening your world', ok: true, later: () => void world.start() };
        if (!world.running) return { label: 'Your world isn’t open', ok: false };
        if (cmd === 'go' || cmd === 'goto' || cmd === 'walk') {
          world.send({ cmd: 'goto', place: opt });
          return { label: `Walking to ${opt}`, ok: true };
        }
        if (cmd === 'remove') {
          world.send({ cmd: 'remove', name: opt });
          return { label: `Taking away ${opt}`, ok: true };
        }
        if (cmd === 'tablet' || cmd === 'tasks') {
          world.setTasks(opt.split(/;|\n/));
          return { label: 'Wrote the tasks on the clay tablet', ok: true };
        }
        return { label: `Don’t know the world action “${what}”`, ok: false };
      }
      case 'oort': {
        // "run", "apply" (the code block of this answer) or "open | scenario".
        const [what, ...rest] = arg.split('|').map((x) => x.trim());
        const cmd = what.toLowerCase();
        if (cmd === 'open') {
          const name = rest.join(' ').trim();
          const url = oort.scenarioUrl(name);
          if (!url) return { label: `No Oort scenario “${name}”`, ok: false };
          return { label: `Opening ${scenarioTitle(url.split('/').pop() ?? null)}`, ok: true, later: () => void oort.openScenario(name) };
        }
        if (!oort.open || store.route.kind !== 'oort') return { label: 'Oort isn’t open', ok: false };
        if (cmd === 'run') return { label: 'Running the code', ok: true, later: () => void oort.run() };
        if (cmd === 'apply') {
          // Done once he has finished: by then his whole answer (with its code block) is in.
          return {
            label: 'Code in the editor, running',
            ok: true,
            later: () => {
              const said = [...gilgamesh.turns].reverse().find((t) => t.who === 'gilgamesh')?.text ?? '';
              const code = lastRustBlock(said);
              if (code) void oort.setCode(code, true);
              else store.toast('error', 'No code to put in the editor', 'Gilgamesh’s answer had no Rust code block.');
            },
          };
        }
        return { label: `Don’t know the Oort action “${what}”`, ok: false };
      }
      case 'radio': {
        // "listen | 98.0 [| fm]", "tune | 121.5", "mute", "scan | fm", "aircraft", "packets", "off".
        const [what, ...rest] = arg.split('|').map((x) => x.trim());
        const cmd = what.toLowerCase();
        const opt = rest.join(' ').trim();
        const go = () => {
          if (store.route.kind !== 'radio') store.navigate({ kind: 'radio' });
        };
        if (cmd === 'listen' || cmd === 'tune' || cmd === 'play') {
          // "98.0", "98.0 MHz", "121.5 am", "1090"; a number under 2000 is MHz.
          const m = opt.match(/(\d+(?:\.\d+)?)\s*(mhz|khz|hz)?/i);
          if (!m) return { label: `No frequency in “${opt}”`, ok: false };
          let f = parseFloat(m[1]);
          const unit = (m[2] ?? '').toLowerCase();
          f = unit === 'khz' ? f * 1e3 : unit === 'hz' ? f : f < 2000 ? f * 1e6 : f;
          if (f < 0.5e6 || f > 1766e6) return { label: `${mhz(f)} is outside what the dongle hears (0.5–1766 MHz)`, ok: false };
          const d = /\bwfm\b|\bfm\b|broadcast/i.test(opt) ? (/\bnfm\b/i.test(opt) ? 'nfm' : 'wfm') : /\bnfm\b|narrow/i.test(opt) ? 'nfm' : /\bam\b/i.test(opt) ? 'am' : demodFor(f);
          const listen = cmd !== 'tune';
          const band = bandOf(f);
          return { label: `${listen ? 'Listening to' : 'Tuning to'} ${mhz(f)}${band ? ` (${band.name})` : ''}`, ok: true, later: () => { go(); void radio.tune(f, d as Demod, listen); } };
        }
        if (cmd === 'mute' || cmd === 'stop listening' || cmd === 'quiet') return { label: 'Radio muted', ok: true, later: () => void radio.listen(false) };
        if (cmd === 'scan') {
          const o = opt.toLowerCase();
          const range = o.match(/(\d+(?:\.\d+)?)\s*[-–to]+\s*(\d+(?:\.\d+)?)/);
          const key = /air/.test(o) ? 'air' : /marine|ship|boat/.test(o) ? 'marine' : /70\s*cm|uhf ham/.test(o) ? '70cm' : /2\s*m|ham|amateur/.test(o) ? '2m' : /pmr|433|ism/.test(o) ? 'pmr' : /everything|all|whole/.test(o) ? 'all' : /vhf/.test(o) ? 'vhf' : 'fm';
          const p = PRESETS.find((x) => x.id === key)!;
          const [from, to] = range ? [parseFloat(range[1]) * 1e6, parseFloat(range[2]) * 1e6] : [p.from, p.to];
          return { label: `Scanning ${mhz(from)}–${mhz(to)}`, ok: true, later: () => { go(); void radio.scan(from, to); } };
        }
        if (cmd === 'watch' || cmd === 'subghz' || cmd === 'drones' || cmd === 'lora' || cmd === 'meshtastic' || cmd === 'identify') {
          // "watch | 433.92", "watch | meshtastic", "watch | 868", "watch | fpv", or just "drones".
          const o = `${cmd} ${opt}`.toLowerCase();
          const num = opt.match(/(\d+(?:\.\d+)?)/);
          const named = /mesh/.test(o) ? (/us|america/.test(o) ? 'mesh-us' : /anz|austral|zealand/.test(o) ? 'mesh-anz' : /asia|923|as923|ph|sg/.test(o) ? 'as923' : 'mesh-eu') : /fpv|video|1\.2|1\.3/.test(o) ? 'fpv' : /315/.test(o) ? '315' : /868|869|eu/.test(o) ? '868' : /91[0-9]|902|928|us/.test(o) ? '915' : /92[0-5]/.test(o) ? 'as923' : null;
          let f = named ? WATCH.find((w) => w.id === named)!.freq : num ? parseFloat(num[1]) : radio.settings.watch;
          if (f < 2000) f *= 1e6;
          if (f < 24e6 || f > 1766e6) return { label: `${mhz(f)} is outside what the dongle hears`, ok: false };
          return { label: `Watching ${mhz(f)} for transmissions`, ok: true, later: () => { go(); void radio.watch(f); } };
        }
        if (cmd === 'aircraft' || cmd === 'adsb' || cmd === 'radar' || cmd === 'planes') return { label: 'Aircraft radar on (1090 MHz)', ok: true, later: () => { go(); void radio.radar(); } };
        if (cmd === 'packets') return { label: 'Packet analyser', ok: true, later: () => { go(); radio.tab = 'packets'; if (radio.mode !== 'adsb' || !radio.on) void radio.radar().then(() => (radio.tab = 'packets')); } };
        if (cmd === 'off' || cmd === 'stop') return { label: radio.on ? 'Radio off' : 'The radio is already off', ok: radio.on, later: () => void radio.stop() };
        if (cmd === 'on' || cmd === 'open') return { label: 'Radio on', ok: true, later: () => { go(); void radio.start(); } };
        return { label: `Don’t know the radio action “${what}”`, ok: false };
      }
      case 'coach': {
        // "listen", "next", "previous", "sentence | 3", "hear", "vowels", "melody", "open | title",
        // "practise | title" (the sentences are the fenced block of his answer).
        const [what, ...rest] = arg.split('|').map((x) => x.trim());
        const cmd = what.toLowerCase();
        const opt = rest.join(' | ').trim();
        // Opens Coach if need be and waits for it to be ready.
        const ready = async () => {
          if (store.route.kind !== 'coach') store.navigate({ kind: 'coach' });
          for (let i = 0; i < 100 && !(coach.controls && coach.ready); i++) await new Promise((r) => setTimeout(r, 100));
          return coach.controls;
        };
        if (cmd === 'practise' || cmd === 'practice' || cmd === 'add' || cmd === 'new') {
          const title = opt || 'Practice from Gilgamesh';
          return {
            label: `New practice text: ${title}`,
            ok: true,
            // Once his answer is all in: its fenced block (or list) is the text.
            later: async () => {
              const said = [...gilgamesh.turns].reverse().find((t) => t.who === 'gilgamesh')?.text ?? '';
              const body = practiceText(said);
              if (!body) {
                store.toast('error', 'No sentences to practise', 'Gilgamesh’s answer had no fenced block or list of sentences.');
                return;
              }
              const c = await ready();
              if (c) await c.addText(title.slice(0, 80), body);
            },
          };
        }
        if (cmd === 'open') {
          if (!opt) return { label: 'Which text?', ok: false };
          return {
            label: `Opening “${opt}” in Coach`,
            ok: true,
            later: async () => {
              const c = await ready();
              if (c && !(await c.openByTitle(opt))) store.toast('error', 'No such text in Coach', `Nothing called “${opt}”.`);
            },
          };
        }
        const simple: Record<string, [string, (c: CoachControls) => unknown]> = {
          listen: ['Playing the reference reading', (c) => c.listen()],
          play: ['Playing the reference reading', (c) => c.listen()],
          next: ['Next sentence', (c) => c.goTo(coach.index + 1)],
          previous: ['Previous sentence', (c) => c.goTo(coach.index - 1)],
          back: ['Previous sentence', (c) => c.goTo(coach.index - 1)],
          hear: ['Playing back their reading', (c) => c.hearMe()],
          vowels: ['Showing the vowel chart', (c) => c.setView('vowels')],
          melody: ['Showing the melody', (c) => c.setView('melody')],
          page: ['Whole page', (c) => c.setMode('page')],
          sentences: ['Sentence by sentence', (c) => c.setMode('sentence')],
        };
        if (cmd === 'sentence' || cmd === 'go') {
          const n = parseInt(opt, 10);
          if (!Number.isFinite(n) || n < 1) return { label: `No sentence number in “${opt}”`, ok: false };
          return { label: `Sentence ${n}`, ok: true, later: async () => { const c = await ready(); await c?.setMode('sentence'); await c?.goTo(n - 1); } };
        }
        const hit = simple[cmd];
        if (hit) return { label: hit[0], ok: true, later: async () => { const c = await ready(); if (c) await hit[1](c); } };
        return { label: `Don’t know the Coach action “${what}”`, ok: false };
      }
      case 'colab': {
        // "run", "run all", "stop", "restart", "new | python" or "open | title".
        const [what, ...rest] = arg.split('|').map((x) => x.trim());
        const cmd = what.toLowerCase();
        const opt = rest.join(' ').trim();
        if (cmd === 'new') {
          const lang = /rust/i.test(opt) ? 'rust' : 'python';
          const t = TEMPLATES.find((t) => t.lang === lang && t.id.endsWith('blank'));
          return { label: `New ${lang === 'rust' ? 'Rust' : 'Python'} notebook`, ok: true, later: () => void colab.create(lang, t) };
        }
        if (cmd === 'open') {
          await Promise.all([colab.loadList('python'), colab.loadList('rust')]);
          const w = opt.toLowerCase();
          const n = [...colab.lists.python, ...colab.lists.rust].find((x) => x.title.toLowerCase().includes(w));
          if (!n) return { label: `No notebook “${opt}”`, ok: false };
          return { label: `Opening ${n.title}`, ok: true, later: () => void colab.open(n.id) };
        }
        // Runs he starts are reported back to him, so he sees what came out (and can fix it).
        // Fixing on his own stops after a few rounds; then it's the owner's turn.
        const lastYou = [...gilgamesh.turns].reverse().find((t) => t.who === 'you');
        colabRounds = lastYou?.quiet ? colabRounds + 1 : 0;
        if (cmd === 'add' || cmd === 'add and run' || cmd === 'insert' || cmd === 'fix' || cmd === 'replace') {
          const fix = cmd === 'fix' || cmd === 'replace';
          const run = fix || cmd === 'add and run' || /\brun\b/i.test(opt);
          // The code block written just before this tag (several cells can come in one answer).
          const sofar = [...gilgamesh.turns].reverse().find((t) => t.who === 'gilgamesh')?.text ?? '';
          const nth = [...sofar.matchAll(/```[^\n]*\n[\s\S]*?```/g)].length;
          let nb = colab.nb && store.route.kind === 'colab' ? colab.nb : null;
          if (fix && !nb) return { label: 'No notebook is open in Colab', ok: false };
          const n = parseInt(opt, 10);
          const target = fix ? (Number.isFinite(n) ? nb!.cells[n - 1] : colab.cell(colab.selected)) : null;
          if (fix && !target) return { label: Number.isFinite(n) ? `No cell ${n}` : 'No cell is selected', ok: false };
          // With no notebook open, the code starts a new one (Python unless he says Rust).
          const lang: 'python' | 'rust' = nb?.lang ?? (/rust/i.test(opt) ? 'rust' : 'python');
          return {
            label: fix ? `Fixing cell ${colab.index(target!.id) + 1}` : `${nb ? '' : `New ${lang === 'rust' ? 'Rust' : 'Python'} notebook: `}${run ? 'adding a cell and running it' : 'adding a cell'}`,
            ok: true,
            // Done once he has finished: by then his whole answer (with its code blocks) is in.
            later: () => {
              colabChain = colabChain.then(async () => {
                const said = [...gilgamesh.turns].reverse().find((t) => t.who === 'gilgamesh')?.text ?? '';
                const code = codeBlock(said, lang, nth);
                if (!code) {
                  store.toast('error', 'No code to put in the notebook', `Gilgamesh’s answer had no ${lang === 'rust' ? 'Rust' : 'Python'} code block.`);
                  return;
                }
                let c;
                if (!colab.nb || store.route.kind !== 'colab' || colab.nb.lang !== lang) {
                  const t = TEMPLATES.find((t) => t.lang === lang && t.id.endsWith('blank'));
                  await colab.create(lang, t, notebookTitle(said));
                  // The blank notebook's empty cell takes the code.
                  const empty = colab.nb?.cells.find((x) => x.type === 'code' && !x.source.trim());
                  c = empty ? colab.replaceCode(empty.id, code) : colab.insertCode(code);
                } else c = fix ? colab.replaceCode(target!.id, code) : colab.insertCode(code);
                if (c && run) {
                  colab.run(c.id);
                  watchColab([c.id]);
                }
              }).catch((e) => store.error(e, 'Couldn’t put the code in the notebook'));
            },
          };
        }
        const nb = colab.nb;
        if (!nb || store.route.kind !== 'colab') return { label: 'No notebook is open in Colab', ok: false };
        if (cmd === 'run all') {
          return { label: 'Running all the cells', ok: true, later: () => { colab.runAll(); watchColab(nb.cells.filter((c) => c.type === 'code').map((c) => c.id)); } };
        }
        if (cmd === 'run') {
          const c = colab.cell(colab.selected);
          if (!c) return { label: 'No cell is selected', ok: false };
          return { label: `Running cell ${colab.index(c.id) + 1}`, ok: true, later: () => { colab.run(c.id); watchColab([c.id]); } };
        }
        if (cmd === 'stop') return { label: colab.queue.length ? 'Stopping the cell' : 'Nothing is running', ok: !!colab.queue.length, later: () => colab.interrupt() };
        if (cmd === 'restart') return { label: 'Restarting the kernel', ok: true, later: () => void colab.restart() };
        return { label: `Don’t know the notebook action “${what}”`, ok: false };
      }
      case 'scrape': {
        // "https://site", "https://site | rag", "stop" or "open | site".
        const [what, ...rest] = arg.split('|').map((x) => x.trim());
        const opt = rest.join(' ').toLowerCase();
        await scraper.load();
        if (/^stop$/i.test(what)) {
          for (const s of scraper.running) void scraper.stop(s.id);
          return { label: scraper.running.length ? 'Stopped saving' : 'Nothing is being saved', ok: !!scraper.running.length };
        }
        if (/^open$/i.test(what)) {
          const s = scraper.list.find((x) => `${x.title} ${x.url}`.toLowerCase().includes(opt));
          if (!s) return { label: `No saved site “${opt}”`, ok: false };
          return { label: `Opening ${s.title}`, ok: true, later: () => void scraper.open(s.id) };
        }
        if (!/^[\w.-]+\.[a-z]{2,}|^https?:\/\//i.test(what)) return { label: `“${what}” isn’t a web address`, ok: false };
        try {
          const s = await scraper.start(what, scraper.options, undefined, /rag|vault/.test(opt));
          return { label: `Saving ${s.host}${/rag|vault/.test(opt) ? ', then adding it to RAG' : ''}`, ok: true, later: () => store.navigate({ kind: 'scraper' }) };
        } catch (e: any) {
          return { label: `Couldn’t save ${what}: ${e?.message ?? e}`, ok: false };
        }
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
      case 'product':
        return await products.fromGilgamesh(arg);
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
