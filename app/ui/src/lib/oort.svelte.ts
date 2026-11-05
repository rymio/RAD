// Oort (Development → Oort): the space-fleet programming game at oort.rs, in RAD's Chromium
// engine (its own process and profile: engine "oort", app/src-tauri/src/browser.rs), with
// Gilgamesh beside it as a tutor (components/OortChat.svelte).
//
// He sees the game through the page itself: the scenario, the code in the player's editor
// (Oort's Monaco editor) and the compiler's output, read with a small script before every
// answer. His Rust goes into the editor with one click, and can be run there (the editor's run
// button: oort.rs compiles it on its own server, so playing needs the internet).
//
// What he knows of Oort comes from its docs, downloaded onto this computer (app/src-tauri/src/
// oort.rs): the oort_api reference, the tutorials and scenarios, the wiki. They're added to the
// vault (project "Oort") for RAG, and searched here for each question, so they work offline.

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { api, rad } from './api';
import { store } from './state.svelte';
import { activity } from './gilgamesh/observe.svelte';
import type * as T from './types';

export const OORT_HOME = 'https://oort.rs/';
const PROJECT = 'Oort';
const SETTINGS = '/v1/app-data/oort_settings';

export interface DocsStatus {
  version: string;
  fetched: number;
  files: string[];
  tutorials: number;
  scenarios: number;
  wiki: number;
  missing: string[];
  /** The Markdown files for RAG, on disk. */
  rag: string[];
  dir: string;
}

export interface Passage {
  file: string;
  heading: string;
  text: string;
  solution: boolean;
}

/** What's on the oort.rs page. */
export interface Game {
  url: string;
  scenario: string | null;
  code: string;
  /** The compiler's last words ("" when the code built). */
  compiler: string;
  status: string;
  /** The ship clicked on in the simulation. */
  picked: string;
}

/** The scenarios in the game's menu (oort.rs). */
export const SCENARIOS: [string, string][] = [
  ['welcome', 'Welcome'], ['tutorial_guns', 'Tutorial 1: Guns'], ['tutorial_acceleration', 'Tutorial 2: Acceleration'],
  ['tutorial_acceleration2', 'Tutorial 3: Acceleration #2'], ['tutorial_rotation', 'Tutorial 4: Rotation'], ['tutorial_lead', 'Tutorial 5: Lead'],
  ['tutorial_deflection', 'Tutorial 6: Deflection'], ['tutorial_radar', 'Tutorial 7: Radar'], ['tutorial_search', 'Tutorial 8: Search'],
  ['tutorial_radio', 'Tutorial 9: Radio'], ['tutorial_missiles', 'Tutorial 10: Missiles'], ['tutorial_squadron', 'Tutorial 11: Squadron'],
  ['tutorial_frigate', 'Tutorial 12: Frigate'], ['tutorial_cruiser', 'Tutorial 13: Cruiser'], ['gunnery', 'Gunnery'],
  ['cruiser_defense', 'Cruiser Defense'], ['planetary_defense', 'Planetary Defense'], ['race', 'Asteroid Race'], ['avoidance', 'Avoidance'],
  ['fighter_duel', 'Fighter Duel'], ['mini_fleet', 'Mini-Fleet'], ['frigate_duel', 'Frigate Duel'], ['cruiser_duel', 'Cruiser Duel'],
  ['asteroid_duel', 'Asteroid Duel'], ['squadrons', 'Squadrons'], ['fleet', 'Fleet'], ['belt', 'Belt'], ['orbit', 'Orbit'],
];
export const scenarioTitle = (id: string | null) => (id ? (SCENARIOS.find(([s]) => s === id)?.[1] ?? id.replace(/_/g, ' ')) : 'Oort');

/** Reads the page: the scenario, the player's code and the compiler's output. */
const READ_JS = `(() => {
  const models = (window.monaco && window.monaco.editor && window.monaco.editor.getModels()) || [];
  const text = (sel) => { const e = document.querySelector(sel); return e ? (e.innerText || '').trim() : ''; };
  const m = location.pathname.match(/scenario\\/([A-Za-z0-9_]+)/);
  const sel = document.querySelector('select');
  return JSON.stringify({
    url: location.href,
    scenario: m ? m[1] : (sel && sel.value) || null,
    code: models[0] ? models[0].getValue() : '',
    compiler: text('.compiler-output').replace(/^Compiler Output\\s*/, ''),
    status: text('.status'),
    picked: text('.picked'),
  });
})()`;

/** Puts code in the player's editor and, if asked, presses its run button (the handler is on
 *  the icon inside it: a click on the button's own div does nothing). */
const writeJs = (code: string, run: boolean) => `(() => {
  const models = (window.monaco && window.monaco.editor && window.monaco.editor.getModels()) || [];
  if (!models[0]) return 'no-editor';
  models[0].setValue(${JSON.stringify(code)});
  ${run ? "const b = document.querySelector('.run_button span') || document.querySelector('.run_button'); if (b) b.click(); else return 'no-run';" : ''}
  return 'ok';
})()`;

class OortState {
  docs = $state<DocsStatus | null>(null);
  docsLoaded = $state(false);
  downloading = $state<{ step: string; done: number; total: number } | null>(null);
  /** The docs version that's in the vault (RAG). */
  ragVersion = $state<string | null>(null);
  adding = $state(false);
  game = $state<Game | null>(null);
  /** The engine has the page open. */
  open = $state(false);

  private listening = false;
  private timer: ReturnType<typeof setInterval> | undefined;

  async load() {
    if (!this.listening) {
      this.listening = true;
      await listen<{ step: string; done: number; total: number }>('oort-docs', (e) => {
        if (this.downloading) this.downloading = e.payload.step === 'done' ? null : e.payload;
      });
    }
    const [docs, set] = await Promise.all([invoke<DocsStatus | null>('oort_docs_status').catch(() => null), api.get<any>(SETTINGS).catch(() => null)]);
    this.docs = docs;
    this.ragVersion = typeof set?.ragVersion === 'string' ? set.ragVersion : null;
    this.docsLoaded = true;
  }

  get inRag() {
    return !!this.docs && this.ragVersion === this.docs.version;
  }

  /** Downloads (or updates) the docs, then adds them to the vault. */
  async download() {
    if (this.downloading) return;
    this.downloading = { step: 'Starting', done: 0, total: 0 };
    try {
      this.docs = await invoke<DocsStatus>('oort_docs_download');
      const d = this.docs;
      store.toast('success', `Oort docs downloaded (oort_api ${d.version})`, `The API reference, ${d.tutorials} tutorials, ${d.scenarios} scenarios and ${d.wiki} wiki pages, on this computer.${d.missing.length ? ` ${d.missing.length} part${d.missing.length > 1 ? 's' : ''} couldn’t be fetched.` : ''}`);
    } catch (e) {
      store.error(e, 'Couldn’t download the Oort docs');
      return;
    } finally {
      this.downloading = null;
    }
    await this.addToRag().catch((e) => store.error(e, 'Couldn’t add the Oort docs to the vault'));
  }

  private async project(): Promise<T.Project> {
    const p = store.projects.find((p) => p.name === PROJECT);
    if (p) return p;
    const created = await api.post<T.Project>('/v1/projects', { name: PROJECT, color: '#2563eb', icon: 'rocket', description: 'The Oort game’s API reference, tutorials and wiki, for Gilgamesh and every chat' });
    store.projects = [...store.projects, created];
    return created;
  }

  /** Uploads the docs' Markdown to the vault's Oort project, where they're indexed (RAG). */
  async addToRag() {
    const d = this.docs;
    if (!d || this.adding) return;
    this.adding = true;
    try {
      const p = await this.project();
      // An update replaces the files of the version before.
      if (this.ragVersion && this.ragVersion !== d.version) {
        const docs = await api.get<T.Document[]>(`/v1/projects/${p.id}/documents`).catch(() => [] as T.Document[]);
        for (const x of docs) if (/^Oort (API reference|tutorials|scenarios|wiki)/.test(x.original_name)) await api.del(`/v1/documents/${x.id}`).catch(() => {});
      }
      const res = await rad.uploadPaths(p.id, d.rag);
      const failed = res.filter((r) => 'error' in r) as { error: string }[];
      if (failed.length === res.length) throw new Error(failed[0]?.error ?? 'Upload failed');
      this.ragVersion = d.version;
      await api.put(SETTINGS, { ragVersion: d.version }).catch(() => {});
      store.toast('success', 'Oort docs are in your vault', `In the Oort project; they’re being indexed, then Gilgamesh and every chat can use them.${failed.length ? ` ${failed.length} file${failed.length > 1 ? 's' : ''} failed.` : ''}`);
    } finally {
      this.adding = false;
    }
  }

  search(query: string, limit = 6, solutions = false) {
    return invoke<Passage[]>('oort_docs_search', { query, limit, solutions }).catch(() => [] as Passage[]);
  }

  // ---------------------------------------------------------------- the page

  /** The view has the game open (or not): keeps reading the page while it does. */
  attach(on: boolean) {
    this.open = on;
    clearInterval(this.timer);
    if (on) this.timer = setInterval(() => void this.refresh(), 4000);
    else activity.seen('oort', null);
  }

  async refresh(): Promise<Game | null> {
    if (!this.open) return this.game;
    try {
      const v = await rad.browserEval('oort', READ_JS);
      if (typeof v === 'string') this.game = JSON.parse(v) as Game;
    } catch {
      // The page is loading, or it's another site (a wiki link): keep what we had.
    }
    const g = this.game;
    activity.seen('oort', g ? `playing “${scenarioTitle(g.scenario)}”${g.compiler ? '; the code doesn’t compile' : ''}` : null);
    return g;
  }

  /** Puts code in the editor (and runs it). */
  async setCode(code: string, run: boolean) {
    if (!this.open) {
      store.toast('error', 'Oort isn’t open', 'Open Development → Oort first.');
      return false;
    }
    const r = await rad.browserEval('oort', writeJs(code.replace(/\n+$/, '\n'), run)).catch(() => 'failed');
    if (r !== 'ok') {
      store.toast('error', 'Couldn’t reach Oort’s editor', r === 'no-editor' ? 'Open a scenario in the game first.' : 'Try again once the page has loaded.');
      return false;
    }
    store.toast('success', run ? 'In the editor, running' : 'In the editor', run ? 'Oort compiles it, then the simulation starts.' : 'Press the run button (or ⌘↩) in the editor to try it.', 2500);
    setTimeout(() => void this.refresh(), run ? 6000 : 300);
    return true;
  }

  async run() {
    if (!this.open) return false;
    const r = await rad.browserEval('oort', `(() => { const b = document.querySelector('.run_button span') || document.querySelector('.run_button'); if (!b) return 'no'; b.click(); return 'ok'; })()`).catch(() => 'no');
    setTimeout(() => void this.refresh(), 6000);
    return r === 'ok';
  }

  /** What Gilgamesh gets with a question, while Oort is open. */
  async context(question: string): Promise<string> {
    if (store.route.kind !== 'oort') return '';
    const g = await this.refresh();
    const wantsAnswer = /\b(solution|solve|answer|stuck|hint|give up)\b/i.test(question);
    const [mine, hits] = await Promise.all([
      g?.scenario ? invoke<Passage[]>('oort_docs_scenario', { id: g.scenario, solution: wantsAnswer }).catch(() => [] as Passage[]) : Promise.resolve([] as Passage[]),
      this.search(`${question} ${g?.compiler.slice(0, 300) ?? ''}`, 6, wantsAnswer),
    ]);
    const seen = new Set(mine.map((p) => p.text));
    const passages = [...mine.filter((p) => !p.heading.startsWith('The scenario')), ...hits.filter((p) => !seen.has(p.text))];
    let budget = 11000;
    const docs: string[] = [];
    for (const p of passages) {
      const s = `### ${p.heading || p.file} (${p.file})\n${p.text}`;
      if (s.length > budget && docs.length) continue;
      docs.push(s.slice(0, budget));
      budget -= s.length;
      if (budget <= 0) break;
    }
    const code = g?.code ? (g.code.length > 9000 ? g.code.slice(0, 9000) + '\n…' : g.code) : '';
    const noDocs = !this.docs ? '\nThe Oort docs aren’t downloaded yet: answer from what you know, and tell the owner they can download them (the button above this chat) so you can use the API reference, tutorials and wiki.' : '';
    return `THE OWNER IS PLAYING OORT (oort.rs, Development → Oort): a programming game where Rust code flies a fleet of spaceships. Scenario: ${scenarioTitle(g?.scenario ?? null)}${g?.scenario ? ` (\`${g.scenario}\`)` : ''}.${g?.status ? ` Simulation: ${g.status}.` : ''}${g?.picked ? ` Selected ship: ${g.picked.replace(/\n/g, ', ')}.` : ''}
THEIR CODE (the player's editor):
\`\`\`rust
${code || '(empty, or the page is still loading)'}
\`\`\`
${g?.compiler ? `COMPILER OUTPUT (the last run failed to build):\n${g.compiler.slice(0, 3000)}\n` : ''}${docs.length ? `FROM THE OORT DOCS (oort_api ${this.docs?.version ?? ''}, the tutorials, the wiki):\n${docs.join('\n\n')}\n` : ''}${noDocs}
You are the owner's tutor for Oort and Rust. Explain the idea first (the physics, the maths, what the API call does), then the code. Use the oort_api as it is in the docs above; never invent functions. Ship code has a \`Ship\` struct with \`new()\` and \`tick(&mut self)\`, called 60 times a second, and starts with \`use oort_api::prelude::*;\`. When you give code, give the whole file in one fenced \`\`\`rust block, so it can replace the editor's code with one click (the owner can also run it). When it doesn't compile, explain the error plainly and give the fixed file. For a tutorial, give hints and the idea before a full solution, unless they ask for the solution. In speech, don't read code aloud; describe it. You can act on the game: [oort: run] runs the code in the editor; [oort: apply] puts the code block of your answer in the editor and runs it; [oort: open | scenario] opens a scenario (by its name in the game's menu or its id).`;
  }

  /** Where the game should go: a scenario by id or name. */
  scenarioUrl(what: string): string | null {
    const w = what.trim().toLowerCase();
    const hit = SCENARIOS.find(([id, title]) => id === w || title.toLowerCase() === w) ?? SCENARIOS.find(([id, title]) => title.toLowerCase().includes(w) || id.includes(w.replace(/\s+/g, '_')));
    return hit ? `${OORT_HOME}scenario/${hit[0]}` : null;
  }

  /** Opens a scenario in the game (the view picks it up). */
  pendingUrl = $state<string | null>(null);
  openScenario(what: string) {
    const url = this.scenarioUrl(what);
    if (!url) return null;
    this.pendingUrl = url;
    store.navigate({ kind: 'oort' });
    return url;
  }
}

export const oort = new OortState();

/** The last fenced Rust block of an answer. */
export function lastRustBlock(md: string): string | null {
  const blocks = [...md.matchAll(/```(?:rust|rs)?[ \t]*\n([\s\S]*?)```/g)].map((m) => m[1]);
  return blocks.at(-1) ?? null;
}
