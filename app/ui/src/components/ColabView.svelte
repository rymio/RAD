<script lang="ts">
  // Rust Colab and Python Colab (Development): notebooks run on this computer, with Gilgamesh
  // beside them. Left: the notebooks and the variables; middle: the cells; right: the chat.
  import { untrack } from 'svelte';
  import { store } from '../lib/state.svelte';
  import { activity } from '../lib/gilgamesh/observe.svelte';
  import { colab, TEMPLATES, type Cell, type Lang } from '../lib/colab.svelte';
  import ColabCell from './ColabCell.svelte';
  import ColabChat from './ColabChat.svelte';
  import Modal from './Modal.svelte';
  import { timeAgo } from '../lib/util';
  import { Play, Square, RotateCcw, Eraser, Plus, Type, Package, Wifi, WifiOff, Download, Upload, FolderOpen, Loader, Trash2, PanelRight, PanelRightClose, NotebookPen, Sigma, BookPlus, Check, FilePlus2, Variable } from '@lucide/svelte';

  let { lang }: { lang: Lang } = $props();

  let chat = $state<ReturnType<typeof ColabChat> | null>(null);
  let showChat = $state(true);
  let packsOpen = $state(false);
  let newOpen = $state(false);
  let chosen = $state<string[]>([]);
  let extra = $state('');
  let confirmDelete = $state<string | null>(null);

  const s = $derived(colab.status);
  const nb = $derived(colab.nb?.lang === lang ? colab.nb : null);
  const side = $derived(s ? (lang === 'python' ? s.python : s.rust) : null);
  const packs = $derived(side?.packs ?? []);
  const name = $derived(lang === 'python' ? 'Python Colab' : 'Rust Colab');
  const list = $derived(colab.lists[lang]);
  const running = $derived(colab.queue.length > 0);

  $effect(() => {
    const l = lang;
    untrack(() => {
      void colab.refresh();
      void colab.restore(l);
    });
  });

  // Gilgamesh sees which notebook is open (the cells themselves go in through colab.describe()).
  $effect(() => {
    activity.seen('colab', nb ? `the ${lang === 'rust' ? 'Rust' : 'Python'} notebook “${nb.title}” (${nb.cells.length} cells)` : null);
  });

  // Saved when leaving.
  $effect(() => () => void colab.flush());

  function ask(cell: Cell, what: 'explain' | 'error' | 'improve') {
    showChat = true;
    const n = colab.index(cell.id) + 1;
    const q = what === 'error' ? `Cell ${n} failed. What does the error mean, and how do I fix it?` : what === 'improve' ? `How could cell ${n} be better: clearer, faster or more idiomatic ${lang === 'rust' ? 'Rust' : 'Python'}? Show the improved cell.` : `Explain cell ${n}: what it does, line by line, and the idea (and maths) behind it.`;
    colab.selected = cell.id;
    void chat?.ask(q);
  }

  function openPacks() {
    chosen = packs.filter((p) => !p.downloaded).map((p) => p.id);
    extra = '';
    packsOpen = true;
  }

  function download() {
    const items = extra.split(/[,\n]/).map((x) => x.trim()).filter(Boolean);
    void colab.download(lang, chosen, items);
  }

  /** Set up Python: its environment and the maths pack, in one go. */
  function setupPython() {
    packsOpen = true;
    chosen = ['maths'];
    void colab.download('python', ['maths']);
  }

  function onKeys(e: KeyboardEvent) {
    const t = e.target as HTMLElement;
    if (!nb || t.closest('textarea, input, select, [contenteditable]')) return;
    const i = colab.index(colab.selected);
    const cells = nb.cells;
    if (e.key === 'ArrowDown' || e.key === 'j') colab.selected = cells[Math.min(i + 1, cells.length - 1)]?.id ?? null;
    else if (e.key === 'ArrowUp' || e.key === 'k') colab.selected = cells[Math.max(i - 1, 0)]?.id ?? null;
    else if (e.key === 'Enter' && e.shiftKey && colab.selected) colab.run(colab.selected);
    else if (e.key === 'Enter' && colab.selected) {
      colab.setEditing(colab.selected, true);
      queueMicrotask(() => (document.querySelector(`[data-cell="${colab.selected}"] textarea`) as HTMLTextAreaElement | null)?.focus());
    } else if (e.key === 'b') colab.addCell('code', '', colab.selected);
    else if (e.key === 'a') colab.addCell('code', '', i > 0 ? cells[i - 1].id : null);
    else if (e.key === 'm' && colab.selected) colab.setType(colab.selected, 'markdown');
    else if (e.key === 'y' && colab.selected) colab.setType(colab.selected, 'code');
    else return;
    e.preventDefault();
    queueMicrotask(() => document.querySelector(`[data-cell="${colab.selected}"]`)?.scrollIntoView({ block: 'nearest' }));
  }
</script>

<svelte:window onkeydown={onKeys} />

<div class="colab">
  <header class="drag">
    <span class="logo">{#if lang === 'rust'}<Sigma size={17} />{:else}<NotebookPen size={17} />{/if}</span>
    {#if nb}
      <input class="title no-drag" value={nb.title} size={Math.max(8, nb.title.length + 1)} onchange={(e) => colab.rename(e.currentTarget.value)} title="Rename the notebook" />
    {:else}
      <h1>{name}</h1>
    {/if}
    <span class="spacer"></span>
    {#if s}
      <select class="input sm net no-drag" title="Offline: only what's already on this computer is used" value={s.network} onchange={(e) => colab.setNetwork(e.currentTarget.value as any)}>
        <option value="auto">{s.offline ? 'Offline (auto)' : 'Online (auto)'}</option>
        <option value="offline">Always offline</option>
        <option value="online">Always online</option>
      </select>
      <span class="netdot no-drag" class:off={s.offline} title={s.offline ? 'Offline: notebooks use what is on this computer' : 'Online'}>{#if s.offline}<WifiOff size={14} />{:else}<Wifi size={14} />{/if}</span>
    {/if}
    <button class="btn sm no-drag" onclick={openPacks}><Package size={13} /> Packages & offline</button>
    <button class="icon-btn no-drag" title={showChat ? 'Hide Gilgamesh' : 'Show Gilgamesh'} onclick={() => (showChat = !showChat)}>{#if showChat}<PanelRightClose size={16} />{:else}<PanelRight size={16} />{/if}</button>
  </header>

  <div class="main">
    <aside class="left">
      <div class="lhead">
        <span class="section-title">Notebooks</span>
        <span class="spacer"></span>
        <button class="icon-btn sm" title="Open .ipynb files (from Google Colab, Jupyter…)" onclick={() => colab.importFiles()}><Upload size={13} /></button>
        <button class="icon-btn sm" title="New notebook" onclick={() => (newOpen = true)}><FilePlus2 size={14} /></button>
      </div>
      <div class="nbs">
        {#each list as n (n.id)}
          <div class="nbrow" class:on={nb?.id === n.id}>
            <button class="nbitem" onclick={() => colab.open(n.id)}>
              <span class="truncate">{n.title}</span>
              <small>{n.cells} cells · {timeAgo(n.updated)}</small>
            </button>
            <button class="icon-btn sm del" title="Delete" onclick={() => (confirmDelete = n.id)}><Trash2 size={12} /></button>
          </div>
        {:else}
          <p class="muted small pad">No notebooks yet.</p>
        {/each}
      </div>
      {#if nb && colab.kernel !== 'off'}
        <div class="lhead"><span class="section-title"><Variable size={12} /> Variables</span></div>
        <div class="vars">
          {#each colab.vars as v (v.name)}
            <div class="var" title={v.preview}>
              <code>{v.name}</code>
              <small class="truncate">{v.type}{v.shape ? ` ${v.shape}` : ''}{v.dtype ? ` ${v.dtype}` : ''}{v.len != null ? ` (${v.len})` : ''}</small>
            </div>
          {:else}
            <p class="muted small pad">None yet.</p>
          {/each}
        </div>
      {/if}
      <div class="lfoot">
        <button class="btn sm ghost block" onclick={() => colab.openFolder()} title="The notebooks' working folder: put data files here"><FolderOpen size={13} /> Data folder</button>
        <button class="btn sm ghost block" disabled={colab.adding} onclick={() => colab.addToRag()} title="PDFs, books and docs you learn from, into the vault: Gilgamesh and every chat can search them"><BookPlus size={13} /> Add to RAG</button>
      </div>
    </aside>

    <section class="center">
      {#if !s}
        <div class="empty"><Loader size={18} class="spin" /></div>
      {:else if !side?.ready}
        <div class="empty">
          <div class="setup card">
            <h2>Set up {name}</h2>
            {#if lang === 'python'}
              {#if s.python.system}
                <p>Colab gets its own Python environment, made from Python {s.python.version} on this computer, with its packages kept in RAD's folder (so they're there offline too).</p>
                <p class="muted small">This also downloads the <b>Maths & data</b> pack ({packs.find((p) => p.id === 'maths')?.size}): NumPy, SciPy, SymPy, pandas, matplotlib. More (PyTorch, transformers…) in <b>Packages & offline</b>.</p>
                <button class="btn primary" disabled={!!colab.downloading || s.offline} onclick={setupPython}>{#if colab.downloading}<Loader size={14} class="spin" /> Setting up…{:else}<Download size={14} /> Set up Python Colab{/if}</button>
                {#if s.offline}<p class="warn small">You're offline: setting up needs the internet once.</p>{/if}
              {:else}
                <p>Python 3 isn't installed on this computer. Install it from <b>python.org</b> (or <code>brew install python</code>), then come back.</p>
                <button class="btn" onclick={() => colab.refresh()}><RotateCcw size={14} /> Check again</button>
              {/if}
            {:else}
              {#if !s.rust.cargo}
                <p>Rust isn't installed on this computer. Install it from <b>rustup.rs</b>, then come back.</p>
              {:else}
                <p>The Rust kernel (evcxr) isn't built yet. In RAD's folder run:</p>
                <pre class="cmd">./scripts/build-colab.sh</pre>
              {/if}
              <button class="btn" onclick={() => colab.refresh()}><RotateCcw size={14} /> Check again</button>
            {/if}
          </div>
        </div>
      {:else if !nb}
        <div class="gallery">
          <h2>Start a {lang === 'rust' ? 'Rust' : 'Python'} notebook</h2>
          <p class="muted">Short lessons that run as they are; ask Gilgamesh about any of them. Or open a notebook from Google Colab or Jupyter (.ipynb, or drop one here).</p>
          <div class="tpls">
            {#each TEMPLATES.filter((t) => t.lang === lang) as t (t.id)}
              {@const need = t.pack ? packs.find((p) => p.id === t.pack) : null}
              <button class="tpl card" onclick={() => colab.create(lang, t)}>
                <strong>{t.title}</strong>
                <span>{t.about}</span>
                {#if need}<small class:missing={!need.downloaded}>{need.downloaded ? `Uses ${need.name}` : `Needs ${need.name} (Packages & offline)`}</small>{/if}
              </button>
            {/each}
          </div>
        </div>
      {:else}
        <div class="toolbar">
          <button class="btn sm" onclick={() => colab.addCell('code')}><Plus size={13} /> Code</button>
          <button class="btn sm" onclick={() => colab.addCell('markdown')}><Type size={13} /> Text</button>
          <span class="sep"></span>
          <button class="btn sm" disabled={running} onclick={() => colab.runAll()}><Play size={13} /> Run all</button>
          <button class="btn sm" disabled={!running} onclick={() => colab.interrupt()}><Square size={12} /> Stop</button>
          <button class="btn sm" onclick={() => colab.restart()} title="A fresh {lang === 'rust' ? 'Rust' : 'Python'}: the variables go"><RotateCcw size={13} /> Restart</button>
          <button class="btn sm ghost" onclick={() => colab.clearOutputs()} title="Clear every output"><Eraser size={13} /></button>
          <span class="spacer"></span>
          <span class="kernel" title={colab.kernelInfo}>
            <span class="dot" class:ok={colab.kernel === 'idle'} class:warn={colab.kernel === 'busy' || colab.kernel === 'starting'} class:bad={colab.kernel === 'dead'}></span>
            {colab.kernel === 'off' ? 'Not started' : colab.kernel === 'starting' ? 'Starting…' : colab.kernel === 'busy' ? 'Running' : colab.kernel === 'dead' ? 'Stopped' : 'Ready'}
          </span>
          <button class="icon-btn sm" title="Save as .ipynb (opens in Google Colab and Jupyter)" onclick={() => colab.exportFile()}><Download size={14} /></button>
        </div>
        <div class="cells">
          {#each nb.cells as c, i (c.id)}
            <ColabCell id={c.id} n={i} onask={ask} />
          {/each}
          <div class="addrow">
            <button class="btn sm ghost" onclick={() => colab.addCell('code', '', nb.cells.at(-1)?.id ?? null)}><Plus size={13} /> Code</button>
            <button class="btn sm ghost" onclick={() => colab.addCell('markdown', '', nb.cells.at(-1)?.id ?? null)}><Type size={13} /> Text</button>
          </div>
          <p class="keys muted small">⇧↩ run · ⌘↩ run in place · ⌥↩ run, new cell below · Esc, then ↑↓ to move, A/B add above/below, M text, Y code</p>
        </div>
      {/if}
    </section>

    {#if showChat && nb}
      <aside class="right">
        <ColabChat bind:this={chat} {nb} />
      </aside>
    {/if}
  </div>
</div>

<Modal bind:open={newOpen} title="New {lang === 'rust' ? 'Rust' : 'Python'} notebook" width={620}>
  <div class="tpls in-modal">
    {#each TEMPLATES.filter((t) => t.lang === lang) as t (t.id)}
      {@const need = t.pack ? packs.find((p) => p.id === t.pack) : null}
      <button class="tpl card" onclick={() => { newOpen = false; void colab.create(lang, t); }}>
        <strong>{t.title}</strong>
        <span>{t.about}</span>
        {#if need}<small class:missing={!need.downloaded}>{need.downloaded ? `Uses ${need.name}` : `Needs ${need.name}`}</small>{/if}
      </button>
    {/each}
  </div>
</Modal>

<Modal bind:open={packsOpen} title="Packages & offline" width={640}>
  {#if s && side}
    <p class="muted small">
      Download what you'll need while you're online; then {name} works anywhere, in transit too.
      {#if lang === 'python'}Python packages are kept in Colab's local packages ({s.python.wheels_mb} MB so far) and installed from there; <code>%pip install</code> keeps what it downloads too.{:else}Crates go into Colab's own store ({s.rust.crates_mb} MB so far); offline, notebooks build from it. <code>:dep</code> downloads the crates it needs when online.{/if}
    </p>
    <div class="packs">
      {#each packs as p (p.id)}
        <label class="pack" class:done={!!p.downloaded}>
          <input type="checkbox" checked={chosen.includes(p.id)} disabled={!!colab.downloading} onchange={(e) => (chosen = e.currentTarget.checked ? [...chosen, p.id] : chosen.filter((x) => x !== p.id))} />
          <div>
            <strong>{p.name}</strong> <small class="muted">{p.size}</small>
            <div class="small muted">{p.about}</div>
            {#if p.downloaded}<div class="small ok"><Check size={11} /> Downloaded {timeAgo(p.downloaded)}{chosen.includes(p.id) ? ' · will update' : ''}</div>{/if}
          </div>
        </label>
      {/each}
    </div>
    <label class="field">
      <span class="label">Other {lang === 'python' ? 'packages' : 'crates'} (comma-separated)</span>
      <input class="input mono" bind:value={extra} placeholder={lang === 'python' ? 'e.g. polars, networkx, jax' : 'e.g. polars, petgraph = "0.8"'} disabled={!!colab.downloading} />
    </label>
    {#if colab.log.length}
      <pre class="log">{colab.log.slice(-60).join('\n')}</pre>
    {/if}
    {#if s.offline}<p class="warn small">You're offline now: downloading needs the internet.</p>{/if}
  {/if}
  {#snippet footer()}
    {#if colab.downloading}
      <button class="btn" onclick={() => colab.stopDownload()}><Square size={12} /> Stop</button>
      <button class="btn primary" disabled><Loader size={14} class="spin" /> Downloading…</button>
    {:else}
      <button class="btn" onclick={() => (packsOpen = false)}>Close</button>
      <button class="btn primary" disabled={(!chosen.length && !extra.trim()) || !!s?.offline} onclick={download}><Download size={14} /> Download</button>
    {/if}
  {/snippet}
</Modal>

<Modal open={!!confirmDelete} title="Delete the notebook?" width={420} onclose={() => (confirmDelete = null)}>
  <p>“{list.find((n) => n.id === confirmDelete)?.title}” goes for good (save it as .ipynb first to keep a copy).</p>
  {#snippet footer()}
    <button class="btn" onclick={() => (confirmDelete = null)}>Cancel</button>
    <button class="btn danger solid" onclick={() => { const id = confirmDelete!; confirmDelete = null; void colab.remove(id).catch((e) => store.error(e, 'Couldn’t delete it')); }}><Trash2 size={13} /> Delete</button>
  {/snippet}
</Modal>

<style>
  .colab {
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
  .logo {
    color: var(--accent);
    display: grid;
    place-items: center;
  }
  h1 {
    margin: 0;
    font-size: 15px;
    font-weight: 650;
  }
  .title {
    font-size: 15px;
    font-weight: 650;
    border: 1px solid transparent;
    background: transparent;
    color: var(--text);
    border-radius: var(--r-xs);
    padding: 3px 6px;
    min-width: 120px;
    max-width: 420px;
    flex: 0 1 auto;
  }
  .title:hover,
  .title:focus {
    border-color: var(--border-strong);
    background: var(--surface);
    outline: none;
  }
  .net {
    width: auto;
  }
  .netdot {
    color: var(--success);
    display: grid;
    place-items: center;
  }
  .netdot.off {
    color: var(--warning);
  }
  .main {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .left {
    width: 230px;
    flex: none;
    display: flex;
    flex-direction: column;
    border-right: 1px solid var(--border);
    min-height: 0;
  }
  .lhead {
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 10px 8px 4px 12px;
  }
  .lhead .section-title {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .nbs {
    flex: 1 1 50%;
    min-height: 60px;
    overflow: auto;
    padding: 0 6px;
  }
  .nbrow {
    display: flex;
    align-items: center;
    border-radius: var(--r-sm);
  }
  .nbrow:hover {
    background: var(--surface-hover);
  }
  .nbrow.on {
    background: var(--surface-active);
  }
  .nbitem {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 1px;
    padding: 6px 8px;
    border: 0;
    background: transparent;
    color: var(--text);
    text-align: left;
    cursor: pointer;
    font-size: 13px;
  }
  .nbitem span {
    max-width: 100%;
  }
  .nbitem small {
    color: var(--text-3);
    font-size: 11px;
  }
  .del {
    opacity: 0;
  }
  .nbrow:hover .del {
    opacity: 1;
  }
  .vars {
    flex: 1 1 40%;
    min-height: 40px;
    overflow: auto;
    padding: 0 12px;
  }
  .var {
    display: flex;
    align-items: baseline;
    gap: 6px;
    padding: 2px 0;
    font-size: 12px;
  }
  .var code {
    font: 12px var(--mono);
    color: var(--text);
  }
  .var small {
    color: var(--text-3);
    min-width: 0;
  }
  .lfoot {
    padding: 8px;
    border-top: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .pad {
    padding: 0 8px;
  }
  .center {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg);
  }
  .toolbar {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 14px;
    border-bottom: 1px solid var(--border);
    background: var(--surface);
    flex-wrap: wrap;
  }
  .sep {
    width: 1px;
    height: 18px;
    background: var(--border-strong);
  }
  .kernel {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-2);
  }
  .cells {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 18px 18px 40px 8px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .addrow {
    display: flex;
    gap: 6px;
    padding-left: 56px;
  }
  .keys {
    padding-left: 60px;
    margin: 4px 0 0;
  }
  .right {
    width: var(--panel-w);
    flex: none;
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-left: 1px solid var(--border);
  }
  .empty {
    flex: 1;
    display: grid;
    place-items: center;
    padding: 24px;
  }
  .setup {
    max-width: 480px;
    padding: 22px 24px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    align-items: flex-start;
  }
  .setup h2,
  .gallery h2 {
    margin: 0;
    font-size: 18px;
  }
  .setup p {
    margin: 0;
    line-height: 1.5;
  }
  .cmd {
    margin: 0;
    padding: 8px 12px;
    background: var(--code-bg);
    border-radius: var(--r-sm);
    font: 12.5px var(--mono);
    user-select: text;
    -webkit-user-select: text;
  }
  .warn {
    color: var(--warning);
  }
  .ok {
    color: var(--success);
    display: inline-flex;
    align-items: center;
    gap: 3px;
  }
  .gallery {
    flex: 1;
    overflow: auto;
    padding: 28px 32px;
  }
  .gallery > p {
    margin: 6px 0 18px;
  }
  .tpls {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: 10px;
  }
  .tpl {
    display: flex;
    flex-direction: column;
    gap: 4px;
    align-items: flex-start;
    text-align: left;
    padding: 14px;
    cursor: pointer;
    border: 1px solid var(--border);
    color: var(--text);
    font: inherit;
  }
  .tpl:hover {
    border-color: var(--accent-ring);
  }
  .tpl span {
    font-size: 12.5px;
    color: var(--text-2);
    line-height: 1.4;
  }
  .tpl small {
    font-size: 11px;
    color: var(--success);
  }
  .tpl small.missing {
    color: var(--warning);
  }
  .packs {
    display: flex;
    flex-direction: column;
    gap: 6px;
    margin: 12px 0;
  }
  .pack {
    display: flex;
    gap: 10px;
    align-items: flex-start;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    cursor: pointer;
  }
  .pack.done {
    background: var(--success-soft);
  }
  .pack input {
    margin-top: 3px;
  }
  .log {
    margin: 10px 0 0;
    max-height: 180px;
    overflow: auto;
    padding: 8px 10px;
    background: var(--code-bg);
    border-radius: var(--r-sm);
    font: 11px/1.45 var(--mono);
    white-space: pre-wrap;
    word-break: break-all;
    user-select: text;
    -webkit-user-select: text;
  }
</style>
