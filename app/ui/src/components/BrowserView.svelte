<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { rad, RadError } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import { vpn } from '../lib/vpn.svelte';
  import { toUrl, hostOf } from '../lib/web';
  import type { BrowserEngine, BrowserState, BrowserTab } from '../lib/types';
  import BrowserChat from './BrowserChat.svelte';
  import { activity } from '../lib/gilgamesh/observe.svelte';
  import { ArrowLeft, ArrowRight, RotateCw, Plus, X, Lock, Globe, Camera, FileDown, Loader, MessageSquare, ZoomIn, ZoomOut, Home, TriangleAlert, Star, Download, FolderOpen, FolderInput, Check } from '@lucide/svelte';

  /** Which engine renders this view: Servo ("Web Browser"), Chromium or Tor. */
  let { kind = 'servo' }: { kind?: BrowserEngine } = $props();
  const HOME = $derived(kind === 'tor' ? 'https://check.torproject.org' : 'https://rymtech.net');
  const engineName = $derived(kind === 'chromium' ? 'Chromium' : kind === 'tor' ? 'Tor' : 'browser');

  let viewport: HTMLDivElement | undefined = $state();
  let canvas: HTMLCanvasElement | undefined = $state();
  let address: HTMLInputElement | undefined = $state();
  let ctx: CanvasRenderingContext2D | null = null;

  let tabs = $state<BrowserTab[]>([]);
  let active = $state(0);
  let addressText = $state('');
  let editingAddress = $state(false);
  let cursor = $state('default');
  let statusText = $state<string | null>(null);
  /** Tor bootstrap status, when this is the Tor browser. */
  let tor = $state<{ status: 'connecting' | 'bootstrapping' | 'ready' | 'error'; progress: number; message?: string } | null>(
    kind === 'tor' ? { status: 'connecting', progress: 0 } : null,
  );
  let engine = $state<'starting' | 'running' | 'missing' | 'stopped'>('starting');
  let engineError = $state('');
  let chatOpen = $state(true);
  let saving = $state<'shot' | 'page' | null>(null);
  let dialog = $state<{ control: number; kind: string; message: string; value: string; host: string } | null>(null);
  let select = $state<{ control: number; options: any[]; selected: number[]; x: number; y: number; w: number } | null>(null);
  let canvasCss = $state({ w: 0, h: 0 });
  /** Chromium saves downloads to the Downloads folder; they can then go into the vault. */
  let downloads = $state<{ id: number; name: string; path: string; received: number; total: number; state: string; vault?: 'saving' | 'saved' }[]>([]);

  const tab = $derived(tabs.find((t) => t.id === active) ?? null);
  // Gilgamesh sees the page that's open.
  $effect(() => {
    const route = kind === 'servo' ? 'browser' : kind;
    if (route !== 'browser' && route !== 'chromium' && route !== 'tor') return;
    activity.seen(route, tab?.url ? `showing the page “${tab.title || hostOf(tab.url) || tab.url}” (${tab.url})${tabs.length > 1 ? `; ${tabs.length} tabs open` : ''}` : null);
    return () => activity.seen(route, null);
  });
  const bookmarked = $derived(!!tab?.url && store.bookmarks.some((b) => b.url === tab.url));

  function toggleBookmark() {
    if (!tab?.url) return;
    if (bookmarked) store.saveBookmarks(store.bookmarks.filter((b) => b.url !== tab.url));
    else {
      store.saveBookmarks([...store.bookmarks, { title: tab.title || hostOf(tab.url) || tab.url, url: tab.url }]);
      store.toast('success', 'Added to favourites', tab.title || tab.url, 2200);
    }
  }

  // Pages the assistant opened ("open servo.org").
  $effect(() => {
    // The assistant opens pages in the Web Browser (Servo).
    const url = store.pendingBrowserUrl;
    if (kind !== 'servo' || !url || engine === 'starting') return;
    store.pendingBrowserUrl = null;
    if (engine === 'running' && tabs.length) send({ cmd: 'new_tab', url });
    else go(url);
  });
  const secure = $derived(tab?.url.startsWith('https://') ?? false);

  $effect(() => {
    if (!editingAddress) addressText = tab?.url ?? '';
  });

  const dpr = () => window.devicePixelRatio || 1;
  /** Page to reopen after Chromium restarts for a VPN change. */
  let vpnRestart: string | null = null;
  const servoUncovered = $derived(kind === 'servo' && vpn.info?.on && vpn.info.settings.scopes.browsers);
  const send = (cmd: Record<string, unknown>) => rad.browserSend(kind, cmd).catch(() => {});

  // ------------------------------------------------------------ frames

  let pulling = false;
  let again = false;
  async function pull() {
    if (pulling) {
      again = true;
      return;
    }
    pulling = true;
    try {
      do {
        again = false;
        draw(await rad.browserFrame(kind));
      } while (again);
    } catch {
      /* engine gone; the 'exited' event explains */
    } finally {
      pulling = false;
    }
  }

  function draw(buf: ArrayBuffer) {
    if (!canvas || buf.byteLength < 16) return;
    const h = new DataView(buf);
    const [w, height, y0, rows] = [h.getUint32(0, true), h.getUint32(4, true), h.getUint32(8, true), h.getUint32(12, true)];
    if (!rows || !w) return;
    if (canvas.width !== w || canvas.height !== height) {
      canvas.width = w;
      canvas.height = height;
      canvasCss = { w: w / dpr(), h: height / dpr() };
      ctx = null;
    }
    ctx ??= canvas.getContext('2d', { alpha: false });
    ctx?.putImageData(new ImageData(new Uint8ClampedArray(buf, 16, rows * w * 4), w, rows), 0, y0);
  }

  // ------------------------------------------------------------ engine

  function viewportSize() {
    const r = viewport!.getBoundingClientRect();
    return { w: Math.max(1, Math.round(r.width * dpr())), h: Math.max(1, Math.round(r.height * dpr())) };
  }

  async function start(url?: string) {
    if (!viewport) return;
    engine = 'starting';
    const { w, h } = viewportSize();
    try {
      const st = await rad.browserStart(kind, w, h, dpr(), url);
      if (st) applyState(st);
      engine = 'running';
      send({ cmd: 'theme', theme: document.documentElement.dataset.theme === 'light' ? 'light' : 'dark' });
    } catch (e) {
      engine = e instanceof RadError && e.code === 'browser_missing' ? 'missing' : 'stopped';
      engineError = e instanceof Error ? e.message : String(e);
    }
  }

  function applyState(st: BrowserState) {
    tabs = st.tabs;
    active = st.active;
  }

  function onEvent(ev: any) {
    switch (ev.type) {
      case 'state':
        applyState(ev);
        break;
      case 'cursor':
        cursor = ev.cursor === 'default' ? 'default' : ev.cursor;
        break;
      case 'status':
        statusText = ev.text;
        break;
      case 'tor':
        tor = { status: ev.status === 'ready' ? 'ready' : ev.status === 'error' ? 'error' : 'bootstrapping', progress: typeof ev.progress === 'number' ? ev.progress : (tor?.progress ?? 0), message: ev.message };
        break;
      case 'dialog':
        dialog = { control: ev.control, kind: ev.kind, message: ev.message, value: ev.default ?? '', host: hostOf(tab?.url ?? '') || 'This page' };
        break;
      case 'select':
        select = { control: ev.control, options: ev.options, selected: ev.selected, x: ev.x / dpr(), y: (ev.y + ev.h) / dpr(), w: ev.w / dpr() };
        break;
      case 'control_hidden':
        if (dialog?.control === ev.control) dialog = null;
        if (select?.control === ev.control) select = null;
        break;
      case 'error':
        store.toast('error', engineName, ev.message);
        break;
      case 'download': {
        const d = { id: ev.id, name: ev.name, path: ev.path, received: ev.received, total: ev.total, state: ev.state };
        const i = downloads.findIndex((x) => x.id === ev.id);
        if (i >= 0) downloads[i] = { ...downloads[i], ...d };
        else downloads = [d, ...downloads].slice(0, 5);
        if (ev.state === 'done') store.toast('success', 'Downloaded', ev.name, 2500);
        else if (ev.state === 'failed') store.toast('error', 'Download failed', ev.name);
        break;
      }
      case 'crashed':
        store.toast('error', 'The page crashed', ev.reason);
        break;
      case 'fatal':
        engineError = ev.message;
        engine = 'stopped';
        break;
      case 'exited':
        if (engine === 'running') engine = 'stopped';
        tabs = [];
        // Stopped because the VPN changed: start again, now with (or without) its proxy.
        if (vpnRestart) {
          const url = vpnRestart;
          vpnRestart = null;
          start(url);
        }
        break;
    }
  }

  onMount(() => {
    const un: Array<() => void> = [];
    // Both engines share these events; each view follows its own.
    listen<string>('browser-frame', (e) => e.payload === kind && pull()).then((u) => un.push(u));
    listen<any>('browser-event', (e) => (e.payload.engine ?? 'servo') === kind && onEvent(e.payload)).then((u) => un.push(u));
    if (kind === 'chromium')
      listen('vpn-changed', () => {
        if (engine !== 'running' && engine !== 'stopped') return;
        const url = tab?.url || HOME;
        if (engine === 'stopped') start(url);
        else vpnRestart = url;
      }).then((u) => un.push(u));
    start();

    let t: ReturnType<typeof setTimeout> | undefined;
    const ro = new ResizeObserver(() => {
      clearTimeout(t);
      t = setTimeout(() => {
        if (engine !== 'running' || !viewport) return;
        const { w, h } = viewportSize();
        send({ cmd: 'resize', w, h, scale: dpr() });
      }, 60);
    });
    if (viewport) ro.observe(viewport);
    return () => {
      ro.disconnect();
      clearTimeout(t);
      un.forEach((u) => u());
    };
  });

  // ------------------------------------------------------------ navigation

  function go(input: string) {
    const url = toUrl(input);
    editingAddress = false;
    address?.blur();
    if (engine !== 'running') return start(url);
    send(tabs.length ? { cmd: 'load', url } : { cmd: 'new_tab', url });
    canvas?.focus();
  }

  async function newTab() {
    await send({ cmd: 'new_tab', url: HOME });
    await tick();
    address?.focus();
    address?.select();
  }

  function closeTab(id: number, e?: Event) {
    e?.stopPropagation();
    send({ cmd: 'close_tab', tab: id });
  }

  function zoom(delta: number | null) {
    const z = delta === null ? 1 : Math.round(((tab?.zoom ?? 1) + delta) * 10) / 10;
    send({ cmd: 'zoom', level: z });
  }

  // ------------------------------------------------------------ input

  function point(e: MouseEvent) {
    const r = canvas!.getBoundingClientRect();
    return { x: (e.clientX - r.left) * dpr(), y: (e.clientY - r.top) * dpr() };
  }

  let moveQueued: MouseEvent | null = null;
  function onMove(e: MouseEvent) {
    if (!moveQueued) requestAnimationFrame(() => {
      if (moveQueued) send({ cmd: 'mouse_move', ...point(moveQueued) });
      moveQueued = null;
    });
    moveQueued = e;
  }

  function onDown(e: MouseEvent) {
    if (e.button === 3) return send({ cmd: 'back' });
    if (e.button === 4) return send({ cmd: 'forward' });
    canvas?.focus();
    select = null;
    send({ cmd: 'mouse_button', action: 'down', button: e.button, ...point(e) });
    const up = (u: MouseEvent) => {
      window.removeEventListener('mouseup', up);
      send({ cmd: 'mouse_button', action: 'up', button: u.button, ...point(u) });
    };
    window.addEventListener('mouseup', up);
  }

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    const k = e.deltaMode === 0 ? dpr() : 1;
    send({ cmd: 'wheel', dx: e.deltaX * k, dy: e.deltaY * k, mode: e.deltaMode, ...point(e) });
  }

  const isMac = document.documentElement.dataset.os === 'macos';

  /** Browser shortcuts; returns true when handled. Others go to the page or bubble to RAD. */
  function shortcut(e: KeyboardEvent) {
    const mod = isMac ? e.metaKey : e.ctrlKey;
    if (!mod) return e.altKey && (e.key === 'ArrowLeft' || e.key === 'ArrowRight') ? (send({ cmd: e.key === 'ArrowLeft' ? 'back' : 'forward' }), true) : false;
    switch (e.key.toLowerCase()) {
      case 'd':
        toggleBookmark();
        return true;
      case 'l':
        address?.focus();
        address?.select();
        return true;
      case 't':
        newTab();
        return true;
      case 'w':
        if (tab) closeTab(tab.id);
        return true;
      case 'r':
        send({ cmd: 'reload' });
        return true;
      case '[':
        send({ cmd: 'back' });
        return true;
      case ']':
        send({ cmd: 'forward' });
        return true;
      case '=':
      case '+':
        zoom(0.1);
        return true;
      case '-':
        zoom(-0.1);
        return true;
      case '0':
        zoom(null);
        return true;
      case 'c':
      case 'x':
      case 'v':
        send({ cmd: 'clipboard', action: { c: 'copy', x: 'cut', v: 'paste' }[e.key.toLowerCase()] });
        return true;
    }
    return false;
  }

  function onKey(e: KeyboardEvent, state: 'down' | 'up') {
    if (e.isComposing) return;
    if (state === 'down' && shortcut(e)) {
      e.preventDefault();
      e.stopPropagation();
      return;
    }
    const mod = isMac ? e.metaKey : e.ctrlKey;
    // Leave app-wide shortcuts (⌘K, ⌘N, ⌘,…) to RAD, except select-all which belongs to the page.
    if (mod && e.key.toLowerCase() !== 'a' && e.key.length === 1) return;
    e.preventDefault();
    e.stopPropagation();
    send({
      cmd: 'key', state, key: e.key, code: e.code, location: e.location, repeat: e.repeat,
      shift: e.shiftKey, ctrl: e.ctrlKey, alt: e.altKey, meta: e.metaKey,
    });
  }

  function onCompositionEnd(e: CompositionEvent) {
    if (e.data) send({ cmd: 'text', text: e.data });
  }

  // ------------------------------------------------------------ page dialogs

  function answerDialog(ok: boolean) {
    if (!dialog) return;
    send({ cmd: 'dialog_reply', control: dialog.control, ok, value: dialog.value });
    dialog = null;
    canvas?.focus();
  }

  function answerSelect(option: number | null) {
    if (!select) return;
    send({ cmd: 'select_reply', control: select.control, option });
    select = null;
    canvas?.focus();
  }

  // ------------------------------------------------------------ vault

  function formatSize(n: number) {
    return n >= 1 << 20 ? `${(n / (1 << 20)).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1024))} KB`;
  }

  async function saveDownload(id: number) {
    const d = downloads.find((x) => x.id === id);
    if (!d) return;
    d.vault = 'saving';
    try {
      const inbox = await store.inboxProject();
      const res = await rad.uploadPaths(inbox.id, [d.path]);
      const err = res.find((r: any) => 'error' in r) as any;
      if (err) throw new Error(err.error);
      d.vault = 'saved';
      store.toast('success', 'Saved to your vault', `${d.name} — in ${inbox.name}. Indexed for chat and search.`);
    } catch (e) {
      d.vault = undefined;
      store.error(e, 'Couldn’t add the file to your vault');
    }
  }

  async function save(what: 'shot' | 'page') {
    if (saving) return;
    saving = what;
    try {
      const inbox = await store.inboxProject();
      const r = what === 'shot' ? await rad.browserSaveScreenshot(kind, inbox.id) : await rad.browserSavePage(kind, inbox.id);
      store.toast(
        'success',
        what === 'shot' ? 'Screenshot saved to your vault' : 'Page saved to your vault',
        `${r.document.title} — in ${inbox.name}. ${what === 'shot' ? 'The vision model will describe it for search.' : 'Indexed for chat and search.'}`,
      );
    } catch (e) {
      store.error(e, what === 'shot' ? 'Screenshot failed' : 'Saving the page failed');
    } finally {
      saving = null;
    }
  }
</script>

<div class="browser">
  <!-- Tabs -->
  <div class="tabs drag">
    {#each tabs as t (t.id)}
      <div class="tab no-drag" class:active={t.id === active} role="tab" tabindex="0" aria-selected={t.id === active}
        onclick={() => send({ cmd: 'activate', tab: t.id })}
        onkeydown={(e) => e.key === 'Enter' && send({ cmd: 'activate', tab: t.id })}
        onauxclick={(e) => e.button === 1 && closeTab(t.id, e)}>
        {#if t.loading}<Loader size={13} class="spin" />{:else}<Globe size={13} />{/if}
        <span class="truncate spacer">{t.title || hostOf(t.url) || 'New tab'}</span>
        <button class="x" aria-label="Close tab" onclick={(e) => closeTab(t.id, e)}><X size={12} /></button>
      </div>
    {/each}
    <button class="icon-btn sm no-drag" title="New tab (⌘T)" onclick={newTab} disabled={engine !== 'running'}><Plus size={15} /></button>
  </div>

  <!-- Navigation -->
  <div class="nav">
    <button class="icon-btn" title="Back (⌘[)" disabled={!tab?.can_back} onclick={() => send({ cmd: 'back' })}><ArrowLeft size={17} /></button>
    <button class="icon-btn" title="Forward (⌘])" disabled={!tab?.can_forward} onclick={() => send({ cmd: 'forward' })}><ArrowRight size={17} /></button>
    <button class="icon-btn" title="Reload (⌘R)" disabled={!tab} onclick={() => send({ cmd: 'reload' })}>
      {#if tab?.loading}<Loader size={16} class="spin" />{:else}<RotateCw size={16} />{/if}
    </button>
    <button class="icon-btn" title="Home" onclick={() => go(HOME)}><Home size={16} /></button>
    <form class="address" class:focus={editingAddress} onsubmit={(e) => (e.preventDefault(), go(addressText))}>
      {#if secure && !editingAddress}<Lock size={13} class="lock" />{:else}<Globe size={13} class="globe" />{/if}
      <input
        bind:this={address}
        bind:value={addressText}
        spellcheck="false"
        autocomplete="off"
        placeholder="Search or enter address"
        onfocus={() => ((editingAddress = true), address?.select())}
        onblur={() => (editingAddress = false)}
        onkeydown={(e) => { if (e.key === 'Escape') { editingAddress = false; addressText = tab?.url ?? ''; address?.blur(); } }}
      />
      {#if tab && tab.zoom !== 1}<button type="button" class="zoom" title="Reset zoom (⌘0)" onclick={() => zoom(null)}>{Math.round(tab.zoom * 100)}%</button>{/if}
      <button type="button" class="star" class:on={bookmarked} title={bookmarked ? 'Remove from favourites' : 'Add to favourites (⌘D)'} disabled={!tab?.url} onclick={toggleBookmark}>
        <Star size={15} fill={bookmarked ? 'currentColor' : 'none'} />
      </button>
    </form>
    <button class="icon-btn" title="Zoom out (⌘−)" disabled={!tab} onclick={() => zoom(-0.1)}><ZoomOut size={16} /></button>
    <button class="icon-btn" title="Zoom in (⌘+)" disabled={!tab} onclick={() => zoom(0.1)}><ZoomIn size={16} /></button>
    <span class="sep"></span>
    <button class="icon-btn" title="Save a screenshot of this page to your vault" disabled={!tab || !!saving} onclick={() => save('shot')}>
      {#if saving === 'shot'}<Loader size={16} class="spin" />{:else}<Camera size={16} />{/if}
    </button>
    <button class="icon-btn" title="Save this page's text to your vault" disabled={!tab || !!saving} onclick={() => save('page')}>
      {#if saving === 'page'}<Loader size={16} class="spin" />{:else}<FileDown size={16} />{/if}
    </button>
    <button class="icon-btn" class:active={chatOpen} title="Ask AI about this page" onclick={() => (chatOpen = !chatOpen)}><MessageSquare size={16} /></button>
  </div>

  {#if store.bookmarks.length}
    <div class="favs">
      {#each store.bookmarks as b (b.url)}
        <button class="fav" title={b.url} onclick={() => go(b.url)}
          oncontextmenu={(e) => { e.preventDefault(); store.saveBookmarks(store.bookmarks.filter((x) => x.url !== b.url)); }}>
          <Globe size={12} /><span class="truncate">{b.title}</span>
        </button>
      {/each}
    </div>
  {/if}

  {#if servoUncovered}
    <div class="vpn-note">This browser (Servo) can’t use RAD’s VPN — pages load over your normal connection. Use Chromium for browsing through the VPN.</div>
  {/if}

  {#if kind === 'tor' && tor && tor.status !== 'ready'}
    <div class="vpn-note tor-note">
      {#if tor.status === 'error'}
        Couldn’t connect to the Tor network{tor.message ? `: ${tor.message}` : ''}.
      {:else}
        Connecting to the Tor network{tor.status === 'bootstrapping' ? ` — ${Math.max(0, Math.round(tor.progress * 100))}%` : '…'}
      {/if}
    </div>
  {/if}

  <!-- Page -->
  <div class="viewport" bind:this={viewport}>
    {#if tab?.loading}<div class="progress"></div>{/if}
    <canvas
      bind:this={canvas}
      tabindex="0"
      style="cursor:{cursor};width:{canvasCss.w}px;height:{canvasCss.h}px"
      onmousemove={onMove}
      onmousedown={onDown}
      onmouseleave={() => send({ cmd: 'mouse_leave' })}
      onwheel={onWheel}
      oncontextmenu={(e) => e.preventDefault()}
      onkeydown={(e) => onKey(e, 'down')}
      onkeyup={(e) => onKey(e, 'up')}
      oncompositionend={onCompositionEnd}
    ></canvas>

    {#if statusText}<div class="status truncate">{statusText}</div>{/if}

    {#if engine === 'missing' || engine === 'stopped'}
      <div class="notice">
        <div class="card">
          <TriangleAlert size={22} />
          <strong>{engine === 'missing' ? `The ${engineName} engine isn’t installed` : `The ${engineName} engine stopped`}</strong>
          <p>{engineError || 'Something went wrong while rendering.'}</p>

          <button class="btn primary sm" onclick={() => start(tab?.url || HOME)}>{engine === 'missing' ? 'Try again' : 'Restart'}</button>
        </div>
      </div>
    {:else if engine === 'running' && !tabs.length}
      <div class="notice">
        <div class="card">
          <Globe size={22} />
          <strong>No open tabs</strong>
          <button class="btn primary sm" onclick={newTab}>New tab</button>
        </div>
      </div>
    {/if}

    {#if dialog}
      <div class="dialog-wrap">
        <form class="dialog" onsubmit={(e) => (e.preventDefault(), answerDialog(true))}>
          <div class="from">{dialog.host} says</div>
          <div class="msg selectable">{dialog.message}</div>
          {#if dialog.kind === 'prompt'}
            <!-- svelte-ignore a11y_autofocus -->
            <input class="input" bind:value={dialog.value} autofocus />
          {/if}
          <div class="row" style="justify-content:flex-end">
            {#if dialog.kind !== 'alert'}<button type="button" class="btn ghost sm" onclick={() => answerDialog(false)}>Cancel</button>{/if}
            <button class="btn primary sm">OK</button>
          </div>
        </form>
      </div>
    {/if}

    {#if select}
      <div class="select-backdrop" role="presentation" onmousedown={() => answerSelect(null)}></div>
      <div class="select-menu" style="left:{select.x}px;top:{select.y}px;min-width:{select.w}px">
        {#each select.options as o}
          {#if 'group' in o}
            <div class="og">{o.group}</div>
          {:else}
            <button class="opt" class:sel={select.selected.includes(o.id)} disabled={o.disabled} onclick={() => answerSelect(o.id)}>{o.label}</button>
          {/if}
        {/each}
      </div>
    {/if}
  </div>

  {#if downloads.length}
    <div class="downloads">
      {#each downloads as d (d.id)}
        <div class="dl">
          <Download size={14} />
          <div class="dl-main">
            <span class="truncate" title={d.path}>{d.name}</span>
            {#if d.state === 'progress' || d.state === 'started'}
              <div class="dl-bar"><div style="width:{d.total > 0 ? Math.round((d.received / d.total) * 100) : 30}%"></div></div>
            {:else}
              <span class="dl-sub">{d.state === 'done' ? formatSize(d.received) : d.state === 'canceled' ? 'Canceled' : 'Failed'}</span>
            {/if}
          </div>
          {#if d.state === 'done'}
            <button class="icon-btn sm" title="Show in folder" onclick={() => rad.fsReveal(d.path)}><FolderOpen size={14} /></button>
            <button class="btn sm" disabled={!!d.vault} onclick={() => saveDownload(d.id)} title="Add this file to your vault (encrypted, indexed for chat)">
              {#if d.vault === 'saving'}<Loader size={13} class="spin" />{:else if d.vault === 'saved'}<Check size={13} />{:else}<FolderInput size={13} />{/if}
              {d.vault === 'saved' ? 'In vault' : 'Save to vault'}
            </button>
          {/if}
          <button class="icon-btn sm" title="Dismiss" onclick={() => (downloads = downloads.filter((x) => x.id !== d.id))}><X size={13} /></button>
        </div>
      {/each}
    </div>
  {/if}

  {#if chatOpen}
    <BrowserChat page={tab} engine={kind} onlink={(href) => go(href)} />
  {/if}
</div>

<style>
  .vpn-note {
    flex: none;
    padding: 6px 14px;
    font-size: 12.5px;
    background: var(--warning-soft);
    color: var(--text-2);
    border-bottom: 1px solid var(--border);
  }
  .tor-note {
    background: var(--accent-soft);
  }
  .browser {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .tabs {
    flex: none;
    display: flex;
    align-items: flex-end;
    gap: 2px;
    height: 40px;
    padding: 6px 10px 0 calc(10px + var(--lead));
    background: var(--bg-sidebar);
    border-bottom: 1px solid var(--border);
    overflow: hidden;
  }
  .tabs > .icon-btn {
    margin: 0 0 5px 4px;
  }
  .tab {
    display: flex;
    align-items: center;
    gap: 7px;
    flex: 0 1 220px;
    min-width: 70px;
    height: 34px;
    padding: 0 6px 0 11px;
    border-radius: var(--r-sm) var(--r-sm) 0 0;
    color: var(--text-3);
    font-size: 12.5px;
    cursor: default;
    transition: background var(--t-fast), color var(--t-fast);
  }
  .tab:hover {
    background: var(--surface-hover);
    color: var(--text-2);
  }
  .tab.active {
    background: var(--surface);
    color: var(--text);
    box-shadow: 0 1px 0 var(--surface), inset 0 0 0 1px var(--border);
    clip-path: inset(-1px -1px 0 -1px);
  }
  .tab :global(svg) {
    flex: none;
  }
  .x {
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    border: none;
    border-radius: 5px;
    background: transparent;
    color: var(--text-3);
    opacity: 0;
  }
  .tab:hover .x,
  .tab.active .x {
    opacity: 1;
  }
  .x:hover {
    background: var(--surface-active);
    color: var(--text);
  }
  .nav {
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
    height: 46px;
    padding: 0 10px;
    border-bottom: 1px solid var(--border);
  }
  .address {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    height: 32px;
    margin: 0 6px;
    padding: 0 6px 0 12px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--surface-2);
    transition: border-color var(--t-fast), box-shadow var(--t-fast);
  }
  .address.focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-ring);
    background: var(--surface);
  }
  .address :global(.lock) {
    color: var(--success);
    flex: none;
  }
  .address :global(.globe) {
    color: var(--text-3);
    flex: none;
  }
  .address input {
    flex: 1;
    min-width: 0;
    border: none;
    outline: none;
    background: transparent;
    color: var(--text);
    font: inherit;
    font-size: 13px;
  }
  .star {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    border: none;
    border-radius: 50%;
    background: transparent;
    color: var(--text-3);
  }
  .star:hover:not(:disabled) {
    background: var(--surface-hover);
    color: var(--text);
  }
  .star.on {
    color: #f5b301;
  }
  .favs {
    flex: none;
    display: flex;
    gap: 2px;
    height: 32px;
    align-items: center;
    padding: 0 10px;
    border-bottom: 1px solid var(--border);
    overflow: hidden;
  }
  .fav {
    display: flex;
    align-items: center;
    gap: 6px;
    max-width: 180px;
    height: 24px;
    padding: 0 9px;
    border: none;
    border-radius: 999px;
    background: transparent;
    color: var(--text-2);
    font-size: 12.5px;
    flex: none;
  }
  .fav:hover {
    background: var(--surface-hover);
    color: var(--text);
  }
  .fav :global(svg) {
    flex: none;
    color: var(--text-3);
  }
  .zoom {
    border: none;
    border-radius: 999px;
    padding: 2px 8px;
    background: var(--accent-soft);
    color: var(--accent);
    font-size: 11.5px;
  }
  .sep {
    width: 1px;
    height: 20px;
    margin: 0 4px;
    background: var(--border);
  }
  .viewport {
    flex: 1;
    min-height: 120px;
    position: relative;
    overflow: hidden;
    background: #fff;
  }
  canvas {
    display: block;
    outline: none;
  }
  .progress {
    position: absolute;
    z-index: 2;
    top: 0;
    left: 0;
    height: 2px;
    width: 40%;
    background: var(--accent);
    animation: slide 1.1s var(--ease) infinite;
  }
  @keyframes slide {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(260%);
    }
  }
  .status {
    position: absolute;
    left: 0;
    bottom: 0;
    max-width: 60%;
    padding: 3px 10px;
    border-top-right-radius: var(--r-sm);
    background: var(--glass);
    border: 1px solid var(--border);
    color: var(--text-2);
    font-size: 12px;
    pointer-events: none;
  }
  .notice {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    background: var(--surface);
  }
  .notice .card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    max-width: 440px;
    padding: 28px 32px;
    text-align: center;
    color: var(--text-2);
  }
  .notice strong {
    color: var(--text);
    font-size: 15px;
  }
  .notice p {
    margin: 0;
    font-size: 13px;
  }
  .dialog-wrap {
    position: absolute;
    inset: 0;
    z-index: 5;
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 40px;
    background: rgba(0, 0, 0, 0.18);
  }
  .dialog {
    display: flex;
    flex-direction: column;
    gap: 12px;
    width: min(440px, 90%);
    padding: 18px;
    border-radius: var(--r-lg);
    background: var(--surface);
    border: 1px solid var(--border);
    box-shadow: var(--shadow-lg);
  }
  .from {
    font-size: 12px;
    color: var(--text-3);
    font-weight: 600;
  }
  .msg {
    white-space: pre-wrap;
    font-size: 13.5px;
    max-height: 40vh;
    overflow: auto;
  }
  .select-backdrop {
    position: absolute;
    inset: 0;
    z-index: 5;
  }
  .select-menu {
    position: absolute;
    z-index: 6;
    max-height: 320px;
    overflow-y: auto;
    padding: 4px;
    border-radius: var(--r-sm);
    background: var(--surface);
    border: 1px solid var(--border-strong);
    box-shadow: var(--shadow-lg);
  }
  .opt {
    display: block;
    width: 100%;
    padding: 5px 10px;
    border: none;
    border-radius: var(--r-xs);
    background: transparent;
    color: var(--text);
    font-size: 13px;
    text-align: left;
  }
  .opt:hover:not(:disabled) {
    background: var(--accent);
    color: var(--on-accent);
  }
  .opt.sel {
    font-weight: 650;
  }
  .opt:disabled {
    color: var(--text-4);
  }
  .og {
    padding: 6px 10px 2px;
    font-size: 11.5px;
    font-weight: 650;
    color: var(--text-3);
  }
  .downloads {
    flex: none;
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 6px 10px;
    border-top: 1px solid var(--border);
    background: var(--surface);
  }
  .dl {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12.5px;
    color: var(--text-2);
  }
  .dl > :global(svg) {
    color: var(--accent);
    flex: none;
  }
  .dl-main {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .dl-sub {
    color: var(--text-3);
    flex: none;
  }
  .dl-bar {
    flex: 1;
    max-width: 220px;
    height: 4px;
    border-radius: 2px;
    background: var(--surface-2);
    overflow: hidden;
  }
  .dl-bar div {
    height: 100%;
    background: var(--accent);
    transition: width 0.2s;
  }
</style>
