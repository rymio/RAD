<script lang="ts">
  import { onMount } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { ask } from '@tauri-apps/plugin-dialog';
  import { revealItemInDir } from '@tauri-apps/plugin-opener';
  import { rad } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import { formatBytes } from '../lib/util';
  import type { SandboxInfo, SandboxProgress, SandboxStatus } from '../lib/types';
  import { Box, Play, RotateCcw, Trash2, FolderOpen, Plus, Loader, Wifi, WifiOff, CircleAlert, Cpu, MemoryStick, HardDrive } from '@lucide/svelte';

  let { onopen }: { onopen: (sb: SandboxInfo) => void } = $props();

  let st = $state<SandboxStatus | null>(null);
  let creating = $state(false);
  let busy = $state<string | null>(null);
  let prog = $state<SandboxProgress | null>(null);
  let form = $state({ name: '', distro: 'alpine', cpus: 2, mem_mb: 1024, disk_gb: 8, network: true });

  const pct = $derived(prog?.total ? Math.floor(((prog.done ?? 0) / prog.total) * 100) : null);

  async function refresh() {
    st = await rad.sandboxStatus().catch(() => null);
    if (st?.progress) prog = st.progress;
    if (st && !st.sandboxes.length) creating = true;
  }

  async function create(e: Event) {
    e.preventDefault();
    busy = 'create';
    try {
      const name = form.name.trim() || st?.distros.find((d) => d.id === form.distro)?.name || 'Sandbox';
      const sb = await rad.sandboxCreate({ ...form, name });
      store.toast('success', 'Sandbox ready', `${sb.name} — opening it in a terminal.`);
      creating = false;
      await refresh();
      const full = st?.sandboxes.find((s) => s.id === sb.id);
      if (full) onopen(full);
    } catch (err) {
      store.error(err, 'Couldn’t create the sandbox');
    } finally {
      busy = null;
      prog = null;
    }
  }

  async function reset(sb: SandboxInfo) {
    if (!(await ask(`Reset “${sb.name}” to a fresh ${sb.distro_name}? Everything installed or saved inside it is erased. Its shared folder is kept.`, { title: 'Reset sandbox', kind: 'warning' }))) return;
    busy = sb.id;
    try {
      await rad.sandboxReset(sb.id);
      store.toast('success', 'Sandbox reset', sb.name);
    } catch (err) {
      store.error(err, 'Reset failed');
    } finally {
      busy = null;
      prog = null;
      refresh();
    }
  }

  async function remove(sb: SandboxInfo) {
    if (!(await ask(`Delete “${sb.name}” and its drive? Its shared folder is deleted too.`, { title: 'Delete sandbox', kind: 'warning' }))) return;
    await rad.sandboxDelete(sb.id).catch((e) => store.error(e));
    refresh();
  }

  async function toggleNet(sb: SandboxInfo) {
    await rad.sandboxUpdate({ id: sb.id, network: !sb.network }).catch((e) => store.error(e));
    refresh();
  }

  async function shared(sb: SandboxInfo) {
    const d = await rad.sandboxSharedDir(sb.id).catch(() => null);
    if (d) revealItemInDir(d);
  }

  onMount(() => {
    refresh();
    const un = listen<SandboxProgress | null>('sandbox-progress', (e) => (prog = e.payload));
    return () => un.then((u) => u());
  });
</script>

{#if !st}
  <div class="center"><Loader size={18} class="spin" /></div>
{:else if !st.supported}
  <div class="note"><CircleAlert size={15} /> Linux sandboxes need a Mac with Apple Silicon for now.</div>
{:else if !st.engine}
  <div class="note">
    <CircleAlert size={15} />
    {#if st.dev_build}
      The sandbox engine isn’t built yet — run <code>./scripts/build-sandbox.sh</code>, then reopen this window.
    {:else}
      This copy of RAD was built without the sandbox engine. Rebuild it with <code>./scripts/build-app.sh</code> and reinstall.
    {/if}
  </div>
{:else}
  <p class="lead">
    Small Linux machines for playing with Python and scripts. Each one has its own drive and sees nothing of this Mac except its
    <strong>shared folder</strong> (<code>~/shared</code> inside). With internet on, it can reach the web but not this Mac’s apps or local servers.
  </p>

  {#each st.sandboxes as sb (sb.id)}
    <div class="sb">
      <Box size={18} class="ic" />
      <div class="spacer">
        <div class="n">{sb.name} {#if sb.running}<span class="pill success">Running</span>{/if}</div>
        <div class="d">
          {sb.distro_name} · {sb.cpus} CPU · {sb.mem_mb >= 1024 ? `${sb.mem_mb / 1024} GB` : `${sb.mem_mb} MB`} RAM ·
          {formatBytes(sb.disk_used)} of {sb.disk_gb} GB
        </div>
      </div>
      {#if busy === sb.id}
        <span class="ph"><Loader size={13} class="spin" /> {prog?.phase ?? 'Working'}{pct !== null ? ` ${pct}%` : ''}…</span>
      {:else}
        <button class="icon-btn" title={sb.network ? 'Internet on — click to cut it off (next start)' : 'No network — click to allow internet (next start)'} onclick={() => toggleNet(sb)}>
          {#if sb.network}<Wifi size={15} />{:else}<WifiOff size={15} />{/if}
        </button>
        <button class="icon-btn" title="Show the shared folder" onclick={() => shared(sb)}><FolderOpen size={15} /></button>
        <button class="icon-btn" title="Reset to a fresh system" disabled={sb.running || !!busy} onclick={() => reset(sb)}><RotateCcw size={15} /></button>
        <button class="icon-btn" title="Delete" disabled={sb.running || !!busy} onclick={() => remove(sb)}><Trash2 size={15} /></button>
        <button class="btn sm primary" onclick={() => onopen(sb)}><Play size={13} /> {sb.running ? 'Show' : 'Open'}</button>
      {/if}
    </div>
  {/each}

  {#if creating}
    <form class="new" onsubmit={create}>
      <div class="section-title">New sandbox</div>
      <div class="distros">
        {#each st.distros as d}
          <button type="button" class="distro" class:on={form.distro === d.id} onclick={() => (form.distro = d.id)} disabled={busy === 'create'}>
            <strong>{d.name}</strong><span>{d.description}</span>
          </button>
        {/each}
      </div>
      <div class="grid">
        <label><span>Name</span><input class="input" bind:value={form.name} placeholder={st.distros.find((d) => d.id === form.distro)?.name} disabled={busy === 'create'} /></label>
        <label><span><Cpu size={12} /> CPUs</span>
          <select class="select" bind:value={form.cpus} disabled={busy === 'create'}>{#each [1, 2, 4, 6] as n}<option value={n}>{n}</option>{/each}</select>
        </label>
        <label><span><MemoryStick size={12} /> Memory</span>
          <select class="select" bind:value={form.mem_mb} disabled={busy === 'create'}>{#each [512, 1024, 2048, 4096, 8192] as n}<option value={n}>{n >= 1024 ? `${n / 1024} GB` : `${n} MB`}</option>{/each}</select>
        </label>
        <label><span><HardDrive size={12} /> Drive</span>
          <select class="select" bind:value={form.disk_gb} disabled={busy === 'create'}>{#each [4, 8, 16, 32, 64] as n}<option value={n}>{n} GB</option>{/each}</select>
        </label>
      </div>
      <label class="net"><input type="checkbox" bind:checked={form.network} disabled={busy === 'create'} /> Internet access <span class="muted">(through a private network — this Mac stays unreachable)</span></label>
      <div class="hint">The drive only takes the space that’s actually used on this Mac.</div>
      {#if busy === 'create'}
        <div class="prog">
          <div class="pl"><Loader size={13} class="spin" /> {prog?.phase ?? 'Starting'}…{pct !== null ? ` ${pct}%` : ''}</div>
          <div class="bar"><div style="width:{pct ?? 8}%" class:indet={pct === null}></div></div>
        </div>
      {:else}
        <div class="row" style="justify-content:flex-end">
          {#if st.sandboxes.length}<button type="button" class="btn ghost" onclick={() => (creating = false)}>Cancel</button>{/if}
          <button class="btn primary"><Plus size={14} /> Create sandbox</button>
        </div>
      {/if}
    </form>
  {:else}
    <button class="btn add" onclick={() => (creating = true)} disabled={!!busy}><Plus size={14} /> New sandbox</button>
  {/if}
{/if}

<style>
  .center {
    display: grid;
    place-items: center;
    padding: 30px;
  }
  .note {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    font-size: 13px;
    color: var(--text-2);
  }
  .lead {
    margin: 0 0 14px;
    font-size: 13px;
    color: var(--text-2);
    line-height: 1.5;
  }
  code {
    font-family: var(--mono);
    font-size: 12px;
    padding: 1px 5px;
    border-radius: var(--r-xs);
    background: var(--code-bg);
  }
  .sb {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    margin-bottom: 8px;
    border: 1px solid var(--border);
    border-radius: var(--r);
  }
  .sb :global(.ic) {
    color: var(--accent);
    flex: none;
  }
  .n {
    font-weight: 600;
    font-size: 13.5px;
  }
  .d,
  .ph {
    font-size: 12px;
    color: var(--text-3);
  }
  .ph {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .new {
    margin-top: 6px;
    padding: 14px;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--surface-2);
  }
  .distros {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 8px;
    margin: 8px 0 12px;
  }
  .distro {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 10px;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    background: var(--surface);
    color: var(--text);
    text-align: left;
    font-size: 13px;
  }
  .distro span {
    font-size: 11.5px;
    color: var(--text-3);
    line-height: 1.35;
  }
  .distro.on {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-ring);
  }
  .grid {
    display: grid;
    grid-template-columns: 2fr 1fr 1fr 1fr;
    gap: 8px;
  }
  .grid label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--text-2);
  }
  .grid label span {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .net {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 12px 0 4px;
    font-size: 13px;
  }
  .hint {
    font-size: 12px;
    color: var(--text-3);
    margin-bottom: 12px;
  }
  .prog {
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .pl {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .bar {
    height: 4px;
    border-radius: 2px;
    background: var(--surface-3);
    overflow: hidden;
  }
  .bar div {
    height: 100%;
    background: var(--accent);
    transition: width var(--t);
  }
  .bar div.indet {
    animation: indet 1.2s var(--ease) infinite;
  }
  @keyframes indet {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(1300%);
    }
  }
  .add {
    margin-top: 4px;
  }
</style>
