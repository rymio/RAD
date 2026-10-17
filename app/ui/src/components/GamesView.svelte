<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { ask } from '@tauri-apps/plugin-dialog';
  import { rad } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import { renderMarkdown } from '../lib/markdown';
  import * as G from '../lib/games';
  import { GameAgent } from '../lib/gameAgent.svelte';
  import Modal from './Modal.svelte';
  import Menu from './Menu.svelte';
  import {
    Gamepad2, Play, Square, Plus, Loader, File, FilePlus, Trash2, Pencil, MoreHorizontal, Sparkles, ArrowUp, CircleCheck, CircleX,
    Eraser, Lock, FileCode, CircleAlert, BookOpen, Folder, FolderPlus, Upload, Image as ImageIcon, Music, Type, ChevronRight, ChevronDown, Copy,
    X, FolderInput,
  } from '@lucide/svelte';

  let games = $state<G.Game[]>([]);
  let loaded = $state(false);
  let loadError = $state<string | null>(null);
  let selId = $state<string | null>(read('rad-game-selected'));
  const game = $derived(games.find((g) => g.id === selId) ?? null);

  let files = $state<G.GameFile[]>([]);
  let path = $state<string | null>(null);
  const current = $derived(files.find((f) => f.path === path) ?? null);
  let saving = $state(false);
  let dirty = new Set<string>();
  let saveTimer: ReturnType<typeof setTimeout> | undefined;

  let status = $state<{ running: string[]; building: string[]; rust: boolean }>({ running: [], building: [], rust: true });
  const running = $derived(!!game && status.running.includes(game.id));
  const building = $derived(!!game && status.building.includes(game.id));
  let out = $state('');
  let outEl: HTMLPreElement | undefined = $state();
  let phase = $state<string | null>(null);

  // Assets: the game's resources/ folder (images, sounds, fonts), stored encrypted.
  let assets = $state<G.GameAsset[]>([]);
  let selAsset = $state<string | null>(null);
  let preview = $state<{ path: string; url: string } | null>(null);
  let closed = $state<Record<string, boolean>>({});
  let dropFolder = $state<string | null>(null);
  let dragging = $state<string | null>(null);
  let uploading = $state(false);
  const MIME: Record<string, string> = { png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', webp: 'image/webp', bmp: 'image/bmp', ogg: 'audio/ogg', wav: 'audio/wav', mp3: 'audio/mpeg', flac: 'audio/flac' };

  /** Folders (explicit ones and the parents of files) and files, for the tree. */
  const tree = $derived.by(() => {
    const folders = new Set<string>(['']);
    for (const a of assets) {
      const parts = a.path.replace(/\/$/, '').split('/');
      const upto = a.path.endsWith('/') ? parts.length : parts.length - 1;
      for (let i = 1; i <= upto; i++) folders.add(parts.slice(0, i).join('/') + '/');
    }
    const parent = (p: string) => {
      const t = p.replace(/\/$/, '');
      const i = t.lastIndexOf('/');
      return i < 0 ? '' : t.slice(0, i + 1);
    };
    return {
      folders: (dir: string) => [...folders].filter((f) => f && parent(f) === dir).sort(),
      files: (dir: string) => assets.filter((a) => !a.path.endsWith('/') && parent(a.path) === dir),
    };
  });
  const assetCount = $derived(assets.filter((a) => !a.path.endsWith('/')).length);
  const leaf = (p: string) => p.replace(/\/$/, '').split('/').pop() ?? p;

  let agent = $state<GameAgent | null>(null);
  let text = $state('');
  let thread: HTMLDivElement | undefined = $state();

  function read(k: string) {
    try {
      return localStorage.getItem(k);
    } catch {
      return null;
    }
  }

  async function refresh() {
    try {
      games = await G.list();
      loadError = null;
    } catch (e: any) {
      loadError = e?.message ?? String(e);
    }
    loaded = true;
    if (!games.some((g) => g.id === selId)) selId = games[0]?.id ?? null;
  }

  async function refreshStatus() {
    status = await rad.gameStatus().catch(() => status);
  }

  async function open(id: string | null) {
    await flush();
    selId = id;
    try {
      if (id) localStorage.setItem('rad-game-selected', id);
    } catch {
      /* private mode */
    }
    files = [];
    assets = [];
    path = null;
    closePreview();
    out = '';
    phase = null;
    agent = null;
    if (!id) return;
    files = await G.files(id).catch((e) => (store.error(e, 'Couldn’t open the game'), []));
    assets = await G.listAssets(id).catch(() => []);
    path = files.find((f) => f.path === 'src/main.rs')?.path ?? files[0]?.path ?? null;
    const g = games.find((x) => x.id === id);
    const a = new GameAgent(id, g?.name ?? 'Game', {
      files: () => files.map((f) => ({ path: f.path, content: f.content })),
      assets: () => assets,
      kind: g?.kind ?? 'ggez',
      build: async () => {
        const cur = games.find((x) => x.id === id);
        if (cur && G.isZx(cur.kind)) return zxBuildAndCheck(cur);
        return rad.gameBuild(id, files.map((f) => ({ path: f.path, content: f.content })));
      },
      write: async (changed) => {
        for (const f of changed) {
          const i = files.findIndex((x) => x.path === f.path);
          if (i >= 0) files[i].content = f.content;
          else files.push(f);
          await G.saveFile(id, f.path, f.content);
        }
        if (!path || !files.some((f) => f.path === path)) path = changed[0]?.path ?? path;
      },
      persist: (turns) => G.saveChat(id, turns).catch(() => {}),
    });
    a.onBuilt = () => run();
    a.restore((await G.loadChat(id).catch(() => [])) as any);
    agent = a;
  }

  // ------------------------------------------------------------------ editing (autosaved, encrypted)

  function edited(v: string) {
    if (!current || !game) return;
    current.content = v;
    dirty.add(current.path);
    clearTimeout(saveTimer);
    saveTimer = setTimeout(flush, 700);
  }

  async function flush() {
    clearTimeout(saveTimer);
    if (!game || !dirty.size) return;
    const id = game.id;
    const paths = [...dirty];
    dirty = new Set();
    saving = true;
    try {
      for (const p of paths) {
        const f = files.find((x) => x.path === p);
        if (f) await G.saveFile(id, p, f.content);
      }
    } catch (e) {
      store.error(e, 'Couldn’t save');
    } finally {
      saving = false;
    }
  }

  function onKey(e: KeyboardEvent) {
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.key === 'Enter') {
      e.preventDefault();
      running ? stop() : run();
    } else if (mod && e.key.toLowerCase() === 's') {
      e.preventDefault();
      flush();
    } else if (e.key === 'Tab' && !mod && !e.altKey) {
      e.preventDefault();
      const el = e.currentTarget as HTMLTextAreaElement;
      const [a, b] = [el.selectionStart, el.selectionEnd];
      edited(el.value.slice(0, a) + '    ' + el.value.slice(b));
      tick().then(() => el.setSelectionRange(a + 4, a + 4));
    }
  }

  async function newFile() {
    if (!game) return;
    const p = window.prompt('New file (in the game’s folder)', 'src/player.rs')?.trim().replace(/^\.?\//, '');
    if (!p || p.includes('..') || files.some((f) => f.path === p)) return;
    files.push({ path: p, content: p.endsWith('.rs') ? '' : '' });
    await G.saveFile(game.id, p, '');
    path = p;
  }

  async function deleteFile(p: string) {
    if (!game || p === 'Cargo.toml' || p === 'src/main.rs') return;
    if (!(await ask(`Delete ${p}?`, { title: 'Delete file', kind: 'warning' }))) return;
    await G.removeFile(game.id, p).catch((e) => store.error(e));
    files = files.filter((f) => f.path !== p);
    if (path === p) path = 'src/main.rs';
  }

  // ------------------------------------------------------------------ assets

  async function reloadAssets() {
    if (game) assets = await G.listAssets(game.id).catch(() => assets);
  }

  /** Adds files from this Mac into `folder` ('' = resources/ itself). */
  async function addFiles(paths: string[], folder: string) {
    if (!game || !paths.length) return;
    uploading = true;
    let added = 0;
    try {
      const read = await rad.gameReadFiles(paths);
      for (const f of read) {
        if (f.error || f.data === undefined) {
          store.toast('error', `${f.name} wasn’t added`, f.error ?? 'Couldn’t read it');
          continue;
        }
        const p = folder + f.name;
        const info = G.assetKind(p) === 'image' ? G.imageSize(f.data) : '';
        await G.putAsset(game.id, p, f.data, f.size, info);
        added++;
      }
      if (added) store.toast('success', `${added} asset${added > 1 ? 's' : ''} added`, folder ? `to ${folder}` : 'to resources/', 2500);
    } catch (e) {
      store.error(e, 'Couldn’t add the files');
    } finally {
      uploading = false;
      if (folder) closed[folder] = false;
      reloadAssets();
    }
  }

  async function pickFiles(folder = selFolder()) {
    const { open: pick } = await import('@tauri-apps/plugin-dialog');
    const r = await pick({
      multiple: true,
      title: `Add assets to resources/${folder}`,
      filters: [
        { name: 'Images, sounds and fonts', extensions: [...G.IMAGE_EXT, ...G.SOUND_EXT, ...G.FONT_EXT] },
        { name: 'Any file', extensions: ['*'] },
      ],
    });
    const paths = r == null ? [] : Array.isArray(r) ? r : [r];
    addFiles(paths, folder);
  }

  /** The folder new things go in: the selected folder, or the selected file's folder. */
  function selFolder() {
    if (!selAsset) return '';
    if (selAsset.endsWith('/')) return selAsset;
    const i = selAsset.lastIndexOf('/');
    return i < 0 ? '' : selAsset.slice(0, i + 1);
  }

  async function newFolder() {
    if (!game) return;
    const base = selFolder();
    const n = window.prompt(`New folder in resources/${base}`, assets.length ? '' : 'sprites')?.trim().replace(/^\/+|\/+$/g, '');
    if (!n || n.includes('..')) return;
    await G.addFolder(game.id, base + n).catch((e) => store.error(e));
    reloadAssets();
  }

  async function move(from: string, toFolder: string) {
    if (!game) return;
    const to = toFolder + leaf(from) + (from.endsWith('/') ? '/' : '');
    if (to === from) return;
    if (assets.some((a) => a.path === to)) {
      store.toast('error', 'Not moved', `${to} already exists`);
      return;
    }
    try {
      await G.moveAsset(game.id, from, to);
      if (selAsset === from) selAsset = to;
      if (preview?.path === from) preview = { ...preview, path: to };
    } catch (e) {
      store.error(e, 'Couldn’t move it');
    }
    reloadAssets();
  }

  async function renameAsset(p: string) {
    if (!game) return;
    const n = window.prompt('Rename', leaf(p))?.trim().replace(/\//g, '');
    if (!n || n === leaf(p)) return;
    const parent = p.replace(/\/$/, '').split('/').slice(0, -1).join('/');
    const to = (parent ? parent + '/' : '') + n + (p.endsWith('/') ? '/' : '');
    if (assets.some((a) => a.path === to)) return store.toast('error', 'Not renamed', `${to} already exists`);
    await G.moveAsset(game.id, p, to).catch((e) => store.error(e));
    if (selAsset === p) selAsset = to;
    if (preview?.path === p) closePreview();
    reloadAssets();
  }

  async function moveAssetTo(p: string) {
    const to = window.prompt(`Move ${leaf(p)} to folder (inside resources/, empty = top)`, selFolder())?.trim().replace(/^\/+/, '');
    if (to === undefined || to.includes('..')) return;
    move(p, to ? to.replace(/\/*$/, '/') : '');
  }

  async function deleteAsset(p: string) {
    if (!game) return;
    const inside = p.endsWith('/') ? assets.filter((a) => a.path.startsWith(p) && !a.path.endsWith('/')).length : 0;
    if (!(await ask(p.endsWith('/') ? `Delete the folder ${leaf(p)}${inside ? ` and the ${inside} file${inside > 1 ? 's' : ''} in it` : ''}?` : `Delete ${leaf(p)}?`, { title: 'Delete asset', kind: 'warning' }))) return;
    await G.deleteAsset(game.id, p).catch((e) => store.error(e));
    if (selAsset?.startsWith(p)) selAsset = null;
    if (preview && preview.path.startsWith(p)) closePreview();
    reloadAssets();
  }

  async function showAsset(p: string) {
    selAsset = p;
    if (!game || p.endsWith('/')) return;
    closePreview();
    const kind = G.assetKind(p);
    if (kind === 'image' || kind === 'sound') {
      const b64 = await G.assetData(game.id, p).catch(() => '');
      const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
      preview = { path: p, url: URL.createObjectURL(new Blob([bytes], { type: MIME[G.ext(p)] ?? 'application/octet-stream' })) };
    } else {
      preview = { path: p, url: '' };
    }
  }

  function closePreview() {
    if (preview?.url) URL.revokeObjectURL(preview.url);
    preview = null;
  }

  function copySnippet(p: string) {
    navigator.clipboard.writeText(G.loadSnippet(p)).then(() => store.toast('success', 'Copied the loading code', undefined, 1500));
  }

  // Moving assets inside the panel (drag a file or folder onto a folder).
  function dragStart(e: DragEvent, p: string) {
    dragging = p;
    e.dataTransfer?.setData('text/plain', p);
    if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
  }
  function dragOver(e: DragEvent, folder: string) {
    if (!dragging) return;
    e.preventDefault();
    dropFolder = folder;
  }
  function dropOn(e: DragEvent, folder: string) {
    e.preventDefault();
    const from = dragging;
    dragging = null;
    dropFolder = null;
    if (from && !(from.endsWith('/') && folder.startsWith(from))) move(from, folder);
  }

  // ------------------------------------------------------------------ games

  let newOpen = $state(false);
  let form = $state<{ name: string; template: keyof typeof G.TEMPLATES }>({ name: '', template: 'dodge' });

  async function createGame(e: Event) {
    e.preventDefault();
    try {
      const g = await G.create(form.name.trim() || 'My Game', form.template);
      newOpen = false;
      form = { name: '', template: 'blank' };
      await refresh();
      await open(g.id);
    } catch (err) {
      store.error(err, 'Couldn’t create the game');
    }
  }

  async function renameGame() {
    if (!game) return;
    const n = window.prompt('Rename game', game.name)?.trim();
    if (!n || n === game.name) return;
    await G.rename(game.id, n).catch((e) => store.error(e));
    await refresh();
    if (agent) agent.name = n;
  }

  async function deleteGame() {
    if (!game) return;
    if (!(await ask(`Delete “${game.name}”, its code and its conversation?`, { title: 'Delete game', kind: 'warning' }))) return;
    if (running) await rad.gameStop(game.id);
    await G.remove(game.id).catch((e) => store.error(e));
    await refresh();
    open(selId);
  }

  // ------------------------------------------------------------------ run / stop

  function append(t: string) {
    out += t;
    if (out.length > 300_000) out = out.slice(-300_000);
    tick().then(() => outEl?.scrollTo({ top: outEl.scrollHeight }));
  }

  /** Builds a Spectrum project, and test-runs it for the agent: what happened, in words. */
  async function zxBuildAndCheck(g: G.Game): Promise<{ ok: boolean; output: string; file?: { name: string; data: string } }> {
    const b = await rad.zxBuild(g.kind, g.name, files.map((f) => ({ path: f.path, content: f.content })));
    if (!b.ok || !b.file) return { ok: false, output: b.output };
    const c = await rad.zxCheck(b.file.name, b.file.data, 3);
    const screen = c.screen.length ? `\nThe screen showed:\n\`\`\`\n${c.screen.join('\n')}\n\`\`\`` : '\nThe screen was blank (no text on it).';
    if (!c.started) return { ok: false, output: `It built, but it didn't load on the Spectrum.${screen}`, file: b.file };
    if (c.crashed) return { ok: false, output: `It built, but it crashed: the Spectrum reset to its start-up screen within 3 seconds.${screen}`, file: b.file };
    if (c.report && !['0', '9', 'D', 'L'].includes(c.report.code))
      return { ok: false, output: `It built, but stopped with the error report "${c.report.code} ${c.report.message}" at line ${c.report.line}, statement ${c.report.statement}.${screen}`, file: b.file };
    const ended = c.report ? ` The program ended (${c.report.code} ${c.report.message}, ${c.report.line}:${c.report.statement}) — a game should keep running.` : ' It was still running (good).';
    return { ok: true, output: `It built and ran for 3 seconds with no keys pressed.${ended}${screen}`, file: b.file };
  }

  async function run() {
    if (!game || running || building) return;
    await flush();
    if (G.isZx(game.kind)) {
      out = '';
      append(`Building ${game.name} for the ZX Spectrum…\n`);
      phase = 'building';
      try {
        const b = await rad.zxBuild(game.kind, game.name, files.map((f) => ({ path: f.path, content: f.content })));
        append(b.output + '\n');
        if (!b.ok || !b.file) {
          phase = 'failed';
          append('\n✖ It didn’t build — see above, or ask the agent to fix it.\n');
          return;
        }
        append(`\n▶ Playing ${b.file.name} in the ZX Spectrum view.\n`);
        phase = null;
        store.pendingZx = { name: b.file.name, data: b.file.data, title: game.name };
        store.navigate({ kind: 'zx' });
      } catch (e: any) {
        phase = 'failed';
        append(`\n${e?.message ?? e}\n`);
      }
      return;
    }
    out = '';
    phase = 'building';
    append(`Building ${game.name}…\n`);
    status.building = [...status.building, game.id];
    try {
      const res = await G.allAssets(game.id);
      if (res.some((a) => !a.path.endsWith('/'))) append(`With ${res.filter((a) => !a.path.endsWith('/')).length} assets in resources/.\n`);
      const r = await rad.gameRun(game.id, files.map((f) => ({ path: f.path, content: f.content })), res, (e) => {
        if (e.o) append(e.o.replace(/^\s+Compiling .*\n$/, ''));
        if (e.phase) phase = e.phase;
        if (e.phase === 'running') append(`\n▶ ${game?.name} is running in its own window.\n`);
        if (e.phase === 'exited' && !e.build_failed) append(`\n■ The game ended${e.code != null && e.code !== 0 ? ` (exit code ${e.code})` : ''}.\n`);
      });
      if (!r.ok) {
        phase = 'failed';
        append('\n✖ The build failed — see the errors above, or ask the agent to fix them.\n');
      }
    } catch (e: any) {
      phase = 'failed';
      append(`\n${e?.message ?? e}\n`);
    } finally {
      refreshStatus();
    }
  }

  async function stop() {
    if (game) await rad.gameStop(game.id);
    refreshStatus();
  }

  // ------------------------------------------------------------------ chat

  function send() {
    const t = text.trim();
    if (!t || !agent || agent.busy) return;
    text = '';
    flush().then(() => agent!.ask(t));
  }

  function chatKey(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      send();
    }
  }

  const IDEAS_BY_KIND: Record<G.GameKind, string[]> = {
    ggez: ['Make it a space shooter with asteroids', 'Add lives and a high score', 'Add a main menu and pause with P', 'Make it a two-player pong'],
    'zx-basic': ['Add a title screen with a flashing "press any key"', 'Make the stars fall faster as the score grows', 'Use a user-defined graphic for the catcher', 'Make it a snake game'],
    'zx-asm': ['Make the ball speed up every 5 hits', 'Add bricks to knock out at the top', 'Add a high score and lives', 'Add a beep when the ball hits a wall'],
  };
  const IDEAS = $derived(IDEAS_BY_KIND[game?.kind ?? 'ggez']);
  let zxTools = $state<{ basic: boolean; asm: boolean }>({ basic: true, asm: true });

  $effect(() => {
    agent?.items.length;
    agent?.status;
    tick().then(() => thread?.scrollTo({ top: thread.scrollHeight }));
  });

  onMount(() => {
    refresh().then(() => open(selId));
    refreshStatus();
    rad.zxTools().then((t) => (zxTools = t)).catch(() => {});
    const un = listen('games-changed', () => refreshStatus());
    // Files dragged in from Finder: into the folder under the pointer (or the selected one).
    const folderAt = (x: number, y: number) => {
      const r = window.devicePixelRatio || 1;
      const el = document.elementFromPoint(x / r, y / r)?.closest('[data-folder]') as HTMLElement | null;
      return el ? (el.dataset.folder ?? '') : null;
    };
    const unDrop = getCurrentWebview().onDragDropEvent((ev) => {
      const p = ev.payload;
      if (!game) return;
      if (p.type === 'enter' || p.type === 'over') dropFolder = folderAt(p.position.x, p.position.y) ?? selFolder();
      else if (p.type === 'leave') dropFolder = null;
      else if (p.type === 'drop') {
        const folder = folderAt(p.position.x, p.position.y) ?? selFolder();
        dropFolder = null;
        addFiles(p.paths, folder);
      }
    });
    return () => {
      flush();
      closePreview();
      un.then((u) => u());
      unDrop.then((u) => u());
    };
  });
</script>

{#snippet assetTree(dir: string, depth: number)}
  {#each tree.folders(dir) as f (f)}
    <div
      class="ai folder"
      class:on={selAsset === f}
      class:drop={dropFolder === f}
      data-folder={f}
      role="treeitem"
      aria-selected={selAsset === f}
      tabindex="-1"
      draggable="true"
      ondragstart={(e) => dragStart(e, f)}
      ondragover={(e) => dragOver(e, f)}
      ondragleave={() => dropFolder === f && (dropFolder = null)}
      ondrop={(e) => dropOn(e, f)}
    >
      <button class="ab" style="padding-left:{6 + depth * 12}px" onclick={() => ((selAsset = f), (closed[f] = !closed[f]))}>
        {#if closed[f]}<ChevronRight size={11} />{:else}<ChevronDown size={11} />{/if}<Folder size={13} /> <span class="truncate">{leaf(f)}</span>
      </button>
      <Menu items={[{ label: 'Add files here…', icon: Upload, onclick: () => pickFiles(f) }, { label: 'Rename…', icon: Pencil, onclick: () => renameAsset(f) }, { label: 'Move to…', icon: FolderInput, onclick: () => moveAssetTo(f) }, null, { label: 'Delete folder', icon: Trash2, danger: true, onclick: () => deleteAsset(f) }]}>
        {#snippet trigger(toggle)}<button class="icon-btn sm more" onclick={toggle} aria-label="Folder menu"><MoreHorizontal size={12} /></button>{/snippet}
      </Menu>
    </div>
    {#if !closed[f]}{@render assetTree(f, depth + 1)}{/if}
  {/each}
  {#each tree.files(dir) as a (a.path)}
    {@const kind = G.assetKind(a.path)}
    <div class="ai" class:on={selAsset === a.path} data-folder={dir} role="treeitem" aria-selected={selAsset === a.path} tabindex="-1" draggable="true" ondragstart={(e) => dragStart(e, a.path)} ondragover={(e) => dragOver(e, dir)} ondrop={(e) => dropOn(e, dir)}>
      <button class="ab" style="padding-left:{20 + depth * 12}px" onclick={() => showAsset(a.path)} title={`/${a.path} · ${a.info ? a.info + ' · ' : ''}${Math.max(1, Math.round(a.size / 1024))} KB`}>
        {#if kind === 'image'}<ImageIcon size={13} />{:else if kind === 'sound'}<Music size={13} />{:else if kind === 'font'}<Type size={13} />{:else}<File size={13} />{/if}
        <span class="truncate">{leaf(a.path)}</span>
      </button>
      <Menu items={[{ label: 'Copy loading code', icon: Copy, onclick: () => copySnippet(a.path) }, { label: 'Rename…', icon: Pencil, onclick: () => renameAsset(a.path) }, { label: 'Move to…', icon: FolderInput, onclick: () => moveAssetTo(a.path) }, null, { label: 'Delete', icon: Trash2, danger: true, onclick: () => deleteAsset(a.path) }]}>
        {#snippet trigger(toggle)}<button class="icon-btn sm more" onclick={toggle} aria-label="Asset menu"><MoreHorizontal size={12} /></button>{/snippet}
      </Menu>
    </div>
  {/each}
{/snippet}

<div class="gv">
  <header class="drag">
    <div class="ttl no-drag"><Gamepad2 size={16} /> Games</div>
    {#if games.length}
      <select class="select no-drag pick" value={selId} onchange={(e) => open((e.currentTarget as HTMLSelectElement).value)}>
        {#each games as g (g.id)}<option value={g.id}>{g.name}</option>{/each}
      </select>
      <span class="muted enc no-drag" title="Saved in the encrypted Games database next to your vault"><Lock size={11} /> {saving ? 'Saving…' : 'Saved, encrypted'}</span>
    {/if}
    <span class="spacer"></span>
    {#if game}
      {#if running}
        <span class="pill success no-drag">Running</span>
        <button class="btn sm danger no-drag" onclick={stop} title="Stop (⌘↵)"><Square size={12} /> Stop</button>
      {:else if building}
        <span class="pill no-drag"><Loader size={11} class="spin" /> Building…</span>
        <button class="btn sm no-drag" onclick={stop}><Square size={12} /> Cancel</button>
      {:else}
        {#if G.isZx(game.kind)}
          <button class="btn sm primary no-drag" onclick={run} disabled={game.kind === 'zx-asm' && !zxTools.asm} title={game.kind === 'zx-asm' && !zxTools.asm ? 'The Z80 assembler isn’t built: run ./scripts/build-zx-tools.sh' : 'Build it and play it on the ZX Spectrum (⌘↵)'}><Play size={13} /> Run on Spectrum</button>
        {:else}
          <button class="btn sm primary no-drag" onclick={run} disabled={!status.rust} title="Build and play (⌘↵)"><Play size={13} /> Run</button>
        {/if}
      {/if}
      <div class="no-drag">
        <Menu items={[{ label: 'Rename…', icon: Pencil, onclick: renameGame }, null, { label: 'Delete game', icon: Trash2, danger: true, onclick: deleteGame }]}>
          {#snippet trigger(toggle)}<button class="icon-btn" onclick={toggle} aria-label="Game menu"><MoreHorizontal size={16} /></button>{/snippet}
        </Menu>
      </div>
    {/if}
    <button class="icon-btn no-drag" title="New game" onclick={() => ((form = { name: '', template: games.length ? 'blank' : 'dodge' }), (newOpen = true))}><Plus size={16} /></button>
  </header>

  {#if !loaded}
    <div class="center"><Loader size={18} class="spin" /></div>
  {:else if loadError}
    <div class="center note"><CircleAlert size={15} /> {loadError}</div>
  {:else if !games.length}
    <div class="center">
      <div class="intro">
        <div class="big"><Gamepad2 size={30} /></div>
        <h2>Make games with AI</h2>
        <p>2D games in Rust with <strong>ggez</strong>. Describe what you want — the agent writes the code, RAD builds it and the game opens in its own window. Games are saved in your encrypted storage.</p>
        {#if !status.rust}<p class="warn"><CircleAlert size={13} /> Rust isn’t installed on this Mac — get it from rustup.rs to build games.</p>{/if}
        <button class="btn primary" onclick={() => ((form = { name: '', template: 'dodge' }), (newOpen = true))}><Plus size={14} /> New game</button>
      </div>
    </div>
  {:else if game}
    <div class="body">
      <aside class="side">
        <div class="sh">
          <span class="section-title">Files</span>
          <span class="spacer"></span>
          <button class="icon-btn sm" title="New file" onclick={newFile}><FilePlus size={13} /></button>
        </div>
        {#each files as f (f.path)}
          <div class="fi" class:on={path === f.path}>
            <button class="fb" onclick={() => ((path = f.path), closePreview())} title={f.path}>
              {#if f.path.endsWith('.rs')}<FileCode size={13} />{:else}<File size={13} />{/if}
              <span class="truncate">{f.path}</span>
            </button>
            {#if f.path !== 'Cargo.toml' && f.path !== 'src/main.rs'}<button class="icon-btn sm del" title="Delete file" onclick={() => deleteFile(f.path)}><Trash2 size={11} /></button>{/if}
          </div>
        {/each}

        {#if !G.isZx(game.kind)}
        <div class="sh assets-h">
          <span class="section-title" title="The game's resources/ folder: load files with paths like &quot;/sprites/player.png&quot;">Assets</span>
          <span class="muted cnt">{assetCount || ''}</span>
          <span class="spacer"></span>
          {#if uploading}<Loader size={12} class="spin" />{/if}
          <button class="icon-btn sm" title="New folder" onclick={newFolder}><FolderPlus size={13} /></button>
          <button class="icon-btn sm" title="Add images, sounds or fonts" onclick={() => pickFiles()}><Upload size={13} /></button>
        </div>
        <div
          class="assets"
          class:drop={dropFolder === ''}
          data-folder=""
          role="tree"
          tabindex="-1"
          ondragover={(e) => dragOver(e, '')}
          ondrop={(e) => dropOn(e, '')}
        >
          <button class="ai root" class:on={selAsset === null} onclick={() => (selAsset = null)} title="The top of the resources/ folder">
            <Folder size={13} /> <span>resources/</span>
          </button>
          {@render assetTree('', 0)}
          {#if !assetCount}
            <div class="hint">Drop images, sounds and fonts here (PNG, JPG, WebP · OGG, WAV, MP3, FLAC · TTF). Make folders and drag files into them.</div>
          {/if}
        </div>
        {/if}
      </aside>

      <section class="main">
        <div class="ed">
          {#if preview}
            {@const kind = G.assetKind(preview.path)}
            {@const meta = assets.find((a) => a.path === preview?.path)}
            <div class="pv">
              <div class="pvh">
                <code>/{preview.path}</code>
                <span class="muted">{kind}{meta?.info ? ` · ${meta.info}` : ''}{meta ? ` · ${Math.max(1, Math.round(meta.size / 1024))} KB` : ''}</span>
                <span class="spacer"></span>
                <button class="btn sm" onclick={() => copySnippet(preview!.path)}><Copy size={12} /> Copy loading code</button>
                <button class="icon-btn sm" title="Back to the code" onclick={closePreview}><X size={14} /></button>
              </div>
              <div class="pvb">
                {#if kind === 'image' && preview.url}
                  <img src={preview.url} alt={preview.path} class="checker" />
                {:else if kind === 'sound' && preview.url}
                  <Music size={40} class="muted" />
                  <audio controls src={preview.url}></audio>
                {:else}
                  <File size={40} class="muted" />
                  <span class="muted">{kind === 'font' ? 'A font — load it with the code below.' : 'Available to the game at this path.'}</span>
                {/if}
              </div>
              <pre class="snippet selectable">{G.loadSnippet(preview.path)}</pre>
            </div>
          {:else if current}
            <textarea class="code selectable" value={current.content} oninput={(e) => edited((e.currentTarget as HTMLTextAreaElement).value)} onkeydown={onKey} spellcheck="false" autocomplete="off" autocapitalize="off"></textarea>
          {:else}
            <div class="center note">Choose a file.</div>
          {/if}
        </div>

        <div class="bottom">
          <div class="chat">
            <div class="ch">
              <Sparkles size={13} class="spark" /> <span class="title">Game agent</span>
              <span class="spacer"></span>
              {#if agent}
                <label class="auto" title="Play the game as soon as the agent's changes build"><input type="checkbox" bind:checked={agent.autoRun} /> Run when built</label>
                <button class="icon-btn sm" title="Clear the conversation" disabled={agent.busy || !agent.items.length} onclick={() => agent?.clear()}><Eraser size={12} /></button>
              {/if}
            </div>
            <div class="thread" bind:this={thread}>
              {#if agent && !agent.items.length}
                <div class="empty">
                  Tell the agent what game to make or what to change — it writes the code, builds it and fixes errors until it runs.
                  <div class="ideas">{#each IDEAS as i}<button class="chip" onclick={() => ((text = i), send())}>{i}</button>{/each}</div>
                </div>
              {/if}
              {#each agent?.items ?? [] as it, i (i)}
                {#if it.kind === 'user'}
                  <div class="user">{it.text}</div>
                {:else if it.kind === 'say'}
                  <div class="say md selectable">{@html renderMarkdown(it.text)}</div>
                {:else if it.kind === 'files'}
                  <div class="note">Updated {#each it.paths as p, j}{j ? ', ' : ''}<button class="link" onclick={() => (path = p)}>{p}</button>{/each}</div>
                {:else if it.kind === 'docs'}
                  <div class="note"><BookOpen size={11} /> Read the ggez docs: “{it.query}”{it.files.length ? ` — ${[...new Set(it.files)].join(', ')}` : ' (nothing found)'}</div>
                {:else if it.kind === 'build'}
                  <details class="build" class:bad={it.ok === false}>
                    <summary>
                      {#if it.ok === null}<Loader size={12} class="spin" /> Building…{:else if it.ok}<CircleCheck size={12} class="ok" /> Built{:else}<CircleX size={12} /> Build failed — fixing{/if}
                    </summary>
                    {#if it.output}<pre class="selectable">{it.output}</pre>{/if}
                  </details>
                {:else}
                  <div class="err">{it.text}</div>
                {/if}
              {/each}
              {#if agent?.busy && agent.status}<div class="st"><Loader size={12} class="spin" /> {agent.status}</div>{/if}
            </div>
            <div class="inrow">
              <textarea rows="1" bind:value={text} onkeydown={chatKey} placeholder="e.g. add enemies that chase the player, make it faster each level…"></textarea>
              {#if agent?.busy}
                <button class="send stop" title="Stop the agent" onclick={() => agent?.stop()}><Square size={12} /></button>
              {:else}
                <button class="send" title="Send (Enter)" disabled={!text.trim()} onclick={send}><ArrowUp size={15} /></button>
              {/if}
            </div>
          </div>
          <pre class="out selectable" bind:this={outEl}>{out || (G.isZx(game.kind) ? 'Press Run on Spectrum (⌘↵): RAD builds the tape and plays it in the ZX Spectrum view.' : status.rust ? 'Press Run (⌘↵) to build and play. Build output and the game’s messages show here.' : 'Rust isn’t installed on this Mac — install it from https://rustup.rs to build and run games.')}</pre>
        </div>
      </section>
    </div>
  {/if}
</div>

<Modal bind:open={newOpen} title="New game" width={500}>
  <form class="nf" id="newgame" onsubmit={createGame}>
    <label><span>Name</span><input class="input" bind:value={form.name} placeholder="e.g. Space Dodge" /></label>
    <div class="kinds">
      {#each G.KINDS as k (k.kind)}
        <button type="button" class="kind" class:on={G.TEMPLATES[form.template].kind === k.kind} onclick={() => (form.template = k.templates[0])} disabled={k.kind === 'zx-asm' && !zxTools.asm}>
          <strong>{k.label}</strong><span>{k.kind === 'zx-asm' && !zxTools.asm ? 'Needs the assembler: run ./scripts/build-zx-tools.sh' : k.note}</span>
        </button>
      {/each}
    </div>
    <label><span>Start from</span>
      <select class="select" bind:value={form.template}>
        {#each G.KINDS.find((k) => k.kind === G.TEMPLATES[form.template].kind)?.templates ?? [] as t}<option value={t}>{G.TEMPLATES[t].label}</option>{/each}
      </select>
    </label>
    <p class="muted small">Then describe the game to the agent — or edit the code yourself.</p>
  </form>
  {#snippet footer()}
    <span class="spacer"></span>
    <button class="btn ghost" onclick={() => (newOpen = false)}>Cancel</button>
    <button class="btn primary" type="submit" form="newgame">Create</button>
  {/snippet}
</Modal>

<style>
  .gv {
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
  .pick {
    max-width: 220px;
    height: 28px;
    padding-top: 0;
    padding-bottom: 0;
    font-size: 12.5px;
  }
  .enc {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 11.5px;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .center {
    flex: 1;
    display: grid;
    place-items: center;
    padding: 24px;
  }
  .note {
    display: flex;
    gap: 6px;
    align-items: center;
    font-size: 13px;
    color: var(--text-2);
  }
  .intro {
    max-width: 470px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    text-align: center;
  }
  .intro h2 {
    margin: 0;
    font-size: 20px;
  }
  .intro p {
    margin: 0 0 4px;
    color: var(--text-2);
    font-size: 13px;
    line-height: 1.5;
  }
  .warn {
    color: var(--warning) !important;
    display: flex;
    gap: 5px;
    align-items: center;
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
  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .side {
    flex: none;
    width: 190px;
    border-right: 1px solid var(--border);
    overflow: auto;
    padding: 0 6px 8px;
  }
  .sh {
    display: flex;
    align-items: center;
    height: 36px;
    padding: 0 2px 0 6px;
  }
  .fi {
    display: flex;
    align-items: center;
    border-radius: var(--r-sm);
  }
  .fi:hover {
    background: var(--surface-hover);
  }
  .fi.on {
    background: var(--surface-active, var(--surface-hover));
  }
  .fb {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 8px;
    border: none;
    background: transparent;
    color: var(--text-2);
    font-family: var(--mono);
    font-size: 12px;
    text-align: left;
  }
  .fi.on .fb {
    color: var(--text);
  }
  .fb :global(svg) {
    flex: none;
    color: var(--text-3);
  }
  .del {
    opacity: 0;
  }
  .fi:hover .del {
    opacity: 1;
  }
  .main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .assets-h {
    margin-top: 10px;
    border-top: 1px solid var(--border);
    padding-top: 4px;
    height: 40px;
  }
  .cnt {
    font-size: 11px;
    margin-left: 4px;
  }
  .assets {
    min-height: 90px;
    padding-bottom: 12px;
    border-radius: var(--r-sm);
  }
  .assets.drop,
  .ai.drop {
    background: var(--accent-soft, var(--surface-hover));
    outline: 1px dashed var(--accent);
  }
  .ai {
    display: flex;
    align-items: center;
    border-radius: var(--r-sm);
  }
  .ai:hover {
    background: var(--surface-hover);
  }
  .ai.on {
    background: var(--surface-active, var(--surface-hover));
  }
  .ai.root {
    width: 100%;
    gap: 6px;
    height: 26px;
    padding: 0 8px;
    border: none;
    background: transparent;
    color: var(--text-3);
    font-family: var(--mono);
    font-size: 11.5px;
  }
  .ab {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 5px;
    height: 26px;
    padding-right: 4px;
    border: none;
    background: transparent;
    color: var(--text-2);
    font-size: 12px;
    text-align: left;
  }
  .ab :global(svg) {
    flex: none;
    color: var(--text-3);
  }
  .ai .more {
    opacity: 0;
  }
  .ai:hover .more {
    opacity: 1;
  }
  .hint {
    padding: 6px 8px;
    font-size: 11.5px;
    color: var(--text-3);
    line-height: 1.45;
  }
  .pv {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .pvh {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 38px;
    padding: 0 10px 0 14px;
    border-bottom: 1px solid var(--border);
    font-size: 12px;
  }
  .pvb {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 16px;
    overflow: auto;
  }
  .pvb img {
    max-width: 100%;
    max-height: 100%;
    min-width: 64px;
    image-rendering: pixelated;
  }
  .checker {
    background: repeating-conic-gradient(#8884 0% 25%, transparent 0% 50%) 50% / 16px 16px;
  }
  .snippet {
    flex: none;
    margin: 0;
    padding: 10px 14px;
    border-top: 1px solid var(--border);
    background: #14141b;
    color: #e6e6ef;
    font-family: var(--mono);
    font-size: 11.5px;
    white-space: pre-wrap;
  }
  .ed {
    flex: 1;
    min-height: 140px;
    display: flex;
  }
  .code {
    flex: 1;
    resize: none;
    border: none;
    outline: none;
    padding: 12px 14px;
    background: var(--surface);
    color: var(--text);
    font-family: var(--mono);
    font-size: 12.5px;
    line-height: 1.55;
    tab-size: 4;
    white-space: pre;
    overflow: auto;
  }
  .bottom {
    flex: none;
    height: 44%;
    min-height: 220px;
    display: flex;
    border-top: 1px solid var(--border);
  }
  .chat {
    flex: 3;
    min-width: 0;
    display: flex;
    flex-direction: column;
    border-right: 1px solid var(--border);
  }
  .ch {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
    height: 34px;
    padding: 0 10px 0 12px;
    border-bottom: 1px solid var(--border);
    font-size: 12.5px;
  }
  .ch :global(.spark) {
    color: var(--accent);
  }
  .title {
    font-weight: 600;
  }
  .auto {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 11.5px;
    color: var(--text-2);
  }
  .thread {
    flex: 1;
    overflow: auto;
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    font-size: 13px;
  }
  .empty {
    color: var(--text-3);
    font-size: 12.5px;
    line-height: 1.5;
  }
  .ideas {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 8px;
  }
  .chip {
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--surface);
    color: var(--text-2);
    font-size: 12px;
    padding: 4px 10px;
  }
  .chip:hover {
    border-color: var(--accent);
    color: var(--accent);
  }
  .user {
    align-self: flex-end;
    max-width: 85%;
    padding: 6px 10px;
    border-radius: 12px;
    background: var(--accent);
    color: white;
    white-space: pre-wrap;
  }
  .say {
    line-height: 1.5;
  }
  .note {
    font-size: 12px;
    color: var(--text-3);
    display: block;
  }
  .link {
    border: none;
    background: none;
    padding: 0;
    color: var(--accent);
    font-family: var(--mono);
    font-size: 11.5px;
  }
  .build summary {
    display: flex;
    align-items: center;
    gap: 5px;
    font-size: 12px;
    color: var(--text-2);
    cursor: pointer;
  }
  .build.bad summary {
    color: var(--warning);
  }
  .build :global(.ok) {
    color: var(--success);
  }
  .build pre {
    margin: 6px 0 0;
    max-height: 200px;
    overflow: auto;
    padding: 8px;
    border-radius: var(--r-sm);
    background: #14141b;
    color: #e6e6ef;
    font-family: var(--mono);
    font-size: 11px;
    white-space: pre-wrap;
  }
  .err {
    color: var(--danger);
    font-size: 12.5px;
  }
  .st {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-3);
  }
  .inrow {
    flex: none;
    display: flex;
    align-items: flex-end;
    gap: 8px;
    padding: 8px 10px;
    border-top: 1px solid var(--border);
  }
  .inrow textarea {
    flex: 1;
    resize: none;
    max-height: 100px;
    border: 1px solid var(--border);
    border-radius: 12px;
    padding: 7px 10px;
    background: var(--surface-2);
    color: var(--text);
    font: inherit;
    font-size: 13px;
    outline: none;
  }
  .send {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: 50%;
    border: none;
    background: var(--accent);
    color: white;
  }
  .send:disabled {
    opacity: 0.4;
  }
  .send.stop {
    background: var(--danger);
  }
  .out {
    flex: 2;
    min-width: 0;
    margin: 0;
    padding: 10px 12px;
    overflow: auto;
    background: #14141b;
    color: #e6e6ef;
    font-family: var(--mono);
    font-size: 11.5px;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .kinds {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .kind {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 8px 10px;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    background: var(--surface);
    color: var(--text);
    text-align: left;
    font-size: 13px;
  }
  .kind span {
    font-size: 11.5px;
    color: var(--text-3);
  }
  .kind.on {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-ring);
  }
  .nf {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .nf label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .small {
    font-size: 12px;
    margin: 0;
  }
</style>
