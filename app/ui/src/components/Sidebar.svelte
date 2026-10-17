<script lang="ts">
  import { store } from '../lib/state.svelte';
  import { musicEngine } from '../lib/media.svelte';
  import { api } from '../lib/api';
  import { dayBucket, PROJECT_COLORS } from '../lib/util';
  import type { Chat } from '../lib/types';
  import Logo from './Logo.svelte';
  import Menu from './Menu.svelte';
  import Modal from './Modal.svelte';
  import VpnToggle from './VpnToggle.svelte';
  import { SquarePen, Search, Plus, Settings, Lock, MoreHorizontal, Pin, PinOff, Trash2, Pencil, Loader, CloudOff, MessageSquare, FolderPlus, Archive, Globe, SquareTerminal, FolderOpen, CalendarDays, Mail, Compass, PanelLeftClose, Music, Clapperboard, CircleHelp, Network, Monitor, Shield, Cog, FileCode, Code, Database, Gamepad2, Joystick, Microscope } from '@lucide/svelte';

  let newProjectOpen = $state(false);
  let npName = $state('');
  let npColor = $state(PROJECT_COLORS[0]);
  let renaming = $state<number | null>(null);
  let renameValue = $state('');
  let showArchived = $state(false);

  const isMac = document.documentElement.dataset.os === 'macos';

  const activeProjects = $derived(store.projects.filter((p) => !p.archived));
  const archived = $derived(store.projects.filter((p) => p.archived));

  const grouped = $derived.by(() => {
    const groups: { label: string; chats: Chat[] }[] = [];
    const pinned = store.chats.filter((c) => c.pinned);
    if (pinned.length) groups.push({ label: 'Pinned', chats: pinned });
    for (const c of store.chats.filter((c) => !c.pinned)) {
      const label = dayBucket(c.updated_at);
      let g = groups.find((g) => g.label === label);
      if (!g) groups.push((g = { label, chats: [] }));
      g.chats.push(c);
    }
    return groups;
  });

  const health = $derived.by(() => {
    if (!store.online) return { cls: 'bad', text: 'Offline — reconnecting…' };
    const h = store.health;
    if (!store.providers.length) return { cls: 'warn', text: 'No model provider yet' };
    if (h.length && h.every((x) => !x.ok)) return { cls: 'bad', text: 'Models unreachable' };
    if (h.some((x) => !x.ok)) return { cls: 'warn', text: 'Some providers offline' };
    return { cls: 'ok', text: store.isLocal ? `On this computer · ${store.whereLabel}` : (store.config?.server_name ?? 'Connected') };
  });

  async function createProject(e: Event) {
    e.preventDefault();
    if (!npName.trim()) return;
    try {
      const p = await api.post<any>('/v1/projects', { name: npName.trim(), color: npColor });
      store.projects = [...store.projects, p];
      newProjectOpen = false;
      npName = '';
      store.navigate({ kind: 'project', id: p.id });
    } catch (err) {
      store.error(err);
    }
  }

  async function togglePin(c: Chat) {
    await api.patch(`/v1/chats/${c.id}`, { pinned: !c.pinned }).catch((e) => store.error(e));
    store.reloadChats();
  }
  async function deleteChat(c: Chat) {
    await api.del(`/v1/chats/${c.id}`).catch((e) => store.error(e));
    store.chats = store.chats.filter((x) => x.id !== c.id);
    if (store.route.kind === 'chat' && store.route.id === c.id) store.navigate({ kind: 'chat', id: null });
  }
  async function commitRename(c: Chat) {
    const t = renameValue.trim();
    renaming = null;
    if (t && t !== c.title) {
      await api.patch(`/v1/chats/${c.id}`, { title: t }).catch((e) => store.error(e));
      store.reloadChats();
    }
  }
</script>

<aside class="sidebar">
  <div class="top drag" class:mac={isMac}>
    <div class="vault no-drag">
      <Logo size={22} />
      <span class="vname truncate">RAD</span>
      <span class="locked" title="Encrypted vault — unlocked"><Lock size={11} /></span>
    </div>
    <VpnToggle />
    <button class="icon-btn no-drag" title="Hide sidebar (⌘\)" onclick={() => store.toggleSidebar()}><PanelLeftClose size={17} /></button>
    <button class="icon-btn no-drag" title="New chat (⌘N)" onclick={() => store.newChat()}><SquarePen size={17} /></button>
  </div>

  <button class="search no-drag" onclick={() => (store.paletteOpen = true)}>
    <Search size={15} /> <span class="spacer">Search or jump to…</span> <span class="kbd">⌘K</span>
  </button>

  <div class="scroll">
    <button class="item web" class:active={store.route.kind === 'browser'} onclick={() => store.navigate({ kind: 'browser' })}>
      <Globe size={15} /> <span class="spacer">Web Browser</span>
    </button>
    <button class="item web" class:active={store.route.kind === 'chromium'} onclick={() => store.navigate({ kind: 'chromium' })}>
      <Compass size={15} /> <span class="spacer">Chromium</span>
    </button>
    <button class="item web" class:active={store.route.kind === 'tor'} onclick={() => store.navigate({ kind: 'tor' })}>
      <Shield size={15} /> <span class="spacer">TOR</span>
    </button>
    <button class="item web" class:active={store.route.kind === 'terminal'} onclick={() => store.navigate({ kind: 'terminal' })}>
      <SquareTerminal size={15} /> <span class="spacer">Terminal</span>
    </button>
    <button class="item web" class:active={store.route.kind === 'games'} onclick={() => store.navigate({ kind: 'games' })}>
      <Gamepad2 size={15} /> <span class="spacer">Games</span>
    </button>
    <button class="item web" class:active={store.route.kind === 'zx'} onclick={() => store.navigate({ kind: 'zx' })}>
      <Joystick size={15} /> <span class="spacer">ZX Spectrum</span>
    </button>
    <button class="item web" class:active={store.route.kind === 'zxlab'} onclick={() => store.navigate({ kind: 'zxlab' })}>
      <Microscope size={15} /> <span class="spacer">Spectrum Lab</span>
    </button>
    <button class="item web" class:active={store.route.kind === 'database'} onclick={() => store.navigate({ kind: 'database' })}>
      <Database size={15} /> <span class="spacer">Database</span>
    </button>
    <button class="item web" class:active={store.route.kind === 'sidex'} onclick={() => store.navigate({ kind: 'sidex' })}>
      <Code size={15} /> <span class="spacer">SideX</span>
    </button>
    <button class="item web" class:active={store.route.kind === 'container' && store.route.type === 'rust'} onclick={() => store.navigate({ kind: 'container', type: 'rust' })}>
      <Cog size={15} /> <span class="spacer">Rust Container</span>
    </button>
    <button class="item web" class:active={store.route.kind === 'container' && store.route.type === 'python'} onclick={() => store.navigate({ kind: 'container', type: 'python' })}>
      <FileCode size={15} /> <span class="spacer">Python Container</span>
    </button>
    <button class="item web" class:active={store.route.kind === 'files'} onclick={() => store.navigate({ kind: 'files' })}>
      <FolderOpen size={15} /> <span class="spacer">Files</span>
    </button>
    <button class="item web" class:active={store.route.kind === 'calendar'} onclick={() => store.navigate({ kind: 'calendar' })}>
      <CalendarDays size={15} /> <span class="spacer">Calendar</span>
    </button>
    <button class="item web" class:active={store.route.kind === 'email'} onclick={() => store.navigate({ kind: 'email' })}>
      <Mail size={15} /> <span class="spacer">Email</span>
    </button>
    <button class="item web" class:active={store.route.kind === 'music'} onclick={() => store.navigate({ kind: 'music' })}>
      <Music size={15} /> <span class="spacer">Music</span>
      {#if musicEngine.status.state === 'playing'}<span class="playing" title="Playing"><i></i><i></i><i></i></span>{/if}
    </button>
    <button class="item web" class:active={store.route.kind === 'video'} onclick={() => store.navigate({ kind: 'video' })}>
      <Clapperboard size={15} /> <span class="spacer">Video</span>
    </button>
    <button class="item web" class:active={store.route.kind === 'rymnet'} onclick={() => store.navigate({ kind: 'rymnet' })}>
      <Network size={15} /> <span class="spacer">RYMNET</span>
    </button>
    <button class="item web" class:active={store.route.kind === 'rymwin'} onclick={() => store.navigate({ kind: 'rymwin' })}>
      <Monitor size={15} /> <span class="spacer">RYMWIN</span>
    </button>
    <div class="sect">
      <div class="sect-h">
        <span class="section-title">Projects</span>
        <button class="icon-btn sm" title="New project" onclick={() => (newProjectOpen = true)}><Plus size={14} /></button>
      </div>
      {#each activeProjects as p (p.id)}
        <button class="item" class:active={store.route.kind === 'project' && store.route.id === p.id} onclick={() => store.navigate({ kind: 'project', id: p.id })}>
          <span class="pdot" style="background:{p.color}"></span>
          <span class="spacer truncate">{p.name}</span>
          <span class="count">{p.document_count + p.note_count || ''}</span>
        </button>
      {:else}
        <button class="item empty" onclick={() => (newProjectOpen = true)}><FolderPlus size={15} /> Create your first project</button>
      {/each}
      {#if archived.length}
        <button class="item sub" onclick={() => (showArchived = !showArchived)}><Archive size={14} /> {showArchived ? 'Hide' : 'Show'} archived ({archived.length})</button>
        {#if showArchived}
          {#each archived as p (p.id)}
            <button class="item dim" class:active={store.route.kind === 'project' && store.route.id === p.id} onclick={() => store.navigate({ kind: 'project', id: p.id })}>
              <span class="pdot" style="background:{p.color}"></span><span class="spacer truncate">{p.name}</span>
            </button>
          {/each}
        {/if}
      {/if}
    </div>

    <div class="sect">
      <div class="sect-h">
        <span class="section-title">Chats</span>
        <button class="icon-btn sm" title="New chat" onclick={() => store.newChat()}><Plus size={14} /></button>
      </div>
      {#each grouped as g (g.label)}
        <div class="bucket">{g.label}</div>
        {#each g.chats as c (c.id)}
          <div class="chat-row" class:active={store.route.kind === 'chat' && store.route.id === c.id}>
            {#if renaming === c.id}
              <!-- svelte-ignore a11y_autofocus -->
              <input class="input rename" autofocus bind:value={renameValue} onblur={() => commitRename(c)} onkeydown={(e) => { if (e.key === 'Enter') commitRename(c); if (e.key === 'Escape') renaming = null; }} />
            {:else}
              <button class="item chat" onclick={() => store.navigate({ kind: 'chat', id: c.id })} ondblclick={() => ((renaming = c.id), (renameValue = c.title))}>
                {#if store.streamingChats[c.id]}<Loader size={13} class="spin" />{:else}<MessageSquare size={13} class="ci" />{/if}
                <span class="spacer truncate">{c.title}</span>
              </button>
              <div class="row-menu">
                <Menu
                  items={[
                    { label: c.pinned ? 'Unpin' : 'Pin', icon: c.pinned ? PinOff : Pin, onclick: () => togglePin(c) },
                    { label: 'Rename', icon: Pencil, onclick: () => ((renaming = c.id), (renameValue = c.title)) },
                    null,
                    { label: 'Delete chat', icon: Trash2, danger: true, onclick: () => deleteChat(c) },
                  ]}
                >
                  {#snippet trigger(toggle)}
                    <button class="icon-btn sm" onclick={toggle} aria-label="Chat menu"><MoreHorizontal size={14} /></button>
                  {/snippet}
                </Menu>
              </div>
            {/if}
          </div>
        {/each}
      {:else}
        <div class="none">Your conversations appear here.</div>
      {/each}
    </div>
  </div>

  <div class="bottom">
    {#if store.uploading}
      <div class="activity">
        <Loader size={13} class="spin" />
        <span class="truncate spacer">{store.uploading.phase === 'hashing' ? 'Preparing' : 'Uploading'} {store.uploading.name}</span>
        {#if store.uploading.total}<span class="pct">{Math.round(((store.uploading.sent ?? 0) / store.uploading.total) * 100)}%</span>{/if}
      </div>
    {:else if store.jobs.queued + store.jobs.running > 0}
      <div class="activity"><Loader size={13} class="spin" /><span class="spacer">Indexing {store.jobs.queued + store.jobs.running} item{store.jobs.queued + store.jobs.running > 1 ? 's' : ''}…</span></div>
    {/if}
    <div class="status">
      {#if store.online}<span class="dot {health.cls}"></span>{:else}<CloudOff size={14} color="var(--danger)" />{/if}
      <span class="truncate spacer" title={health.text}>{health.text}</span>
      <button class="icon-btn" title="About RAD" onclick={() => (store.aboutOpen = true)}><CircleHelp size={16} /></button>
      <button class="icon-btn" title="Settings (⌘,)" class:active={store.route.kind === 'settings'} onclick={() => store.navigate({ kind: 'settings' })}><Settings size={16} /></button>
      <button class="icon-btn" title="Lock (⌘⇧L)" onclick={() => store.lock()}><Lock size={16} /></button>
    </div>
  </div>
</aside>

<Modal bind:open={newProjectOpen} title="New project" width={420}>
  <form onsubmit={createProject}>
    <div class="field">
      <label class="label" for="pn">Name</label>
      <!-- svelte-ignore a11y_autofocus -->
      <input id="pn" class="input" bind:value={npName} placeholder="e.g. House, Taxes 2025, Thesis" autofocus />
    </div>
    <div class="field">
      <span class="label">Color</span>
      <div class="swatches">
        {#each PROJECT_COLORS as c}
          <button type="button" class="sw" class:sel={npColor === c} style="background:{c}" onclick={() => (npColor = c)} aria-label={c}></button>
        {/each}
      </div>
    </div>
    <div class="row" style="justify-content:flex-end">
      <button type="button" class="btn ghost" onclick={() => (newProjectOpen = false)}>Cancel</button>
      <button class="btn primary" disabled={!npName.trim()}>Create project</button>
    </div>
  </form>
</Modal>

<style>
  .sidebar {
    width: var(--sidebar-w);
    flex: none;
    display: flex;
    flex-direction: column;
    background: var(--bg-sidebar);
    min-height: 0;
  }
  :global(html:not(.translucent)) .sidebar {
    background: var(--bg);
  }
  .top {
    display: flex;
    align-items: center;
    gap: 6px;
    height: var(--titlebar-h);
    padding: 0 10px 0 14px;
    flex: none;
  }
  .top.mac {
    padding-left: 84px;
  }
  .vault {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .vname {
    font-weight: 700;
    letter-spacing: 0.02em;
  }
  .locked {
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: var(--success-soft);
    color: var(--success);
  }
  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 2px 10px 8px;
    height: 34px;
    padding: 0 8px 0 11px;
    border-radius: var(--r-sm);
    border: 1px solid var(--border);
    background: color-mix(in oklab, var(--surface) 70%, transparent);
    color: var(--text-3);
    font-size: 13px;
    transition: border-color var(--t-fast), background var(--t-fast);
  }
  .search:hover {
    border-color: var(--border-strong);
    background: var(--surface);
  }
  .scroll {
    flex: 1;
    overflow-y: auto;
    padding: 0 8px 12px;
  }
  .sect {
    margin-top: 12px;
  }
  .sect-h {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 4px 4px 8px;
  }
  .item {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    height: 32px;
    padding: 0 8px;
    border: none;
    border-radius: var(--r-sm);
    background: transparent;
    color: var(--text-2);
    font-size: 13.5px;
    text-align: left;
    transition: background var(--t-fast), color var(--t-fast);
  }
  .item:hover {
    background: var(--surface-hover);
    color: var(--text);
  }
  .item.active,
  .chat-row.active .item {
    background: var(--surface-active);
    color: var(--text);
    font-weight: 550;
  }
  .item.web :global(svg) {
    color: var(--accent);
  }
  .item.empty,
  .item.sub {
    color: var(--text-3);
    font-size: 12.5px;
  }
  .item.dim {
    opacity: 0.7;
  }
  .pdot {
    width: 9px;
    height: 9px;
    border-radius: 3px;
    flex: none;
    margin-left: 2px;
    box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.08);
  }
  .count {
    font-size: 11.5px;
    color: var(--text-4);
    font-variant-numeric: tabular-nums;
  }
  .bucket {
    padding: 10px 8px 3px;
    font-size: 11.5px;
    color: var(--text-4);
    font-weight: 550;
  }
  .chat-row {
    position: relative;
  }
  .item.chat :global(.ci) {
    color: var(--text-4);
    flex: none;
  }
  .row-menu {
    position: absolute;
    right: 4px;
    top: 4px;
    opacity: 0;
    transition: opacity var(--t-fast);
  }
  .chat-row:hover .row-menu,
  .chat-row:focus-within .row-menu {
    opacity: 1;
  }
  .chat-row:hover .item.chat {
    padding-right: 30px;
  }
  .rename {
    height: 30px;
  }
  .none {
    padding: 6px 8px;
    color: var(--text-4);
    font-size: 12.5px;
  }
  .bottom {
    flex: none;
    padding: 6px 8px 10px;
    border-top: 1px solid var(--border);
  }
  .activity {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 28px;
    padding: 0 8px;
    font-size: 12px;
    color: var(--accent);
  }
  .pct {
    font-variant-numeric: tabular-nums;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 8px;
    height: 36px;
    padding: 0 2px 0 10px;
    font-size: 12.5px;
    color: var(--text-3);
  }
  .swatches {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  .sw {
    width: 24px;
    height: 24px;
    border-radius: 8px;
    border: 2px solid transparent;
    box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.1);
    transition: transform var(--t-fast) var(--ease-spring);
  }
  .sw:hover {
    transform: scale(1.12);
  }
  .sw.sel {
    border-color: var(--surface);
    box-shadow: 0 0 0 2px var(--text);
  }
  /* Music is playing: three bouncing bars. */
  .playing {
    display: inline-flex;
    align-items: flex-end;
    gap: 2px;
    height: 11px;
  }
  .playing i {
    width: 2px;
    height: 100%;
    background: var(--accent);
    border-radius: 1px;
    animation: bars 0.9s ease-in-out infinite alternate;
  }
  .playing i:nth-child(2) {
    animation-delay: -0.3s;
  }
  .playing i:nth-child(3) {
    animation-delay: -0.6s;
  }
  @keyframes bars {
    from {
      transform: scaleY(0.3);
    }
    to {
      transform: scaleY(1);
    }
  }
  .playing i {
    transform-origin: bottom;
  }
</style>
