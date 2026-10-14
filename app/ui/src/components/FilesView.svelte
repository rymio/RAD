<script lang="ts">
  import { onMount } from 'svelte';
  import { rad } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import { formatBytes } from '../lib/util';
  import type { Document, FsEntry, FsListing, FsPlace, FsPreview, Id, SandboxInfo } from '../lib/types';
  import { Home, Monitor, FileText, Download, Image, HardDrive, Folder, File, ArrowLeft, ArrowUp, RefreshCw, Eye, EyeOff, Sparkles, FolderInput, ExternalLink, Search, Loader, Box } from '@lucide/svelte';

  const ICONS: Record<string, any> = { home: Home, desktop: Monitor, docs: FileText, downloads: Download, pictures: Image, drive: HardDrive };
  const IMAGE = /\.(png|jpe?g|gif|webp|bmp|svg|tiff?)$/i;

  let places = $state<FsPlace[]>([]);
  let listing = $state<FsListing | null>(null);
  let history = $state<string[]>([]);
  let error = $state('');
  let loading = $state(false);
  let showHidden = $state(false);
  let filter = $state('');
  let selected = $state<Set<string>>(new Set());
  let anchor = $state<string | null>(null);
  let preview = $state<{ entry: FsEntry; data: FsPreview | null } | null>(null);
  let busy = $state<'import' | 'ask' | null>(null);
  let target = $state<Id | null>(null);
  let sandboxes = $state<SandboxInfo[]>([]);
  let sandboxTarget = $state('');

  async function sendToSandbox() {
    const sb = sandboxes.find((s) => s.id === sandboxTarget);
    if (!sb || !chosen.length) return;
    try {
      const n = await rad.sandboxSend(sb.id, chosen.map((e) => e.path));
      store.toast('success', `${n} file${n === 1 ? '' : 's'} sent to ${sb.name}`, 'Inside the sandbox they’re in ~/shared.');
    } catch (e) {
      store.error(e, 'Couldn’t send the files');
    }
  }

  const visible = $derived(
    (listing?.entries ?? []).filter((e) => (showHidden || !e.hidden) && (!filter.trim() || e.name.toLowerCase().includes(filter.trim().toLowerCase()))),
  );
  const chosen = $derived(visible.filter((e) => selected.has(e.path) && !e.is_dir));
  const crumbs = $derived.by(() => {
    const p = listing?.path ?? '';
    const sep = p.includes('\\') ? '\\' : '/';
    const parts = p.split(sep).filter(Boolean);
    const out: { name: string; path: string }[] = sep === '/' ? [{ name: '/', path: '/' }] : [];
    let acc = sep === '/' ? '' : '';
    for (const part of parts) {
      acc = acc ? `${acc}${sep}${part}` : sep === '/' ? `/${part}` : `${part}${sep}`;
      out.push({ name: part, path: acc });
    }
    return out;
  });
  const projects = $derived(store.projects.filter((p) => !p.archived));

  async function open(path?: string, push = true) {
    loading = true;
    error = '';
    try {
      const l = await rad.fsList(path);
      if (push && listing) history = [...history, listing.path];
      listing = l;
      selected = new Set();
      preview = null;
      filter = '';
      try {
        localStorage.setItem('rad-files-dir', l.path);
      } catch {
        /* ignore */
      }
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  function back() {
    const prev = history.at(-1);
    if (!prev) return;
    history = history.slice(0, -1);
    open(prev, false);
  }

  async function click(e: FsEntry, ev: MouseEvent) {
    if (ev.shiftKey && anchor) {
      const names = visible.map((x) => x.path);
      const [a, b] = [names.indexOf(anchor), names.indexOf(e.path)].sort((x, y) => x - y);
      selected = new Set(names.slice(a, b + 1));
    } else if (ev.metaKey || ev.ctrlKey) {
      const s = new Set(selected);
      s.has(e.path) ? s.delete(e.path) : s.add(e.path);
      selected = s;
      anchor = e.path;
    } else {
      selected = new Set([e.path]);
      anchor = e.path;
    }
    if (!e.is_dir && selected.size === 1) {
      preview = { entry: e, data: null };
      preview = { entry: e, data: await rad.fsPreview(e.path).catch(() => null) };
    }
  }

  function dbl(e: FsEntry) {
    if (e.is_dir) open(e.path);
    else rad.fsOpen(e.path).catch((err) => store.error(err));
  }

  async function addToVault() {
    if (!chosen.length) return;
    busy = 'import';
    try {
      const project = target ?? (await store.inboxProject()).id;
      const res = await rad.uploadPaths(project, chosen.map((e) => e.path));
      const ok = res.filter((r) => 'document' in r).length;
      const errs = res.filter((r): r is { error: string } => 'error' in r);
      if (ok) store.toast('success', `${ok} file${ok > 1 ? 's' : ''} added to ${store.projectMap.get(project)?.name ?? 'your vault'}`, 'Encrypted into your vault and indexed for search.');
      for (const er of errs) store.toast('error', 'Couldn’t add a file', er.error);
    } catch (e) {
      store.error(e, 'Adding files failed');
    } finally {
      busy = null;
    }
  }

  /** Adds the files to the Inbox and starts a chat with them attached. */
  async function ask() {
    if (!chosen.length) return;
    busy = 'ask';
    try {
      const inbox = await store.inboxProject();
      const res = await rad.uploadPaths(inbox.id, chosen.map((e) => e.path), 'chat');
      const docs = res.filter((r): r is { document: Document; duplicate: boolean } => 'document' in r).map((r) => r.document);
      for (const er of res.filter((r): r is { error: string } => 'error' in r)) store.toast('error', 'Couldn’t attach a file', er.error);
      if (docs.length) {
        store.pendingAttachments = docs;
        store.navigate({ kind: 'chat', id: null });
      }
    } catch (e) {
      store.error(e, 'Couldn’t attach the files');
    } finally {
      busy = null;
    }
  }

  function onKey(e: KeyboardEvent) {
    if ((e.target as HTMLElement)?.tagName === 'INPUT') return;
    if (e.key === 'Backspace' && (e.metaKey || e.altKey)) back();
    if (e.key === 'Enter' && selected.size === 1) {
      const it = visible.find((x) => selected.has(x.path));
      if (it) dbl(it);
    }
  }

  const when = (ms: number) => (ms ? new Date(ms).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' }) : '');

  onMount(async () => {
    places = await rad.fsPlaces().catch(() => []);
    rad.sandboxStatus().then((s) => {
      if (s.supported && s.engine) {
        sandboxes = s.sandboxes;
        sandboxTarget = sandboxes[0]?.id ?? '';
      }
    }).catch(() => {});
    let last: string | undefined;
    try {
      last = localStorage.getItem('rad-files-dir') ?? undefined;
    } catch {
      /* ignore */
    }
    await open(last, false);
    if (error && last) open(undefined, false);
  });
</script>

<svelte:window onkeydown={onKey} />

<div class="files">
  <header class="drag">
    <div class="nav no-drag">
      <button class="icon-btn" title="Back" disabled={!history.length} onclick={back}><ArrowLeft size={17} /></button>
      <button class="icon-btn" title="Enclosing folder" disabled={!listing?.parent} onclick={() => listing?.parent && open(listing.parent)}><ArrowUp size={17} /></button>
      <button class="icon-btn" title="Refresh" onclick={() => listing && open(listing.path, false)}><RefreshCw size={15} /></button>
    </div>
    <div class="crumbs no-drag">
      {#each crumbs as c, i}
        {#if i > 0 && crumbs[0].name !== '/' || i > 1}<span class="sep">›</span>{/if}
        <button class="crumb" onclick={() => open(c.path)}>{c.name}</button>
      {/each}
    </div>
    <div class="find no-drag">
      <Search size={14} />
      <input placeholder="Filter" bind:value={filter} spellcheck="false" />
    </div>
    <button class="icon-btn no-drag" title={showHidden ? 'Hide hidden files' : 'Show hidden files'} onclick={() => (showHidden = !showHidden)}>
      {#if showHidden}<EyeOff size={16} />{:else}<Eye size={16} />{/if}
    </button>
  </header>

  <div class="main">
    <nav class="places">
      <div class="section-title">Places</div>
      {#each places as p}
        {@const Icon = ICONS[p.icon] ?? Folder}
        <button class="place" class:active={listing?.path === p.path} onclick={() => open(p.path)}><Icon size={15} /> <span class="truncate">{p.name}</span></button>
      {/each}
    </nav>

    <div class="list">
      {#if error}
        <div class="err">{error}</div>
      {:else if loading && !listing}
        <div class="err"><Loader size={16} class="spin" /></div>
      {:else}
        <div class="row head"><span class="n">Name</span><span class="s">Size</span><span class="m">Modified</span></div>
        {#each visible as e (e.path)}
          <button class="row item" class:sel={selected.has(e.path)} class:hid={e.hidden} onclick={(ev) => click(e, ev)} ondblclick={() => dbl(e)}>
            <span class="n">
              {#if e.is_dir}<Folder size={15} class="dir" />{:else if IMAGE.test(e.name)}<Image size={15} />{:else}<File size={15} />{/if}
              <span class="truncate">{e.name}</span>
            </span>
            <span class="s">{e.is_dir ? '—' : formatBytes(e.size)}</span>
            <span class="m">{when(e.modified)}</span>
          </button>
        {:else}
          <div class="err muted">{filter ? 'Nothing matches the filter.' : 'This folder is empty.'}</div>
        {/each}
      {/if}
    </div>

    {#if preview}
      <aside class="preview">
        <div class="pv">
          {#if !preview.data}
            <Loader size={18} class="spin" />
          {:else if preview.data.kind === 'image'}
            <img src={`data:${preview.data.mime};base64,${preview.data.data}`} alt={preview.entry.name} />
          {:else if preview.data.kind === 'text'}
            <pre class="selectable">{preview.data.text}{preview.data.truncated ? '\n…' : ''}</pre>
          {:else}
            <File size={40} strokeWidth={1.2} />
          {/if}
        </div>
        <div class="meta">
          <strong class="selectable">{preview.entry.name}</strong>
          <span>{formatBytes(preview.entry.size)} · {when(preview.entry.modified)}</span>
          <div class="row" style="gap:6px;margin-top:6px">
            <button class="btn sm" onclick={() => rad.fsOpen(preview!.entry.path)}><ExternalLink size={13} /> Open</button>
            <button class="btn sm ghost" onclick={() => rad.fsReveal(preview!.entry.path)}>Show in folder</button>
          </div>
        </div>
      </aside>
    {/if}
  </div>

  <footer>
    <span class="muted">{chosen.length ? `${chosen.length} file${chosen.length > 1 ? 's' : ''} selected` : 'Select files to add them to your vault or ask about them. ⌘/Shift-click selects several.'}</span>
    <span class="spacer"></span>
    {#if sandboxes.length}
      <select class="select" bind:value={sandboxTarget} title="Linux sandbox">
        {#each sandboxes as sb}<option value={sb.id}>{sb.name}</option>{/each}
      </select>
      <button class="btn sm" disabled={!chosen.length} onclick={sendToSandbox}><Box size={14} /> Send to sandbox</button>
      <span class="sep"></span>
    {/if}
    <select class="select" bind:value={target} title="Project to add the files to">
      <option value={null}>Inbox</option>
      {#each projects.filter((p) => p.name !== 'Inbox') as p}<option value={p.id}>{p.name}</option>{/each}
    </select>
    <button class="btn sm" disabled={!chosen.length || !!busy} onclick={addToVault}>
      {#if busy === 'import'}<Loader size={14} class="spin" />{:else}<FolderInput size={14} />{/if} Add to vault
    </button>
    <button class="btn primary sm" disabled={!chosen.length || !!busy} onclick={ask}>
      {#if busy === 'ask'}<Loader size={14} class="spin" />{:else}<Sparkles size={14} />{/if} Ask AI about {chosen.length > 1 ? 'these' : 'this'}
    </button>
  </footer>
</div>

<style>
  .files {
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
  .nav {
    display: flex;
    gap: 2px;
  }
  .crumbs {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 2px;
    overflow: hidden;
    white-space: nowrap;
  }
  .crumb {
    border: none;
    background: transparent;
    padding: 3px 6px;
    border-radius: var(--r-xs);
    color: var(--text-2);
    font-size: 13px;
    max-width: 200px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .crumb:last-child {
    color: var(--text);
    font-weight: 600;
  }
  .crumb:hover {
    background: var(--surface-hover);
  }
  .sep {
    color: var(--text-4);
  }
  .find {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 30px;
    padding: 0 10px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--text-3);
  }
  .find input {
    width: 140px;
    border: none;
    outline: none;
    background: transparent;
    color: var(--text);
    font: inherit;
    font-size: 13px;
  }
  .main {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .places {
    flex: none;
    width: 180px;
    padding: 12px 8px;
    border-right: 1px solid var(--border);
    overflow-y: auto;
  }
  .places .section-title {
    padding: 0 8px 6px;
  }
  .place {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 30px;
    padding: 0 8px;
    border: none;
    border-radius: var(--r-sm);
    background: transparent;
    color: var(--text-2);
    font-size: 13px;
    text-align: left;
  }
  .place:hover {
    background: var(--surface-hover);
  }
  .place.active {
    background: var(--surface-active);
    color: var(--text);
  }
  .list {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    padding: 4px 8px 12px;
  }
  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 90px 170px;
    gap: 12px;
    align-items: center;
  }
  .head {
    position: sticky;
    top: 0;
    z-index: 1;
    height: 30px;
    padding: 0 10px;
    background: var(--surface);
    color: var(--text-3);
    font-size: 11.5px;
    font-weight: 600;
    border-bottom: 1px solid var(--border);
  }
  .item {
    width: 100%;
    height: 30px;
    padding: 0 10px;
    border: none;
    border-radius: var(--r-xs);
    background: transparent;
    color: var(--text);
    font-size: 13px;
    text-align: left;
  }
  .item:hover {
    background: var(--surface-hover);
  }
  .item.sel {
    background: var(--accent-soft);
  }
  .item.hid {
    opacity: 0.6;
  }
  .n {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .n :global(svg) {
    flex: none;
    color: var(--text-3);
  }
  .n :global(.dir) {
    color: var(--accent);
  }
  .s,
  .m {
    color: var(--text-3);
    font-size: 12px;
    font-variant-numeric: tabular-nums;
  }
  .s {
    text-align: right;
  }
  .err {
    display: grid;
    place-items: center;
    padding: 40px 20px;
    color: var(--text-2);
    font-size: 13px;
    text-align: center;
  }
  .preview {
    flex: none;
    width: 300px;
    display: flex;
    flex-direction: column;
    border-left: 1px solid var(--border);
    min-height: 0;
  }
  .pv {
    flex: 1;
    min-height: 0;
    display: grid;
    place-items: center;
    padding: 12px;
    overflow: auto;
    background: var(--surface-2);
    color: var(--text-3);
  }
  .pv img {
    max-width: 100%;
    max-height: 100%;
    object-fit: contain;
    border-radius: var(--r-sm);
  }
  .pv pre {
    align-self: start;
    justify-self: stretch;
    margin: 0;
    font-family: var(--mono);
    font-size: 11.5px;
    white-space: pre-wrap;
    word-break: break-word;
    color: var(--text);
  }
  .meta {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 12px 14px;
    border-top: 1px solid var(--border);
    font-size: 12px;
    color: var(--text-3);
  }
  .meta strong {
    color: var(--text);
    font-size: 13px;
    word-break: break-all;
  }
  footer {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 14px;
    border-top: 1px solid var(--border);
    font-size: 12.5px;
  }
  .sep {
    width: 1px;
    height: 20px;
    background: var(--border);
  }
  footer .select {
    width: 160px;
    height: 30px;
  }
</style>
