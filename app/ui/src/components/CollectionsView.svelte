<script lang="ts">
  // Collections: folders and files dropped on RAD (PDF, MP3, FLAC, M4A, MP4), kept on this
  // computer. Read, watch and listen here; move on to Media when wanted. See lib/collections.svelte.ts.
  import { onMount, tick } from 'svelte';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { open as pickPaths, ask as confirmDialog } from '@tauri-apps/plugin-dialog';
  import { store } from '../lib/state.svelte';
  import { collections, fileUrl, stem, COLLECTION_EXTENSIONS, type Entry, type FileRef } from '../lib/collections.svelte';
  import { formatBytes, timeAgo } from '../lib/util';
  import { fmtTime } from '../lib/media.svelte';
  import PdfView from './PdfView.svelte';
  import Menu from './Menu.svelte';
  import Modal from './Modal.svelte';
  import { LibraryBig, Folder, FolderPlus, FilePlus, FileText, Music, Clapperboard, ChevronRight, ArrowLeft, Play, Pause, SkipBack, SkipForward, X, MoreHorizontal, Pencil, Trash2, FolderInput, DatabaseZap, Check, Loader, Upload, FolderOpen, Volume2, ExternalLink, BookOpen, Eye, Headphones } from '@lucide/svelte';

  let selected = $state<Set<string>>(new Set());
  let filter = $state('');
  let dropOver = $state(false);
  /** The folder row a drop would go into (null: the open folder). */
  let dropDir = $state<string | null>(null);
  let adding = $state<string | null>(null);
  let videoError = $state(false);

  let nameOpen = $state(false);
  let nameMode = $state<'folder' | 'rename'>('folder');
  let nameValue = $state('');
  let nameTarget: Entry | null = null;
  let nameInput: HTMLInputElement | undefined = $state();

  const crumbs = $derived(collections.dir ? collections.dir.split('/') : []);
  const shown = $derived(collections.entries.filter((e) => !filter.trim() || e.name.toLowerCase().includes(filter.trim().toLowerCase())));
  const allSelected = $derived(shown.length > 0 && shown.every((e) => selected.has(e.path)));
  const viewing = $derived(collections.viewing);
  const song = $derived(collections.playing);

  onMount(() => {
    collections.load().catch((e) => store.error(e, 'Couldn’t open Collections'));
    let un: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((ev) => {
        const p = ev.payload;
        if (p.type === 'enter' || p.type === 'over') {
          dropOver = true;
          dropDir = folderAt(p.position.x, p.position.y);
        } else if (p.type === 'leave') {
          dropOver = false;
          dropDir = null;
        } else if (p.type === 'drop') {
          const into = folderAt(p.position.x, p.position.y) ?? collections.dir;
          dropOver = false;
          dropDir = null;
          void collections.import(p.paths, into);
        }
      })
      .then((u) => (un = u));
    return () => un?.();
  });

  // A new folder shows nothing selected.
  $effect(() => {
    void collections.dir;
    selected = new Set();
    filter = '';
  });

  /** The folder row under the pointer (the drop position is in physical pixels). */
  function folderAt(x: number, y: number): string | null {
    if (viewing) return null;
    const r = window.devicePixelRatio || 1;
    const el = document.elementFromPoint(x / r, y / r)?.closest<HTMLElement>('[data-drop-dir]');
    return el?.dataset.dropDir ?? null;
  }

  const ref = (e: Entry): FileRef => ({ path: e.path, name: e.name, kind: e.kind!, size: e.size });

  function activate(e: Entry) {
    if (e.is_dir) collections.open(e.path);
    else {
      videoError = false;
      void collections.play(ref(e));
    }
  }

  function toggleSel(path: string) {
    const s = new Set(selected);
    if (s.has(path)) s.delete(path);
    else s.add(path);
    selected = s;
  }

  function toggleAll() {
    selected = allSelected ? new Set() : new Set(shown.map((e) => e.path));
  }

  async function addFiles() {
    const r = await pickPaths({ multiple: true, filters: [{ name: 'PDF, music and video', extensions: COLLECTION_EXTENSIONS }] });
    const list = Array.isArray(r) ? r : r ? [r] : [];
    if (list.length) await collections.import(list.map(String));
  }

  async function addFolder() {
    const r = await pickPaths({ directory: true, multiple: true });
    const list = Array.isArray(r) ? r : r ? [r] : [];
    if (list.length) await collections.import(list.map(String));
  }

  async function askName(mode: 'folder' | 'rename', target: Entry | null = null) {
    nameMode = mode;
    nameTarget = target;
    nameValue = target ? (target.is_dir ? target.name : stem(target.name)) : 'New folder';
    nameOpen = true;
    await tick();
    nameInput?.select();
  }

  async function commitName(ev?: Event) {
    ev?.preventDefault();
    const v = nameValue.trim();
    if (!v) return;
    nameOpen = false;
    try {
      if (nameMode === 'folder') await collections.mkdir(v);
      else if (nameTarget) await collections.rename(nameTarget, v);
    } catch (e) {
      store.error(e, nameMode === 'folder' ? 'Couldn’t make the folder' : 'Couldn’t rename it');
    }
  }

  async function remove(paths: string[]) {
    const one = paths.length === 1 ? collections.entries.find((e) => e.path === paths[0]) : null;
    const what = one ? `“${one.name}”${one.is_dir ? ' and everything in it' : ''}` : `${paths.length} items`;
    const ok = await confirmDialog(`Delete ${what} from Collections? The copy RAD keeps on this computer is deleted.`, { title: 'Delete', kind: 'warning' });
    if (!ok) return;
    await collections.remove(paths).catch((e) => store.error(e, 'Couldn’t delete'));
    selected = new Set();
  }

  async function toMedia(paths: string[]) {
    await collections.toMedia(paths);
    selected = new Set();
  }

  async function toRag(path: string) {
    adding = path;
    try {
      await collections.addToRag(path);
    } catch (e) {
      store.error(e, 'Couldn’t add it to the vault');
    } finally {
      adding = null;
    }
  }

  const where = (e: Entry) => (e.kind === 'pdf' ? 'Books' : e.kind === 'audio' ? 'Music' : e.kind === 'video' ? 'Video' : 'Media');

  function folderInfo(e: Entry) {
    if (!e.count) return 'Empty';
    const parts = [e.audio && `${e.audio} song${e.audio > 1 ? 's' : ''}`, e.video && `${e.video} video${e.video > 1 ? 's' : ''}`, e.pdf && `${e.pdf} PDF${e.pdf > 1 ? 's' : ''}`].filter(Boolean);
    return `${parts.join(' · ')} · ${formatBytes(e.size)}`;
  }

  function menuFor(e: Entry) {
    const playable = e.is_dir ? e.audio + e.video > 0 : true;
    return [
      e.is_dir
        ? { label: 'Open folder', icon: FolderOpen, onclick: () => collections.open(e.path) }
        : { label: e.kind === 'pdf' ? 'Open' : e.kind === 'audio' ? 'Listen' : 'Watch', icon: e.kind === 'pdf' ? Eye : e.kind === 'audio' ? Headphones : Play, onclick: () => activate(e) },
      ...(e.is_dir && playable ? [{ label: e.audio ? 'Listen to all' : 'Watch', icon: e.audio ? Headphones : Play, onclick: () => collections.playFolder(e) }] : []),
      ...(!e.is_dir && e.kind !== 'pdf' ? [{ label: `Play in the ${e.kind === 'audio' ? 'Music' : 'Video'} player`, icon: ExternalLink, onclick: () => collections.playInMedia(ref(e)) }] : []),
      null,
      { label: e.is_dir ? 'Move all to Media' : `Move to ${where(e)}`, icon: FolderInput, disabled: collections.moving || (e.is_dir && !e.count), onclick: () => toMedia([e.path]) },
      ...(!e.is_dir && e.kind === 'pdf'
        ? [{ label: collections.inVault[e.path] ? 'In your vault (RAG)' : 'Add to RAG', icon: collections.inVault[e.path] ? Check : DatabaseZap, disabled: !!collections.inVault[e.path] || adding === e.path, onclick: () => toRag(e.path) }]
        : []),
      null,
      { label: 'Rename', icon: Pencil, onclick: () => askName('rename', e) },
      { label: 'Show in Finder', icon: FolderOpen, onclick: () => collections.reveal(e.path) },
      { label: 'Delete', icon: Trash2, danger: true, onclick: () => remove([e.path]) },
    ];
  }

  function onKey(e: KeyboardEvent) {
    const t = e.target as HTMLElement | null;
    if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return;
    if (nameOpen) return;
    if (e.key === 'Escape' && viewing) {
      e.preventDefault();
      collections.closeViewer();
    } else if (e.key === ' ' && song && !viewing) {
      e.preventDefault();
      collections.toggle();
    } else if (e.key === 'Backspace' && !viewing && collections.dir && !selected.size) {
      e.preventDefault();
      collections.open(crumbs.slice(0, -1).join('/'));
    } else if ((e.key === 'Delete' || e.key === 'Backspace') && selected.size && !viewing) {
      e.preventDefault();
      void remove([...selected]);
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="coll">
  <header class="drag">
    {#if viewing}
      <button class="icon-btn no-drag" title="Back (Esc)" onclick={() => collections.closeViewer()}><ArrowLeft size={16} /></button>
      {#if viewing.kind === 'pdf'}<FileText size={16} />{:else}<Clapperboard size={16} />{/if}
      <h1 class="truncate">{stem(viewing.name)}</h1>
      <span class="spacer"></span>
      {#if viewing.kind === 'pdf'}
        <button class="btn sm ghost no-drag" disabled={!!collections.inVault[viewing.path] || adding === viewing.path} title="Upload it to your vault, so every chat (and Gilgamesh) can search it" onclick={() => toRag(viewing!.path)}>
          {#if adding === viewing.path}<Loader size={13} class="spin" />{:else if collections.inVault[viewing.path]}<Check size={13} />{:else}<DatabaseZap size={13} />{/if}
          {collections.inVault[viewing.path] ? 'In RAG' : 'Add to RAG'}
        </button>
      {:else}
        <button class="btn sm ghost no-drag" title="Play it in the Video player (libVLC)" onclick={() => collections.playInMedia(viewing!)}><ExternalLink size={13} /> Video player</button>
      {/if}
      <button class="btn sm no-drag" disabled={collections.moving} onclick={() => toMedia([viewing!.path])}>
        {#if collections.moving}<Loader size={13} class="spin" />{:else if viewing.kind === 'pdf'}<BookOpen size={13} />{:else}<FolderInput size={13} />{/if}
        Move to {viewing.kind === 'pdf' ? 'Books' : 'Video'}
      </button>
    {:else}
      <LibraryBig size={17} />
      <nav class="crumbs no-drag">
        <button class="crumb" class:cur={!crumbs.length} onclick={() => collections.open('')}>Collections</button>
        {#each crumbs as c, i (i)}
          <ChevronRight size={13} class="sep" />
          <button class="crumb truncate" class:cur={i === crumbs.length - 1} onclick={() => collections.open(crumbs.slice(0, i + 1).join('/'))}>{c}</button>
        {/each}
      </nav>
      <span class="spacer"></span>
      {#if collections.entries.length > 8}<input class="input sm no-drag search" placeholder="Filter…" bind:value={filter} />{/if}
      <button class="icon-btn no-drag" title="New folder" onclick={() => askName('folder')}><FolderPlus size={16} /></button>
      <button class="btn sm ghost no-drag" disabled={!!collections.importing} onclick={addFiles}><FilePlus size={14} /> Add files</button>
      <button class="btn sm primary no-drag" disabled={!!collections.importing} onclick={addFolder}>{#if collections.importing}<Loader size={13} class="spin" />{:else}<Upload size={13} />{/if} Add folder</button>
    {/if}
  </header>

  {#if collections.importing}
    <div class="bar busy">
      <Loader size={13} class="spin" />
      <span class="truncate">Copying into Collections{collections.importing.done ? ` · ${collections.importing.done} file${collections.importing.done > 1 ? 's' : ''}` : '…'}{collections.importing.name ? ` · ${collections.importing.name}` : ''}</span>
    </div>
  {/if}

  {#if viewing}
    <div class="viewer">
      {#if viewing.kind === 'pdf'}
        {#key viewing.path}<PdfView url={fileUrl(viewing.path)} />{/key}
      {:else}
        <div class="film">
          {#key viewing.path}
            <!-- svelte-ignore a11y_media_has_caption -->
            <video src={fileUrl(viewing.path)} controls autoplay onplay={() => collections.quietMusic()} onerror={() => (videoError = true)}></video>
          {/key}
          {#if videoError}
            <div class="vfail">
              <p>This video can’t be played here (its format isn’t one the window can show).</p>
              <button class="btn primary sm" onclick={() => collections.playInMedia(viewing!)}><ExternalLink size={13} /> Play in the Video player</button>
            </div>
          {/if}
        </div>
      {/if}
    </div>
  {:else}
    {#if selected.size}
      <div class="bar sel">
        <span><strong>{selected.size}</strong> selected</span>
        <span class="spacer"></span>
        <button class="btn sm" disabled={collections.moving} onclick={() => toMedia([...selected])}>{#if collections.moving}<Loader size={13} class="spin" />{:else}<FolderInput size={13} />{/if} Move to Media</button>
        <button class="btn sm ghost danger" onclick={() => remove([...selected])}><Trash2 size={13} /> Delete</button>
        <button class="icon-btn sm" title="Clear the selection" onclick={() => (selected = new Set())}><X size={14} /></button>
      </div>
    {/if}

    <div class="body" class:drop={dropOver && !dropDir}>
      {#if !collections.loaded}
        <div class="center"><Loader size={20} class="spin" /></div>
      {:else if !collections.entries.length}
        <div class="center">
          <div class="empty">
            <div class="glyph"><LibraryBig size={28} /></div>
            {#if collections.dir}
              <h2>This folder is empty</h2>
              <p>Drop files or folders here, or add them with the buttons above.</p>
            {:else}
              <h2>Everything in one place</h2>
              <p>Drop whole folders or single files on this window: PDFs, music (MP3, FLAC, M4A) and videos (MP4). They’re copied to this computer and keep their folders. Read, listen and watch here, and move them to Music, Video or Books when you want.</p>
              <div class="row">
                <button class="btn primary" onclick={addFolder}><Upload size={14} /> Add a folder</button>
                <button class="btn" onclick={addFiles}><FilePlus size={14} /> Add files</button>
              </div>
            {/if}
          </div>
        </div>
      {:else}
        <div class="list" role="list">
          <div class="row head">
            <input type="checkbox" checked={allSelected} onchange={toggleAll} title="Select all" />
            <span class="name">Name</span>
            <span class="info">Contents</span>
            <span class="date">Added</span>
            <span class="acts"></span>
          </div>
          {#each shown as e (e.path)}
            <div class="row item" role="listitem" class:selected={selected.has(e.path)} class:target={dropOver && dropDir === e.path} class:now={song?.path === e.path || (e.is_dir && song?.path.startsWith(`${e.path}/`))} data-drop-dir={e.is_dir ? e.path : undefined}>
              <input type="checkbox" checked={selected.has(e.path)} onchange={() => toggleSel(e.path)} />
              <button class="name" onclick={() => activate(e)} title={e.name}>
                <span class="ico {e.is_dir ? 'dir' : e.kind}">
                  {#if e.is_dir}<Folder size={16} />{:else if e.kind === 'pdf'}<FileText size={16} />{:else if e.kind === 'audio'}<Music size={16} />{:else}<Clapperboard size={16} />{/if}
                </span>
                <span class="truncate">{e.is_dir ? e.name : stem(e.name)}</span>
                {#if !e.is_dir}<span class="ext">{e.name.split('.').pop()?.toUpperCase()}</span>{/if}
                {#if collections.inVault[e.path]}<span class="rag" title="In your vault (searchable from any chat)"><Check size={10} /> RAG</span>{/if}
              </button>
              <span class="info muted truncate">{e.is_dir ? folderInfo(e) : formatBytes(e.size)}</span>
              <span class="date muted">{timeAgo(e.modified)}</span>
              <span class="acts">
                {#if e.is_dir}
                  {#if e.audio || e.video}
                    <button class="icon-btn sm" title={e.audio ? 'Listen to all' : 'Watch'} onclick={() => collections.playFolder(e)}>{#if e.audio}<Headphones size={14} />{:else}<Play size={14} />{/if}</button>
                  {/if}
                {:else}
                  <button class="icon-btn sm" title={e.kind === 'pdf' ? 'Open' : e.kind === 'audio' ? 'Listen' : 'Watch'} onclick={() => activate(e)}>
                    {#if e.kind === 'pdf'}<Eye size={14} />{:else if e.kind === 'audio'}<Headphones size={14} />{:else}<Play size={14} />{/if}
                  </button>
                {/if}
                <button class="icon-btn sm" title="Move {e.is_dir ? 'everything' : 'it'} to {where(e)}" disabled={collections.moving || (e.is_dir && !e.count)} onclick={() => toMedia([e.path])}><FolderInput size={14} /></button>
                <Menu items={menuFor(e)} width={230}>
                  {#snippet trigger(toggle: () => void)}<button class="icon-btn sm" title="More" onclick={toggle}><MoreHorizontal size={14} /></button>{/snippet}
                </Menu>
              </span>
            </div>
          {/each}
          {#if !shown.length}<div class="none muted">Nothing matches “{filter}”.</div>{/if}
        </div>
      {/if}
      {#if dropOver && !dropDir}
        <div class="dropnote"><Upload size={18} /> Drop to add to {collections.dir ? `“${crumbs.at(-1)}”` : 'Collections'}</div>
      {:else if dropOver && dropDir}
        <div class="dropnote"><FolderInput size={18} /> Drop into “{dropDir.split('/').pop()}”</div>
      {/if}
    </div>
  {/if}

  {#if song}
    <div class="player">
      <div class="ctl">
        <button class="icon-btn" title="Previous" onclick={() => collections.step(-1)}><SkipBack size={15} /></button>
        <button class="icon-btn playbtn" title="{collections.paused ? 'Play' : 'Pause'} (space)" onclick={() => collections.toggle()}>{#if collections.paused}<Play size={16} />{:else}<Pause size={16} />{/if}</button>
        <button class="icon-btn" title="Next" disabled={collections.at >= collections.queue.length - 1} onclick={() => collections.step(1)}><SkipForward size={15} /></button>
      </div>
      <div class="now">
        <div class="title truncate" title={song.path}>
          <Music size={13} /> {stem(song.name)}
          {#if collections.queue.length > 1}<span class="muted small">· {collections.at + 1} of {collections.queue.length}</span>{/if}
        </div>
        {#if collections.audioError}
          <div class="err small">{collections.audioError} <button class="link" onclick={() => collections.playInMedia(song!)}>Play in the Music player</button></div>
        {:else}
          <div class="seek">
            <span class="t">{fmtTime(collections.time * 1000)}</span>
            <input type="range" min="0" max={collections.duration || 0} step="0.1" value={collections.time} oninput={(e) => collections.seek(Number(e.currentTarget.value))} />
            <span class="t">{fmtTime(collections.duration * 1000)}</span>
          </div>
        {/if}
      </div>
      <div class="vol" title="Volume">
        <Volume2 size={14} />
        <input type="range" min="0" max="1" step="0.01" value={collections.volume} oninput={(e) => collections.setVolume(Number(e.currentTarget.value))} />
      </div>
      <button class="btn sm ghost" disabled={collections.moving} title="Move this song to the Music player" onclick={() => toMedia([song!.path])}><FolderInput size={13} /> Move to Music</button>
      <button class="icon-btn sm" title="Stop" onclick={() => collections.stopListening()}><X size={14} /></button>
    </div>
  {/if}
</div>

<Modal bind:open={nameOpen} title={nameMode === 'folder' ? 'New folder' : 'Rename'} width={400}>
  <form onsubmit={commitName} class="nameform">
    <input class="input" bind:this={nameInput} bind:value={nameValue} placeholder="Name" />
    <div class="row end">
      <button type="button" class="btn ghost" onclick={() => (nameOpen = false)}>Cancel</button>
      <button type="submit" class="btn primary" disabled={!nameValue.trim()}>{nameMode === 'folder' ? 'Create' : 'Rename'}</button>
    </div>
  </form>
</Modal>

<style>
  .coll {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  header {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    height: var(--titlebar-h);
    padding: 0 12px 0 calc(14px + var(--lead));
    border-bottom: 1px solid var(--border);
  }
  h1 {
    margin: 0;
    font-size: 15px;
    font-weight: 650;
    min-width: 0;
  }
  .spacer {
    flex: 1;
  }
  .small {
    font-size: 12px;
  }
  .search {
    width: 160px;
  }
  .crumbs {
    display: flex;
    align-items: center;
    gap: 2px;
    min-width: 0;
    overflow: hidden;
  }
  .crumbs :global(.sep) {
    flex: none;
    color: var(--text-4);
  }
  .crumb {
    background: none;
    border: 0;
    padding: 3px 6px;
    border-radius: var(--r-xs);
    font: inherit;
    font-size: 14px;
    color: var(--text-2);
    cursor: pointer;
    max-width: 220px;
  }
  .crumb:hover {
    background: var(--surface-hover);
    color: var(--text);
  }
  .crumb.cur {
    color: var(--text);
    font-weight: 650;
  }
  .bar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 14px;
    border-bottom: 1px solid var(--border);
    font-size: 12.5px;
  }
  .bar.busy {
    color: var(--text-2);
    background: var(--accent-softer);
  }
  .bar.sel {
    background: var(--accent-soft);
  }
  .btn.danger {
    color: var(--danger);
  }
  .body {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: auto;
    display: flex;
    flex-direction: column;
  }
  .body.drop {
    outline: 2px dashed var(--accent-ring);
    outline-offset: -8px;
    background: var(--accent-softer);
  }
  .center {
    flex: 1;
    display: grid;
    place-items: center;
    padding: 24px;
  }
  .empty {
    max-width: 500px;
    text-align: center;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
  }
  .empty h2 {
    margin: 4px 0 0;
    font-size: 19px;
  }
  .empty p {
    margin: 0;
    color: var(--text-2);
    line-height: 1.5;
  }
  .empty .row {
    display: flex;
    gap: 8px;
    margin-top: 4px;
  }
  .glyph {
    width: 60px;
    height: 60px;
    border-radius: 18px;
    display: grid;
    place-items: center;
    background: var(--accent-soft);
    color: var(--accent);
  }
  .list {
    padding: 6px 8px 16px;
  }
  .list .row {
    display: grid;
    grid-template-columns: 22px minmax(0, 1fr) minmax(0, 220px) 90px 84px;
    align-items: center;
    gap: 10px;
    padding: 0 8px;
    min-height: 38px;
    border-radius: var(--r-sm);
  }
  .list .head {
    min-height: 30px;
    font-size: 11.5px;
    color: var(--text-3);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .list .item:hover {
    background: var(--surface-hover);
  }
  .list .item.selected {
    background: var(--accent-softer);
  }
  .list .item.target {
    background: var(--accent-soft);
    outline: 2px solid var(--accent-ring);
  }
  .list .item.now .name {
    color: var(--accent);
  }
  .list .name {
    display: flex;
    align-items: center;
    gap: 9px;
    min-width: 0;
    background: none;
    border: 0;
    padding: 0;
    font: inherit;
    font-size: 13.5px;
    color: var(--text);
    text-align: left;
    cursor: pointer;
  }
  .ico {
    flex: none;
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border-radius: 7px;
    background: var(--surface-2);
    color: var(--text-2);
  }
  .ico.dir {
    color: #d29922;
  }
  .ico.pdf {
    color: #e5534b;
  }
  .ico.audio {
    color: var(--accent);
  }
  .ico.video {
    color: #2f9e8f;
  }
  .ext,
  .rag {
    flex: none;
    font-size: 10px;
    font-weight: 600;
    padding: 1px 5px;
    border-radius: 4px;
    background: var(--surface-2);
    color: var(--text-3);
  }
  .rag {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    background: var(--success-soft);
    color: var(--success);
  }
  .info,
  .date {
    font-size: 12.5px;
  }
  .acts {
    display: flex;
    gap: 2px;
    justify-content: flex-end;
  }
  .none {
    padding: 20px;
    text-align: center;
  }
  .dropnote {
    position: sticky;
    bottom: 14px;
    align-self: center;
    margin-top: auto;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 14px;
    border-radius: 999px;
    background: var(--accent);
    color: var(--on-accent);
    font-size: 13px;
    box-shadow: var(--shadow);
    pointer-events: none;
  }
  .viewer {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .film {
    position: relative;
    flex: 1;
    min-height: 0;
    display: grid;
    place-items: center;
    background: #000;
  }
  .film video {
    max-width: 100%;
    max-height: 100%;
    width: 100%;
    height: 100%;
  }
  .vfail {
    position: absolute;
    inset: auto 0 40px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    color: #ddd;
    font-size: 13px;
  }
  .player {
    flex: none;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 14px;
    border-top: 1px solid var(--border);
    background: var(--surface);
  }
  .ctl {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .playbtn {
    background: var(--accent);
    color: var(--on-accent);
    border-radius: 999px;
  }
  .playbtn:hover {
    background: var(--accent-hover);
  }
  .now {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .title {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    font-weight: 550;
  }
  .seek {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .seek input {
    flex: 1;
    accent-color: var(--accent);
  }
  .seek .t {
    font-size: 11px;
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
    min-width: 36px;
  }
  .vol {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text-3);
  }
  .vol input {
    width: 80px;
    accent-color: var(--accent);
  }
  .err {
    color: var(--danger);
  }
  .link {
    background: none;
    border: 0;
    padding: 0;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
    text-decoration: underline;
  }
  .nameform {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .nameform .row.end {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  @media (max-width: 760px) {
    .list .row {
      grid-template-columns: 22px minmax(0, 1fr) 84px;
    }
    .list .info,
    .list .date {
      display: none;
    }
    .vol {
      display: none;
    }
  }
</style>
