<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { ask } from '@tauri-apps/plugin-dialog';
  import { revealItemInDir } from '@tauri-apps/plugin-opener';
  import { rad } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import { formatBytes } from '../lib/util';
  import { containerAgent } from '../lib/termAgent.svelte';
  import type { ContainerEntry, ContainerInfo, ContainerKindId, ContainerStatus, SandboxProgress } from '../lib/types';
  import AgentDock from './AgentDock.svelte';
  import Menu from './Menu.svelte';
  import {
    Cog, FileCode, Play, Square, Power, Plus, Loader, CircleAlert, FolderOpen, SquareTerminal, RotateCcw, Trash2, MoreHorizontal,
    File, Folder, ChevronRight, ChevronDown, Save, Wifi, WifiOff, Cpu, MemoryStick, HardDrive, Eraser, RefreshCw, FilePlus, Plug,
  } from '@lucide/svelte';

  let { kind }: { kind: ContainerKindId } = $props();

  const KEY = `rad-container-${kind}`;
  const HIDE = new Set(['target', '.git', '__pycache__', '.venv', 'node_modules', '.mypy_cache', '.pytest_cache']);

  let st = $state<ContainerStatus | null>(null);
  let prog = $state<SandboxProgress | null>(null);
  let creating = $state(false);
  let busy = $state<string | null>(null);
  let selectedId = $state<string | null>(read(KEY));

  const k = $derived(st?.kinds.find((x) => x.id === kind) ?? null);
  const mine = $derived(st?.containers.filter((c) => c.kind === kind) ?? []);
  const sel = $derived(mine.find((c) => c.id === selectedId) ?? mine[0] ?? null);
  const agent = $derived(sel?.running ? containerAgent(sel.id, sel.name, sel.kind === 'python' ? 'Python 3' : 'Rust') : null);
  const Icon = $derived(kind === 'rust' ? Cog : FileCode);

  // Project files and the editor.
  let tree = $state<Record<string, ContainerEntry[]>>({});
  let openDirs = $state<Record<string, boolean>>({});
  let path = $state<string | null>(null);
  let text = $state('');
  let saved = $state('');
  let loading = $state(false);
  const dirty = $derived(path !== null && text !== saved);
  let editor: HTMLTextAreaElement | undefined = $state();

  // Output of Run and of the agent's commands.
  let out = $state('');
  let runId = $state<string | null>(null);
  let exit = $state<number | null | undefined>(undefined);
  let outEl: HTMLPreElement | undefined = $state();

  let portsText = $state('');

  function read(key: string) {
    try {
      return localStorage.getItem(key);
    } catch {
      return null;
    }
  }

  function select(id: string) {
    if (id === sel?.id) return;
    selectedId = id;
    try {
      localStorage.setItem(KEY, id);
    } catch {
      /* private mode */
    }
    tree = {};
    path = null;
    text = saved = '';
    out = '';
    exit = undefined;
  }

  async function refresh() {
    const s = await rad.containerStatus().catch(() => null);
    if (!s) return;
    st = s;
    if (s.progress) prog = s.progress;
    if (!s.containers.some((c) => c.kind === kind) && !busy) creating = true;
  }

  // ------------------------------------------------------------------ lifecycle

  async function create(e?: Event) {
    e?.preventDefault();
    busy = 'create';
    try {
      const c = await rad.containerCreate(kind);
      creating = false;
      await refresh();
      select(c.id);
      await start(c.id);
    } catch (err) {
      store.error(err, 'Couldn’t create the container');
    } finally {
      busy = null;
      prog = null;
    }
  }

  async function start(id = sel?.id) {
    if (!id) return;
    busy = id;
    try {
      await rad.containerStart(id);
      await refresh();
      await loadTree();
      if (!path && k) await openFile(`${st!.project}/${k.files[0]}`);
    } catch (err) {
      store.error(err, 'Couldn’t start the container');
    } finally {
      busy = null;
    }
  }

  async function stop() {
    if (!sel) return;
    if (dirty && !(await ask(`Discard unsaved changes to ${name(path!)}?`, { title: 'Stop container', kind: 'warning' }))) return;
    busy = sel.id;
    try {
      if (runId) await rad.containerCancel(runId);
      await rad.containerStop(sel.id);
      path = null;
      text = saved = '';
      tree = {};
    } catch (err) {
      store.error(err, 'Couldn’t stop the container');
    } finally {
      busy = null;
      refresh();
    }
  }

  async function restart() {
    await stop();
    await start();
  }

  async function reset() {
    if (!sel) return;
    if (!(await ask(`Reset “${sel.name}” to a fresh ${sel.kind_name} container? Its project, packages and everything else on its drive are erased. Its shared folder is kept.`, { title: 'Reset container', kind: 'warning' }))) return;
    busy = sel.id;
    try {
      await rad.containerReset(sel.id);
      path = null;
      text = saved = '';
      tree = {};
      store.toast('success', 'Container reset', sel.name);
    } catch (err) {
      store.error(err, 'Reset failed');
    } finally {
      busy = null;
      prog = null;
      refresh();
    }
  }

  async function remove() {
    if (!sel) return;
    if (!(await ask(`Delete “${sel.name}” and its drive? Its shared folder is deleted too.`, { title: 'Delete container', kind: 'warning' }))) return;
    busy = sel.id;
    try {
      await rad.containerDelete(sel.id);
      selectedId = null;
    } catch (err) {
      store.error(err, 'Delete failed');
    } finally {
      busy = null;
      refresh();
    }
  }

  async function rename() {
    if (!sel) return;
    const n = window.prompt('Container name', sel.name)?.trim();
    if (n && n !== sel.name) await rad.containerUpdate({ id: sel.id, name: n }).catch((e) => store.error(e));
    refresh();
  }

  async function setOpt(opts: { cpus?: number; mem_mb?: number; network?: boolean; ports?: number[] }) {
    if (!sel) return;
    await rad.containerUpdate({ id: sel.id, ...opts }).catch((e) => store.error(e));
    await refresh();
    if (sel?.running) store.toast('info', 'Saved', 'Takes effect when the container restarts.', 3000);
  }

  function savePorts() {
    if (!sel) return;
    const ports = [...new Set(portsText.split(/[\s,]+/).map(Number).filter((p) => Number.isInteger(p) && p > 0 && p < 65536))];
    if (ports.join(',') === sel.ports.join(',')) return;
    setOpt({ ports });
  }

  async function shared() {
    if (!sel) return;
    const d = await rad.containerSharedDir(sel.id).catch(() => null);
    if (d) revealItemInDir(d);
  }

  function terminal() {
    if (!sel) return;
    store.pendingTerminal = { kind: 'container', id: sel.id };
    store.navigate({ kind: 'terminal' });
  }

  // ------------------------------------------------------------------ files

  const name = (p: string) => p.split('/').pop() ?? p;

  async function loadDir(dir: string) {
    if (!sel?.running) return;
    const entries = await rad.containerList(sel.id, dir).catch(() => [] as ContainerEntry[]);
    tree[dir] = entries.filter((e) => !HIDE.has(e.name));
  }

  async function loadTree() {
    if (!st || !sel?.running) return;
    const dirs = [st.project, ...Object.keys(openDirs).filter((d) => openDirs[d])];
    await Promise.all(dirs.map(loadDir));
  }

  async function toggleDir(dir: string) {
    openDirs[dir] = !openDirs[dir];
    if (openDirs[dir]) await loadDir(dir);
  }

  async function openFile(p: string) {
    if (!sel?.running || p === path) return;
    if (dirty && !(await ask(`Discard unsaved changes to ${name(path!)}?`, { title: 'Open file', kind: 'warning' }))) return;
    loading = true;
    try {
      const data = await rad.containerRead(sel.id, p);
      path = p;
      text = saved = data;
    } catch (err) {
      store.error(err, `Couldn’t open ${name(p)}`);
    } finally {
      loading = false;
    }
  }

  async function save() {
    if (!sel || !path || !dirty) return;
    const data = text;
    try {
      await rad.containerWrite(sel.id, path, data);
      saved = data;
    } catch (err) {
      store.error(err, 'Couldn’t save');
    }
  }

  async function newFile() {
    if (!sel?.running || !st) return;
    const rel = window.prompt(`New file (relative to ${st.project})`, kind === 'rust' ? 'src/lib.rs' : 'utils.py')?.trim();
    if (!rel || rel.includes('..')) return;
    const p = rel.startsWith('/') ? rel : `${st.project}/${rel}`;
    try {
      await rad.containerWrite(sel.id, p, '');
      const parent = p.slice(0, p.lastIndexOf('/'));
      if (parent !== st.project) openDirs[parent] = true;
      await loadTree();
      await openFile(p);
    } catch (err) {
      store.error(err, 'Couldn’t create the file');
    }
  }

  /** After the agent works, show what it changed (unless the user is mid-edit). */
  async function reloadAfterAgent() {
    await loadTree();
    if (sel?.running && path && !dirty) {
      const data = await rad.containerRead(sel.id, path).catch(() => null);
      if (data !== null && text === saved) text = saved = data;
    }
  }

  function onEditorKey(e: KeyboardEvent) {
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.key.toLowerCase() === 's') {
      e.preventDefault();
      save();
    } else if (mod && e.key === 'Enter') {
      e.preventDefault();
      run();
    } else if (e.key === 'Tab' && !mod && !e.altKey) {
      e.preventDefault();
      const el = e.currentTarget as HTMLTextAreaElement;
      const [a, b] = [el.selectionStart, el.selectionEnd];
      const ind = kind === 'rust' || kind === 'python' ? '    ' : '\t';
      text = text.slice(0, a) + ind + text.slice(b);
      tick().then(() => el.setSelectionRange(a + ind.length, a + ind.length));
    }
  }

  // ------------------------------------------------------------------ running

  const MAX_OUT = 400_000;

  function append(t: string) {
    out += t;
    if (out.length > MAX_OUT) out = out.slice(out.length - MAX_OUT);
    const stick = outEl && outEl.scrollHeight - outEl.scrollTop - outEl.clientHeight < 40;
    if (stick !== false) tick().then(() => outEl?.scrollTo({ top: outEl.scrollHeight }));
  }

  async function run(command = k?.run) {
    if (!sel?.running || !command || runId) return;
    await save();
    const id = `r${Date.now()}`;
    runId = id;
    exit = undefined;
    out = '';
    append(`$ ${command === k?.run ? (kind === 'rust' ? 'cargo run' : 'python main.py') : command}\n`);
    try {
      exit = await rad.containerRun(sel.id, command, id, (e) => append(e.o ?? e.e ?? ''));
    } catch (err: any) {
      append(`\n${err?.message ?? err}\n`);
      exit = -1;
    } finally {
      runId = null;
      loadTree();
    }
  }

  function cancel() {
    if (runId) rad.containerCancel(runId);
  }

  // The agent's commands appear in Output; when it's done, the editor shows its changes.
  $effect(() => {
    const a = agent;
    if (!a) return;
    a.onCommand = (c) => append(`${out && !out.endsWith('\n') ? '\n' : ''}agent$ ${c.split('\n')[0]}${c.includes('\n') ? ' …' : ''}\n`);
    return () => {
      if (a.onCommand) a.onCommand = null;
    };
  });
  let wasBusy = false;
  $effect(() => {
    const b = agent?.busy ?? false;
    if (wasBusy && !b) reloadAfterAgent();
    wasBusy = b;
  });

  // Follow the selected container.
  $effect(() => {
    if (sel) portsText = sel.ports.join(', ');
  });
  let loadedFor = '';
  $effect(() => {
    const key = sel?.running ? sel.id : '';
    if (key === loadedFor) return;
    loadedFor = key;
    if (key && k && st) {
      loadTree().then(() => {
        if (!path) openFile(`${st!.project}/${k!.files[0]}`);
      });
    }
  });

  const pct = $derived(prog?.total ? Math.floor(((prog.done ?? 0) / prog.total) * 100) : null);
  const gb = (mb: number) => (mb >= 1024 ? `${+(mb / 1024).toFixed(1)} GB` : `${mb} MB`);
  const memChoices = $derived.by(() => {
    const max = st?.host.mem_mb ?? 8192;
    return [1024, 2048, 4096, 6144, 8192, 12288, 16384, 24576, 32768].filter((m) => m <= max);
  });
  const cpuChoices = $derived([1, 2, 4, 6, 8].filter((n) => n <= Math.max(1, st?.host.cpus ?? 4)));

  onMount(() => {
    refresh();
    const poll = setInterval(refresh, 4000);
    const un = [listen('containers-changed', () => refresh()), listen<SandboxProgress | null>('container-progress', (e) => (prog = e.payload))];
    return () => {
      clearInterval(poll);
      un.forEach((p) => p.then((u) => u()));
    };
  });

  function rel(p: string) {
    return st && p.startsWith(st.project + '/') ? p.slice(st.project.length + 1) : p;
  }
</script>

{#snippet dirTree(dir: string, depth: number)}
  {#each tree[dir] ?? [] as e (e.name)}
    {@const p = `${dir}/${e.name}`}
    {#if e.dir}
      <button class="fe" style="padding-left:{8 + depth * 12}px" onclick={() => toggleDir(p)}>
        {#if openDirs[p]}<ChevronDown size={12} />{:else}<ChevronRight size={12} />{/if}<Folder size={13} class="fi" /> <span class="truncate">{e.name}</span>
      </button>
      {#if openDirs[p]}{@render dirTree(p, depth + 1)}{/if}
    {:else}
      <button class="fe" class:on={path === p} style="padding-left:{20 + depth * 12}px" onclick={() => openFile(p)} title={rel(p)}>
        <File size={13} class="fi" /> <span class="truncate">{e.name}</span>
      </button>
    {/if}
  {/each}
{/snippet}

<div class="cv">
  <header class="drag">
    <div class="ttl no-drag"><Icon size={16} /> {kind === 'rust' ? 'Rust' : 'Python'} Container</div>
    {#if mine.length}
      <select class="select sm no-drag pick" value={sel?.id} onchange={(e) => select((e.currentTarget as HTMLSelectElement).value)} disabled={!!busy}>
        {#each mine as c (c.id)}<option value={c.id}>{c.name}{c.running ? ' ●' : ''}</option>{/each}
      </select>
    {/if}
    {#if sel}
      {#if sel.running}<span class="pill success no-drag">Running</span>{:else if busy === sel.id || sel.starting}<span class="pill no-drag"><Loader size={11} class="spin" /> Working…</span>{:else}<span class="pill no-drag">Stopped</span>{/if}
    {/if}
    <span class="spacer"></span>
    {#if sel}
      {#if sel.running}
        <button class="btn sm no-drag" title="Open a shell in the Terminal" onclick={terminal}><SquareTerminal size={13} /> Terminal</button>
        <button class="btn sm no-drag" title="Stop the container (its files are kept)" disabled={!!busy} onclick={stop}><Power size={13} /> Stop</button>
      {:else}
        <button class="btn sm primary no-drag" disabled={!!busy || sel.starting} onclick={() => start()}><Play size={13} /> Start</button>
      {/if}
      <div class="no-drag">
        <Menu
          items={[
            { label: 'Shared folder', icon: FolderOpen, onclick: shared },
            { label: 'Rename…', icon: File, onclick: rename },
            ...(sel.running ? [{ label: 'Restart', icon: RefreshCw, onclick: restart }] : []),
            null,
            { label: 'Reset to fresh', icon: RotateCcw, onclick: reset },
            { label: 'Delete container', icon: Trash2, danger: true, onclick: remove },
          ]}
        >
          {#snippet trigger(toggle)}
            <button class="icon-btn" onclick={toggle} disabled={!!busy} aria-label="Container menu"><MoreHorizontal size={16} /></button>
          {/snippet}
        </Menu>
      </div>
    {/if}
    {#if mine.length}
      <button class="icon-btn no-drag" title={`New ${kind === 'rust' ? 'Rust' : 'Python'} container`} disabled={!!busy} onclick={() => create()}><Plus size={16} /></button>
    {/if}
  </header>

  {#if !st}
    <div class="center"><Loader size={18} class="spin" /></div>
  {:else if !st.supported}
    <div class="center note"><CircleAlert size={15} /> Containers need a Mac with Apple Silicon for now.</div>
  {:else if !st.engine}
    <div class="center note">
      <CircleAlert size={15} />
      {#if st.dev_build}
        The container engine isn’t built yet — run <code>./scripts/build-sandbox.sh</code>, then reopen this view.
      {:else}
        This copy of RAD was built without the container engine. Rebuild it with <code>./scripts/build-app.sh</code> and reinstall.
      {/if}
    </div>
  {:else if !mine.length || (creating && busy === 'create')}
    <div class="center">
      <form class="intro" onsubmit={create}>
        <div class="big"><Icon size={28} /></div>
        <h2>{k?.name} Container</h2>
        <p>{k?.description}</p>
        <ul>
          <li><HardDrive size={13} /> Its own persistent drive that grows as it fills — only what’s used takes space on this Mac.</li>
          <li><MemoryStick size={13} /> Memory is a ceiling: it only takes what it uses, up to that (freed when it stops).</li>
          <li><Wifi size={13} /> Internet for {kind === 'rust' ? 'crates.io' : 'PyPI'}, but it can’t reach this Mac — or switch it to isolated.</li>
          <li><FolderOpen size={13} /> <code>/root/shared</code> inside is a folder on your Mac, for results.</li>
        </ul>
        {#if busy === 'create'}
          <div class="prog">
            <div class="pl"><Loader size={13} class="spin" /> {prog?.phase ?? 'Starting'}…{pct !== null ? ` ${pct}%` : ''}</div>
            <div class="bar"><div style="width:{pct ?? 8}%" class:indet={pct === null}></div></div>
            {#if !k?.ready}<div class="hint">The first {k?.name} container is set up once (Debian and the toolchain) — a few minutes. The next ones are instant.</div>{/if}
          </div>
        {:else}
          <button class="btn primary"><Plus size={14} /> Create {k?.name} container</button>
          {#if !k?.ready}<div class="hint">First time: downloads Debian and the {k?.name} toolchain (a few minutes). After that new containers are instant.</div>{/if}
        {/if}
      </form>
    </div>
  {:else if sel}
    <div class="body">
      <aside class="tree">
        <div class="th">
          <span class="section-title">Project</span>
          <span class="spacer"></span>
          <button class="icon-btn sm" title="New file" disabled={!sel.running} onclick={newFile}><FilePlus size={13} /></button>
          <button class="icon-btn sm" title="Refresh" disabled={!sel.running} onclick={loadTree}><RefreshCw size={12} /></button>
        </div>
        <div class="tl">
          {#if sel.running && st}
            {@render dirTree(st.project, 0)}
          {:else}
            <div class="none">Start the container to see its files.</div>
          {/if}
        </div>
      </aside>

      <section class="main">
        <div class="edbar">
          <span class="fname truncate">{path ? rel(path) : 'No file open'}{#if dirty}<span class="dot" title="Unsaved changes"></span>{/if}</span>
          <span class="spacer"></span>
          <button class="btn sm ghost" title="Save (⌘S)" disabled={!dirty} onclick={save}><Save size={13} /> Save</button>
          {#if runId}
            <button class="btn sm danger" onclick={cancel}><Square size={12} /> Stop</button>
          {:else}
            <button class="btn sm primary" title="Save and run (⌘↵)" disabled={!sel.running} onclick={() => run()}><Play size={13} /> Run</button>
          {/if}
        </div>
        <div class="ed">
          {#if !sel.running}
            <div class="center note">
              {#if busy === sel.id || sel.starting}<Loader size={15} class="spin" /> Starting…{:else}
                <button class="btn primary" onclick={() => start()}><Play size={14} /> Start {sel.name}</button>{/if}
            </div>
          {:else if loading}
            <div class="center"><Loader size={16} class="spin" /></div>
          {:else if path}
            <textarea bind:this={editor} bind:value={text} spellcheck="false" autocomplete="off" autocapitalize="off" onkeydown={onEditorKey} class="code selectable"></textarea>
          {:else}
            <div class="center note">Choose a file on the left.</div>
          {/if}
        </div>
        <div class="outbar">
          <span class="section-title">Output</span>
          {#if runId}<Loader size={12} class="spin" />{:else if exit !== undefined}<span class="ex" class:bad={exit !== 0}>{exit === null ? 'stopped' : `exit ${exit}`}</span>{/if}
          <span class="spacer"></span>
          <button class="icon-btn sm" title="Clear" onclick={() => ((out = ''), (exit = undefined))}><Eraser size={12} /></button>
        </div>
        <pre class="out selectable" bind:this={outEl}>{out || (sel.running ? `Run executes “${kind === 'rust' ? 'cargo run' : 'python main.py'}” in ${st?.project}.` : '')}</pre>
      </section>

      <aside class="info">
        <div class="section-title">Resources</div>
        <div class="stat">
          <Cpu size={13} /><span class="spacer">CPUs</span>
          <select class="select sm" value={sel.cpus} onchange={(e) => setOpt({ cpus: +(e.currentTarget as HTMLSelectElement).value })}>
            {#each cpuChoices as n}<option value={n}>{n}</option>{/each}
          </select>
        </div>
        <div class="stat">
          <MemoryStick size={13} /><span class="spacer">Memory up to</span>
          <select class="select sm" value={sel.mem_mb} onchange={(e) => setOpt({ mem_mb: +(e.currentTarget as HTMLSelectElement).value })}>
            {#each memChoices.includes(sel.mem_mb) ? memChoices : [...memChoices, sel.mem_mb].sort((a, b) => a - b) as m}<option value={m}>{gb(m)}</option>{/each}
          </select>
        </div>
        {#if sel.stats}
          <div class="meter" title="Memory the container is using"><div style="width:{Math.min(100, (sel.stats.mem_used / Math.max(1, sel.stats.mem_total)) * 100)}%"></div></div>
          <div class="sub">{formatBytes(sel.stats.mem_used)} in use</div>
        {/if}
        <div class="stat"><HardDrive size={13} /><span class="spacer">Drive</span><span class="v">{sel.disk_gb} GB, grows</span></div>
        {#if sel.stats}
          <div class="meter" title="Drive space used"><div style="width:{Math.min(100, (sel.stats.disk_used / Math.max(1, sel.stats.disk_total)) * 100)}%"></div></div>
          <div class="sub">{formatBytes(sel.stats.disk_used)} of {formatBytes(sel.stats.disk_total)} · {formatBytes(sel.disk_on_mac)} on this Mac</div>
        {:else}
          <div class="sub">{formatBytes(sel.disk_on_mac)} on this Mac</div>
        {/if}
        {#if sel.grow_pending}<div class="sub warn">Grows to {sel.disk_gb} GB at the next start.</div>{/if}

        <div class="section-title gap">Network</div>
        <div class="seg">
          <button class:on={sel.network} onclick={() => !sel.network && setOpt({ network: true })}><Wifi size={12} /> Internet</button>
          <button class:on={!sel.network} onclick={() => sel.network && setOpt({ network: false })}><WifiOff size={12} /> Isolated</button>
        </div>
        <div class="sub">{sel.network ? 'Reaches the internet; this Mac and your network stay unreachable.' : 'No network at all. Packages can’t be installed.'}</div>
        <label class="ports">
          <span><Plug size={12} /> Ports to this Mac</span>
          <input class="input sm" bind:value={portsText} placeholder="e.g. 8000, 5000" onblur={savePorts} onkeydown={(e) => e.key === 'Enter' && savePorts()} disabled={!sel.network} />
        </label>
        {#if sel.ports.length && sel.network}
          <div class="sub">{#each sel.ports as p, i}{i ? ', ' : ''}<code>127.0.0.1:{p}</code>{/each} → the container</div>
        {/if}

        <div class="section-title gap">Files</div>
        <button class="btn sm wide" onclick={shared}><FolderOpen size={13} /> Shared folder</button>
        <div class="sub"><code>/root/shared</code> inside — put results there.</div>
        {#if sel.running}<button class="btn sm wide" onclick={terminal}><SquareTerminal size={13} /> Open a shell</button>{/if}
      </aside>
    </div>
    {#if agent}
      <AgentDock {agent} />
    {/if}
  {/if}
</div>

<style>
  .cv {
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
    max-width: 200px;
  }
  .pick,
  .stat select,
  .ports input {
    height: 28px;
    padding-top: 0;
    padding-bottom: 0;
    font-size: 12.5px;
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
    gap: 8px;
    align-items: center;
    justify-content: center;
    font-size: 13px;
    color: var(--text-2);
  }
  code {
    font-family: var(--mono);
    font-size: 11.5px;
    padding: 1px 4px;
    border-radius: var(--r-xs);
    background: var(--code-bg);
  }
  .intro {
    max-width: 480px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    text-align: center;
  }
  .big {
    display: grid;
    place-items: center;
    width: 56px;
    height: 56px;
    border-radius: 16px;
    background: var(--accent-soft, var(--surface-2));
    color: var(--accent);
  }
  .intro h2 {
    margin: 0;
    font-size: 19px;
  }
  .intro p {
    margin: 0;
    color: var(--text-2);
    font-size: 13px;
  }
  .intro ul {
    list-style: none;
    margin: 4px 0 8px;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 7px;
    text-align: left;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .intro li {
    display: flex;
    gap: 8px;
    align-items: flex-start;
  }
  .intro li :global(svg) {
    flex: none;
    margin-top: 2px;
    color: var(--text-3);
  }
  .hint {
    font-size: 12px;
    color: var(--text-3);
  }
  .prog {
    width: 100%;
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .pl {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
  }
  .bar,
  .meter {
    height: 4px;
    border-radius: 2px;
    background: var(--surface-3);
    overflow: hidden;
  }
  .bar div,
  .meter div {
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

  .body {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .tree {
    flex: none;
    width: 190px;
    display: flex;
    flex-direction: column;
    border-right: 1px solid var(--border);
    min-height: 0;
  }
  .th,
  .outbar,
  .edbar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
    height: 36px;
    padding: 0 8px 0 12px;
  }
  .tl {
    flex: 1;
    overflow: auto;
    padding-bottom: 8px;
  }
  .fe {
    display: flex;
    align-items: center;
    gap: 5px;
    width: 100%;
    height: 26px;
    padding-right: 8px;
    border: none;
    background: transparent;
    color: var(--text-2);
    font-size: 12.5px;
    text-align: left;
  }
  .fe:hover {
    background: var(--surface-hover);
  }
  .fe.on {
    background: var(--surface-active, var(--surface-hover));
    color: var(--text);
    font-weight: 550;
  }
  .fe :global(.fi) {
    flex: none;
    color: var(--text-3);
  }
  .none {
    padding: 10px 12px;
    font-size: 12px;
    color: var(--text-3);
  }
  .main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .edbar {
    border-bottom: 1px solid var(--border);
  }
  .fname {
    font-family: var(--mono);
    font-size: 12px;
    color: var(--text-2);
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--accent);
    flex: none;
  }
  .ed {
    flex: 3;
    min-height: 120px;
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
  .outbar {
    border-top: 1px solid var(--border);
  }
  .ex {
    font-size: 11.5px;
    font-family: var(--mono);
    color: var(--success);
  }
  .ex.bad {
    color: var(--danger);
  }
  .out {
    flex: 2;
    min-height: 80px;
    margin: 0;
    padding: 8px 14px 12px;
    overflow: auto;
    background: #14141b;
    color: #e6e6ef;
    font-family: var(--mono);
    font-size: 12px;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .info {
    flex: none;
    width: 240px;
    padding: 12px;
    border-left: 1px solid var(--border);
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 12.5px;
  }
  .gap {
    margin-top: 12px;
  }
  .stat {
    display: flex;
    align-items: center;
    gap: 7px;
    color: var(--text-2);
  }
  .stat :global(svg) {
    color: var(--text-3);
    flex: none;
  }
  .stat .v {
    color: var(--text);
  }
  .stat select {
    width: 90px;
  }
  .sub {
    font-size: 11.5px;
    color: var(--text-3);
    line-height: 1.4;
  }
  .sub.warn {
    color: var(--warning);
  }
  .seg {
    display: flex;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    overflow: hidden;
  }
  .seg button {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 5px;
    height: 28px;
    border: none;
    background: transparent;
    color: var(--text-2);
    font-size: 12px;
  }
  .seg button.on {
    background: var(--accent);
    color: white;
  }
  .ports {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-top: 4px;
    color: var(--text-2);
  }
  .ports span {
    display: flex;
    align-items: center;
    gap: 5px;
  }
  .wide {
    justify-content: center;
  }
</style>
