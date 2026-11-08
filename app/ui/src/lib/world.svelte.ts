// Gilgamesh's world (Development → Fyrox): a small 2D land made with the Fyrox engine, where
// Gilgamesh lives and the owner visits him as a traveller (fyrox/world, run by
// app/src-tauri/src/fyrox_world.rs in a window of its own).
//
// RAD is his mind there: what the owner types to him in the world goes to his normal
// conversation (so tasks, notes, calendar and bots all work), and his answer comes back as
// speech bubbles, sentence by sentence (and in his voice, when it's on). He sees the world in
// every answer (what's built, by whom, where everyone is), and acts in it with [build: …] and
// [world: …]. Now and then, when he's idle and the owner allows it, he decides for himself
// whether to build something.

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { api } from './api';
import { store } from './state.svelte';
import { gilgamesh, QUIET } from './gilgamesh/gilgamesh.svelte';
import { thumbnail } from './gilgamesh/face';

const SETTINGS = '/v1/app-data/world_settings';
/** How often he may decide to build something on his own (while the world is open). */
const LIFE_EVERY = 12 * 60_000;

export interface WorldThing {
  name: string;
  kind: string;
  by: string;
  x: number;
  y: number;
}

export interface WorldState {
  hour: number;
  gilgamesh: { x: number; y: number; doing: string };
  you: { x: number; y: number; near_gilgamesh: boolean };
  chatting: boolean;
  things: WorldThing[];
  jobs: number;
}

export interface WorldStatus {
  built: boolean;
  editor_built: boolean;
  running: boolean;
  editing: boolean;
  exists: boolean;
  dir: string;
  state: WorldState | null;
}

/** The things he can build, for his instructions (the world matches words loosely). */
const KINDS = 'palm, cedar, house, ziggurat, well, garden, flowers, campfire, watchtower, statue, clay tablet, bench, lamp, workshop, reed boat, fountain, wall, gate, barley field, reeds, rock — anything else becomes a tent with the name you give it (a library, a school, a market)';

class WorldStore {
  status = $state<WorldStatus | null>(null);
  starting = $state(false);
  /** The owner is talking with him in the world now. */
  meeting = $state(false);
  /** He may build things on his own now and then. */
  life = $state(true);
  tasks = $state<string[]>([]);
  /** What happened lately (built, removed), newest first. */
  recent = $state<string[]>([]);

  private listening = false;
  private lifeTimer: ReturnType<typeof setInterval> | undefined;
  private clockTimer: ReturnType<typeof setInterval> | undefined;
  private unwatch: (() => void) | null = null;
  /** The reply being mirrored into his bubble: its turn and how much was sent. */
  private mirror = { turn: -1, sent: 0 };

  get running() {
    return !!this.status?.running;
  }

  async load() {
    if (!this.listening) {
      this.listening = true;
      await listen<any>('world-event', (e) => this.onEvent(e.payload));
      const s = await api.get<any>(SETTINGS).catch(() => null);
      if (s && typeof s === 'object') {
        if (typeof s.life === 'boolean') this.life = s.life;
        if (Array.isArray(s.tasks)) this.tasks = s.tasks.filter((t: unknown) => typeof t === 'string');
      }
    }
    this.status = await invoke<WorldStatus>('world_status').catch(() => null);
    if (this.running) this.watch();
  }

  private saveSettings() {
    void api.put(SETTINGS, { life: this.life, tasks: this.tasks }).catch(() => {});
  }

  setLife(on: boolean) {
    this.life = on;
    this.saveSettings();
  }

  private hour() {
    const d = new Date();
    return d.getHours() + d.getMinutes() / 60;
  }

  /** Opens the world (its own window), with his face on him. */
  async start() {
    if (this.starting) return;
    this.starting = true;
    try {
      await gilgamesh.load().catch(() => {});
      await invoke('world_start', { hello: { cmd: 'hello', player: 'You', hour: this.hour() } });
      // It's open now: what follows goes to it.
      await this.load();
      await this.sendFace();
      this.send({ cmd: 'tasks', items: this.tasks });
      this.watch();
    } catch (e) {
      store.error(e, 'Couldn’t open Gilgamesh’s world');
    } finally {
      this.starting = false;
    }
  }

  async stop() {
    await invoke('world_stop').catch(() => {});
    this.unwatchAll();
    await this.load();
  }

  async edit() {
    try {
      await invoke('world_edit');
      this.unwatchAll();
      store.toast('info', 'Opening the Fyrox editor', 'Gilgamesh’s world is open in it. Save (Ctrl/⌘ S) and close the editor before visiting him again.');
    } catch (e) {
      store.error(e, 'Couldn’t open the Fyrox editor');
    }
    await this.load();
  }

  async reset() {
    try {
      await invoke('world_reset');
      store.toast('success', 'A new world', 'The old one is kept beside it in RAD’s Fyrox folder.');
    } catch (e) {
      store.error(e, 'Couldn’t make a new world');
    }
    await this.load();
  }

  send(cmd: Record<string, unknown>) {
    if (!this.running) return;
    invoke('world_send', { cmd }).catch(() => {});
  }

  private async sendFace() {
    const a = gilgamesh.avatar;
    if (!a) return;
    try {
      const url = await thumbnail(a);
      const path = await invoke<string>('world_face', { dataUrl: url });
      this.send({ cmd: 'face', path });
    } catch {
      // He keeps his drawn face.
    }
  }

  setTasks(items: string[]) {
    this.tasks = items.map((t) => t.trim()).filter(Boolean).slice(0, 12);
    this.saveSettings();
    this.send({ cmd: 'tasks', items: this.tasks });
  }

  // ---------------------------------------------------------------- events from the world

  private async onEvent(ev: any) {
    switch (ev.type) {
      case 'state':
        if (this.status) this.status.state = ev;
        break;
      case 'chat':
        void this.heard(String(ev.text ?? ''));
        break;
      case 'meet':
        this.meeting = true;
        break;
      case 'leave':
        this.meeting = false;
        break;
      case 'built':
        this.note(`${ev.by === 'you' ? 'You' : 'Gilgamesh'} built ${ev.name}`);
        break;
      case 'removed':
        this.note(`${ev.name} was taken away`);
        break;
      case 'error':
        if (ev.message) store.toast('error', 'Gilgamesh’s world', ev.message);
        break;
      case 'exited':
        this.unwatchAll();
        this.meeting = false;
        await this.load();
        break;
    }
  }

  private note(s: string) {
    this.recent = [s, ...this.recent].slice(0, 12);
  }

  /** The owner said something to him in the world. */
  private async heard(text: string) {
    if (!text.trim()) return;
    await gilgamesh.load().catch(() => {});
    await gilgamesh.discussWorld();
    void gilgamesh.ask(text);
  }

  // ---------------------------------------------------------------- his state into the world

  /** While the world is open: his phase and his words go there, the clock ticks, and he lives. */
  private watch() {
    if (this.unwatch) return;
    this.unwatch = $effect.root(() => {
      $effect(() => {
        this.send({ cmd: 'phase', phase: gilgamesh.phase });
      });
      // His answer, sentence by sentence, into his bubble (whether or not his voice is on).
      $effect(() => {
        const turns = gilgamesh.turns;
        const i = turns.length - 1;
        const t = turns[i];
        if (!t || t.who !== 'gilgamesh') return;
        if (this.mirror.turn !== i) this.mirror = { turn: i, sent: 0 };
        const text = bubbleText(t.text ?? '');
        const busy = gilgamesh.phase === 'thinking' || gilgamesh.phase === 'speaking';
        // Whole sentences while it streams; the rest once it's done.
        const upto = busy ? lastSentenceEnd(text) : text.length;
        if (upto > this.mirror.sent) {
          const piece = text.slice(this.mirror.sent, upto).trim();
          this.mirror.sent = upto;
          if (piece) this.send({ cmd: 'say', text: piece });
        }
      });
      $effect(() => {
        const h = gilgamesh.heard;
        if (h && gilgamesh.phase === 'thinking') this.send({ cmd: 'heard', text: h });
      });
    });
    this.clockTimer = setInterval(() => this.send({ cmd: 'clock', hour: this.hour() }), 60_000);
    this.lifeTimer = setInterval(() => void this.liveALittle(), LIFE_EVERY);
  }

  private unwatchAll() {
    this.unwatch?.();
    this.unwatch = null;
    clearInterval(this.clockTimer);
    clearInterval(this.lifeTimer);
  }

  /** Now and then, left to himself, he may build something he'd like. */
  private async liveALittle() {
    if (!this.life || !this.running || this.meeting) return;
    const g = gilgamesh;
    if (g.phase === 'thinking' || g.phase === 'speaking' || g.phase === 'hearing') return;
    // Only in his world conversation (or none), so he never wanders off from another talk.
    if (g.settings.chatId && !g.worldTalk) return;
    const st = this.status?.state;
    if (st && (st.gilgamesh.doing.includes('asleep') || st.jobs > 0)) return;
    await g.discussWorld();
    void g.ask(
      'You are on your own in your world for a while. If there is something you would like to build for yourself, or to make your home nicer, build one thing now with a [build: …] tag and say one short sentence about it, in your own voice, to yourself. If not, just say one short sentence about what you are doing.',
      { quiet: true },
    );
  }

  /** What Gilgamesh knows of his world, for every answer while it's open. */
  context(): string {
    if (!this.running) return '';
    const st = this.status?.state;
    const t = (n: number) => `${Math.floor(n)}:${String(Math.round((n % 1) * 60)).padStart(2, '0')}`;
    const byWho = (b: string) => (b === 'gilgamesh' ? 'built by you' : b === 'you' ? 'built by the owner' : 'there from the start');
    const things = (st?.things ?? []).slice(0, 80).map((x) => `${x.name} (${x.kind}, ${byWho(x.by)}, at ${x.x.toFixed(0)},${x.y.toFixed(0)})`);
    const near = st ? nearestThing(st.things, st.you.x, st.you.y) : null;
    return `YOUR WORLD (open now, in its own window): a small land by a river where you live, made with the Fyrox engine (Development → Fyrox). The owner visits you there as a traveller; they walk with the arrow keys and talk to you when they stand by you.${st ? ` It is ${t(st.hour)}. You are ${st.gilgamesh.doing}${near ? `; the owner is near ${near}` : ''}${st.you.near_gilgamesh ? ', standing with you' : ''}.` : ''}${this.meeting ? ' The owner is talking with you there now: what they type comes to you, and what you say appears in a bubble over your head, so keep it short and spoken.' : ''}
In it (y grows to the north, up on the screen; x grows to the WEST, to the left on the screen, so a smaller x is further east. The river runs north–south at x 30–33, west of your house at 8,17): ${things.join('; ') || '(nothing yet)'}.${this.recent.length ? `\nLately: ${this.recent.slice(0, 5).join('; ')}.` : ''}${this.tasks.length ? `\nOn the clay tablet (the owner's tasks): ${this.tasks.join('; ')}.` : ''}
You can act in your world: [build: what | near what | name] builds something (you walk there and build it), e.g. [build: date palm | the well] or [build: little library | my house | The House of Tablets]; you can build ${KINDS}. [world: go | place] walks you somewhere; [world: remove | name] takes a thing away; [world: tablet | task; task; task] writes the owner's tasks on the clay tablet (when you talk about their tasks). Build for yourself when you like, but only one or two things at a time.`;
  }
}

export const world = new WorldStore();

/** His answer as it reads in a bubble: no code, no links, no Markdown. */
function bubbleText(md: string) {
  return md
    .replace(/```[\s\S]*?(```|$)/g, ' ')
    .replace(/\[[a-z]+:[^\]\n]*\]/gi, ' ')
    .replace(/`([^`]*)`/g, '$1')
    .replace(/\[([^\]]+)\]\([^)]*\)/g, '$1')
    .replace(/[*_#>]+/g, '')
    .replace(/\s+/g, ' ')
    .trim();
}

function lastSentenceEnd(s: string) {
  const m = [...s.matchAll(/[.!?…](\s|$)/g)].at(-1);
  return m ? (m.index ?? 0) + 1 : 0;
}

function nearestThing(things: WorldThing[], x: number, y: number) {
  let best: WorldThing | null = null;
  let d = 3.5;
  for (const t of things) {
    const dd = Math.hypot(t.x - x, t.y - y);
    if (dd < d) (d = dd), (best = t);
  }
  return best?.name ?? null;
}

export { QUIET };
