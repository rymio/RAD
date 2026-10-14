<script lang="ts">
  import { onMount } from 'svelte';
  import { open as openDialog, save, ask } from '@tauri-apps/plugin-dialog';
  import { store } from '../lib/state.svelte';
  import { api, rad } from '../lib/api';
  import { docKind, PROJECT_COLORS } from '../lib/util';
  import type { Document, Id, Project } from '../lib/types';
  import DocCard from './DocCard.svelte';
  import NoteEditor from './NoteEditor.svelte';
  import Preview from './Preview.svelte';
  import Modal from './Modal.svelte';
  import Menu from './Menu.svelte';
  import { Upload, StickyNote, MessageSquarePlus, MoreHorizontal, Settings2, Archive, ArchiveRestore, Trash2, Search, X, Camera, LayoutGrid, Files, Image as ImageIcon, Film, UploadCloud, Loader } from '@lucide/svelte';

  let { projectId }: { projectId: Id } = $props();

  let docs = $state<Document[]>([]);
  let loading = $state(true);
  let tab = $state<'all' | 'files' | 'notes' | 'images' | 'media'>('all');
  let q = $state('');
  let note = $state<{ id: Id | null; initial?: string } | null>(null);
  let preview = $state<Document | null>(null);
  let editOpen = $state(false);
  let edit = $state({ name: '', description: '', color: '', system_prompt: '' });
  let renameDoc = $state<Document | null>(null);
  let renameValue = $state('');
  let moveDoc = $state<Document | null>(null);

  const project = $derived<Project | undefined>(store.projectMap.get(projectId));

  async function load() {
    try {
      docs = await api.get<Document[]>(`/v1/projects/${projectId}/documents`);
    } catch (e) {
      store.error(e);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    store.rev.docs[projectId] = store.rev.docs[projectId] ?? 0;
    load();
    if (store.route.kind === 'project' && store.route.doc) {
      api.get<Document>(`/v1/documents/${store.route.doc}`).then((d) => openDoc(d)).catch(() => {});
    }
    return () => delete store.rev.docs[projectId];
  });

  // Refresh when documents change on any device.
  let lastRev = 0;
  $effect(() => {
    const r = store.rev.docs[projectId] ?? 0;
    if (r !== lastRev) {
      lastRev = r;
      if (r) load();
    }
  });

  const filtered = $derived.by(() => {
    let list = docs;
    if (tab === 'files') list = list.filter((d) => d.source_kind !== 'note');
    if (tab === 'notes') list = list.filter((d) => d.source_kind === 'note');
    if (tab === 'images') list = list.filter((d) => d.mime.startsWith('image/'));
    if (tab === 'media') list = list.filter((d) => ['audio', 'video'].includes(docKind(d)));
    if (q.trim()) {
      const s = q.toLowerCase();
      list = list.filter((d) => d.title.toLowerCase().includes(s) || d.original_name.toLowerCase().includes(s) || d.tags.some((t) => t.toLowerCase().includes(s)));
    }
    return list;
  });

  const counts = $derived({
    all: docs.length,
    files: docs.filter((d) => d.source_kind !== 'note').length,
    notes: docs.filter((d) => d.source_kind === 'note').length,
    images: docs.filter((d) => d.mime.startsWith('image/')).length,
    media: docs.filter((d) => ['audio', 'video'].includes(docKind(d))).length,
  });

  async function upload() {
    const sel = await openDialog({ multiple: true, title: `Add files to ${project?.name}` });
    if (!sel) return;
    const paths = Array.isArray(sel) ? sel : [sel];
    const res = await rad.uploadPaths(projectId, paths).catch((e) => (store.error(e, 'Upload failed'), []));
    const ok = res.filter((r) => 'document' in r).length;
    if (ok) store.toast('success', `${ok} file${ok > 1 ? 's' : ''} added`, 'Encrypted into your vault and indexing now.');
    for (const r of res) if ('error' in r) store.toast('error', 'Upload failed', r.error);
    load();
  }

  async function screenshot() {
    try {
      const r = await rad.captureScreenshot(projectId);
      if (r) {
        store.toast('success', 'Screenshot added', 'RAD is reading it now.');
        load();
      }
    } catch (e) {
      store.error(e, 'Screenshot failed');
    }
  }

  async function onPaste(e: ClipboardEvent) {
    if (note || preview || editOpen || (e.target as HTMLElement)?.closest('input,textarea')) return;
    const files = Array.from(e.clipboardData?.files ?? []);
    if (files.length) {
      e.preventDefault();
      for (const f of files) {
        const name = f.name && f.name !== 'image.png' ? f.name : `Pasted ${new Date().toLocaleString().replace(/[/:]/g, '-')}.png`;
        await rad.uploadBytes(projectId, name, await f.arrayBuffer(), 'paste').catch((err) => store.error(err, 'Paste failed'));
      }
      store.toast('success', 'Pasted into ' + project?.name);
      load();
      return;
    }
    const text = e.clipboardData?.getData('text/plain');
    if (text?.trim()) {
      e.preventDefault();
      note = { id: null, initial: text };
    }
  }

  function openDoc(d: Document) {
    if (d.source_kind === 'note') note = { id: d.id };
    else preview = d;
  }

  async function del(d: Document) {
    const ok = await ask(`Delete “${d.title}” from your vault? This can't be undone.`, { title: 'Delete', kind: 'warning', okLabel: 'Delete' });
    if (!ok) return;
    await api.del(`/v1/documents/${d.id}`).catch((e) => store.error(e));
    docs = docs.filter((x) => x.id !== d.id);
    if (preview?.id === d.id) preview = null;
  }

  async function patchDoc(d: Document, body: Record<string, unknown>) {
    try {
      const nd = await api.patch<Document>(`/v1/documents/${d.id}`, body);
      docs = body.project_id && body.project_id !== projectId ? docs.filter((x) => x.id !== d.id) : docs.map((x) => (x.id === d.id ? nd : x));
    } catch (e) {
      store.error(e);
    }
  }

  async function reindex(d: Document) {
    await api.post(`/v1/documents/${d.id}/reindex`).catch((e) => store.error(e));
    load();
  }

  async function exportDoc(d: Document) {
    const dest = await save({ defaultPath: d.original_name });
    if (!dest) return;
    await rad.exportDocument(d.id, dest).then(() => store.toast('success', 'Exported', dest)).catch((e) => store.error(e));
  }

  function openEdit() {
    if (!project) return;
    edit = { name: project.name, description: project.description, color: project.color, system_prompt: project.system_prompt };
    editOpen = true;
  }

  async function saveEdit(e: Event) {
    e.preventDefault();
    await api.patch(`/v1/projects/${projectId}`, edit).catch((err) => store.error(err));
    editOpen = false;
    store.reloadProjects();
  }

  async function archive(v: boolean) {
    await api.patch(`/v1/projects/${projectId}`, { archived: v }).catch((e) => store.error(e));
    store.reloadProjects();
  }

  async function removeProject() {
    const ok = await ask(`Delete “${project?.name}” and all ${docs.length} items in it? This permanently removes them from your vault.`, { title: 'Delete project', kind: 'warning', okLabel: 'Delete project' });
    if (!ok) return;
    await api.del(`/v1/projects/${projectId}`).catch((e) => store.error(e));
    store.projects = store.projects.filter((p) => p.id !== projectId);
    store.navigate({ kind: 'chat', id: null });
  }

  const tabs = [
    { id: 'all', label: 'All', icon: LayoutGrid },
    { id: 'files', label: 'Files', icon: Files },
    { id: 'notes', label: 'Notes', icon: StickyNote },
    { id: 'images', label: 'Images', icon: ImageIcon },
    { id: 'media', label: 'Audio & video', icon: Film },
  ] as const;
</script>

<svelte:window onpaste={onPaste} />

{#if project}
  <div class="pv">
    <header class="drag">
      <div class="ptitle no-drag">
        <span class="pd" style="background:{project.color}"></span>
        <h1 class="truncate">{project.name}</h1>
        {#if project.archived}<span class="pill">Archived</span>{/if}
      </div>
      <div class="actions no-drag">
        <button class="btn sm" onclick={() => store.newChat([projectId])}><MessageSquarePlus size={14} /> Ask about this</button>
        <button class="btn sm" onclick={() => (note = { id: null })}><StickyNote size={14} /> New note</button>
        <button class="btn sm" onclick={screenshot} title="Capture a screenshot into this project"><Camera size={14} /></button>
        <button class="btn sm primary" onclick={upload}><Upload size={14} /> Add files</button>
        <Menu
          items={[
            { label: 'Edit project…', icon: Settings2, onclick: openEdit },
            project.archived ? { label: 'Unarchive', icon: ArchiveRestore, onclick: () => archive(false) } : { label: 'Archive', icon: Archive, onclick: () => archive(true) },
            null,
            { label: 'Delete project…', icon: Trash2, danger: true, onclick: removeProject },
          ]}
        >
          {#snippet trigger(toggle)}<button class="icon-btn" onclick={toggle} aria-label="Project menu"><MoreHorizontal size={17} /></button>{/snippet}
        </Menu>
      </div>
    </header>

    <div class="content">
      {#if project.description}<p class="desc">{project.description}</p>{/if}
      <div class="toolbar">
        <div class="tabs">
          {#each tabs as t}
            <button class:on={tab === t.id} onclick={() => (tab = t.id)}><t.icon size={13} /> {t.label} <span class="c">{counts[t.id]}</span></button>
          {/each}
        </div>
        <span class="spacer"></span>
        <div class="search">
          <Search size={14} />
          <input placeholder="Filter by name or tag" bind:value={q} />
          {#if q}<button class="icon-btn sm" onclick={() => (q = '')}><X size={12} /></button>{/if}
        </div>
      </div>

      {#if loading}
        <div class="grid">{#each Array(8) as _}<div class="card skel shimmer"></div>{/each}</div>
      {:else if !docs.length}
        <div class="empty fade-up">
          <div class="em-ic"><UploadCloud size={34} strokeWidth={1.5} /></div>
          <h2>Bring your knowledge into {project.name}</h2>
          <p>Drop files anywhere in this window — PDFs, Word, Markdown, spreadsheets, slides, images, screenshots, MP3s and MP4s. Paste images or text with <span class="kbd">⌘V</span>.</p>
          <div class="row" style="justify-content:center;margin-top:18px">
            <button class="btn primary" onclick={upload}><Upload size={15} /> Add files</button>
            <button class="btn" onclick={() => (note = { id: null })}><StickyNote size={15} /> Write a note</button>
          </div>
        </div>
      {:else if !filtered.length}
        <div class="none">Nothing matches.</div>
      {:else}
        <div class="grid">
          {#each filtered as d (d.id)}
            <DocCard
              doc={d}
              onopen={() => openDoc(d)}
              onrename={() => ((renameDoc = d), (renameValue = d.title))}
              ondelete={() => del(d)}
              onreindex={() => reindex(d)}
              onexport={() => exportDoc(d)}
              onpin={() => patchDoc(d, { pinned: !d.pinned })}
              onmove={() => (moveDoc = d)}
            />
          {/each}
        </div>
      {/if}
      {#if store.jobs.queued + store.jobs.running > 0}
        <div class="indexing"><Loader size={13} class="spin" /> RAD is reading and indexing {store.jobs.queued + store.jobs.running} item(s)…</div>
      {/if}
    </div>
  </div>

  {#if preview}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="pv-backdrop" onclick={() => (preview = null)}>
      <div class="pv-sheet" onclick={(e) => e.stopPropagation()}>
        <header>
          <span class="truncate t">{preview.title}</span>
          <span class="spacer"></span>
          <button class="btn sm" onclick={() => { const d = preview!; preview = null; store.newChat([projectId]).then(() => window.dispatchEvent(new CustomEvent('rad-attach', { detail: [d] }))); }}><MessageSquarePlus size={13} /> Ask about this file</button>
          <button class="icon-btn" onclick={() => (preview = null)}><X size={17} /></button>
        </header>
        <Preview documentId={preview.id} mime={preview.mime} title={preview.title} />
      </div>
    </div>
  {/if}

  {#if note}
    <NoteEditor projectId={projectId} noteId={note.id} initial={note.initial ?? ''} onclose={() => ((note = null), load())} />
  {/if}

  <Modal bind:open={editOpen} title="Edit project" width={520}>
    <form onsubmit={saveEdit}>
      <div class="field"><label class="label" for="en">Name</label><input id="en" class="input" bind:value={edit.name} /></div>
      <div class="field"><label class="label" for="ed">Description</label><input id="ed" class="input" bind:value={edit.description} placeholder="What lives here?" /></div>
      <div class="field">
        <span class="label">Color</span>
        <div class="swatches">{#each PROJECT_COLORS as c}<button type="button" class="sw" class:sel={edit.color === c} style="background:{c}" onclick={() => (edit.color = c)} aria-label={c}></button>{/each}</div>
      </div>
      <div class="field">
        <label class="label" for="es">Instructions for RAD</label>
        <textarea id="es" class="textarea" rows="4" bind:value={edit.system_prompt} placeholder="e.g. Answer as my tax assistant. Amounts are in EUR. Prefer the most recent documents."></textarea>
        <div class="hint">Used when a chat is scoped to this project only.</div>
      </div>
      <div class="row" style="justify-content:flex-end"><button type="button" class="btn ghost" onclick={() => (editOpen = false)}>Cancel</button><button class="btn primary">Save</button></div>
    </form>
  </Modal>

  <Modal open={!!renameDoc} title="Rename" width={420} onclose={() => (renameDoc = null)}>
    <form onsubmit={(e) => { e.preventDefault(); if (renameDoc) patchDoc(renameDoc, { title: renameValue }); renameDoc = null; }}>
      <!-- svelte-ignore a11y_autofocus -->
      <input class="input" bind:value={renameValue} autofocus />
      <div class="row" style="justify-content:flex-end;margin-top:14px"><button type="button" class="btn ghost" onclick={() => (renameDoc = null)}>Cancel</button><button class="btn primary">Rename</button></div>
    </form>
  </Modal>

  <Modal open={!!moveDoc} title="Move to project" width={380} onclose={() => (moveDoc = null)}>
    <div class="move-list">
      {#each store.projects.filter((p) => p.id !== projectId && !p.archived) as p}
        <button class="move-item" onclick={() => { if (moveDoc) patchDoc(moveDoc, { project_id: p.id }); moveDoc = null; store.reloadProjects(); }}>
          <span class="pd sm" style="background:{p.color}"></span>{p.name}
        </button>
      {:else}
        <div class="muted">Create another project first.</div>
      {/each}
    </div>
  </Modal>
{/if}

<style>
  .pv {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  header {
    flex: none;
    display: flex;
    align-items: center;
    gap: 12px;
    height: var(--titlebar-h);
    padding: 0 14px 0 calc(26px + var(--lead));
    border-bottom: 1px solid var(--border);
  }
  .ptitle {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }
  .pd {
    width: 12px;
    height: 12px;
    border-radius: 4px;
    flex: none;
  }
  .pd.sm {
    width: 9px;
    height: 9px;
    border-radius: 3px;
  }
  h1 {
    margin: 0;
    font-size: 15px;
    font-weight: 650;
  }
  .actions {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .content {
    flex: 1;
    overflow: auto;
    padding: 20px 26px 40px;
  }
  .desc {
    margin: 0 0 14px;
    color: var(--text-2);
  }
  .toolbar {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 16px;
  }
  .tabs {
    display: flex;
    gap: 2px;
    padding: 3px;
    border-radius: var(--r);
    background: var(--surface-2);
    border: 1px solid var(--border);
  }
  .tabs button {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 11px;
    border: none;
    border-radius: 8px;
    background: transparent;
    color: var(--text-3);
    font-size: 12.5px;
    font-weight: 550;
    transition: all var(--t-fast);
  }
  .tabs button:hover {
    color: var(--text);
  }
  .tabs button.on {
    background: var(--surface);
    color: var(--text);
    box-shadow: var(--shadow-sm);
  }
  .tabs .c {
    font-size: 11px;
    color: var(--text-4);
    font-variant-numeric: tabular-nums;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 6px;
    width: 240px;
    height: 34px;
    padding: 0 6px 0 11px;
    border-radius: var(--r-sm);
    border: 1px solid var(--border);
    background: var(--surface);
    color: var(--text-3);
  }
  .search input {
    flex: 1;
    border: none;
    outline: none;
    background: transparent;
    font-size: 13px;
    user-select: text;
    -webkit-user-select: text;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(196px, 1fr));
    gap: 14px;
  }
  .skel {
    height: 196px;
    border: none;
  }
  .empty {
    max-width: 520px;
    margin: 8vh auto 0;
    text-align: center;
  }
  .em-ic {
    width: 76px;
    height: 76px;
    margin: 0 auto 16px;
    border-radius: 24px;
    display: grid;
    place-items: center;
    color: var(--accent);
    background: var(--accent-soft);
  }
  .empty h2 {
    margin: 0 0 8px;
    font-size: 20px;
    letter-spacing: -0.02em;
  }
  .empty p {
    margin: 0;
    color: var(--text-2);
    line-height: 1.6;
  }
  .none {
    padding: 40px;
    text-align: center;
    color: var(--text-3);
  }
  .indexing {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 18px;
    font-size: 12.5px;
    color: var(--accent);
  }
  .pv-backdrop {
    position: fixed;
    inset: 0;
    z-index: 420;
    display: flex;
    justify-content: flex-end;
    background: rgba(8, 8, 16, 0.3);
    animation: pop 140ms var(--ease) both;
  }
  .pv-sheet {
    width: min(760px, 70vw);
    height: 100%;
    display: flex;
    flex-direction: column;
    background: var(--surface-2);
    border-left: 1px solid var(--border-strong);
    box-shadow: var(--shadow-lg);
    animation: sheet 220ms var(--ease) both;
  }
  @keyframes sheet {
    from {
      transform: translateX(40px);
      opacity: 0.6;
    }
  }
  .pv-sheet header {
    padding: 0 10px 0 18px;
    gap: 8px;
  }
  .pv-sheet .t {
    font-weight: 600;
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
  }
  .sw.sel {
    border-color: var(--surface);
    box-shadow: 0 0 0 2px var(--text);
  }
  .move-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .move-item {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 36px;
    padding: 0 10px;
    border: none;
    border-radius: var(--r-sm);
    background: transparent;
    text-align: left;
  }
  .move-item:hover {
    background: var(--surface-hover);
  }
</style>
