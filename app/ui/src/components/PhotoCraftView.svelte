<script lang="ts">
  // PhotoCraft, the image editor: install it, open it (its own window), the pictures in its
  // folder, and edits by request (lib/photocraft.svelte.ts), in the window or in the background.
  import { onMount } from 'svelte';
  import { open as pickFile } from '@tauri-apps/plugin-dialog';
  import { store } from '../lib/state.svelte';
  import { activity } from '../lib/gilgamesh/observe.svelte';
  import { photocraft, type Target } from '../lib/photocraft.svelte';
  import { formatBytes, timeAgo } from '../lib/util';
  import { Palette, Download, Loader, ExternalLink, FolderOpen, ImagePlus, Play, CircleStop, Image as ImageIcon, RefreshCw } from '@lucide/svelte';

  const p = photocraft;
  let request = $state('');
  let target = $state<Target>('app');
  let file = $state('');
  let installing = $state(false);
  const job = $derived(p.jobs[0] ?? null);

  $effect(() => {
    activity.seen('photocraft', p.status?.installed ? `PhotoCraft ${p.status.running ? 'open' : 'closed'}; ${p.files.length} files in its folder` : 'PhotoCraft not installed');
    return () => activity.seen('photocraft', null);
  });

  onMount(() => {
    void p.refresh();
    const t = setInterval(() => void p.refresh(), 5000);
    return () => clearInterval(t);
  });

  async function install() {
    installing = true;
    try {
      await p.install();
    } catch (e) {
      store.error(e, 'Couldn’t install PhotoCraft');
    } finally {
      installing = false;
    }
  }

  async function openApp(name?: string) {
    try {
      await p.open(name);
    } catch (e) {
      store.error(e, 'Couldn’t open PhotoCraft');
    }
  }

  async function addFiles() {
    const picked = await pickFile({ multiple: true, filters: [{ name: 'Images', extensions: ['png', 'jpg', 'jpeg', 'psd', 'psb', 'tif', 'tiff', 'webp', 'gif', 'bmp', 'heic', 'pcraft', 'kra', 'ora'] }] });
    const list = Array.isArray(picked) ? picked : picked ? [picked] : [];
    try {
      for (const f of list) await p.importFile(String(f));
      await p.refresh();
    } catch (e) {
      store.error(e, 'Couldn’t add the picture');
    }
  }

  function go(e: Event) {
    e.preventDefault();
    const text = request.trim();
    if (!text || p.busy()) return;
    request = '';
    p.edit(text, { target, from: 'owner', file: file || undefined }).catch((err) => store.error(err, 'Couldn’t edit'));
  }

  const time = (ms: number) => new Date(ms).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' });
</script>

<div class="pc">
  <header class="drag">
    <Palette size={17} />
    <h1>PhotoCraft</h1>
    {#if p.status?.installed}<span class="muted small">{p.status.version ?? ''}</span>{/if}
    <span class="spacer"></span>
    {#if p.status?.installed}
      <button class="btn sm no-drag" onclick={() => p.reveal()}><FolderOpen size={13} /> Folder</button>
      <button class="btn sm primary no-drag" onclick={() => openApp()}><ExternalLink size={13} /> {p.status.running ? 'Show PhotoCraft' : 'Open PhotoCraft'}</button>
    {/if}
  </header>

  {#if !p.status}
    <div class="center"><Loader size={20} class="spin" /></div>
  {:else if !p.status.installed}
    <div class="center">
      <div class="empty">
        <div class="glyph"><Palette size={28} /></div>
        <h2>Draw and edit pictures</h2>
        <p>PhotoCraft is an open-source, Photoshop-style editor: layers, masks, adjustment layers, brushes with pen pressure, text, shapes, filters and PSD files. It runs in its own window; Gilgamesh and your bots can edit with it too. It’s in early alpha.</p>
        {#if !p.status.supported}
          <p class="muted">RAD installs it on a Mac only for now.</p>
        {:else if p.status.downloading}
          <div class="bar"><i style="width:{Math.round((p.status.downloading.done / p.status.downloading.total) * 100)}%"></i></div>
          <p class="muted small">{p.status.downloading.phase} · {formatBytes(p.status.downloading.done)} of {formatBytes(p.status.downloading.total)}</p>
        {:else}
          <button class="btn primary" disabled={installing} onclick={install}>{#if installing}<Loader size={14} class="spin" />{:else}<Download size={14} />{/if} Install PhotoCraft {p.status.pinned} ({formatBytes(p.status.bytes)})</button>
          <p class="muted small">The signed release from its makers, checked against its checksum, kept in RAD’s own folder.</p>
        {/if}
      </div>
    </div>
  {:else}
    <div class="split">
      <section class="files">
        <div class="sh">
          <strong>Pictures</strong>
          <span class="muted small truncate" title={p.status.work_dir}>{p.status.work_dir.replace(/^\/Users\/[^/]+/, '~')}</span>
          <span class="spacer"></span>
          <button class="icon-btn sm" title="Refresh" onclick={() => p.refresh()}><RefreshCw size={13} /></button>
          <button class="btn sm" onclick={addFiles}><ImagePlus size={13} /> Add</button>
        </div>
        {#each p.files as f (f.name)}
          <div class="file" class:sel={file === f.name}>
            <ImageIcon size={14} />
            <button class="name truncate" title="Open in PhotoCraft" onclick={() => openApp(f.name)}>{f.name}</button>
            <small>{formatBytes(f.size)} · {timeAgo(f.modified)}</small>
            <button class="btn sm ghost" title="Use this picture for the next request" onclick={() => (file = file === f.name ? '' : f.name)}>{file === f.name ? 'Chosen' : 'Choose'}</button>
          </div>
        {:else}
          <p class="muted pad">No pictures yet. Add some, or draw in PhotoCraft and save into this folder. PhotoCraft (and its helpers) can only read and write here.</p>
        {/each}
      </section>

      <section class="agent">
        <div class="sh"><strong>Ask for an edit</strong><span class="muted small">Gilgamesh can do this too: “edit … in PhotoCraft”.</span></div>
        <form class="ask" onsubmit={go}>
          <textarea class="input" rows="3" placeholder={file ? `What to do with ${file}…` : 'e.g. “Make a 1080×1080 poster with a sunset gradient and the title Summer in white”'} bind:value={request}></textarea>
          <div class="row" style="gap:8px">
            <div class="seg">
              <button type="button" class:on={target === 'app'} title="In the PhotoCraft window, as you watch" onclick={() => (target = 'app')}>In the window</button>
              <button type="button" class:on={target === 'headless'} title="In the background (no window); saved into the folder" onclick={() => (target = 'headless')}>In the background</button>
            </div>
            {#if file}<span class="pill accent truncate">{file}</span>{/if}
            <span class="spacer"></span>
            {#if p.busy()}
              <button type="button" class="btn sm danger" onclick={() => p.stop()}><CircleStop size={13} /> Stop</button>
            {:else}
              <button class="btn sm primary" disabled={!request.trim()}><Play size={13} /> Go</button>
            {/if}
          </div>
        </form>
        {#if job}
          <div class="jobhead">
            <strong class="truncate">{job.instruction}</strong>
            <span class="pill" class:success={job.status === 'done'} class:danger={job.status === 'failed'} class:accent={job.status === 'running'}>{job.status === 'running' ? 'Working' : job.status === 'done' ? 'Done' : job.status === 'failed' ? 'Didn’t finish' : 'Stopped'}</span>
          </div>
          <ol class="steps selectable">
            {#each job.steps as s, i (i)}
              <li data-k={s.kind} class:bad={s.ok === false}><span class="t">{time(s.at)}</span><span class="x">{s.text}</span></li>
            {/each}
            {#if job.status === 'running'}<li data-k="say"><span class="t"></span><span class="x"><Loader size={12} class="spin" /> {p.now}</span></li>{/if}
          </ol>
          {#if job.output}<button class="btn sm" onclick={() => openApp(job.output!)}><ExternalLink size={13} /> Open {job.output}</button>{/if}
        {/if}
        {#if p.picture}<img class="pic" src={p.picture} alt="The latest view of the work" />{/if}
      </section>
    </div>
  {/if}
</div>

<style>
  .pc {
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
  }
  .small {
    font-size: 12px;
  }
  .pad {
    padding: 8px 2px;
    font-size: 13px;
    line-height: 1.5;
  }
  .center {
    flex: 1;
    display: grid;
    place-items: center;
    padding: 24px;
  }
  .empty {
    max-width: 480px;
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
  .glyph {
    width: 60px;
    height: 60px;
    border-radius: 18px;
    display: grid;
    place-items: center;
    color: var(--accent);
    background: var(--accent-soft);
  }
  .bar {
    width: 280px;
    height: 6px;
    border-radius: 99px;
    background: var(--surface-3);
    overflow: hidden;
  }
  .bar i {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  .split {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .files {
    flex: 1;
    min-width: 0;
    overflow: auto;
    padding: 12px 14px;
    border-right: 1px solid var(--border);
  }
  .agent {
    flex: 1;
    min-width: 0;
    overflow: auto;
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .sh {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 8px;
    min-width: 0;
  }
  .file {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 5px 6px;
    border-radius: 8px;
    font-size: 13px;
  }
  .file:hover,
  .file.sel {
    background: var(--surface-2);
  }
  .file .name {
    flex: 1;
    min-width: 0;
    text-align: left;
  }
  .file .name:hover {
    text-decoration: underline;
  }
  .file small {
    color: var(--text-3);
    flex: none;
  }
  .ask {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  textarea.input {
    resize: vertical;
    height: auto;
    padding-top: 8px;
    padding-bottom: 8px;
  }
  .seg {
    display: flex;
    gap: 2px;
    padding: 2px;
    border-radius: 8px;
    background: var(--surface-2);
  }
  .seg button {
    padding: 4px 9px;
    border-radius: 6px;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .seg button.on {
    background: var(--surface-3);
    color: var(--text);
    font-weight: 600;
  }
  .jobhead {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .steps {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 13px;
  }
  .steps li {
    display: flex;
    gap: 10px;
    line-height: 1.45;
  }
  .steps .t {
    flex: none;
    width: 38px;
    color: var(--text-3);
    font-size: 11.5px;
  }
  .steps .x {
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .steps li[data-k='act'] .x {
    font-family: var(--mono, ui-monospace, monospace);
    font-size: 12px;
    color: var(--text-2);
  }
  .steps li[data-k='result'] .x {
    font-size: 12px;
    color: var(--text-3);
    padding-left: 12px;
  }
  .steps li[data-k='user'] .x {
    font-weight: 600;
  }
  .steps li[data-k='done'] .x {
    padding: 8px 10px;
    border-radius: 9px;
    background: var(--accent-soft);
  }
  .steps li.bad .x,
  .steps li[data-k='error'] .x {
    color: var(--danger, #e5534b);
  }
  .pic {
    width: 100%;
    border-radius: 8px;
    border: 1px solid var(--border);
  }
</style>
