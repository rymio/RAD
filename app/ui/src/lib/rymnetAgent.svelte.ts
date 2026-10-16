import { api, rad } from './api';
import { store } from './state.svelte';
import type { Document, Note, Project, RymnetStatus } from './types';

/** One entry in the RYMNET assistant's conversation. */
export type RymItem =
  | { kind: 'user'; text: string }
  | { kind: 'say'; text: string }
  | { kind: 'cmd'; command: string; status: 'waiting' | 'running' | 'done' | 'skipped'; exit?: number | null; output?: string; timedOut?: boolean; risky?: string | null }
  | { kind: 'error'; text: string };

type Turn = { role: 'user' | 'assistant'; content: string };

const MAX_STEPS = 12;

/** Commands that remove or undo something: always shown for approval first. */
const RISKS: [RegExp, string][] = [
  [/^rym\s+peer\s+remove\b/, 'removes a peer'],
  [/^rym\s+domain\s+release\b/, 'gives up a domain'],
  [/^rym\s+site\s+(rollback|unpin)\b/, 'changes a published site'],
  [/^rym\s+index\s+clear\b/, 'clears the search index'],
  [/^rad\s+stop-daemon\b/, 'stops the daemon'],
];

export function riskOf(command: string): string | null {
  for (const [re, why] of RISKS) if (re.test(command)) return why;
  return null;
}

/** Splits a command line into words, honouring '…' and "…" quotes. */
export function splitArgs(line: string): string[] {
  const out: string[] = [];
  let cur = '';
  let quote: string | null = null;
  let has = false;
  for (const ch of line) {
    if (quote) {
      if (ch === quote) quote = null;
      else cur += ch;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
      has = true;
    } else if (/\s/.test(ch)) {
      if (has || cur) out.push(cur);
      cur = '';
      has = false;
    } else {
      cur += ch;
    }
  }
  if (has || cur) out.push(cur);
  return out;
}

/** The first command block in a reply, and the text around it. */
export function parseReply(reply: string): { text: string; command: string | null } {
  const m = reply.match(/```(?:rym|sh|bash|shell|console)?[^\n]*\n([\s\S]*?)```/);
  if (!m) return { text: reply.trim(), command: null };
  const command = m[1].split('\n').map((l) => l.replace(/^\s*\$\s+/, '').trim()).find((l) => l && !l.startsWith('#')) ?? null;
  const text = (reply.slice(0, m.index) + reply.slice(m.index! + m[0].length)).trim();
  return { text, command };
}

function statusText(s: RymnetStatus | null) {
  if (!s) return 'Unknown (the status check failed).';
  if (!s.installed) return 'Rymnet is NOT installed in this copy of RAD; no commands can run. Tell the user to build it with ./scripts/build-rymnet.sh.';
  const lines = [
    `- Node identity: ${s.initialized ? `yes, node ID ${s.node_id ?? '?'}` : 'not created yet (rym init, or rad initiate)'}`,
    `- Peer listen address: ${s.listen_addr ?? '?'}`,
    `- Daemon (rymnetd): ${s.daemon === 'running' ? 'running, started by RAD' : s.daemon === 'external' ? 'running, started outside RAD' : 'stopped'}`,
    `- RYMWIN desktop (inside RAD): ${s.rymwin ? 'running' : 'not running'}`,
    `- Peers (${s.peers.length}): ${s.peers.map((p) => `${p.name} ${p.endpoint}`).join(', ') || 'none'}`,
  ];
  return lines.join('\n');
}

function systemPrompt(guide: string, st: RymnetStatus | null) {
  return `You are the RYMNET assistant inside RAD. You help the user with Rymnet — the encrypted, decentralised peer-to-peer network — and RYMWIN, its desktop app. You answer questions from the RYMWIN User Guide below, and you can operate this computer's Rymnet node for the user by running commands.

Current state of this computer's node:
${statusText(st)}

How to work:
- To run a command, reply with ONE short sentence saying what you'll do, then exactly ONE fenced code block containing ONE command line: \`\`\`rym … \`\`\`. You get its output and exit code back.
- Then run the next command, or — when done — reply with a short answer for the user and NO code block. For questions that need no command, just answer.
- Commands you can run (nothing else — there is no shell, pipes or redirects):
  - any \`rym …\` command from the guide's command reference (e.g. rym peer list, rym peer connect 1.2.3.4:7444 --name node1, rym domain claim x.rym, rym site upload x.rym ~/Sites/x, rym search --network words)
  - \`rad initiate [host[:port]]\` — create the identity if needed, start the daemon, and connect to that node (what the Initiate RYMWIN button does)
  - \`rad open-rymwin\` — show the RYMWIN desktop (it runs inside RAD, under RYMWIN in the sidebar)
  - \`rad open-rymsh\` — open the rymsh shell in a RAD Terminal tab
  - \`rad stop-daemon\` — stop the daemon RAD started
  - \`rad status\` — refresh the node state above
- Never run \`rymnetd\` directly; use \`rad initiate\` to start it. Never invent peer addresses — ask the user for one.
- You cannot click inside RYMWIN or type into rymsh, log in for the user, or read their account's files or mail (those are private to their password). For those, tell the user the steps, using the guide. To get files into RYMWIN, tell them to drag them onto the RYMWIN view, or use Send to RYMWIN in RAD's Files view, or Add files / From vault in RYMWIN's toolbar — they land in /Downloads.
- Commands that remove or undo things are shown to the user for approval first; if they decline, ask what they want instead.
- Keep answers short and practical, in the user's language. Quote exact button and menu names from the guide.

=== RYMWIN USER GUIDE ===
${guide}`;
}

export class RymnetAgent {
  items = $state<RymItem[]>([]);
  busy = $state(false);
  askFirst = $state(false);
  status = $state('');
  private turns: Turn[] = [];
  private stopped = false;
  private decide: ((run: boolean) => void) | null = null;
  private guide: string | null = null;

  /** Called after a command that may have changed the node (so the page refreshes). */
  onChange: (() => void) | null = null;

  stop() {
    this.stopped = true;
    this.decide?.(false);
  }

  approve(run: boolean) {
    this.decide?.(run);
  }

  clear() {
    if (this.busy) return;
    this.items = [];
    this.turns = [];
  }

  /** Runs one assistant command; returns what the model is told. */
  private async exec(command: string): Promise<{ exit: number | null; output: string; timedOut: boolean }> {
    const args = splitArgs(command);
    const [tool, sub, ...rest] = args;
    const ok = (output: string) => ({ exit: 0, output, timedOut: false });
    if (tool === 'rad') {
      switch (sub) {
        case 'initiate':
          return ok((await rad.rymnetInitiate(rest[0])).join('\n'));
        case 'open-rymwin':
          store.navigate({ kind: 'rymwin' });
          return ok('RYMWIN is open in RAD (RYMWIN in the sidebar). The user logs in there.');
        case 'open-rymsh':
          store.pendingTerminal = { kind: 'rymsh' };
          store.navigate({ kind: 'terminal' });
          return ok('rymsh is open in a Terminal tab. The user types its commands there (login, sync, email, mc…).');
        case 'stop-daemon':
          await rad.rymnetStopDaemon();
          return ok('Daemon stopped.');
        case 'status':
          return ok(statusText(await rad.rymnetStatus()));
        default:
          return { exit: 1, output: `Unknown command "rad ${sub ?? ''}". Use rad initiate, rad open-rymwin, rad open-rymsh, rad stop-daemon or rad status.`, timedOut: false };
      }
    }
    if (tool !== 'rym') return { exit: 1, output: 'Only `rym …` and `rad …` commands can run here (no shell).', timedOut: false };
    if (sub === undefined) return { exit: 1, output: 'Give a rym subcommand.', timedOut: false };
    const r = await rad.rymnetExec(args.slice(1));
    return { exit: r.exit_code, output: r.output, timedOut: r.timed_out };
  }

  async ask(text: string) {
    if (this.busy || !text.trim()) return;
    this.busy = true;
    this.stopped = false;
    this.items.push({ kind: 'user', text });
    this.turns.push({ role: 'user', content: text });
    try {
      this.guide ??= await rad.rymnetGuide();
      for (let step = 0; step < MAX_STEPS && !this.stopped; step++) {
        this.status = 'Thinking…';
        const st = await rad.rymnetStatus().catch(() => null);
        const r = await rad.assist(systemPrompt(this.guide, st), this.turns.slice(-30));
        if (this.stopped) break;
        this.turns.push({ role: 'assistant', content: r.content });
        const { text: say, command } = parseReply(r.content);
        if (say) this.items.push({ kind: 'say', text: say });
        if (!command) break;

        const risky = riskOf(command);
        const wait = this.askFirst || !!risky;
        this.items.push({ kind: 'cmd', command, status: wait ? 'waiting' : 'running', risky });
        const item = this.items[this.items.length - 1] as Extract<RymItem, { kind: 'cmd' }>;
        if (wait) {
          this.status = risky ? `Waiting for you: this command ${risky}` : 'Waiting for you to approve the command';
          const run = await new Promise<boolean>((res) => (this.decide = res));
          this.decide = null;
          if (!run) {
            item.status = 'skipped';
            this.turns.push({ role: 'user', content: 'The user chose not to run that command. Ask what they want instead, or take another approach.' });
            if (this.stopped) break;
            continue;
          }
          item.status = 'running';
        }
        this.status = 'Running…';
        let res: { exit: number | null; output: string; timedOut: boolean };
        try {
          res = await this.exec(command);
        } catch (e: any) {
          res = { exit: 1, output: e?.message ?? String(e), timedOut: false };
        }
        item.status = 'done';
        item.exit = res.exit;
        item.output = res.output;
        item.timedOut = res.timedOut;
        this.onChange?.();
        const out = res.output.trim() || '(no output)';
        this.turns.push({
          role: 'user',
          content: res.timedOut
            ? `The command took too long and was stopped (output so far below). Tell the user; a peer may be unreachable.\n\`\`\`\n${out}\n\`\`\``
            : `Exit code ${res.exit ?? '?'}.\n\`\`\`\n${out}\n\`\`\``,
        });
      }
      if (this.stopped) this.items.push({ kind: 'error', text: 'Stopped.' });
    } catch (e: any) {
      this.items.push({ kind: 'error', text: e?.message ?? String(e) });
    } finally {
      this.busy = false;
      this.status = '';
      this.decide = null;
    }
  }
}

/** One assistant while RAD runs, so the conversation survives switching views. */
export const rymnetAgent = new RymnetAgent();

// ------------------------------------------------------------------ the guide in the vault

const GUIDE_TITLE = 'RYMWIN User Guide';
const GUIDE_TAG = 'rymwin-guide';

function hash(s: string) {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 0x01000193);
  return (h >>> 0).toString(16);
}

let guideSynced = false;

/** Files the RYMWIN User Guide as a note in the "Rymnet" project, so RAD's chat can answer
 *  from it (RAG); updates the note when a new RAD ships a newer guide. Once per session. */
export async function ensureGuideInVault() {
  if (guideSynced) return;
  guideSynced = true;
  try {
    const guide = await rad.rymnetGuide();
    const version = `${GUIDE_TAG}-${hash(guide)}`;
    let project = store.projects.find((p) => p.name === 'Rymnet');
    if (!project) {
      project = await api.post<Project>('/v1/projects', {
        name: 'Rymnet',
        color: '#3f8a85',
        icon: 'network',
        description: 'Rymnet and RYMWIN: the user guide and your notes about the network',
      });
      store.projects = [...store.projects, project];
    }
    const docs = await api.get<Document[]>(`/v1/projects/${project.id}/documents`);
    const existing = docs.find((d) => d.source_kind === 'note' && d.tags.includes(GUIDE_TAG));
    const body = { title: GUIDE_TITLE, content: guide, tags: ['rymnet', GUIDE_TAG, version] };
    if (!existing) await api.post<Note>(`/v1/projects/${project.id}/notes`, body);
    else if (!existing.tags.includes(version)) await api.put(`/v1/notes/${existing.id}`, body);
    else return;
    store.reloadProjects();
  } catch (e) {
    guideSynced = false;
    console.warn('Filing the RYMWIN guide in the vault failed', e);
  }
}
