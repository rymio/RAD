<script lang="ts">
  import { onMount } from 'svelte';
  import { store } from '../lib/state.svelte';
  import { rad } from '../lib/api';
  import type { RymnetStatus } from '../lib/types';
  import { ensureGuideInVault } from '../lib/rymnetAgent.svelte';
  import RymnetAssistant from './RymnetAssistant.svelte';
  import { Network, Monitor, Zap, Loader, RotateCw, Trash2, Square, Copy, BookOpen, SquareTerminal, ArrowRight } from '@lucide/svelte';

  let st = $state<RymnetStatus | null>(null);
  let address = $state('');
  let busy = $state<'' | 'initiate'>('');
  let rymshOpened = $state(false);
  let steps = $state<string[]>([]);
  let error = $state('');
  let showLog = $state(false);

  const daemonLabel = $derived(
    st?.daemon === 'running' ? 'Running (started by RAD)' : st?.daemon === 'external' ? 'Running (started outside RAD)' : 'Stopped',
  );

  async function refresh() {
    try {
      st = await rad.rymnetStatus();
    } catch (e: any) {
      error = e.message;
    }
  }

  function startRymwin() {
    store.navigate({ kind: 'rymwin' });
  }

  function showRymsh() {
    store.pendingTerminal = { kind: 'rymsh' };
    store.navigate({ kind: 'terminal' });
  }

  /** After joining the network: a rymsh session in Terminal, like any other terminal tab. */
  async function openRymshTab() {
    try {
      const open = (await rad.termList()).some((s) => s.kind === 'rymsh' && !s.exited);
      if (!open) await rad.termOpen({ kind: 'rymsh', cols: 110, rows: 32 });
      rymshOpened = true;
    } catch (e: any) {
      steps = [...steps, `Couldn’t open rymsh: ${e.message}`];
    }
  }

  async function initiate() {
    busy = 'initiate';
    error = '';
    steps = [];
    try {
      steps = await rad.rymnetInitiate(address);
      if (address.trim()) address = '';
      await openRymshTab();
    } catch (e: any) {
      error = e.message;
    } finally {
      busy = '';
      refresh();
    }
  }

  async function removePeer(name: string) {
    try {
      await rad.rymnetRemovePeer(name);
    } catch (e: any) {
      store.toast('error', 'Couldn’t remove the peer', e.message);
    }
    refresh();
  }

  async function stopDaemon() {
    await rad.rymnetStopDaemon().catch(() => {});
    refresh();
  }

  function openGuide() {
    const p = store.projects.find((p) => p.name === 'Rymnet');
    if (p) store.navigate({ kind: 'project', id: p.id });
  }

  onMount(() => {
    refresh();
    ensureGuideInVault();
    const t = setInterval(refresh, 4000);
    return () => clearInterval(t);
  });
</script>

<div class="rymnet">
  <header class="drag">
    <h1>RYMNET</h1>
    <span class="spacer"></span>
    <button class="btn sm ghost no-drag" title="The RYMWIN User Guide, in the Rymnet project — RAD's chat answers from it" onclick={openGuide}><BookOpen size={14} /> User guide</button>
    <button class="btn sm ghost no-drag" onclick={refresh}><RotateCw size={14} /> Refresh</button>
  </header>

  <div class="split">
  <div class="body">
    {#if st && !st.installed}
      <div class="missing">
        <Network size={30} strokeWidth={1.5} />
        <h2>Rymnet isn’t available</h2>
        <p class="hint">Build it with <code>./scripts/build-rymnet.sh</code>, then come back here.</p>
        <button class="btn primary" onclick={refresh}><RotateCw size={14} /> Try again</button>
      </div>
    {:else}
      <section class="card launch">
        <button class="app-icon" onclick={startRymwin} title="Start RYMWIN">
          <span class="tile"><Monitor size={30} strokeWidth={1.6} /></span>
          <span class="lbl">Start RYMWIN</span>
        </button>
        <div class="desc">
          <h2>Rymnet Desktop</h2>
          <p>Opens RYMWIN inside RAD (RYMWIN in the sidebar): File Manager, Browser, Mail, Chat, Terminal and the other Rymnet apps. Log in with your Rymnet account. Drag files from Finder onto it, or use Send to RYMWIN in Files, to copy them into its /Downloads.</p>
          <div class="acts">
            <div class="pill" class:on={st?.rymwin}>{st?.rymwin ? 'Running' : 'Not running'}</div>
            <button class="btn sm ghost" onclick={showRymsh}><SquareTerminal size={13} /> rymsh terminal</button>
          </div>
        </div>
      </section>

      <section class="card">
        <h2>Initiate RYMWIN</h2>
        <p class="hint top">Joins this computer to the Rymnet network: creates the node identity if needed, starts the peer daemon, and connects to the node you enter. That node’s peers are verified and added too.</p>
        <form class="row" onsubmit={(e) => { e.preventDefault(); initiate(); }}>
          <input class="input mono" placeholder="Other node: host or host:port (default port 7444)" bind:value={address} spellcheck="false" autocomplete="off" />
          <button class="btn primary" type="submit" disabled={busy === 'initiate'}>
            {#if busy === 'initiate'}<Loader size={14} class="spin" />{:else}<Zap size={14} />{/if} Initiate RYMWIN
          </button>
        </form>
        {#if steps.length || error}
          <div class="out selectable">
            {#each steps as s}<div>✓ {s}</div>{/each}
            {#if error}<div class="err">✗ {error}</div>{/if}
          </div>
          {#if rymshOpened && !error}
            <div class="next">
              <span>Joined. A <b>rymsh</b> shell is open in Terminal.</span>
              <button class="btn sm" onclick={showRymsh}><SquareTerminal size={13} /> Open rymsh</button>
              <button class="btn sm primary" onclick={startRymwin}><Monitor size={13} /> Open RYMWIN <ArrowRight size={13} /></button>
            </div>
          {/if}
        {/if}

        <dl>
          <dt>Node ID</dt>
          <dd class="mono">
            {#if st?.node_id}
              <span class="selectable truncate">{st.node_id}</span>
              <button class="icon-btn sm" title="Copy" onclick={() => navigator.clipboard.writeText(st!.node_id!)}><Copy size={13} /></button>
            {:else}—{/if}
          </dd>
          <dt>Peer port</dt>
          <dd class="mono">{st?.listen_addr ?? '—'}</dd>
          <dt>Daemon</dt>
          <dd>
            <span class="dot" class:on={st?.daemon !== 'stopped'}></span>{daemonLabel}
            {#if st?.daemon === 'running'}<button class="btn sm ghost" onclick={stopDaemon}><Square size={11} /> Stop</button>{/if}
            {#if st?.daemon_log.length}<button class="btn sm ghost" onclick={() => (showLog = !showLog)}>{showLog ? 'Hide log' : 'Log'}</button>{/if}
          </dd>
        </dl>
        {#if showLog && st?.daemon_log.length}<pre class="log selectable">{st.daemon_log.join('\n')}</pre>{/if}
      </section>

      <section class="card">
        <h2>Peers <span class="count">{st?.peers.length ?? 0}</span></h2>
        {#if st?.peers.length}
          <div class="peers">
            {#each st.peers as p (p.name)}
              <div class="peer">
                <span class="dot on"></span>
                <span class="name">{p.name}</span>
                <span class="ep mono selectable">{p.endpoint}</span>
                <span class="pk mono truncate" title={p.public_key}>{p.public_key}</span>
                <button class="icon-btn sm" title="Remove peer" onclick={() => removePeer(p.name)}><Trash2 size={13} /></button>
              </div>
            {/each}
          </div>
        {:else}
          <p class="hint">No peers yet. Enter another node’s address above and press Initiate RYMWIN.</p>
        {/if}
      </section>
    {/if}
  </div>
  {#if !st || st.installed}
    <aside class="side"><RymnetAssistant onchange={refresh} /></aside>
  {/if}
  </div>
</div>

<style>
  .rymnet {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  header {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
    height: var(--titlebar-h);
    padding: 0 12px 0 calc(14px + var(--lead));
    border-bottom: 1px solid var(--border);
  }
  h1 {
    margin: 0;
    font-size: 15px;
    font-weight: 650;
  }
  .spacer {
    flex: 1;
  }
  .split {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .side {
    flex: none;
    width: clamp(340px, 38%, 480px);
    min-height: 0;
    border-left: 1px solid var(--border);
  }
  .body {
    flex: 1;
    min-width: 0;
    min-height: 0;
    overflow: auto;
    padding: 20px 16px 28px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 14px;
  }
  .card {
    width: 100%;
    max-width: 760px;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--r);
    box-shadow: var(--shadow-sm);
    padding: 16px 18px;
  }
  h2 {
    margin: 0;
    font-size: 14px;
    font-weight: 650;
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .hint.top {
    margin: 4px 0 12px;
  }
  .launch {
    display: flex;
    gap: 18px;
    align-items: center;
  }
  .app-icon {
    flex: none;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 7px;
    padding: 6px;
    border: none;
    background: none;
    border-radius: var(--r);
  }
  .app-icon:hover .tile {
    transform: translateY(-1px);
    box-shadow: var(--shadow);
  }
  .app-icon:active .tile {
    transform: scale(0.97);
  }
  .tile {
    width: 64px;
    height: 64px;
    border-radius: 16px;
    display: grid;
    place-items: center;
    /* Rymnet Desktop's teal. */
    background: linear-gradient(160deg, #3f8a85, #2b6763);
    color: #fff;
    box-shadow: var(--shadow-sm);
    transition:
      transform var(--t-fast) var(--ease),
      box-shadow var(--t-fast) var(--ease);
  }
  .lbl {
    font-size: 12px;
    font-weight: 550;
    color: var(--text-2);
  }
  .desc p {
    margin: 4px 0 8px;
    font-size: 13px;
    color: var(--text-2);
    line-height: 1.5;
  }
  .acts {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .next {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 10px;
    font-size: 13px;
    color: var(--text-2);
  }
  .next span {
    flex: 1;
    min-width: 180px;
  }
  .pill {
    display: inline-block;
    font-size: 11.5px;
    padding: 2px 9px;
    border-radius: 999px;
    background: var(--surface-3);
    color: var(--text-3);
  }
  .pill.on {
    background: var(--success-soft);
    color: var(--success);
  }
  .row {
    display: flex;
    gap: 8px;
  }
  .row .input {
    flex: 1;
    min-width: 0;
  }
  .out {
    margin-top: 10px;
    padding: 9px 11px;
    border-radius: var(--r-sm);
    background: var(--surface-2);
    font-family: var(--mono);
    font-size: 12px;
    line-height: 1.6;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .err {
    color: var(--danger);
  }
  dl {
    display: grid;
    grid-template-columns: 90px 1fr;
    gap: 7px 12px;
    margin: 14px 0 0;
    font-size: 13px;
  }
  dt {
    color: var(--text-3);
  }
  dd {
    margin: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .mono {
    font-family: var(--mono);
    font-size: 12px;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--text-4);
    flex: none;
  }
  .dot.on {
    background: var(--success);
  }
  .log {
    margin: 10px 0 0;
    max-height: 220px;
    overflow: auto;
    padding: 9px 11px;
    border-radius: var(--r-sm);
    background: var(--code-bg);
    font-family: var(--mono);
    font-size: 11.5px;
    line-height: 1.5;
  }
  .count {
    font-size: 11.5px;
    font-weight: 500;
    color: var(--text-3);
  }
  .peers {
    margin-top: 10px;
    display: flex;
    flex-direction: column;
  }
  .peer {
    display: grid;
    grid-template-columns: auto 110px 170px 1fr auto;
    align-items: center;
    gap: 10px;
    padding: 7px 4px;
    border-top: 1px solid var(--border);
    font-size: 13px;
  }
  .peer .name {
    font-weight: 550;
  }
  .peer .pk {
    color: var(--text-3);
  }
  .missing {
    margin: auto;
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: 8px;
    color: var(--text-2);
  }
  @media (max-width: 900px) {
    .split {
      flex-direction: column;
      overflow: auto;
    }
    .body {
      flex: none;
      overflow: visible;
    }
    .side {
      width: auto;
      height: 520px;
      border-left: none;
      border-top: 1px solid var(--border);
    }
  }
  @media (max-width: 640px) {
    .launch {
      flex-direction: column;
      align-items: flex-start;
    }
    .row {
      flex-direction: column;
    }
    .peer {
      grid-template-columns: auto 1fr auto;
    }
    .peer .pk,
    .peer .ep {
      display: none;
    }
  }
</style>
