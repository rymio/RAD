<script lang="ts">
  // Products: the owner's products (SaaS + iOS + Android) and the road to production: the
  // description, store listings drafted to the stores' limits, pictures checked against the
  // stores' sizes, a website on gnu.au, and bots that fill in App Store Connect and Google Play
  // Console. Gilgamesh is beside it (ProductChat). See lib/products.svelte.ts.
  import { onMount } from 'svelte';
  import { open as pickFiles, ask as confirmDialog } from '@tauri-apps/plugin-dialog';
  import { invoke } from '@tauri-apps/api/core';
  import { rad } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import { bots } from '../lib/bots.svelte';
  import { products, gnu, LIMITS, ASSET_KINDS, GUIDE_SOURCES, STORE_LOGIN_DOMAIN, bytes, type Product, type Tab, type AssetKind, type Platform } from '../lib/products.svelte';
  import ProductChat from './ProductChat.svelte';
  import Modal from './Modal.svelte';
  import { Package, Plus, PanelRight, PanelRightClose, Settings2, Trash2, Wand2, Globe, Rocket, BrainCircuit, Check, X, CircleAlert, Hand, Loader, ImagePlus, ExternalLink, Eye, Upload, FolderOpen, BookOpen, Bot as BotIcon } from '@lucide/svelte';

  const CHAT_KEY = 'rad.products.chat';
  let showChat = $state(true);
  try {
    showChat = localStorage.getItem(CHAT_KEY) !== '0';
  } catch {}
  function toggleChat() {
    showChat = !showChat;
    try {
      localStorage.setItem(CHAT_KEY, showChat ? '1' : '0');
    } catch {}
  }

  const P = products;
  const p = $derived(P.current);
  const busy = $derived(p ? P.busy[p.id] : undefined);
  const checks = $derived(p ? P.checks(p) : null);

  const TABS: { id: Tab; label: string; show?: (p: Product) => boolean }[] = [
    { id: 'overview', label: 'Overview' },
    { id: 'ios', label: 'App Store', show: (p) => p.platforms.ios },
    { id: 'android', label: 'Google Play', show: (p) => p.platforms.android },
    { id: 'assets', label: 'Pictures' },
    { id: 'website', label: 'Website' },
    { id: 'launch', label: 'Launch' },
  ];
  const PLATFORM_LABEL: Record<Platform, string> = { saas: 'SaaS', ios: 'iOS', android: 'Android' };

  onMount(() => {
    void P.load().then(() => {
      if (!P.openId && P.list.length) P.openId = P.list[0].id;
    });
    void bots.load();
  });

  const touch = () => p && P.save(p, true);

  async function run<R>(f: () => Promise<R>, ok?: (r: R) => string | null) {
    try {
      const r = await f();
      const msg = ok?.(r);
      if (msg) store.toast('success', p?.name ?? 'Products', msg);
      return r;
    } catch (e) {
      store.error(e, 'Products');
    }
  }

  async function newProduct() {
    const name = window.prompt('Name of the product');
    if (name?.trim()) await P.create(name);
  }

  async function removeProduct() {
    if (!p) return;
    if (!(await confirmDialog(`Delete “${p.name}” and its pictures from this computer? Its copy in the vault and its memory go too. The gnu.au site and the store listings are left as they are.`, { title: 'Delete product', kind: 'warning' }))) return;
    await run(() => P.remove(p.id));
  }

  async function draft(which: 'ios' | 'android' | 'both') {
    if (!p) return;
    const filled = (which !== 'android' && p.ios.description) || (which !== 'ios' && p.android.full);
    if (filled && !(await confirmDialog('Replace the listing text with a new draft from the description?', { title: 'Draft listings' }))) return;
    await run(() => P.draftListings(p, which), (left) => (left.length ? null : 'Listings drafted within the stores’ limits'));
  }

  async function addPictures(kind: AssetKind) {
    if (!p) return;
    const picked = await pickFiles({ multiple: true, filters: [{ name: 'Pictures', extensions: ['png', 'jpg', 'jpeg'] }] });
    const paths = Array.isArray(picked) ? picked : picked ? [picked] : [];
    if (paths.length) await run(() => P.addAssets(p, paths as string[], kind));
  }

  // ---------------------------------------------------------------- website
  let subdomain = $state('');
  let preview = $state<string | null>(null);
  $effect(() => {
    if (p && !subdomain) subdomain = p.gnu?.subdomain ?? p.name.toLowerCase().replace(/[^a-z0-9-]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 32);
  });
  async function build() {
    if (!p) return;
    const failed = await run(() => P.buildSite(p, subdomain));
    if (failed && !failed.length) store.toast('success', p.name, 'The website is built (not published yet): preview it, then publish.');
  }
  async function publish() {
    if (!p?.gnu) return;
    if (!(await confirmDialog(`Publish ${p.gnu.url}? It goes live for everyone.`, { title: 'Publish the website' }))) return;
    await run(() => P.publishSite(p), (url) => `Live at ${url}`);
  }
  async function showPreview(slug = 'index') {
    if (!p) return;
    const doc = await run(() => P.previewSite(p, slug));
    if (doc) preview = doc;
  }

  // ---------------------------------------------------------------- launch
  let launchBot = $state('');
  let launchExtra = $state('');
  $effect(() => {
    if (!launchBot && bots.bots.length) launchBot = bots.bots[0].id;
  });
  async function launch(platform: 'ios' | 'android') {
    if (!p || !launchBot) return;
    const failing = checks![platform].filter((c) => c.level === 'must' && !c.ok);
    if (failing.length && !(await confirmDialog(`${failing.length} thing${failing.length === 1 ? ' is' : 's are'} still missing (${failing.map((c) => c.label).join(', ')}). Start the bot anyway? It will fill in what it can and report the rest.`, { title: 'Not quite ready' }))) return;
    await run(() => P.launch(p, platform, launchBot, launchExtra), (b) => `${b.name} is on it: watch it in Bots. It stops before submitting for review.`);
  }

  // ---------------------------------------------------------------- settings
  let settingsOpen = $state(false);
  let gnuBase = $state('');
  let gnuToken = $state('');
  let fetching = $state(false);
  async function openSettings() {
    const s = await gnu.status().catch(() => null);
    gnuBase = s?.base ?? 'https://gnu.au/api/v1';
    gnuToken = '';
    settingsOpen = true;
  }
  async function saveGnu(forget = false) {
    const s = await run(() => gnu.configure(gnuBase, forget ? '' : gnuToken.trim() || null));
    if (!s) return;
    P.gnuInfo = s;
    gnuToken = '';
    if (s.has_token) {
      const me = await gnu.raw('GET', '/me/').catch((e) => ({ status: 0, body: String(e) }));
      if (me.status === 200) store.toast('success', 'gnu.au connected', `Signed in as ${me.body.username} (${me.body.plan ?? 'no plan'}; token ${me.body.token?.scope ?? 'session'}).`);
      else store.toast('error', 'gnu.au didn’t accept the token', typeof me.body === 'string' ? me.body : (me.body?.detail ?? `HTTP ${me.status}`), 9000);
    } else store.toast('info', 'gnu.au', 'Token forgotten');
  }
  async function fetchGuides() {
    fetching = true;
    const r = await run(() => P.fetchGuides());
    fetching = false;
    if (r) store.toast(r.failed.length ? 'info' : 'success', 'Store guides', `${r.ok} fetched for offline reading${r.failed.length ? `; ${r.failed.length} couldn’t be: ${r.failed.join(' · ')}` : ''}`, 9000);
  }

  const count = (s: string) => [...s.trim()].length;
  const when = (ms: number) => new Date(ms).toLocaleString('en-GB', { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' });
  const openUrl = (url: string) => store.runAction({ kind: 'open_url', url });
</script>

{#snippet counter(v: string, max: number, inBytes = false)}
  {@const n = inBytes ? bytes(v.trim()) : count(v)}
  <span class="count" class:over={n > max}>{n}/{max}{inBytes ? ' bytes' : ''}</span>
{/snippet}

{#snippet checkRow(c: { ok: boolean; level: string; label: string; detail?: string; tab: Tab })}
  <button class="check" class:ok={c.ok} class:manual={c.level === 'manual'} onclick={() => (P.tab = c.tab)}>
    {#if c.ok}<Check size={13} />{:else if c.level === 'manual'}<Hand size={13} />{:else if c.level === 'should'}<CircleAlert size={13} />{:else}<X size={13} />{/if}
    <span class="cl">{c.label}</span>
    {#if c.detail}<small class="truncate">{c.detail}</small>{/if}
  </button>
{/snippet}

<div class="products-view">
  <header class="drag">
    <span class="logo"><Package size={17} /></span>
    <h1>Products</h1>
    <span class="sub">From description to App Store, Google Play and the web</span>
    <span class="spacer"></span>
    <button class="btn sm ghost no-drag" onclick={openSettings} title="gnu.au and the store guides"><Settings2 size={14} /> gnu.au &amp; guides</button>
    <button class="icon-btn no-drag" title={showChat ? 'Hide Gilgamesh' : 'Show Gilgamesh'} onclick={toggleChat}>{#if showChat}<PanelRightClose size={16} />{:else}<PanelRight size={16} />{/if}</button>
  </header>
  <div class="main">
    <nav class="list">
      <button class="btn sm primary block" onclick={newProduct}><Plus size={14} /> New product</button>
      {#each P.list as x (x.id)}
        <button class="item" class:cur={x.id === P.openId} onclick={() => (P.openId = x.id)}>
          <strong class="truncate">{x.name}</strong>
          <span class="pills">
            {#each P.readiness(x) as r (r.platform)}
              <span class="pill" class:done={r.done === r.total} title="{PLATFORM_LABEL[r.platform]}: {r.done} of {r.total} must-haves">{PLATFORM_LABEL[r.platform]} {r.done}/{r.total}</span>
            {/each}
          </span>
          {#if P.busy[x.id]}<small class="truncate"><Loader size={10} class="spin" /> {P.busy[x.id]}</small>{/if}
        </button>
      {:else}
        {#if P.loaded}<p class="empty">No products yet. Make one, write what it is in your own words, and RAD drafts the rest.</p>{/if}
      {/each}
    </nav>

    <section class="work">
      {#if !p}
        <div class="blank">
          <Package size={36} />
          <p>Pick a product, or make a new one.</p>
        </div>
      {:else}
        <div class="phead">
          <div class="titles">
            <input class="name" bind:value={p.name} oninput={touch} aria-label="Product name" />
            <input class="tagline" bind:value={p.tagline} oninput={touch} placeholder="One line: what it does, for whom" aria-label="Tagline" />
          </div>
          <button class="btn sm" onclick={() => run(() => P.addToRag(p), () => 'In your vault (Products) and Gilgamesh’s memories')} disabled={!!busy} title="Put the product in the vault and Gilgamesh’s memories (run again after changes)">
            <BrainCircuit size={14} /> {p.rag ? 'Update in RAG' : 'Add to RAG'}
          </button>
          <button class="icon-btn" title="Open its folder" onclick={() => run(async () => rad.fsReveal(await invoke<string>('products_folder', { id: p.id })))}><FolderOpen size={15} /></button>
          <button class="icon-btn" title="Delete the product" onclick={removeProduct}><Trash2 size={15} /></button>
        </div>
        <div class="tabs">
          {#each TABS.filter((t) => !t.show || t.show(p)) as t (t.id)}
            <button class="tab" class:cur={P.tab === t.id} onclick={() => (P.tab = t.id)}>{t.label}</button>
          {/each}
        </div>
        {#if busy}<div class="busy"><Loader size={13} class="spin" /> {busy}</div>{/if}

        <div class="pane">
          {#if P.tab === 'overview'}
            <div class="row">
              <span class="label">Platforms</span>
              {#each ['saas', 'ios', 'android'] as const as k (k)}
                <label class="tog"><input type="checkbox" bind:checked={p.platforms[k]} onchange={touch} /> {k === 'saas' ? 'SaaS web platform' : k === 'ios' ? 'iOS app' : 'Android app'}</label>
              {/each}
            </div>
            <div class="field">
              <label class="label" for="pd">Detailed description <span class="muted">(your own words; the listings, the website and Gilgamesh all work from it)</span></label>
              <textarea id="pd" class="input" rows="12" bind:value={p.description} oninput={touch} placeholder="What it is, the problem it solves, how it works, what makes it different…"></textarea>
            </div>
            <div class="grid2">
              <div class="field">
                <label class="label" for="pf">Key features <span class="muted">(one per line)</span></label>
                <textarea id="pf" class="input" rows="5" bind:value={p.features} oninput={touch}></textarea>
              </div>
              <div class="field">
                <label class="label" for="pa">Who it’s for</label>
                <textarea id="pa" class="input" rows="5" bind:value={p.audience} oninput={touch}></textarea>
              </div>
              <div class="field">
                <label class="label" for="pp">Pricing</label>
                <textarea id="pp" class="input" rows="4" bind:value={p.pricing} oninput={touch} placeholder="Free; Pro $9/month…"></textarea>
              </div>
              <div class="field">
                <label class="label" for="pdata">User data it collects <span class="muted">(feeds App Privacy, Data safety and the privacy policy)</span></label>
                <textarea id="pdata" class="input" rows="4" bind:value={p.data} oninput={touch} placeholder="Email for the account; usage analytics (not linked to the user); payments via Stripe…"></textarea>
              </div>
            </div>
            <h3>Company and contact <span class="muted">(App Review and Google Play show or need these)</span></h3>
            <div class="grid3">
              <input class="input" bind:value={p.company} oninput={touch} placeholder="Company" />
              <input class="input" bind:value={p.contactName} oninput={touch} placeholder="Contact name" />
              <input class="input" bind:value={p.contactEmail} oninput={touch} placeholder="Contact email" />
              <input class="input" bind:value={p.contactPhone} oninput={touch} placeholder="Phone (+61…)" />
              <input class="input span2" bind:value={p.address} oninput={touch} placeholder="Address" />
            </div>
            <h3>Addresses <span class="muted">(the website build fills in the ones it makes)</span></h3>
            <div class="grid2">
              {#each [['website', 'Website'], ['app', 'Web app (SaaS)'], ['support', 'Support URL'], ['privacy', 'Privacy policy URL'], ['terms', 'Terms URL'], ['marketing', 'Marketing URL']] as [k, l] (k)}
                <label class="url"><span>{l}</span><input class="input" bind:value={p.urls[k as keyof Product['urls']]} oninput={touch} placeholder="https://…" /></label>
              {/each}
            </div>
          {:else if P.tab === 'ios'}
            {@const i = p.ios}
            <div class="actions">
              <button class="btn sm primary" disabled={!!busy} onclick={() => draft('ios')}><Wand2 size={14} /> Draft from the description</button>
              <span class="muted">Limits from App Store Connect’s platform version information.</span>
            </div>
            <div class="grid3">
              <label class="url"><span>Bundle ID</span><input class="input mono" bind:value={i.bundleId} oninput={touch} placeholder="au.com.company.app" /></label>
              <label class="url"><span>SKU</span><input class="input mono" bind:value={i.sku} oninput={touch} placeholder={p.id} /></label>
              <label class="url"><span>Version</span><input class="input" bind:value={i.version} oninput={touch} /></label>
              <label class="url"><span>Primary category</span><input class="input" bind:value={i.primaryCategory} oninput={touch} /></label>
              <label class="url"><span>Secondary category</span><input class="input" bind:value={i.secondaryCategory} oninput={touch} /></label>
              <label class="url"><span>Price</span><input class="input" bind:value={i.price} oninput={touch} /></label>
            </div>
            <div class="field"><label class="label" for="in">App name {@render counter(i.name, LIMITS.ios.name)}</label><input id="in" class="input" bind:value={i.name} oninput={touch} /></div>
            <div class="field"><label class="label" for="is">Subtitle {@render counter(i.subtitle, LIMITS.ios.subtitle)}</label><input id="is" class="input" bind:value={i.subtitle} oninput={touch} /></div>
            <div class="field"><label class="label" for="ip">Promotional text {@render counter(i.promo, LIMITS.ios.promo)} <span class="muted">(can change any time without a new version)</span></label><textarea id="ip" class="input" rows="2" bind:value={i.promo} oninput={touch}></textarea></div>
            <div class="field"><label class="label" for="id">Description {@render counter(i.description, LIMITS.ios.description)}</label><textarea id="id" class="input" rows="10" bind:value={i.description} oninput={touch}></textarea></div>
            <div class="field"><label class="label" for="ik">Keywords {@render counter(i.keywords, LIMITS.ios.keywords, true)} <span class="muted">(comma-separated; don’t repeat the name or company)</span></label><input id="ik" class="input" bind:value={i.keywords} oninput={touch} /></div>
            <div class="field"><label class="label" for="iw">What’s new {@render counter(i.whatsNew, LIMITS.ios.whatsNew)}</label><textarea id="iw" class="input" rows="3" bind:value={i.whatsNew} oninput={touch}></textarea></div>
            <div class="grid2">
              <div class="field"><label class="label" for="ic">Copyright</label><input id="ic" class="input" bind:value={i.copyright} oninput={touch} placeholder="2026 Company Pty Ltd" /></div>
              <div class="row">
                <label class="tog"><input type="checkbox" bind:checked={i.iPad} onchange={touch} /> Runs on iPad</label>
                <label class="tog"><input type="checkbox" bind:checked={i.signIn} onchange={touch} /> Needs a sign-in</label>
              </div>
            </div>
            {#if i.signIn}
              <div class="field">
                <label class="label" for="idl">Demo account for App Review <span class="muted">(the name of a saved login in Bots → Logins; the password never goes in here)</span></label>
                <select id="idl" class="select" bind:value={i.demoLogin} onchange={touch}>
                  <option value="">—</option>
                  {#each bots.secrets as s (s.id)}<option value={s.name}>{s.name} ({s.domain})</option>{/each}
                </select>
              </div>
            {/if}
            <div class="grid2">
              <div class="field"><label class="label" for="iar">Age rating answers</label><textarea id="iar" class="input" rows="5" bind:value={i.ageRating} oninput={touch}></textarea></div>
              <div class="field"><label class="label" for="ipr">App Privacy answers</label><textarea id="ipr" class="input" rows="5" bind:value={i.privacy} oninput={touch}></textarea></div>
            </div>
            <div class="field"><label class="label" for="irn">Notes for App Review</label><textarea id="irn" class="input" rows="3" bind:value={i.reviewNotes} oninput={touch}></textarea></div>
          {:else if P.tab === 'android'}
            {@const a = p.android}
            <div class="actions">
              <button class="btn sm primary" disabled={!!busy} onclick={() => draft('android')}><Wand2 size={14} /> Draft from the description</button>
              <span class="muted">Limits from Play Console’s “Create and set up your app”.</span>
            </div>
            <div class="grid3">
              <label class="url"><span>Package name</span><input class="input mono" bind:value={a.packageName} oninput={touch} placeholder="au.com.company.app" /></label>
              <label class="url"><span>Category</span><input class="input" bind:value={a.category} oninput={touch} /></label>
              <label class="url"><span>Tags</span><input class="input" bind:value={a.tags} oninput={touch} /></label>
            </div>
            <div class="row">
              <label class="tog"><input type="checkbox" bind:checked={a.free} onchange={touch} /> Free</label>
              <label class="tog"><input type="checkbox" bind:checked={a.ads} onchange={touch} /> Contains ads</label>
            </div>
            <div class="field"><label class="label" for="an">App name {@render counter(a.name, LIMITS.android.name)}</label><input id="an" class="input" bind:value={a.name} oninput={touch} /></div>
            <div class="field"><label class="label" for="as">Short description {@render counter(a.short, LIMITS.android.short)}</label><input id="as" class="input" bind:value={a.short} oninput={touch} /></div>
            <div class="field"><label class="label" for="af">Full description {@render counter(a.full, LIMITS.android.full)}</label><textarea id="af" class="input" rows="10" bind:value={a.full} oninput={touch}></textarea></div>
            <div class="field"><label class="label" for="ar">Release notes {@render counter(a.releaseNotes, LIMITS.android.releaseNotes)}</label><textarea id="ar" class="input" rows="3" bind:value={a.releaseNotes} oninput={touch}></textarea></div>
            <div class="grid2">
              <div class="field"><label class="label" for="acr">Content rating answers</label><textarea id="acr" class="input" rows="5" bind:value={a.contentRating} oninput={touch}></textarea></div>
              <div class="field"><label class="label" for="ads">Data safety answers</label><textarea id="ads" class="input" rows="5" bind:value={a.dataSafety} oninput={touch}></textarea></div>
              <div class="field"><label class="label" for="ata">Target age groups</label><input id="ata" class="input" bind:value={a.targetAge} oninput={touch} placeholder="18 and over" /></div>
              <div class="field">
                <label class="label" for="aac">App access <span class="muted">(how reviewers get in)</span></label>
                <input id="aac" class="input" bind:value={a.appAccess} oninput={touch} placeholder="All functionality available without special access" />
              </div>
            </div>
            <div class="field">
              <label class="label" for="adl">Demo account login <span class="muted">(saved login name, if reviewers need one)</span></label>
              <select id="adl" class="select" bind:value={a.demoLogin} onchange={touch}>
                <option value="">—</option>
                {#each bots.secrets as s (s.id)}<option value={s.name}>{s.name} ({s.domain})</option>{/each}
              </select>
            </div>
          {:else if P.tab === 'assets'}
            {#each ASSET_KINDS.filter((k) => !k.platform || p.platforms[k.platform]) as k (k.kind)}
              {@const list = P.assetsOf(p, k.kind)}
              <div class="kind">
                <div class="khead">
                  <strong>{k.label}</strong>
                  <span class="muted">{k.hint}</span>
                  <span class="spacer"></span>
                  <button class="btn sm" onclick={() => addPictures(k.kind)}><ImagePlus size={14} /> Add</button>
                </div>
                {#if list.length}
                  <div class="shots">
                    {#each list as a (a.name)}
                      <figure>
                        {#await P.thumb(p, a.name)}<div class="ph"></div>{:then src}<img {src} alt={a.name} />{:catch}<div class="ph"></div>{/await}
                        <button class="del icon-btn sm" title="Remove" onclick={() => P.removeAsset(p, a.name)}><X size={12} /></button>
                        <figcaption>{a.width}×{a.height}{a.alpha ? ' · alpha' : ''} · {Math.round(a.size / 1024)} KB</figcaption>
                      </figure>
                    {/each}
                  </div>
                {/if}
              </div>
            {/each}
            <p class="muted">Need a resized icon or a feature graphic? Ask Gilgamesh to make one in PhotoCraft.</p>
          {:else if P.tab === 'website'}
            {#if !P.gnuInfo?.has_token}
              <div class="note">Add your gnu.au API token first (a <b>write</b> token from Account → API tokens on gnu.au). <button class="btn sm" onclick={openSettings}>gnu.au settings</button></div>
            {/if}
            <p class="muted">RAD writes the site from the description with gnu.au’s blocks: home, features{p.pricing ? ', pricing' : ''}, support, privacy policy and terms, using your pictures. The stores take the support and privacy pages as the product’s URLs. Read the privacy policy and terms before publishing: they’re a draft, not legal advice.</p>
            <div class="row">
              {#if p.gnu}
                <span>Site: <button class="link" onclick={() => openUrl(p.gnu!.url)}>{p.gnu.url} <ExternalLink size={11} /></button></span>
                <span class="muted">{p.gnu.published ? `published ${when(p.gnu.published)}` : 'not published'}{p.gnu.built ? ` · built ${when(p.gnu.built)}` : ''}</span>
              {:else}
                <label class="url sub"><span>Subdomain</span><span class="subd"><input class="input mono" bind:value={subdomain} /> .gnu.au</span></label>
              {/if}
            </div>
            <div class="actions">
              <button class="btn sm primary" disabled={!!busy || !P.gnuInfo?.has_token} onclick={build}><Globe size={14} /> {p.gnu ? 'Rebuild the pages' : 'Build the website'}</button>
              {#if p.gnu}
                <button class="btn sm" disabled={!!busy} onclick={() => showPreview()}><Eye size={14} /> Preview</button>
                <button class="btn sm" disabled={!!busy || !p.gnu.built} onclick={publish}><Upload size={14} /> Publish</button>
              {/if}
            </div>
            {#if p.gnu && Object.keys(p.gnu.pages).length}
              <div class="pages">
                {#each Object.keys(p.gnu.pages) as slug (slug)}
                  <button class="chip" onclick={() => showPreview(slug)}>/{slug === 'index' ? '' : slug}</button>
                {/each}
              </div>
            {/if}
            {#if preview}
              <div class="preview">
                <div class="pbar"><span class="muted">Draft preview (rendered by gnu.au)</span><span class="spacer"></span><button class="icon-btn sm" onclick={() => (preview = null)}><X size={13} /></button></div>
                <iframe title="Preview" sandbox="" srcdoc={preview}></iframe>
              </div>
            {/if}
          {:else if P.tab === 'launch'}
            {#if checks}
              <div class="ready">
                {#each (['saas', 'ios', 'android'] as const).filter((k) => p.platforms[k]) as k (k)}
                  <div class="col">
                    <h3>{k === 'saas' ? 'SaaS & website' : k === 'ios' ? 'App Store' : 'Google Play'}</h3>
                    {#each checks[k] as c (c.label)}{@render checkRow(c)}{/each}
                  </div>
                {/each}
              </div>
            {/if}
            {#if p.platforms.ios || p.platforms.android}
              <h3>Fill in the stores with a bot</h3>
              {#if !bots.bots.length}
                <div class="note">Make a bot first (Bots), and give it your App Store Connect login (domain <b>apple.com</b>) and Google Play Console login (domain <b>google.com</b>) under Bots → Logins. <button class="btn sm" onclick={() => store.navigate({ kind: 'bots' })}>Open Bots</button></div>
              {:else}
                <div class="row">
                  <label class="url"><span>Bot</span>
                    <select class="select" bind:value={launchBot}>
                      {#each bots.bots as b (b.id)}<option value={b.id}>{b.name}{bots.busy(b.id) ? ' (busy)' : ''}</option>{/each}
                    </select>
                  </label>
                  {#each (['ios', 'android'] as const).filter((k) => p.platforms[k]) as k (k)}
                    {@const logins = launchBot ? P.storeLogins(launchBot, k) : []}
                    <span class="muted login" class:bad={!logins.length}>{k === 'ios' ? 'Apple' : 'Google'} login: {logins.length ? logins.map((s) => s.name).join(', ') : `none for ${STORE_LOGIN_DOMAIN[k]}`}</span>
                  {/each}
                </div>
                <textarea class="input" rows="2" bind:value={launchExtra} placeholder="Anything else for the bot (optional): e.g. “only Australia and New Zealand”, “primary language English (U.S.)”"></textarea>
                <div class="actions">
                  {#if p.platforms.ios}<button class="btn sm primary" disabled={!!busy || !launchBot} onclick={() => launch('ios')}><Rocket size={14} /> Fill in App Store Connect</button>{/if}
                  {#if p.platforms.android}<button class="btn sm primary" disabled={!!busy || !launchBot} onclick={() => launch('android')}><Rocket size={14} /> Fill in Google Play Console</button>{/if}
                  <button class="btn sm ghost" onclick={() => store.navigate({ kind: 'bots', bot: launchBot })}><BotIcon size={14} /> Watch in Bots</button>
                </div>
                <p class="muted">The bot gets the listing and the pictures on its computer, logs in with your saved login (you’re asked for 2FA codes), fills in every section and uploads the pictures. It never submits for review, picks a build or rolls out: that stays with you. Uploading the build itself (Xcode / Transporter, the .aab) is yours too.</p>
              {/if}
            {/if}
            <h3>History</h3>
            {#each p.log as l (l.at)}
              <div class="log" class:bad={l.ok === false}><small>{when(l.at)}</small> {l.text}</div>
            {:else}
              <p class="muted">Nothing yet.</p>
            {/each}
          {/if}
        </div>
      {/if}
    </section>
    {#if showChat}
      <aside class="side"><ProductChat /></aside>
    {/if}
  </div>
</div>

<Modal bind:open={settingsOpen} title="gnu.au and the store guides" width={600}>
  <div class="field">
    <span class="label">gnu.au API</span>
    <input class="input mono" bind:value={gnuBase} placeholder="https://gnu.au/api/v1" />
    <input class="input mono" type="password" bind:value={gnuToken} placeholder={P.gnuInfo?.has_token ? 'Token saved in the keychain (paste a new one to replace it)' : 'gnu_… (a write token: gnu.au → Account → API tokens)'} style="margin-top:6px" />
    <div class="actions">
      <button class="btn sm primary" onclick={() => saveGnu()}>Save and test</button>
      {#if P.gnuInfo?.has_token}<button class="btn sm ghost" onclick={() => saveGnu(true)}>Forget the token</button>{/if}
    </div>
    <p class="hint">The token stays in this Mac’s keychain; RAD calls gnu.au itself, so the page never sees it.</p>
  </div>
  <div class="field">
    <span class="label">Store guides <span class="muted">(the official pages, saved here so drafting and Gilgamesh work offline)</span></span>
    <div class="guides">
      {#each GUIDE_SOURCES as s (s.url)}
        {@const g = P.guides.find((x) => x.url === s.url)}
        <div class="guide">
          <span class="st">{s.store === 'apple' ? 'Apple' : 'Google'}</span>
          <button class="link truncate" onclick={() => openUrl(s.url)}>{s.label}</button>
          <span class="spacer"></span>
          <small class="muted">{g ? `${g.words.toLocaleString()} words, ${new Date(g.fetched).toLocaleDateString('en-GB')}` : 'not fetched'}</small>
        </div>
      {/each}
    </div>
    <div class="actions">
      <button class="btn sm" disabled={fetching} onclick={fetchGuides}>{#if fetching}<Loader size={13} class="spin" />{:else}<BookOpen size={14} />{/if} {P.guides.length ? 'Fetch again' : 'Fetch the guides'}</button>
      <button class="btn sm" disabled={!P.guides.length} onclick={() => run(() => P.guidesToRag())}><BrainCircuit size={14} /> Add to RAG</button>
    </div>
  </div>
</Modal>

<style>
  .products-view {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--bg);
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
    font-size: 15px;
    font-weight: 600;
    margin: 0;
  }
  .sub {
    font-size: 12px;
    color: var(--text-3);
  }
  .spacer {
    flex: 1;
  }
  .muted {
    color: var(--text-3);
    font-weight: normal;
  }
  .main {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .list {
    width: 220px;
    flex: none;
    border-right: 1px solid var(--border);
    padding: 10px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    overflow: auto;
  }
  .item {
    text-align: left;
    border: 0;
    background: none;
    padding: 7px 8px;
    border-radius: var(--r-sm);
    display: flex;
    flex-direction: column;
    gap: 3px;
    color: var(--text);
    cursor: pointer;
    min-width: 0;
  }
  .item:hover {
    background: var(--surface-2);
  }
  .item.cur {
    background: var(--accent-soft);
  }
  .item strong {
    font-size: 13px;
    font-weight: 600;
  }
  .item small {
    font-size: 11px;
    color: var(--text-3);
  }
  .pills {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }
  .pill {
    font-size: 10.5px;
    padding: 0 6px;
    border-radius: 999px;
    background: var(--surface-3);
    color: var(--text-2);
  }
  .pill.done {
    background: color-mix(in srgb, #16a34a 22%, transparent);
    color: var(--text);
  }
  .empty {
    font-size: 12.5px;
    color: var(--text-3);
    line-height: 1.5;
    padding: 6px 4px;
  }
  .work {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .blank {
    margin: auto;
    text-align: center;
    color: var(--text-3);
  }
  .phead {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 16px 6px;
  }
  .titles {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .name,
  .tagline {
    border: 0;
    background: none;
    color: var(--text);
    outline: none;
    padding: 2px 0;
  }
  .name {
    font-size: 19px;
    font-weight: 650;
  }
  .tagline {
    font-size: 13px;
    color: var(--text-2);
  }
  .tabs {
    display: flex;
    gap: 2px;
    padding: 0 12px;
    border-bottom: 1px solid var(--border);
  }
  .tab {
    border: 0;
    background: none;
    padding: 8px 10px;
    font-size: 13px;
    color: var(--text-2);
    cursor: pointer;
    border-bottom: 2px solid transparent;
  }
  .tab.cur {
    color: var(--text);
    border-bottom-color: var(--accent);
  }
  .busy {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 16px;
    font-size: 12.5px;
    background: var(--accent-soft);
  }
  .pane {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 14px 16px 30px;
  }
  .pane h3 {
    font-size: 13px;
    font-weight: 600;
    margin: 18px 0 8px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 14px;
    flex-wrap: wrap;
    margin-bottom: 12px;
    font-size: 13px;
  }
  .tog {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    font-size: 13px;
  }
  .grid2,
  .grid3 {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0 12px;
  }
  .grid3 {
    grid-template-columns: repeat(3, 1fr);
    gap: 8px 10px;
    margin-bottom: 14px;
  }
  .span2 {
    grid-column: span 2;
  }
  .url {
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 12px;
    color: var(--text-3);
    margin-bottom: 8px;
  }
  .subd {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--text-2);
  }
  .label .count {
    margin-left: 6px;
  }
  .count {
    font-size: 11px;
    color: var(--text-3);
    font-weight: normal;
  }
  .count.over {
    color: var(--danger, #dc2626);
    font-weight: 600;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    margin: 4px 0 14px;
    font-size: 12px;
  }
  .note {
    padding: 10px 12px;
    border-radius: var(--r-sm);
    background: var(--surface-2);
    border: 1px solid var(--border);
    font-size: 13px;
    margin-bottom: 12px;
    display: flex;
    gap: 10px;
    align-items: center;
    flex-wrap: wrap;
  }
  .pane > p.muted {
    font-size: 12.5px;
    line-height: 1.5;
  }
  .kind {
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    padding: 10px 12px;
    margin-bottom: 10px;
  }
  .khead {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 13px;
  }
  .khead .muted {
    font-size: 12px;
  }
  .shots {
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
    margin-top: 10px;
  }
  figure {
    margin: 0;
    position: relative;
    width: 110px;
  }
  figure img,
  .ph {
    width: 110px;
    height: 150px;
    object-fit: contain;
    background: var(--surface-2);
    border-radius: 6px;
    border: 1px solid var(--border);
    display: block;
  }
  figcaption {
    font-size: 10.5px;
    color: var(--text-3);
    margin-top: 3px;
  }
  .del {
    position: absolute;
    top: 3px;
    right: 3px;
    background: var(--surface);
  }
  .link {
    border: 0;
    background: none;
    padding: 0;
    color: var(--accent);
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 3px;
    font-size: inherit;
    min-width: 0;
  }
  .pages {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
    margin-bottom: 10px;
  }
  .chip {
    font-size: 12px;
    padding: 2px 9px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--surface-2);
    color: var(--text-2);
    cursor: pointer;
  }
  .preview {
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    overflow: hidden;
  }
  .pbar {
    display: flex;
    align-items: center;
    padding: 4px 8px;
    font-size: 12px;
    border-bottom: 1px solid var(--border);
  }
  .preview iframe {
    width: 100%;
    height: 560px;
    border: 0;
    background: white;
    display: block;
  }
  .ready {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(230px, 1fr));
    gap: 14px;
  }
  .col h3 {
    margin-top: 0 !important;
  }
  .check {
    display: grid;
    grid-template-columns: 16px 1fr;
    column-gap: 6px;
    width: 100%;
    text-align: left;
    border: 0;
    background: none;
    padding: 4px 4px;
    border-radius: 6px;
    font-size: 12.5px;
    color: var(--danger, #dc2626);
    cursor: pointer;
  }
  .check:hover {
    background: var(--surface-2);
  }
  .check.ok {
    color: #16a34a;
  }
  .check.manual {
    color: var(--text-3);
  }
  .check .cl {
    color: var(--text);
  }
  .check small {
    grid-column: 2;
    color: var(--text-3);
    font-size: 11px;
  }
  .login.bad {
    color: var(--danger, #dc2626);
  }
  .log {
    font-size: 12.5px;
    padding: 3px 0;
    line-height: 1.45;
  }
  .log small {
    color: var(--text-3);
    margin-right: 6px;
  }
  .log.bad {
    color: var(--danger, #dc2626);
  }
  .guides {
    display: flex;
    flex-direction: column;
    gap: 3px;
    margin-top: 6px;
  }
  .guide {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12.5px;
  }
  .st {
    font-size: 10.5px;
    padding: 0 6px;
    border-radius: 999px;
    background: var(--surface-3);
    flex: none;
  }
  .side {
    width: 340px;
    flex: none;
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-left: 1px solid var(--border);
  }
</style>
