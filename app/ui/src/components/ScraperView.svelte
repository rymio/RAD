<script lang="ts">
  // Browsers → Scraper: save a whole website for offline reading, read the copy here, and add it
  // to the vault (RAG). See lib/scraper.svelte.ts and app/src-tauri/src/scraper.rs.
  import { onMount } from 'svelte';
  import { ask as confirmDialog } from '@tauri-apps/plugin-dialog';
  import { store } from '../lib/state.svelte';
  import { scraper, fmtBytes, DEFAULT_OPTIONS, type Site } from '../lib/scraper.svelte';
  import { timeAgo } from '../lib/util';
  import Menu from './Menu.svelte';
  import { CloudDownload, ArrowLeft, Loader, Square, RotateCw, Trash2, DatabaseZap, Check, MoreHorizontal, FolderOpen, Globe, Settings2, FileText, ExternalLink, CircleAlert, Search } from '@lucide/svelte';

  let url = $state('');
  let showOptions = $state(false);
  let ragAfter = $state(false);
  let starting = $state(false);
  let filter = $state('');

  const site = $derived(scraper.site);
  const page = $derived(scraper.page);
  const o = $derived(scraper.options);
  const shown = $derived(
    scraper.pages.filter((p) => {
      const f = filter.trim().toLowerCase();
      return !f || p.title.toLowerCase().includes(f) || p.url.toLowerCase().includes(f);
    }),
  );

  onMount(() => {
    scraper.load().catch((e) => store.error(e, 'Couldn’t load your saved sites'));
  });

  // Gilgamesh reads the page in front of the owner.
  $effect(() => {
    void scraper.pageFile;
    void scraper.loadPageText();
  });

  async function start(e?: Event) {
    e?.preventDefault();
    if (!url.trim()) return;
    starting = true;
    try {
      await scraper.start(url, scraper.options, undefined, ragAfter);
      url = '';
    } catch (err) {
      store.error(err, 'Couldn’t start');
    } finally {
      starting = false;
    }
  }

  async function remove(s: Site) {
    const vault = !!s.rag && (await confirmDialog(`Also take its ${s.rag.docs.length} pages out of your vault?`, { title: 'Delete saved site', kind: 'warning', okLabel: 'Yes, from the vault too', cancelLabel: 'Keep them in the vault' }));
    const ok = await confirmDialog(`Delete the offline copy of “${s.title}” (${fmtBytes(s.bytes)})?`, { title: 'Delete saved site', kind: 'warning' });
    if (ok) await scraper.remove(s.id, vault).catch((e) => store.error(e, 'Couldn’t delete it'));
  }

  async function reveal(s: Site) {
    const { revealItemInDir } = await import('@tauri-apps/plugin-opener');
    await revealItemInDir(await scraper.folder(s.id)).catch((e) => store.error(e, 'Couldn’t open the folder'));
  }

  const pct = (s: Site) => {
    const total = s.pages + s.queued;
    return s.status === 'running' ? Math.min(100, Math.round((s.pages / Math.max(1, Math.min(total, s.options.max_pages))) * 100)) : 100;
  };
  const statusText = (s: Site) =>
    s.status === 'running' ? (s.current ?? 'Starting…') : s.status === 'failed' ? (s.message ?? 'Failed') : s.status === 'stopped' ? `Stopped${s.message ? `: ${s.message}` : ''}` : `Saved ${timeAgo(s.updated)}${s.message ? ` · ${s.message}` : ''}`;
</script>

<div class="scr">
  <header class="drag">
    {#if site}
      <button class="icon-btn no-drag" title="Your saved sites" onclick={() => scraper.close()}><ArrowLeft size={16} /></button>
      <div class="ttl">
        <h1 class="truncate">{site.title}</h1>
        <span class="muted small truncate">{page?.url ?? site.url}</span>
      </div>
      <span class="spacer"></span>
      <span class="badge no-drag" title="Nothing here comes from the internet"><span class="dot"></span> Offline copy</span>
      {#if page}<button class="btn sm no-drag" title="Open the live page in RAD’s web browser" onclick={() => store.runAction({ kind: 'open_url', url: page.url })}><ExternalLink size={13} /> Live page</button>{/if}
      {#if site.rag && !site.rag_stale}
        <button class="btn sm no-drag" disabled title="In the vault: every chat and Gilgamesh can use it"><Check size={13} /> In RAG</button>
      {:else}
        <button class="btn sm primary no-drag" disabled={!!scraper.ragBusy || site.status === 'running'} onclick={() => scraper.addToRag(site.id)}>{#if scraper.ragBusy === site.id}<Loader size={13} class="spin" /> {scraper.ragProgress}{:else}<DatabaseZap size={13} /> {site.rag ? 'Update RAG' : 'Add to RAG'}{/if}</button>
      {/if}
    {:else}
      <CloudDownload size={17} />
      <h1>Scraper</h1>
      <span class="muted small">save whole websites to read offline, and add them to RAG</span>
      <span class="spacer"></span>
    {/if}
  </header>

  {#if site}
    <div class="viewer">
      <aside class="pages">
        <div class="find"><Search size={13} /><input class="input sm" placeholder="Find a page…" bind:value={filter} /></div>
        <div class="plist">
          {#each shown as p (p.file)}
            <button class="pg" class:on={p.file === scraper.pageFile} title={p.url} onclick={() => scraper.show(p.file)}>
              {#if p.kind === 'document'}<FileText size={12} />{/if}
              <span class="truncate">{p.title}</span>
              <small class="muted truncate">{p.url.replace(/^https?:\/\/[^/]+/, '') || '/'}</small>
            </button>
          {:else}
            <p class="muted small pad">{scraper.pages.length ? 'No page matches.' : site.status === 'running' ? 'Pages appear when the copy is finished.' : 'No pages were saved.'}</p>
          {/each}
        </div>
        <p class="muted small pad">{site.pages} pages · {site.documents} documents · {site.assets} files · {fmtBytes(site.bytes)}</p>
      </aside>
      <div class="frame">
        {#if scraper.frameFile}
          <!-- The copy is static and private: no scripts, forms or pop-ups, nothing loaded from the internet. -->
          <iframe title={page?.title ?? 'Saved page'} src={scraper.fileUrl(site.id, scraper.frameFile)} sandbox=""></iframe>
        {:else}
          <div class="center muted">Choose a page.</div>
        {/if}
      </div>
    </div>
  {:else}
    <div class="list">
      <form class="new" onsubmit={start}>
        <div class="row">
          <Globe size={16} class="muted" />
          <input class="input url" placeholder="https://example.com — the site (or the section) to save" bind:value={url} />
          <button class="icon-btn" type="button" class:on={showOptions} title="Options" onclick={() => (showOptions = !showOptions)}><Settings2 size={15} /></button>
          <button class="btn primary" disabled={!url.trim() || starting}>{#if starting}<Loader size={14} class="spin" />{:else}<CloudDownload size={14} />{/if} Save site</button>
        </div>
        <label class="check"><input type="checkbox" bind:checked={ragAfter} /> Add it to RAG when it’s saved, so Gilgamesh and every chat can use it</label>
        {#if showOptions}
          <div class="opts">
            <label>What to save
              <select class="input sm" value={o.scope} onchange={(e) => scraper.saveOptions({ scope: e.currentTarget.value as any })}>
                <option value="site">The whole site</option>
                <option value="section">This section only (pages under the address’s folder)</option>
                <option value="subdomains">The site and its subdomains</option>
              </select>
            </label>
            <label>At most
              <select class="input sm" value={String(o.max_pages)} onchange={(e) => scraper.saveOptions({ max_pages: Number(e.currentTarget.value) })}>
                {#each [25, 100, 250, 500, 1000, 2500, 5000, 10000] as n (n)}<option value={String(n)}>{n.toLocaleString()} pages</option>{/each}
              </select>
            </label>
            <label>Links deep
              <select class="input sm" value={String(o.max_depth)} onchange={(e) => scraper.saveOptions({ max_depth: Number(e.currentTarget.value) })}>
                {#each [1, 2, 3, 5, 8, 12, 20, 50] as n (n)}<option value={String(n)}>{n}</option>{/each}
              </select>
            </label>
            <label>Pace
              <select class="input sm" value={String(o.delay_ms)} onchange={(e) => scraper.saveOptions({ delay_ms: Number(e.currentTarget.value) })}>
                <option value="1000">Gentle (1 page a second)</option>
                <option value="250">Normal</option>
                <option value="50">Fast (your own sites)</option>
              </select>
            </label>
            <label class="check"><input type="checkbox" checked={o.assets} onchange={(e) => scraper.saveOptions({ assets: e.currentTarget.checked })} /> Images, styles and fonts (pages look as they did)</label>
            <label class="check"><input type="checkbox" checked={o.documents} onchange={(e) => scraper.saveOptions({ documents: e.currentTarget.checked })} /> Linked documents (PDF, Word, Excel, PowerPoint, EPUB)</label>
            <label class="check"><input type="checkbox" checked={o.robots} onchange={(e) => scraper.saveOptions({ robots: e.currentTarget.checked })} /> Respect robots.txt and “nofollow”</label>
            <button class="btn sm ghost" type="button" onclick={() => scraper.saveOptions(DEFAULT_OPTIONS)}>Defaults</button>
          </div>
        {/if}
        <p class="muted small">Pages are saved on this computer as they are served (sites that only draw themselves with JavaScript save little). Save only sites you’re allowed to copy.</p>
      </form>

      {#if !scraper.loaded}
        <div class="center"><Loader size={20} class="spin" /></div>
      {:else if scraper.list.length}
        <div class="sites">
          {#each scraper.list as s (s.id)}
            <div class="site">
              <button class="main" disabled={!s.pages && s.status !== 'running'} onclick={() => scraper.open(s.id)}>
                <span class="name truncate">{s.title}</span>
                <span class="muted small truncate">{s.url}</span>
                <span class="stats small">
                  {s.pages} pages{s.documents ? ` · ${s.documents} documents` : ''} · {s.assets} files · {fmtBytes(s.bytes)}{s.errors ? ` · ${s.errors} errors` : ''}{s.blocked ? ` · ${s.blocked} not allowed by robots.txt` : ''}
                </span>
                {#if s.status === 'running'}<span class="bar"><i style="width:{pct(s)}%"></i></span>{/if}
                <span class="st small truncate" class:bad={s.status === 'failed'}>{#if s.status === 'running'}<Loader size={11} class="spin" />{:else if s.status === 'failed'}<CircleAlert size={11} />{/if} {statusText(s)}</span>
              </button>
              <div class="acts">
                {#if s.status === 'running'}
                  <button class="btn sm" onclick={() => scraper.stop(s.id)}><Square size={12} /> Stop</button>
                {:else if s.rag && !s.rag_stale}
                  <span class="inrag" title="Every chat and Gilgamesh can use it"><Check size={12} /> In RAG</span>
                {:else if s.pages}
                  <button class="btn sm primary" disabled={!!scraper.ragBusy} onclick={() => scraper.addToRag(s.id)}>{#if scraper.ragBusy === s.id}<Loader size={12} class="spin" /> {scraper.ragProgress}{:else}<DatabaseZap size={12} /> {s.rag ? 'Update RAG' : 'Add to RAG'}{/if}</button>
                {/if}
                <Menu
                  items={[
                    { label: 'Read offline', icon: Globe, disabled: !s.pages, onclick: () => scraper.open(s.id) },
                    { label: 'Save again (update)', icon: RotateCw, disabled: s.status === 'running', onclick: () => scraper.start(s.url, s.options, s.id, !!s.rag) },
                    { label: 'Show in Finder', icon: FolderOpen, onclick: () => reveal(s) },
                    ...(s.rag ? [{ label: 'Take out of RAG', icon: DatabaseZap, onclick: () => scraper.removeFromRag(s.id) }] : []),
                    null,
                    { label: 'Delete', icon: Trash2, danger: true, onclick: () => remove(s) },
                  ]}
                >
                  {#snippet trigger(toggle: () => void)}<button class="icon-btn sm" title="More" onclick={toggle}><MoreHorizontal size={14} /></button>{/snippet}
                </Menu>
              </div>
            </div>
          {/each}
        </div>
      {:else}
        <div class="empty">
          <div class="glyph"><CloudDownload size={28} /></div>
          <h2>Websites, kept</h2>
          <p>Give the Scraper an address and it saves the site: every page with its pictures and styles, and the PDFs and documents it links to. Read it here without the internet, and add it to RAG so Gilgamesh and every chat know what’s in it and can cite it.</p>
          <p class="muted small">You can also ask Gilgamesh: “save docs.example.com and add it to RAG”.</p>
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .scr {
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
  .ttl {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-width: 0;
    flex: 0 1 auto;
  }
  .small {
    font-size: 12px;
  }
  .spacer {
    flex: 1;
  }
  .badge {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-2);
    padding: 3px 9px;
    border-radius: 999px;
    background: var(--surface-2);
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: #16a34a;
  }
  .on {
    color: var(--accent);
  }
  .list {
    flex: 1;
    overflow: auto;
    padding: 22px 24px;
    max-width: 820px;
    width: 100%;
    margin: 0 auto;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }
  .new {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding: 14px;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--surface);
  }
  .new p {
    margin: 0;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .url {
    flex: 1;
  }
  .check {
    display: flex;
    align-items: center;
    gap: 7px;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .opts {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px 16px;
    padding-top: 6px;
    border-top: 1px solid var(--border);
  }
  .opts label:not(.check) {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--text-3);
  }
  .opts .btn {
    justify-self: start;
  }
  .sites {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--r);
    overflow: hidden;
  }
  .site {
    display: flex;
    align-items: center;
    gap: 8px;
    padding-right: 10px;
  }
  .site + .site {
    border-top: 1px solid var(--border);
  }
  .main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 3px;
    padding: 11px 14px;
    border: 0;
    background: none;
    color: var(--text);
    text-align: left;
    cursor: pointer;
  }
  .main:hover:not(:disabled) {
    background: var(--surface-2);
  }
  .main:disabled {
    cursor: default;
  }
  .name {
    font-size: 14px;
    font-weight: 600;
    max-width: 100%;
  }
  .stats {
    color: var(--text-2);
  }
  .st {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--text-3);
    max-width: 100%;
  }
  .st.bad {
    color: #dc2626;
  }
  .bar {
    width: 100%;
    height: 4px;
    border-radius: 2px;
    background: var(--surface-2);
    overflow: hidden;
  }
  .bar i {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 0.3s;
  }
  .acts {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
  }
  .inrag {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 12px;
    color: #16a34a;
  }
  .center {
    flex: 1;
    display: grid;
    place-items: center;
  }
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: 10px;
    padding: 30px 20px;
  }
  .empty h2 {
    margin: 4px 0 0;
    font-size: 19px;
  }
  .empty p {
    margin: 0;
    max-width: 520px;
    line-height: 1.55;
    color: var(--text-2);
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
  .viewer {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .pages {
    width: 270px;
    flex: none;
    display: flex;
    flex-direction: column;
    border-right: 1px solid var(--border);
    min-height: 0;
  }
  .find {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px;
    color: var(--text-3);
  }
  .find input {
    flex: 1;
  }
  .plist {
    flex: 1;
    overflow: auto;
    padding: 0 6px;
  }
  .pg {
    width: 100%;
    display: grid;
    grid-template-columns: auto 1fr;
    column-gap: 5px;
    align-items: center;
    padding: 6px 8px;
    border: 0;
    border-radius: var(--r-sm);
    background: none;
    color: var(--text);
    text-align: left;
    cursor: pointer;
    font-size: 12.5px;
  }
  .pg > span {
    grid-column: 2;
  }
  .pg > small {
    grid-column: 1 / -1;
    font-size: 11px;
  }
  .pg:hover {
    background: var(--surface-2);
  }
  .pg.on {
    background: var(--accent-soft);
    color: var(--accent);
  }
  .pad {
    margin: 0;
    padding: 8px 12px;
  }
  .frame {
    flex: 1;
    min-width: 0;
    display: flex;
    background: #fff;
  }
  iframe {
    flex: 1;
    border: 0;
    width: 100%;
    height: 100%;
    background: #fff;
  }
</style>
