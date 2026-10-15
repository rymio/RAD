<script lang="ts">
  import { tick } from 'svelte';
  import { renderMarkdown } from '../lib/markdown';
  import { rad } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import { TASKS, type TermAgent } from '../lib/termAgent.svelte';
  import {
    Sparkles, ArrowUp, Square, ChevronDown, ChevronUp, Minus, Plus, Trash2, Play, SkipForward, CircleCheck, CircleX, Loader, Clock,
    KeyRound, Upload, Download, Zap, RefreshCw, TriangleAlert, Info,
  } from '@lucide/svelte';

  let { agent }: { agent: TermAgent } = $props();

  /** The small form under the header: sudo password, or a file to download. */
  let panel = $state<'sudo' | 'download' | 'tasks' | null>(null);
  let secret = $state('');
  let remotePath = $state('');
  let transferring = $state(false);

  const tasks = $derived(TASKS[agent.kind]);
  const title = $derived(agent.kind === 'ssh' ? 'Server agent' : agent.kind === 'local' ? 'Shell agent' : 'Sandbox agent');
  const where = $derived(agent.kind === 'ssh' ? `on ${agent.name}` : agent.kind === 'local' ? 'on this computer' : `in ${agent.name}`);
  const f = $derived(agent.facts);
  const diskHigh = $derived(+(f?.disk?.match(/\((\d+)%\)/)?.[1] ?? 0) >= 85);

  function runTask(prompt: string) {
    panel = null;
    if (agent.busy) return;
    collapsed = false;
    agent.ask(prompt);
  }

  async function saveSudo(e: Event) {
    e.preventDefault();
    await agent.setSudo(secret || null).catch((err) => store.error(err, 'Couldn’t set the password'));
    secret = '';
    panel = null;
  }

  async function upload() {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const r = await open({ multiple: true, title: `Upload to ${agent.name}` });
    const paths = r == null ? [] : Array.isArray(r) ? r : [r];
    if (!paths.length) return;
    transferring = true;
    try {
      const dest = await rad.sshUpload(agent.termId, paths);
      const names = paths.map((p) => p.split(/[\\/]/).pop()).join(', ');
      agent.note(`the user uploaded ${names} to ${dest} on the server`);
      store.toast('success', 'Uploaded', `${names} → ${dest}`, 3000);
    } catch (err) {
      store.error(err, 'Upload failed');
    } finally {
      transferring = false;
    }
  }

  async function download(e: Event) {
    e.preventDefault();
    const p = remotePath.trim();
    if (!p) return;
    transferring = true;
    try {
      const local = await rad.sshDownload(agent.termId, p);
      panel = null;
      remotePath = '';
      store.toast('success', 'Downloaded', local, 4000);
      rad.fsReveal(local).catch(() => {});
    } catch (err) {
      store.error(err, 'Download failed');
    } finally {
      transferring = false;
    }
  }

  const saved = (() => {
    try {
      return JSON.parse(localStorage.getItem('rad-terminal-agent') ?? localStorage.getItem('rad-sandbox-agent') ?? '{}');
    } catch {
      return {};
    }
  })();
  let collapsed = $state<boolean>(saved.collapsed ?? false);
  let height = $state<number>(saved.height ?? 220);
  let text = $state('');
  let input: HTMLTextAreaElement | undefined = $state();
  let scroller: HTMLDivElement | undefined = $state();
  let open = $state<Record<number, boolean>>({});

  $effect(() => {
    try {
      localStorage.setItem('rad-terminal-agent', JSON.stringify({ collapsed, height }));
    } catch {
      /* private mode */
    }
  });

  $effect(() => {
    agent.items.length;
    agent.status;
    tick().then(() => scroller?.scrollTo({ top: scroller.scrollHeight }));
  });

  function send() {
    const t = text.trim();
    if (!t || agent.busy) return;
    text = '';
    autosize();
    agent.ask(t);
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      send();
    }
  }

  function autosize() {
    if (!input) return;
    input.style.height = 'auto';
    input.style.height = Math.min(input.scrollHeight, 120) + 'px';
  }

  function grow(by: number) {
    height = Math.max(100, Math.min(Math.round(window.innerHeight * 0.65), height + by));
  }

  function startResize(e: PointerEvent) {
    const y0 = e.clientY;
    const h0 = height;
    const move = (m: PointerEvent) => (height = Math.max(100, Math.min(window.innerHeight * 0.65, h0 + (y0 - m.clientY))));
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }

  async function expand() {
    collapsed = false;
    await tick();
    input?.focus();
  }
</script>

{#if collapsed}
  <section class="dock slim">
    <button class="ask" onclick={expand}>
      <Sparkles size={14} />
      <span class="truncate">{agent.busy ? agent.status || 'Working…' : `Ask the agent to do something ${where}…`}</span>
    </button>
    <button class="icon-btn sm" title="Show the agent" onclick={expand}><ChevronUp size={15} /></button>
  </section>
{:else}
  <section class="dock">
    <div class="grip" role="separator" aria-orientation="horizontal" onpointerdown={startResize}></div>
    <div class="head">
      <Sparkles size={14} class="spark" />
      <span class="title">{title}</span>
      <span class="muted sub truncate">runs commands {where}{#if agent.cwd} · <code>{agent.cwd}</code>{/if}</span>
      <span class="spacer"></span>
      {#if agent.kind !== 'sandbox'}
        <button class="icon-btn sm" class:active={panel === 'tasks'} title="Ready-made tasks" onclick={() => (panel = panel === 'tasks' ? null : 'tasks')}><Zap size={13} /></button>
        <button class="icon-btn sm" class:active={panel === 'sudo' || agent.hasSudo} title={agent.hasSudo ? 'Sudo password set (in memory only)' : 'Set the sudo password'} onclick={() => (panel = panel === 'sudo' ? null : 'sudo')}><KeyRound size={13} /></button>
      {/if}
      {#if agent.kind === 'ssh'}
        <button class="icon-btn sm" title="Upload files to the server" disabled={transferring} onclick={upload}><Upload size={13} /></button>
        <button class="icon-btn sm" class:active={panel === 'download'} title="Download a file from the server" disabled={transferring} onclick={() => (panel = panel === 'download' ? null : 'download')}><Download size={13} /></button>
      {/if}
      <label class="askfirst" title="Show each command and wait for Run before it runs. Risky commands (deleting, stopping, rebooting, firewall…) always ask.">
        <input type="checkbox" bind:checked={agent.askFirst} /> Ask before running
      </label>
      <button class="icon-btn sm" title="Clear the conversation" disabled={agent.busy || !agent.items.length} onclick={() => agent.clear()}><Trash2 size={13} /></button>
      <button class="icon-btn sm" title="Smaller" onclick={() => grow(-60)}><Minus size={13} /></button>
      <button class="icon-btn sm" title="Bigger" onclick={() => grow(60)}><Plus size={13} /></button>
      <button class="icon-btn sm" title="Collapse" onclick={() => (collapsed = true)}><ChevronDown size={15} /></button>
    </div>

    {#if agent.kind !== 'sandbox'}
      <div class="facts">
        {#if f}
          {#if f.os}<span class="fact" title="Operating system">{f.os}</span>{/if}
          {#if f.user}<span class="fact" title="Logged in as">{f.root === '1' ? 'root' : f.user}{#if f.root !== '1' && f.sudo}&nbsp;· sudo{/if}</span>{/if}
          {#if f.uptime}<span class="fact" title="Uptime">up {f.uptime}</span>{/if}
          {#if f.load}<span class="fact" title="Load (1, 5, 15 min){f.cpus ? ` on ${f.cpus} CPUs` : ''}">load {f.load}</span>{/if}
          {#if f.mem}<span class="fact" title="Memory used / total">mem {f.mem}</span>{/if}
          {#if f.disk}<span class="fact" class:warn={diskHigh} title="Disk used on /">disk {f.disk}</span>{/if}
          {#if f.failed && f.failed !== '0'}<button class="fact bad" title="Failed services — ask the agent" onclick={() => runTask('Which systemd services have failed, and why? Suggest fixes.')}><TriangleAlert size={11} /> {f.failed} failed</button>{/if}
          {#if f.docker}<span class="fact">docker</span>{/if}
        {:else if agent.factsLoading}
          <span class="fact"><Loader size={11} class="spin" /> Looking at the {agent.kind === 'ssh' ? 'server' : 'computer'}…</span>
        {:else}
          <span class="fact">{agent.kind === 'ssh' ? 'Not connected yet — log in in the terminal, then refresh' : 'No details yet'}</span>
        {/if}
        <button class="icon-btn sm" title="Refresh" disabled={agent.factsLoading} onclick={() => agent.loadFacts()}><RefreshCw size={11} /></button>
      </div>
    {/if}

    {#if panel === 'sudo'}
      <form class="panel" onsubmit={saveSudo}>
        <KeyRound size={13} />
        <span class="pl">Sudo password for {agent.kind === 'ssh' ? (f?.user ?? 'this user') + ' on ' + agent.name : 'this computer'}</span>
        <!-- svelte-ignore a11y_autofocus -->
        <input type="password" class="input sm" bind:value={secret} placeholder={agent.hasSudo ? '•••••••• (set)' : 'password'} autocomplete="off" autofocus />
        <button class="btn sm primary" type="submit" disabled={!secret}>Use</button>
        {#if agent.hasSudo}<button class="btn sm ghost" type="button" onclick={() => agent.setSudo(null).then(() => (panel = null))}>Forget</button>{/if}
        <span class="hint">Kept in memory for this terminal only, never saved.</span>
      </form>
    {:else if panel === 'download'}
      <form class="panel" onsubmit={download}>
        <Download size={13} />
        <!-- svelte-ignore a11y_autofocus -->
        <input class="input sm grow" bind:value={remotePath} placeholder={`Path on the server, e.g. /var/log/nginx/error.log${agent.cwd ? ' (or relative to ' + agent.cwd + ')' : ''}`} spellcheck="false" autofocus />
        <button class="btn sm primary" type="submit" disabled={!remotePath.trim() || transferring}>{#if transferring}<Loader size={12} class="spin" />{/if} Download</button>
      </form>
    {:else if panel === 'tasks'}
      <div class="panel tasks">
        {#each tasks as t (t.label)}<button class="chip" disabled={agent.busy} title={t.prompt} onclick={() => runTask(t.prompt)}>{t.label}</button>{/each}
      </div>
    {/if}

    <div class="thread" bind:this={scroller} style="height:{height}px">
      {#if !agent.items.length}
        <div class="empty">
          {#if agent.kind === 'ssh'}
            Tell the agent what to do on this server — it investigates and makes changes with shell commands, and you watch them run in the terminal above. Deleting, stopping, rebooting and firewall changes always wait for your OK.
          {:else if agent.kind === 'local'}
            Tell the agent what to do on this computer — it runs the commands for you and you watch them in the terminal above.
          {:else}
            Tell the agent what to do — it runs the commands in this sandbox and you watch them in the terminal above.
          {/if}
          <div class="ex">
            {#each tasks as t (t.label)}<button class="chip" title={t.prompt} onclick={() => runTask(t.prompt)}>{t.label}</button>{/each}
          </div>
        </div>
      {/if}
      {#each agent.items as it, i}
        {#if it.kind === 'user'}
          <div class="user">{it.text}</div>
        {:else if it.kind === 'say'}
          <div class="say md selectable">{@html renderMarkdown(it.text)}</div>
        {:else if it.kind === 'error'}
          <div class="err">{it.text}</div>
        {:else if it.kind === 'note'}
          <div class="note"><Info size={12} /> {it.text}</div>
        {:else}
          <div class="cmd" class:fail={it.status === 'done' && it.exit !== 0}>
            <button class="cl" onclick={() => (open[i] = !open[i])} disabled={it.status !== 'done'}>
              {#if it.status === 'running'}<Loader size={13} class="spin" />
              {:else if it.status === 'waiting'}<Clock size={13} />
              {:else if it.status === 'skipped'}<SkipForward size={13} />
              {:else if it.timedOut}<Clock size={13} />
              {:else if it.exit === 0}<CircleCheck size={13} class="ok" />
              {:else}<CircleX size={13} class="bad" />{/if}
              <code class="selectable">{it.command}</code>
              {#if it.status === 'done'}<span class="ex-code">{it.timedOut ? 'still running' : `exit ${it.exit}`}</span>{/if}
            </button>
            {#if it.risky && it.status !== 'done'}
              <div class="risk"><TriangleAlert size={12} /> This command {it.risky}.</div>
            {/if}
            {#if it.status === 'waiting'}
              <div class="approve">
                <button class="btn sm primary" onclick={() => agent.approve(true)}><Play size={12} /> Run</button>
                <button class="btn sm ghost" onclick={() => agent.approve(false)}>Skip</button>
              </div>
            {/if}
            {#if open[i] && it.output !== undefined}
              <pre class="out selectable">{it.output.trim() || '(no output)'}</pre>
            {/if}
          </div>
        {/if}
      {/each}
      {#if agent.busy && agent.status}<div class="status"><Loader size={12} class="spin" /> {agent.status}</div>{/if}
    </div>

    <div class="input-row">
      <textarea
        bind:this={input}
        bind:value={text}
        rows="1"
        placeholder={agent.kind === 'ssh' ? 'e.g. why is the site slow? renew the TLS certificate, set up a nightly backup…' : agent.kind === 'local' ? 'e.g. find large files in Downloads, what is using port 3000…' : 'e.g. list current dir, install numpy and plot something…'}
        onkeydown={onKey}
        oninput={autosize}
      ></textarea>
      {#if agent.busy}
        <button class="send stop" title="Stop the agent" onclick={() => agent.stop()}><Square size={13} /></button>
      {:else}
        <button class="send" title="Send (Enter)" disabled={!text.trim()} onclick={send}><ArrowUp size={16} /></button>
      {/if}
    </div>
  </section>
{/if}

<style>
  .dock {
    flex: none;
    display: flex;
    flex-direction: column;
    border-top: 1px solid var(--border);
    background: var(--surface);
  }
  .dock.slim {
    flex-direction: row;
    align-items: center;
    gap: 6px;
    height: 40px;
    padding: 0 10px 0 14px;
  }
  .ask {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 28px;
    padding: 0 12px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--text-3);
    font-size: 13px;
    text-align: left;
  }
  .ask :global(svg),
  .head :global(.spark) {
    color: var(--accent);
    flex: none;
  }
  .grip {
    height: 6px;
    margin-top: -3px;
    cursor: ns-resize;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 10px 4px 14px;
  }
  .title {
    font-weight: 600;
    font-size: 13px;
  }
  .sub {
    font-size: 12px;
  }
  .askfirst {
    display: flex;
    align-items: center;
    gap: 5px;
    margin-right: 6px;
    font-size: 12px;
    color: var(--text-2);
  }
  .thread {
    overflow-y: auto;
    padding: 6px 14px 10px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 13px;
  }
  .empty {
    color: var(--text-3);
    font-size: 12.5px;
  }
  .ex {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 8px;
  }
  .user {
    align-self: flex-end;
    max-width: 80%;
    padding: 6px 11px;
    border-radius: 14px 14px 4px 14px;
    background: var(--user-bubble);
    white-space: pre-wrap;
  }
  .say {
    max-width: 92%;
  }
  .say :global(p) {
    margin: 2px 0;
  }
  .err {
    color: var(--danger);
    font-size: 12.5px;
  }
  .note {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text-3);
    font-size: 12px;
  }
  .risk {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 0 10px 6px 31px;
    font-size: 12px;
    color: var(--warning);
  }
  .facts {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 5px;
    padding: 0 10px 6px 14px;
  }
  .fact {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 1px 8px;
    border-radius: 999px;
    background: var(--surface-2);
    border: 1px solid var(--border);
    color: var(--text-2);
    font-size: 11.5px;
    white-space: nowrap;
  }
  .fact.warn {
    color: var(--warning);
    border-color: color-mix(in oklab, var(--warning) 40%, var(--border));
  }
  .fact.bad {
    color: var(--danger);
    border-color: color-mix(in oklab, var(--danger) 40%, var(--border));
    background: var(--danger-soft);
  }
  .panel {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin: 0 14px 8px;
    padding: 8px 10px;
    border-radius: var(--r-sm);
    border: 1px solid var(--border);
    background: var(--surface-2);
    font-size: 12.5px;
  }
  .panel :global(svg) {
    color: var(--accent);
  }
  .panel .input {
    width: 220px;
    height: 28px;
    font-size: 12.5px;
  }
  .chip:disabled {
    opacity: 0.5;
  }
  .panel .grow {
    flex: 1;
    min-width: 200px;
  }
  .panel .hint {
    color: var(--text-3);
    font-size: 11.5px;
  }
  .panel.tasks {
    gap: 6px;
  }
  .sub code {
    font-family: var(--mono);
    font-size: 11.5px;
  }
  .cmd {
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    background: var(--surface-2);
    overflow: hidden;
  }
  .cmd.fail {
    border-color: color-mix(in oklab, var(--danger) 40%, var(--border));
  }
  .cl {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    width: 100%;
    padding: 6px 10px;
    border: none;
    background: transparent;
    color: var(--text);
    text-align: left;
  }
  .cl :global(svg) {
    flex: none;
    margin-top: 2px;
    color: var(--text-3);
  }
  .cl :global(.ok) {
    color: var(--success);
  }
  .cl :global(.bad) {
    color: var(--danger);
  }
  .cl code {
    flex: 1;
    min-width: 0;
    font-family: var(--mono);
    font-size: 12px;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .ex-code {
    flex: none;
    font-size: 11px;
    color: var(--text-3);
  }
  .approve {
    display: flex;
    gap: 6px;
    padding: 0 10px 8px 31px;
  }
  .out {
    margin: 0;
    max-height: 220px;
    overflow: auto;
    padding: 8px 10px;
    border-top: 1px solid var(--border);
    background: #14141b;
    color: #e6e6ef;
    font-family: var(--mono);
    font-size: 11.5px;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-3);
  }
  .input-row {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    margin: 0 14px 12px;
    padding: 5px 5px 5px 13px;
    border-radius: var(--r-lg);
    border: 1px solid var(--border);
    background: var(--surface-2);
  }
  .input-row:focus-within {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-ring);
  }
  textarea {
    flex: 1;
    min-height: 22px;
    max-height: 120px;
    padding: 5px 0;
    border: none;
    outline: none;
    resize: none;
    background: transparent;
    color: var(--text);
    font: inherit;
    font-size: 13.5px;
  }
  .send {
    display: grid;
    place-items: center;
    flex: none;
    width: 30px;
    height: 30px;
    border: none;
    border-radius: 50%;
    background: var(--accent);
    color: var(--on-accent);
  }
  .send:disabled {
    opacity: 0.35;
  }
  .send.stop {
    background: var(--text);
    color: var(--surface);
  }
</style>
