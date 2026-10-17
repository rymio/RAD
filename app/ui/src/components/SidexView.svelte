<script lang="ts">
  import { onMount } from 'svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import { rad } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import type { ContainerInfo, SidexStatus } from '../lib/types';
  import { Code, FolderOpen, Folder, X, Loader, CircleAlert, ExternalLink, Cog, FileCode, GitBranch, SquareTerminal, Puzzle } from '@lucide/svelte';

  let st = $state<SidexStatus | null>(null);
  let containers = $state<ContainerInfo[]>([]);
  let opening = $state<string | null>(null);

  async function refresh() {
    st = await rad.sidexStatus().catch(() => null);
    containers = (await rad.containerStatus().catch(() => null))?.containers ?? [];
  }

  async function launch(folder?: string) {
    opening = folder ?? '';
    try {
      await rad.sidexOpen(folder);
      await refresh();
    } catch (e) {
      store.error(e, 'Couldn’t open SideX');
    } finally {
      // SideX takes a moment to show its window.
      setTimeout(() => (opening = null), 1200);
    }
  }

  async function pick() {
    const r = await open({ directory: true, multiple: false, title: 'Open a folder in SideX' });
    if (typeof r === 'string') launch(r);
  }

  async function forget(path: string) {
    await rad.sidexForget(path);
    refresh();
  }

  async function containerFolder(c: ContainerInfo) {
    const d = await rad.containerSharedDir(c.id).catch((e) => (store.error(e), null));
    if (d) launch(d);
  }

  const short = (p: string) => p.replace(/^\/Users\/[^/]+/, '~');

  onMount(() => {
    refresh();
    const t = setInterval(refresh, 5000);
    return () => clearInterval(t);
  });
</script>

<div class="sx">
  <header class="drag">
    <div class="ttl no-drag"><Code size={16} /> SideX</div>
    <span class="muted sub">VS Code’s editor on Tauri — opens in its own window</span>
    <span class="spacer"></span>
    {#if st?.running}<span class="pill success no-drag">Running</span>{/if}
  </header>

  {#if !st}
    <div class="center"><Loader size={18} class="spin" /></div>
  {:else if !st.supported}
    <div class="center note"><CircleAlert size={15} /> SideX comes with RAD on macOS for now.</div>
  {:else if !st.available}
    <div class="center note">
      <CircleAlert size={15} />
      {#if st.dev_build}
        SideX isn’t built yet — run <code>./scripts/build-sidex.sh</code>, then reopen this view.
      {:else}
        This copy of RAD was built without SideX. Rebuild it with <code>./scripts/build-app.sh</code> and reinstall.
      {/if}
    </div>
  {:else}
    <div class="body">
      <section class="hero">
        <div class="big"><Code size={30} /></div>
        <h2>SideX</h2>
        <p>The Visual Studio Code workbench without Electron: the same editor, terminal and Git, on a Rust backend and the system’s webview.</p>
        <div class="feats">
          <span><SquareTerminal size={12} /> Terminal</span>
          <span><GitBranch size={12} /> Git</span>
          <span><Puzzle size={12} /> Open VSX extensions</span>
        </div>
        <div class="row">
          <button class="btn primary" onclick={pick} disabled={opening !== null}><FolderOpen size={14} /> Open folder…</button>
          <button class="btn" onclick={() => launch()} disabled={opening !== null}>
            {#if opening === ''}<Loader size={14} class="spin" />{:else}<ExternalLink size={14} />{/if}
            {st.running ? 'Show SideX' : 'Open SideX'}
          </button>
        </div>
      </section>

      <section class="list">
        <div class="section-title">Recent folders</div>
        {#each st.recent as r (r.path)}
          <div class="it" class:gone={!r.exists}>
            <button class="open" disabled={!r.exists || opening !== null} onclick={() => launch(r.path)} title={r.exists ? `Open ${r.path} in SideX` : 'This folder doesn’t exist any more'}>
              {#if opening === r.path}<Loader size={14} class="spin" />{:else}<Folder size={14} />{/if}
              <span class="nm truncate">{r.name}</span>
              <span class="pth truncate">{short(r.path)}</span>
            </button>
            <button class="icon-btn sm" title="Remove from the list" onclick={() => forget(r.path)}><X size={13} /></button>
          </div>
        {:else}
          <div class="none">Folders you open in SideX from here show up here.</div>
        {/each}

        {#if containers.length}
          <div class="section-title gap">Container shared folders</div>
          {#each containers as c (c.id)}
            <div class="it">
              <button class="open" disabled={opening !== null} onclick={() => containerFolder(c)} title="Open this container’s shared folder (/root/shared inside it)">
                {#if c.kind === 'rust'}<Cog size={14} />{:else}<FileCode size={14} />{/if}
                <span class="nm truncate">{c.name}</span>
                <span class="pth">shared folder</span>
              </button>
            </div>
          {/each}
          <div class="none">Code that runs in a container goes to its project (/work/app) — copy it to /root/shared to edit it here, or edit it in the container’s own editor.</div>
        {/if}
      </section>
    </div>
  {/if}
</div>

<style>
  .sx {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  header {
    flex: none;
    display: flex;
    align-items: center;
    gap: 10px;
    height: var(--titlebar-h);
    padding: 0 12px 0 calc(14px + var(--lead));
    border-bottom: 1px solid var(--border);
  }
  .ttl {
    display: flex;
    align-items: center;
    gap: 7px;
    font-weight: 650;
    font-size: 14px;
  }
  .ttl :global(svg) {
    color: var(--accent);
  }
  .sub {
    font-size: 12.5px;
  }
  .center {
    flex: 1;
    display: grid;
    place-items: center;
    padding: 24px;
  }
  .note {
    display: flex;
    gap: 8px;
    align-items: center;
    justify-content: center;
    font-size: 13px;
    color: var(--text-2);
  }
  code {
    font-family: var(--mono);
    font-size: 12px;
    padding: 1px 5px;
    border-radius: var(--r-xs);
    background: var(--code-bg);
  }
  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 28px;
    padding: 40px 20px;
  }
  .hero {
    max-width: 520px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    text-align: center;
  }
  .big {
    display: grid;
    place-items: center;
    width: 60px;
    height: 60px;
    border-radius: 16px;
    background: var(--accent-soft, var(--surface-2));
    color: var(--accent);
  }
  h2 {
    margin: 0;
    font-size: 20px;
  }
  p {
    margin: 0;
    color: var(--text-2);
    font-size: 13px;
    line-height: 1.5;
  }
  .feats {
    display: flex;
    gap: 12px;
    font-size: 12px;
    color: var(--text-3);
  }
  .feats span {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .row {
    display: flex;
    gap: 8px;
    margin-top: 6px;
  }
  .list {
    width: 100%;
    max-width: 560px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .gap {
    margin-top: 14px;
  }
  .it {
    display: flex;
    align-items: center;
    gap: 4px;
    border-radius: var(--r-sm);
  }
  .it:hover {
    background: var(--surface-hover);
  }
  .open {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 34px;
    padding: 0 10px;
    border: none;
    background: transparent;
    color: var(--text);
    font-size: 13px;
    text-align: left;
  }
  .open :global(svg) {
    flex: none;
    color: var(--text-3);
  }
  .nm {
    font-weight: 550;
    flex: none;
    max-width: 40%;
  }
  .pth {
    color: var(--text-3);
    font-size: 12px;
    min-width: 0;
  }
  .gone {
    opacity: 0.5;
  }
  .none {
    padding: 6px 10px;
    font-size: 12px;
    color: var(--text-3);
    line-height: 1.45;
  }
</style>
