import { rad } from './api';

/** One entry in a terminal agent's conversation. */
export type AgentItem =
  | { kind: 'user'; text: string }
  | { kind: 'say'; text: string }
  | { kind: 'note'; text: string }
  | { kind: 'cmd'; command: string; status: 'waiting' | 'running' | 'done' | 'skipped'; exit?: number | null; output?: string; timedOut?: boolean; risky?: string | null }
  | { kind: 'error'; text: string };

/** Where the agent works: a Linux sandbox, a server over SSH, this computer's shell, or a
 *  Rust / Python container. */
export type AgentKind = 'sandbox' | 'ssh' | 'local' | 'container';

type Turn = { role: 'user' | 'assistant'; content: string };

const MAX_STEPS = 16;

/** What the dock knows about the machine (from a quick, read-only check). */
export type Facts = Record<string, string>;

const FACTS_SCRIPT = `[ -r /etc/os-release ] && . /etc/os-release
echo "os=\${PRETTY_NAME:-$(uname -sr)}"
[ "$(uname -s)" = Darwin ] && echo "os=macOS $(sw_vers -productVersion 2>/dev/null)"
echo "host=$(hostname 2>/dev/null)"
echo "user=$(id -un)"
echo "arch=$(uname -m)"
echo "uptime=$(uptime 2>/dev/null | sed -e 's/.*up *//' -e 's/, *[0-9]* user.*//' -e 's/,  *load.*//')"
echo "load=$( (cut -d' ' -f1-3 /proc/loadavg) 2>/dev/null || sysctl -n vm.loadavg 2>/dev/null | tr -d '{}' | xargs)"
echo "cpus=$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null)"
free -m 2>/dev/null | awk '/^Mem:/{printf "mem=%s/%s MB\\n",$3,$2}'
df -kP / 2>/dev/null | awk 'NR==2{printf "disk=%.0f/%.0f GB (%s)\\n",$3/1048576,$2/1048576,$5}'
[ "$(id -u)" = 0 ] && echo root=1
command -v sudo >/dev/null 2>&1 && echo sudo=1
command -v systemctl >/dev/null 2>&1 && echo "failed=$(systemctl --failed --no-legend --plain 2>/dev/null | grep -c .)"
command -v docker >/dev/null 2>&1 && echo docker=1
for p in apt-get dnf yum apk pacman zypper brew; do command -v $p >/dev/null 2>&1 && { echo "pkg=$p"; break; }; done
true`;

function parseFacts(out: string): Facts {
  const f: Facts = {};
  for (const line of out.split('\n')) {
    const i = line.indexOf('=');
    if (i > 0) f[line.slice(0, i).trim()] = line.slice(i + 1).trim();
  }
  return f;
}

/** Commands that delete, stop, lock out or reboot: always shown for approval first. */
const RISKS: [RegExp, string][] = [
  [/\brm\s+(-[a-zA-Z]*[rRf][a-zA-Z]*\s+)+/, 'deletes files'],
  [/\b(reboot|shutdown|poweroff|halt)\b|\binit\s+[06]\b|systemctl\s+(reboot|poweroff|halt)/, 'reboots or shuts down'],
  [/\b(mkfs(\.\w+)?|fdisk|parted|wipefs|sgdisk)\b|\bdd\b.*\bof=/, 'writes to a disk'],
  [/\biptables\s+(-F|--flush|-X|-P\s+\w+\s+DROP)|\bnft\s+flush|\bufw\s+(disable|reset|default\s+deny|deny)/, 'changes the firewall'],
  [/sshd_config|\bsystemctl\s+(stop|disable|mask|restart)\s+(ssh|sshd|networking|network|NetworkManager|systemd-networkd)\b/, 'can lock you out of SSH'],
  [/\b(userdel|deluser|passwd|chpasswd|usermod\s+-L|gpasswd\s+-d)\b/, 'changes user accounts'],
  [/\b(chmod|chown)\s+(-[a-zA-Z]*R|--recursive)/, 'changes permissions recursively'],
  [/\bDROP\s+(DATABASE|TABLE|SCHEMA)\b|\bTRUNCATE\s+TABLE\b|\bdropdb\b/i, 'deletes database data'],
  [/\bdocker\s+(system\s+prune|volume\s+(rm|prune)|rm\s+-f|compose\s+down\s+-v)|\bpodman\s+system\s+prune/, 'removes containers or volumes'],
  [/\b(apt(-get)?|dnf|yum|zypper)\s+(-y\s+)?(remove|purge|autoremove|erase)\b|\bpacman\s+-R|\bapk\s+del\b|\bbrew\s+uninstall\b/, 'uninstalls software'],
  [/\b(kill|pkill|killall)\b/, 'stops processes'],
  [/\bsystemctl\s+(stop|disable|mask)\b|\bservice\s+\S+\s+stop\b/, 'stops a service'],
  [/\bcrontab\s+-r\b/, 'deletes the crontab'],
  [/\bgit\s+(reset\s+--hard|clean\s+-[a-zA-Z]*f|push\s+.*(--force|-f\b))/, 'discards git work'],
  [/(^|[^>&])>\s*\/(etc|boot|usr)\//, 'overwrites a system file'],
  [/\btruncate\s+-s\s*0\b|\bshred\b/, 'erases file contents'],
];

export function riskOf(command: string): string | null {
  for (const [re, why] of RISKS) if (re.test(command)) return why;
  return null;
}

/** One-click jobs offered in the dock. */
export const TASKS: Record<AgentKind, { label: string; prompt: string }[]> = {
  ssh: [
    { label: 'Health check', prompt: 'Give me a health check of this server: uptime and load, CPU and memory use, disk space, the top processes, failed services and anything worrying in recent logs. Summarise with a verdict.' },
    { label: 'Disk usage', prompt: 'What is using the disk space? Show the biggest directories and what is safe to clean up (logs, caches, old kernels, docker images). Do not delete anything.' },
    { label: 'Pending updates', prompt: 'Check for pending package updates, highlighting security updates and whether a reboot is required. Do not install anything.' },
    { label: 'Security review', prompt: 'Do a security review: listening ports and their services, firewall status, SSH settings (root login, password auth), recent failed logins, users with sudo, automatic security updates, world-writable files in /etc. Report findings with suggested fixes; change nothing.' },
    { label: 'Services & errors', prompt: 'List failed or restarting services and the most important errors in the system logs from the last 24 hours, with likely causes and fixes.' },
    { label: 'Web server', prompt: 'Check the web server (nginx, apache or caddy): is it running, is the config valid, which sites are enabled, TLS certificate expiry dates, and recent errors in its logs.' },
    { label: 'Docker', prompt: 'Show Docker status: running containers with health and restart counts, CPU/memory use, unhealthy or restarting containers with their recent logs, and disk used by images and volumes.' },
    { label: 'Network', prompt: 'Show the network setup: IP addresses, listening ports with their processes, DNS resolvers, default route, and check that the server can reach the internet.' },
    { label: 'Scheduled jobs', prompt: 'List scheduled jobs (all users’ crontabs, /etc/cron.*, systemd timers) with when they last ran and whether they succeeded.' },
  ],
  local: [
    { label: 'Disk usage', prompt: 'What is using the most disk space in my home folder? Show the biggest folders and caches that are safe to clear. Do not delete anything.' },
    { label: 'Busy processes', prompt: 'Which processes are using the most CPU and memory right now?' },
    { label: 'Open ports', prompt: 'Which programs are listening on network ports on this computer?' },
    { label: 'Outdated packages', prompt: 'Check for outdated Homebrew (or system) packages and tell me what would be upgraded. Do not upgrade anything.' },
  ],
  container: [
    { label: 'Explain project', prompt: 'Look at the project in /work/app and explain what it does and how it is laid out.' },
    { label: 'Run & fix', prompt: 'Run the project. If it fails, find the cause, fix it and run it again until it works.' },
    { label: 'Add tests', prompt: 'Add a few meaningful tests for the project and run them.' },
    { label: 'Web server', prompt: 'Turn the project into a small web server on port 8000 that answers / with a JSON greeting, start it in the background and check it answers with curl.' },
  ],
  sandbox: [
    { label: 'List files', prompt: 'list the current directory' },
    { label: 'Python', prompt: 'install python and print the version' },
    { label: 'Primes', prompt: 'write a python script that prints the first 20 primes and run it' },
  ],
};

function factsText(f: Facts | null) {
  if (!f) return 'Unknown (the check did not run).';
  const keys: [string, string][] = [['os', 'OS'], ['host', 'Hostname'], ['user', 'Logged in as'], ['arch', 'Architecture'], ['cpus', 'CPUs'], ['mem', 'Memory used'], ['disk', 'Disk / used'], ['load', 'Load'], ['uptime', 'Up'], ['pkg', 'Package manager'], ['failed', 'Failed systemd units']];
  const lines = keys.filter(([k]) => f[k]).map(([k, l]) => `- ${l}: ${f[k]}`);
  if (f.docker) lines.push('- Docker is installed');
  return lines.join('\n');
}

const COMMON = `How to work:
- To run a command, reply with ONE short sentence saying what you'll do, then exactly ONE fenced code block: \`\`\`sh … \`\`\`. You get its output and exit code back.
- Then run the next command, or — when the task is done — reply with a short answer for the user (findings, what changed, what to do next) and NO code block.
- Use non-interactive commands only: no editors, pagers, REPLs, top/htop or prompts (use -y, top -bn1, ps, cat instead of less, --no-pager for journalctl/systemctl). Write files with cat <<'EOF' > file … EOF.
- Keep output short (head, tail, grep, wc, -n 50). If something fails, read the error and fix it; don't repeat the same failing command.
- Don't show secrets (private keys, passwords, tokens, .env values) in full: redact them.`;

function systemPrompt(kind: AgentKind, name: string, distro: string, facts: Facts | null) {
  if (kind === 'container') {
    const rust = /rust/i.test(distro);
    return `You are a software engineering agent working inside "${name}", an isolated ${distro} development container (a Debian 13 Linux VM on the user's Mac). You are root. The user sees each command you run and its output.

The project lives in /work/app and the editor shows its files. ${
      rust
        ? 'It is a Cargo project: build and run with cargo run (cargo build, cargo test, cargo clippy, cargo fmt). Add crates with cargo add NAME (from crates.io). Rust is installed with rustup.'
        : 'It is a Python project: main.py is the entry point and requirements.txt lists packages. A virtualenv (/opt/venv) is active: install with pip install NAME and add it to requirements.txt. Run with python main.py.'
    }

${COMMON}
- System packages: apt-get install -y PKG (run apt-get update first if a package is not found).
- Each command runs with sh; the working directory carries over between commands, variables don't. Commands are stopped after 10 minutes.
- To hand results to the user, write files to /root/shared — that folder is on their Mac.
- Long-running programs (servers) must be started in the background: nohup CMD > /tmp/NAME.log 2>&1 & — then check them with curl or the log. Ports the user forwarded are reachable from their Mac at 127.0.0.1.
- Never run reboot, poweroff, or kill the agent (rad-agent).`;
  }
  if (kind === 'sandbox') {
    const pm = /alpine/i.test(distro)
      ? 'apk (apk add --no-cache PKG)'
      : /debian|ubuntu/i.test(distro)
        ? 'apt (apt-get install -y PKG; run apt-get update first if a package is not found)'
        : /arch/i.test(distro)
          ? 'pacman (pacman -S --noconfirm PKG)'
          : 'the distro package manager';
    return `You are an agent working inside "${name}", an isolated ${distro} Linux sandbox VM on the user's computer. You are root. The user watches your commands run in their terminal. Commands run in the sandbox's shell, where the working directory and variables carry over.

${COMMON}
- Install missing tools with ${pm}.
- The user's shared folder is /root/shared (it is also on their Mac). Never run exit, reboot, poweroff or kill the shell.`;
  }
  if (kind === 'ssh') {
    const root = facts?.root === '1';
    return `You are a careful, experienced Linux server administrator working on the server "${name}" over SSH, on behalf of the user who manages it. The user watches every command and its output in their terminal.

The server:
${factsText(facts)}

${COMMON}
- Each command runs with sh on the server (not the user's interactive shell). The working directory carries over between commands; variables and exported settings don't.
- ${root ? 'You are root.' : `You are ${facts?.user ?? 'a regular user'}. Use sudo for administrative work. If sudo answers that a password is required, stop and tell the user to set the sudo password with the key button in the agent panel.`}
- Investigate before changing anything, and explain what you are about to change. Before editing a config file, back it up (cp FILE FILE.bak.$(date +%Y%m%d%H%M%S)). Validate configs before applying them (nginx -t, apachectl configtest, sshd -t, named-checkconf, docker compose config…) and prefer reload over restart.
- Never lock the user out: be extremely careful with sshd_config, firewall rules (always allow the SSH port first), network configuration and user accounts. Never reboot or shut down unless the user explicitly asks.
- Commands that delete data, stop services or change access are shown to the user for approval before they run; if the user declines, find another way or ask.
- For long jobs (big upgrades, builds), run them in the background with nohup … > /tmp/job.log 2>&1 & and check the log, rather than waiting more than a few minutes.`;
  }
  return `You are an agent working in a shell on the user's own computer, as their user. The user watches your commands in their terminal.

This computer:
${factsText(facts)}

${COMMON}
- Each command runs with sh in the user's environment (their PATH, so Homebrew tools work). The working directory carries over between commands; variables don't. Start from the home folder unless told otherwise.
- This is the user's personal machine: never delete, move or overwrite their files unless they ask, don't change system settings without asking, and use sudo only when needed (if it asks for a password, tell the user to set it with the key button in the agent panel).`;
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

export class TermAgent {
  items = $state<AgentItem[]>([]);
  busy = $state(false);
  askFirst = $state(false);
  status = $state('');
  facts = $state<Facts | null>(null);
  factsLoading = $state(false);
  cwd = $state<string | null>(null);
  hasSudo = $state(false);
  /** For a container agent: the container it works in. */
  containerId: string | null = null;
  /** Called before each command runs (the container view shows them in its output). */
  onCommand: ((command: string) => void) | null = null;
  private turns: Turn[] = [];
  private stopped = false;
  private decide: ((run: boolean) => void) | null = null;

  constructor(
    public termId: number,
    public kind: AgentKind,
    public name: string,
    public distro = 'Linux',
  ) {
    // A real server: look before leaping.
    this.askFirst = kind === 'ssh';
    if (kind !== 'sandbox' && kind !== 'container') this.loadFacts();
  }

  /** Runs one command where this agent works. */
  private exec(command: string, timeout: number, quiet = false) {
    if (this.kind === 'container')
      return rad.containerExec(this.containerId!, command, this.cwd, timeout).then((r) => ({ ...r, cancelled: false }));
    if (this.kind === 'sandbox') return rad.sandboxExec(this.termId, command, timeout).then((r) => ({ ...r, cancelled: false, cwd: null as string | null }));
    return rad.agentExec(this.termId, command, timeout, quiet);
  }

  /** A quick read-only look at the machine (OS, load, memory, disk, failed services). Over
   *  SSH the user may still be logging in, so it tries again for a minute. */
  async loadFacts(retries = this.kind === 'ssh' ? 15 : 0) {
    if (this.kind === 'sandbox' || this.kind === 'container' || this.factsLoading) return;
    this.factsLoading = true;
    let ok = false;
    try {
      const r = await this.exec(FACTS_SCRIPT, 30, true);
      ok = r.output.includes('os=');
      if (ok) this.facts = parseFacts(r.output);
    } catch {
      /* the terminal has closed */
      retries = 0;
    } finally {
      this.factsLoading = false;
    }
    if (!ok && retries > 0) setTimeout(() => !this.facts && this.loadFacts(retries - 1), 4000);
  }

  stop() {
    this.stopped = true;
    this.decide?.(false);
    if (this.kind === 'ssh' || this.kind === 'local') rad.agentCancel(this.termId).catch(() => {});
  }

  /** Answer to an approval request. */
  approve(run: boolean) {
    this.decide?.(run);
  }

  clear() {
    if (this.busy) return;
    this.items = [];
    this.turns = [];
  }

  async setSudo(password: string | null) {
    await rad.agentSetSudo(this.termId, password);
    this.hasSudo = !!password;
  }

  /** Something the user did that the agent should know about (an upload…). */
  note(text: string) {
    this.items.push({ kind: 'note', text });
    this.turns.push({ role: 'user', content: `(For your information: ${text})` });
  }

  async ask(text: string) {
    if (this.busy || !text.trim()) return;
    this.busy = true;
    this.stopped = false;
    this.items.push({ kind: 'user', text });
    this.turns.push({ role: 'user', content: text });
    try {
      if ((this.kind === 'ssh' || this.kind === 'local') && !this.facts) await this.loadFacts();
      for (let step = 0; step < MAX_STEPS && !this.stopped; step++) {
        this.status = 'Thinking…';
        const r = await rad.assist(systemPrompt(this.kind, this.name, this.distro, this.facts), this.turns.slice(-40));
        if (this.stopped) break;
        this.turns.push({ role: 'assistant', content: r.content });
        const { text: say, command } = parseReply(r.content);
        if (say) this.items.push({ kind: 'say', text: say });
        if (!command) break;

        // Sandboxes and containers are isolated; elsewhere, risky commands always wait for the user.
        const risky = this.kind === 'sandbox' || this.kind === 'container' ? null : riskOf(command);
        const wait = this.askFirst || !!risky;
        this.items.push({ kind: 'cmd', command, status: wait ? 'waiting' : 'running', risky });
        const item = this.items[this.items.length - 1] as Extract<AgentItem, { kind: 'cmd' }>;
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
        this.status = this.kind === 'ssh' ? `Running on ${this.name}…` : this.kind === 'sandbox' ? 'Running in the sandbox…' : this.kind === 'container' ? `Running in ${this.name}…` : 'Running…';
        const limit = this.kind === 'sandbox' ? 180 : this.kind === 'container' ? 600 : 300;
        this.onCommand?.(command);
        const res = await this.exec(command, limit);
        item.status = 'done';
        item.exit = res.exit_code;
        item.output = res.output;
        item.timedOut = res.timed_out || res.cancelled;
        if (res.cwd) this.cwd = res.cwd;
        const out = res.output.trim() || '(no output)';
        if (res.cancelled) {
          this.turns.push({ role: 'user', content: `The user stopped that command. Output so far:\n\`\`\`\n${out}\n\`\`\`` });
          break;
        }
        const where = res.cwd ? ` Working directory: ${res.cwd}.` : '';
        this.turns.push({
          role: 'user',
          content: res.timed_out
            ? this.kind === 'sandbox'
              ? `The command is still running after 3 minutes (output so far below). Don't wait for it; tell the user, or press on differently.\n\`\`\`\n${out}\n\`\`\``
              : `The command took longer than ${limit / 60} minutes and was stopped (output so far below). Run long jobs in the background with nohup and check their log instead.\n\`\`\`\n${out}\n\`\`\``
            : `Exit code ${res.exit_code}.${where}${res.truncated ? ' (Output truncated to the end.)' : ''}\n\`\`\`\n${out}\n\`\`\``,
        });
        if (res.timed_out && this.kind === 'sandbox') break;
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

/** One agent per container, kept while RAD runs. */
const containerAgents = new Map<string, TermAgent>();

export function containerAgent(id: string, name: string, kindName: string) {
  let a = containerAgents.get(id);
  if (!a) {
    a = new TermAgent(0, 'container', name, kindName);
    a.containerId = id;
    containerAgents.set(id, a);
  }
  a.name = name;
  return a;
}

/** One agent per terminal tab, kept while RAD runs. */
const agents = new Map<number, TermAgent>();

export function agentFor(termId: number, kind: AgentKind, name: string, distro?: string) {
  let a = agents.get(termId);
  if (!a) {
    a = new TermAgent(termId, kind, name, distro);
    agents.set(termId, a);
  }
  return a;
}
