<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { Terminal } from '@xterm/xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import { WebLinksAddon } from '@xterm/addon-web-links';
  import '@xterm/xterm/css/xterm.css';
  import { rad } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import type { SandboxInfo, SshHost, TermInfo } from '../lib/types';
  import SandboxManager from './SandboxManager.svelte';
  import AgentDock from './AgentDock.svelte';
  import { agentFor } from '../lib/termAgent.svelte';
  import { platform } from '../lib/api';
  import Modal from './Modal.svelte';
  import { Plus, X, SquareTerminal, Server, KeyRound, Trash2, Box, Network } from '@lucide/svelte';

  interface View {
    term: Terminal;
    fit: FitAddon;
    el: HTMLDivElement;
  }

  let sessions = $state<TermInfo[]>([]);
  let active = $state<number | null>(null);
  let host: HTMLDivElement | undefined = $state();
  const views = new Map<number, View>();

  let sshOpen = $state(false);
  let sandboxesOpen = $state(false);
  /** Code waiting for a sandbox to run in (chosen or created in the manager). */
  let pendingRun = $state<{ code: string; lang: string } | null>(null);
  let form = $state<SshHost>({ host: '', user: '', port: 22, identity_file: '' });

  const THEME = {
    background: '#14141b',
    foreground: '#e6e6ef',
    cursor: '#c9c3ff',
    selectionBackground: '#4b4580',
    black: '#1d1d26', red: '#ff6b7f', green: '#7ee2a8', yellow: '#f6d77a', blue: '#7aa7ff', magenta: '#c49bff', cyan: '#6fdcf0', white: '#d8d8e4',
    brightBlack: '#5c5c70', brightRed: '#ff8fa0', brightGreen: '#a2f0c3', brightYellow: '#fbe7a3', brightBlue: '#a3c2ff', brightMagenta: '#d8bcff', brightCyan: '#9eeaf7', brightWhite: '#ffffff',
  };

  /** Name and distro of each sandbox, for the agent dock. */
  let sbMeta = $state<Record<string, { name: string; distro: string }>>({});
  const activeSession = $derived(sessions.find((s) => s.id === active) ?? null);
  /** The AI agent for the active tab: in a sandbox, on an SSH server, or in a local shell. */
  const agent = $derived.by(() => {
    const s = activeSession;
    if (!s || s.exited) return null;
    if (s.kind === 'sandbox') {
      if (!s.sandbox) return null;
      const m = sbMeta[s.sandbox];
      return agentFor(s.id, 'sandbox', m?.name ?? s.title, m?.distro ?? 'Linux');
    }
    if (s.kind === 'ssh') return agentFor(s.id, 'ssh', s.title);
    if (s.kind === 'local' && platform() !== 'windows') return agentFor(s.id, 'local', s.title);
    return null;
  });

  async function refresh() {
    sessions = await rad.termList().catch(() => []);
    if (sessions.some((s) => s.kind === 'sandbox')) {
      rad.sandboxStatus().then((st) => {
        sbMeta = Object.fromEntries(st.sandboxes.map((b) => [b.id, { name: b.name, distro: b.distro_name }]));
      }).catch(() => {});
    }
    if (active === null || !sessions.some((s) => s.id === active)) active = sessions.at(-1)?.id ?? null;
  }

  /** Creates the xterm for a session (once per visit) and connects it. */
  function view(s: TermInfo): View {
    let v = views.get(s.id);
    if (v) return v;
    const el = document.createElement('div');
    el.className = 'term';
    host!.appendChild(el);
    const term = new Terminal({
      fontFamily: getComputedStyle(document.documentElement).getPropertyValue('--mono') || 'Menlo, monospace',
      fontSize: 13,
      lineHeight: 1.15,
      cursorBlink: true,
      allowProposedApi: false,
      scrollback: 10_000,
      theme: THEME,
      macOptionIsMeta: true,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    // Links open in RAD's own browser.
    term.loadAddon(new WebLinksAddon((_e, uri) => store.runAction({ kind: 'open_url', url: uri })));
    term.open(el);
    term.onData((d) => rad.termWrite(s.id, d).catch(() => {}));
    term.onResize(({ cols, rows }) => rad.termResize(s.id, cols, rows).catch(() => {}));
    rad.termAttach(s.id, (d) => {
      if (d instanceof ArrayBuffer) term.write(new Uint8Array(d));
      else if (d && 'exit' in d) {
        term.write(`\r\n\x1b[2m[${d.exit} — press ⌘W or close the tab]\x1b[0m\r\n`);
        refresh();
      }
    });
    v = { term, fit, el };
    views.set(s.id, v);
    return v;
  }

  // Show the active session, sized to the space.
  $effect(() => {
    const id = active;
    const list = sessions;
    if (!host) return;
    for (const s of list) {
      const v = view(s);
      v.el.style.display = s.id === id ? 'block' : 'none';
    }
    for (const [sid, v] of views) {
      if (!list.some((s) => s.id === sid)) {
        v.term.dispose();
        v.el.remove();
        views.delete(sid);
      }
    }
    if (id !== null) {
      tick().then(() => {
        const v = views.get(id);
        if (!v) return;
        v.fit.fit();
        v.term.focus();
      });
    }
  });

  function size() {
    // A first guess before xterm has measured itself; fit() corrects it right after.
    const r = host?.getBoundingClientRect();
    return { cols: Math.max(20, Math.floor((r?.width ?? 800) / 8)), rows: Math.max(5, Math.floor((r?.height ?? 480) / 17)) };
  }

  async function openLocal() {
    try {
      const s = await rad.termOpen({ kind: 'local', ...size() });
      await refresh();
      active = s.id;
    } catch (e) {
      store.error(e, 'Couldn’t open a terminal');
    }
  }

  /** The Rymnet shell: shows the open one, or opens it. */
  async function openRymsh() {
    await refresh();
    const open = sessions.find((s) => s.kind === 'rymsh' && !s.exited);
    if (open) {
      active = open.id;
      return;
    }
    try {
      const s = await rad.termOpen({ kind: 'rymsh', ...size() });
      await refresh();
      active = s.id;
    } catch (e) {
      store.error(e, 'Couldn’t open rymsh');
    }
  }

  async function openSsh(h: SshHost) {
    const target = { host: h.host.trim(), user: h.user?.trim() || undefined, port: h.port && h.port !== 22 ? Number(h.port) : undefined, identity_file: h.identity_file?.trim() || undefined };
    try {
      const s = await rad.termOpen({ kind: 'ssh', ...target, ...size() });
      await refresh();
      active = s.id;
      store.rememberSshHost(target);
    } catch (e) {
      store.error(e, 'Couldn’t start SSH');
    }
  }

  const LAST_SANDBOX = 'rad-last-sandbox';

  async function openSandbox(sb: { id: string }, script?: { code: string; lang: string }) {
    try {
      const s = await rad.termOpen({ kind: 'sandbox', sandbox: sb.id, script, ...size() });
      await refresh();
      active = s.id;
      try {
        localStorage.setItem(LAST_SANDBOX, sb.id);
      } catch {
        /* ignore */
      }
    } catch (e) {
      store.error(e, 'Couldn’t start the sandbox');
    }
  }

  function fromManager(sb: SandboxInfo) {
    sandboxesOpen = false;
    const run = pendingRun ?? undefined;
    pendingRun = null;
    openSandbox(sb, run);
  }

  /** Runs code in the last-used sandbox, or asks which one (or to create one). */
  async function runCode(code: string, lang: string) {
    const st = await rad.sandboxStatus().catch(() => null);
    if (!st?.supported || !st.engine) {
      pendingRun = { code, lang };
      sandboxesOpen = true;
      return;
    }
    let last: string | null = null;
    try {
      last = localStorage.getItem(LAST_SANDBOX);
    } catch {
      /* ignore */
    }
    const sb = st.sandboxes.find((s) => s.id === last) ?? (st.sandboxes.length === 1 ? st.sandboxes[0] : null);
    if (sb) return openSandbox(sb, { code, lang });
    pendingRun = { code, lang };
    sandboxesOpen = true;
  }

  async function close(id: number, e?: Event) {
    e?.stopPropagation();
    await rad.termClose(id).catch(() => {});
    await refresh();
  }

  function connect(e: Event) {
    e.preventDefault();
    if (!form.host.trim()) return;
    sshOpen = false;
    openSsh(form);
  }

  async function pickKey() {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const f = await open({ multiple: false, directory: false, title: 'Choose a private key' });
    if (typeof f === 'string') form.identity_file = f;
  }

  function forget(h: SshHost) {
    const list = store.sshHosts.filter((x) => x !== h);
    store.sshHosts = list;
    import('../lib/api').then(({ api }) => api.put('/v1/app-data/ssh_hosts', list).catch(() => {}));
  }

  const label = (h: SshHost) => `${h.user ? h.user + '@' : ''}${h.host}${h.port && h.port !== 22 ? ':' + h.port : ''}`;

  // Requests from the assistant ("open ssh to coursemat.au").
  $effect(() => {
    const a = store.pendingTerminal;
    if (!a || !host) return;
    store.pendingTerminal = null;
    if (a.kind === 'ssh' && 'host' in a && a.host) openSsh({ host: a.host, user: a.user, port: a.port, identity_file: a.identity_file });
    else if (a.kind === 'sandbox_run' && 'code' in a) runCode(a.code, a.lang);
    else if (a.kind === 'sandbox' && 'id' in a) openSandbox({ id: a.id });
    else if (a.kind === 'rymsh') openRymsh();
    else openLocal();
  });

  function onKey(e: KeyboardEvent) {
    const mod = document.documentElement.dataset.os === 'macos' ? e.metaKey : e.ctrlKey && e.shiftKey;
    if (!mod) return;
    if (e.key.toLowerCase() === 't') {
      e.preventDefault();
      openLocal();
    } else if (e.key.toLowerCase() === 'w' && active !== null) {
      e.preventDefault();
      close(active);
    }
  }

  onMount(() => {
    refresh();
    const ro = new ResizeObserver(() => {
      if (active !== null) views.get(active)?.fit.fit();
    });
    if (host) ro.observe(host);
    const un = listen('term-changed', refresh);
    return () => {
      ro.disconnect();
      un.then((u) => u());
      for (const [id, v] of views) {
        rad.termDetach(id).catch(() => {});
        v.term.dispose();
      }
      views.clear();
    };
  });
</script>

<svelte:window onkeydown={onKey} />

<div class="terminal">
  <div class="tabs drag">
    {#each sessions as s (s.id)}
      <div class="tab no-drag" class:active={s.id === active} class:dead={!!s.exited} role="tab" tabindex="0" aria-selected={s.id === active}
        onclick={() => (active = s.id)} onkeydown={(e) => e.key === 'Enter' && (active = s.id)}>
        {#if s.kind === 'ssh'}<Server size={13} />{:else if s.kind === 'sandbox'}<Box size={13} />{:else if s.kind === 'rymsh'}<Network size={13} />{:else}<SquareTerminal size={13} />{/if}
        <span class="truncate spacer">{s.title}</span>
        <button class="x" aria-label="Close terminal" onclick={(e) => close(s.id, e)}><X size={12} /></button>
      </div>
    {/each}
    <button class="icon-btn sm no-drag" title="New terminal (⌘T)" onclick={openLocal}><Plus size={15} /></button>
    <button class="btn ghost sm no-drag ssh" onclick={() => ((form = { host: '', user: '', port: 22, identity_file: '' }), (sshOpen = true))}><Server size={13} /> SSH…</button>
    <button class="btn ghost sm no-drag ssh" onclick={() => (sandboxesOpen = true)}><Box size={13} /> Sandboxes…</button>
  </div>

  <div class="body" bind:this={host}>
    {#if !sessions.length}
      <div class="empty">
        <SquareTerminal size={30} strokeWidth={1.5} />
        <strong>No terminals open</strong>
        <p>Open a shell on this computer, connect to a server over SSH, or start an isolated Linux sandbox to play with Python and scripts. You can also ask in any chat: “open ssh to coursemat.au”.</p>
        <div class="row">
          <button class="btn primary sm" onclick={openLocal}><Plus size={14} /> New terminal</button>
          <button class="btn sm" onclick={() => (sshOpen = true)}><Server size={14} /> Connect over SSH…</button>
          <button class="btn sm" onclick={() => (sandboxesOpen = true)}><Box size={14} /> Linux sandbox…</button>
        </div>
        {#if store.sshHosts.length}
          <div class="hosts">
            <div class="section-title">Recent servers</div>
            {#each store.sshHosts as h}
              <div class="host">
                <button class="go" onclick={() => openSsh(h)}><Server size={14} /> <span class="truncate">{label(h)}</span>{#if h.identity_file}<KeyRound size={12} class="k" />{/if}</button>
                <button class="icon-btn sm" title="Forget" onclick={() => forget(h)}><Trash2 size={13} /></button>
              </div>
            {/each}
          </div>
        {/if}
      </div>
    {/if}
  </div>
  {#if agent}
    {#key agent}<AgentDock {agent} />{/key}
  {/if}
</div>

<Modal bind:open={sandboxesOpen} title={pendingRun ? 'Run in which sandbox?' : 'Linux sandboxes'} width={640} onclose={() => (pendingRun = null)}>
  <SandboxManager onopen={fromManager} />
</Modal>

<Modal bind:open={sshOpen} title="Connect over SSH" width={440}>
  <form onsubmit={connect}>
    <div class="field">
      <label class="label" for="sh">Host</label>
      <!-- svelte-ignore a11y_autofocus -->
      <input id="sh" class="input" bind:value={form.host} placeholder="coursemat.au or 10.0.0.5" autofocus spellcheck="false" autocomplete="off" />
    </div>
    <div class="row" style="gap:10px">
      <div class="field" style="flex:1">
        <label class="label" for="su">User</label>
        <input id="su" class="input" bind:value={form.user} placeholder="default" spellcheck="false" autocomplete="off" />
      </div>
      <div class="field" style="width:100px">
        <label class="label" for="sp">Port</label>
        <input id="sp" class="input" type="number" min="1" max="65535" bind:value={form.port} />
      </div>
    </div>
    <div class="field">
      <label class="label" for="sk">Private key <span class="muted">(optional)</span></label>
      <div class="row" style="gap:8px">
        <input id="sk" class="input" style="flex:1" bind:value={form.identity_file} placeholder="~/.ssh/id_ed25519" spellcheck="false" />
        <button type="button" class="btn sm" onclick={pickKey}>Choose…</button>
      </div>
      <div class="hint">Passwords are typed in the terminal and never stored. Keys stay where they are on disk.</div>
    </div>
    {#if store.sshHosts.length}
      <div class="recent">
        {#each store.sshHosts.slice(0, 6) as h}
          <button type="button" class="chip" onclick={() => (form = { ...h, port: h.port ?? 22 })}>{label(h)}</button>
        {/each}
      </div>
    {/if}
    <div class="row" style="justify-content:flex-end">
      <button type="button" class="btn ghost" onclick={() => (sshOpen = false)}>Cancel</button>
      <button class="btn primary" disabled={!form.host.trim()}>Connect</button>
    </div>
  </form>
</Modal>

<style>
  .terminal {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .tabs {
    flex: none;
    display: flex;
    align-items: flex-end;
    gap: 2px;
    height: 40px;
    padding: 6px 10px 0 calc(10px + var(--lead));
    background: var(--bg-sidebar);
    border-bottom: 1px solid var(--border);
    overflow: hidden;
  }
  .tabs > .icon-btn,
  .tabs > .ssh {
    margin: 0 0 5px 4px;
  }
  .tab {
    display: flex;
    align-items: center;
    gap: 7px;
    flex: 0 1 220px;
    min-width: 80px;
    height: 34px;
    padding: 0 6px 0 11px;
    border-radius: var(--r-sm) var(--r-sm) 0 0;
    color: var(--text-3);
    font-size: 12.5px;
    cursor: default;
  }
  .tab:hover {
    background: var(--surface-hover);
  }
  .tab.active {
    background: #14141b;
    color: #e6e6ef;
  }
  .tab.dead span {
    text-decoration: line-through;
    opacity: 0.7;
  }
  .tab :global(svg) {
    flex: none;
  }
  .x {
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    border: none;
    border-radius: 5px;
    background: transparent;
    color: inherit;
    opacity: 0;
  }
  .tab:hover .x,
  .tab.active .x {
    opacity: 0.8;
  }
  .x:hover {
    background: rgba(127, 127, 160, 0.25);
  }
  .body {
    flex: 1;
    min-height: 0;
    position: relative;
    background: #14141b;
    padding: 6px 0 0 10px;
  }
  .body :global(.term) {
    position: absolute;
    inset: 6px 4px 4px 10px;
  }
  .empty {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding: 24px;
    background: var(--surface);
    color: var(--text-2);
    text-align: center;
  }
  .empty strong {
    color: var(--text);
    font-size: 15px;
  }
  .empty p {
    margin: 0;
    max-width: 420px;
    font-size: 13px;
  }
  .hosts {
    width: min(420px, 100%);
    margin-top: 18px;
    text-align: left;
  }
  .host {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .go {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 32px;
    padding: 0 10px;
    border: none;
    border-radius: var(--r-sm);
    background: transparent;
    color: var(--text);
    font-size: 13px;
    text-align: left;
  }
  .go:hover {
    background: var(--surface-hover);
  }
  .go :global(.k) {
    color: var(--text-3);
  }
  .recent {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: 14px;
  }
</style>
