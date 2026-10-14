import { rad } from './api';

/** One entry in the sandbox agent's conversation. */
export type AgentItem =
  | { kind: 'user'; text: string }
  | { kind: 'say'; text: string }
  | { kind: 'cmd'; command: string; status: 'waiting' | 'running' | 'done' | 'skipped'; exit?: number | null; output?: string; timedOut?: boolean }
  | { kind: 'error'; text: string };

type Turn = { role: 'user' | 'assistant'; content: string };

const MAX_STEPS = 12;

function systemPrompt(distro: string, name: string) {
  const pm = /alpine/i.test(distro)
    ? 'apk (apk add --no-cache PKG)'
    : /debian|ubuntu/i.test(distro)
      ? 'apt (apt-get install -y PKG; run apt-get update first if a package is not found)'
      : /arch/i.test(distro)
        ? 'pacman (pacman -S --noconfirm PKG)'
        : 'the distro package manager';
  return `You are an agent working inside "${name}", an isolated ${distro} Linux sandbox VM on the user's computer. You are root. The user watches your commands run in their terminal.

How to work:
- To run a command, reply with ONE short sentence saying what you'll do, then exactly ONE fenced code block: \`\`\`sh … \`\`\`. It runs in the sandbox's shell (working directory and variables carry over) and you get the output and exit code back.
- Then run the next command, or — when the task is done — reply with a short answer for the user and NO code block.
- Use non-interactive commands only: no editors, pagers, REPLs or prompts (use -y / --noconfirm, cat instead of less, python3 script.py instead of python3). Write files with cat <<'EOF' > file … EOF.
- Keep output short (head, tail, grep, wc). Install missing tools with ${pm}.
- The user's shared folder is /root/shared (it is also on their Mac). Never run exit, reboot, poweroff or kill the shell.
- If something fails, read the error and fix it; don't repeat the same failing command.`;
}

/** The first shell code block in a reply, and the text around it. */
export function parseReply(reply: string): { text: string; command: string | null } {
  const m = reply.match(/```(?:sh|bash|shell|console|zsh)?[^\n]*\n([\s\S]*?)```/);
  if (!m) return { text: reply.trim(), command: null };
  const command = m[1]
    .split('\n')
    .map((l) => l.replace(/^\s*(?:\$|#)\s(?=\S)/, (p) => (p.trim() === '#' ? p : '')))
    .join('\n')
    .trim();
  const text = (reply.slice(0, m.index) + reply.slice(m.index! + m[0].length)).trim();
  return { text, command: command || null };
}

export class SandboxAgent {
  items = $state<AgentItem[]>([]);
  busy = $state(false);
  askFirst = $state(false);
  status = $state('');
  private turns: Turn[] = [];
  private stopped = false;
  private decide: ((run: boolean) => void) | null = null;

  constructor(
    public termId: number,
    public name: string,
    public distro: string,
  ) {}

  stop() {
    this.stopped = true;
    this.decide?.(false);
  }

  /** Answer to "Ask before running". */
  approve(run: boolean) {
    this.decide?.(run);
  }

  clear() {
    if (this.busy) return;
    this.items = [];
    this.turns = [];
  }

  async ask(text: string) {
    if (this.busy || !text.trim()) return;
    this.busy = true;
    this.stopped = false;
    this.items.push({ kind: 'user', text });
    this.turns.push({ role: 'user', content: text });
    try {
      for (let step = 0; step < MAX_STEPS && !this.stopped; step++) {
        this.status = 'Thinking…';
        const r = await rad.assist(systemPrompt(this.distro, this.name), this.turns.slice(-40));
        if (this.stopped) break;
        this.turns.push({ role: 'assistant', content: r.content });
        const { text: say, command } = parseReply(r.content);
        if (say) this.items.push({ kind: 'say', text: say });
        if (!command) break;

        this.items.push({ kind: 'cmd', command, status: this.askFirst ? 'waiting' : 'running' });
        const item = this.items[this.items.length - 1] as Extract<AgentItem, { kind: 'cmd' }>;
        if (this.askFirst) {
          this.status = 'Waiting for you to approve the command';
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
        this.status = 'Running in the sandbox…';
        const res = await rad.sandboxExec(this.termId, command, 180);
        item.status = 'done';
        item.exit = res.exit_code;
        item.output = res.output;
        item.timedOut = res.timed_out;
        const out = res.output.trim() || '(no output)';
        this.turns.push({
          role: 'user',
          content: res.timed_out
            ? `The command is still running after 3 minutes (output so far below). Don't wait for it; tell the user, or press on differently.\n\`\`\`\n${out}\n\`\`\``
            : `Exit code ${res.exit_code}.${res.truncated ? ' (Output truncated to the end.)' : ''}\n\`\`\`\n${out}\n\`\`\``,
        });
        if (res.timed_out) break;
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

/** One agent per sandbox terminal tab, kept while RAD runs. */
const agents = new Map<number, SandboxAgent>();

export function agentFor(termId: number, name: string, distro: string) {
  let a = agents.get(termId);
  if (!a) {
    a = new SandboxAgent(termId, name, distro);
    agents.set(termId, a);
  }
  return a;
}
