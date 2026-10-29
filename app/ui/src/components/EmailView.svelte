<script lang="ts">
  import { onMount } from 'svelte';
  import DOMPurify from 'dompurify';
  import { rad } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import { formatBytes } from '../lib/util';
  import type { MailFolder, MailMessage, MailSummary } from '../lib/types';
  import EmailChat from './EmailChat.svelte';
  import { activity } from '../lib/gilgamesh/observe.svelte';
  import {
    Mail, Inbox, Send, FileEdit, Archive, ShieldAlert, Trash2, Star, Folder, Search, RefreshCw, Settings, Loader, Paperclip, MailOpen, X, ImageOff, FileText, Code,
  } from '@lucide/svelte';

  const ICONS: Record<string, any> = { inbox: Inbox, sent: Send, drafts: FileEdit, archive: Archive, all: Archive, junk: ShieldAlert, trash: Trash2, flagged: Star };

  const account = $derived(store.emailAccount ?? null);
  let hasPassword = $state<boolean | null>(null);
  let folders = $state<MailFolder[]>([]);
  let folder = $state('INBOX');
  let list = $state<MailSummary[]>([]);
  let total = $state(0);
  let next = $state<number | null>(null);
  let loading = $state(false);
  let more = $state(false);
  let error = $state('');
  let query = $state('');
  let activeQuery = $state('');
  let openUid = $state<number | null>(null);
  let open = $state<MailMessage | null>(null);
  let reading = $state(false);
  let readError = $state('');
  let plain = $state(false);
  let blocked = $state(0);
  let host: HTMLDivElement | undefined = $state();

  const unread = $derived(list.filter((m) => !m.seen).length);
  // Gilgamesh sees the folder and the email being read (not its text).
  $effect(() => {
    activity.seen('email', `in the ${folder} folder (${total} emails, ${unread} unread on screen)${open ? `; reading “${open.subject}” from ${open.from}` : ''}`);
    return () => activity.seen('email', null);
  });
  const folderLabel = $derived(folders.find((f) => f.name === folder)?.label ?? 'Inbox');

  onMount(() => {
    if (store.emailAccount === undefined) store.loadAppData('email_account');
    // Pick up new mail every few minutes while the view is open.
    const t = setInterval(() => !activeQuery && !loading && account && hasPassword && refresh(true), 3 * 60_000);
    return () => clearInterval(t);
  });

  // (Re)connect when the account changes.
  let connectedFor = '';
  $effect(() => {
    const a = account;
    const k = a ? `${a.username}@${a.host}:${a.port}` : '';
    if (k === connectedFor) return;
    connectedFor = k;
    folders = [];
    list = [];
    open = null;
    openUid = null;
    if (!a) return;
    rad.mailHasPassword(a).then((ok) => {
      hasPassword = ok;
      if (!ok) return;
      rad.mailFolders(a).then((f) => (folders = f)).catch((e) => (error = e.message));
      refresh();
    });
  });

  async function refresh(quiet = false) {
    if (!account) return;
    if (!quiet) loading = true;
    error = '';
    try {
      const p = await rad.mailList(account, folder, activeQuery || null, null);
      if (quiet && list.length) {
        // Keep the older pages already loaded.
        const oldest = Math.min(...p.messages.map((m) => m.uid));
        list = [...p.messages, ...list.filter((m) => m.uid < oldest)];
      } else {
        list = p.messages;
        next = p.next;
      }
      total = p.total;
    } catch (e: any) {
      if (!quiet) error = e.message ?? String(e);
    } finally {
      loading = false;
    }
  }

  async function loadMore() {
    if (!account || next == null || more) return;
    more = true;
    try {
      const p = await rad.mailList(account, folder, activeQuery || null, next);
      const have = new Set(list.map((m) => m.uid));
      list = [...list, ...p.messages.filter((m) => !have.has(m.uid))];
      next = p.next;
    } catch (e) {
      store.error(e, 'Couldn’t load more mail');
    } finally {
      more = false;
    }
  }

  function pickFolder(name: string) {
    folder = name;
    query = activeQuery = '';
    open = null;
    openUid = null;
    refresh();
  }

  function search(e: Event) {
    e.preventDefault();
    activeQuery = query.trim();
    refresh();
  }
  function clearSearch() {
    query = activeQuery = '';
    refresh();
  }

  async function read(m: MailSummary) {
    if (!account) return;
    openUid = m.uid;
    reading = true;
    readError = '';
    blocked = 0;
    try {
      const msg = await rad.mailRead(account, folder, m.uid);
      if (openUid !== m.uid) return;
      open = msg;
      plain = !msg.html;
      // Opening marks it read on the server.
      list = list.map((x) => (x.uid === m.uid ? { ...x, seen: true } : x));
    } catch (e: any) {
      if (openUid === m.uid) readError = e.message ?? String(e);
    } finally {
      if (openUid === m.uid) reading = false;
    }
  }

  async function setFlag(flags: { seen?: boolean; flagged?: boolean }) {
    if (!account || !open) return;
    const uid = open.uid;
    try {
      await rad.mailSetFlags(account, folder, uid, flags);
      list = list.map((x) => (x.uid === uid ? { ...x, ...flags } : x));
      if (flags.flagged !== undefined) open = { ...open, flagged: flags.flagged };
      if (flags.seen === false) {
        open = null;
        openUid = null;
      }
    } catch (e) {
      store.error(e, 'Couldn’t update the message');
    }
  }

  // ---- rendering the message body, isolated from the app's styles and scripts

  function sanitize(html: string) {
    const clean = DOMPurify.sanitize(html, {
      WHOLE_DOCUMENT: false,
      // Keep a leading style block (common in HTML mail); it only applies inside the shadow root.
      FORCE_BODY: true,
      FORBID_TAGS: ['form', 'input', 'button', 'textarea', 'select', 'link', 'meta', 'base', 'iframe', 'frame', 'object', 'embed', 'audio', 'video', 'svg'],
      FORBID_ATTR: ['srcset', 'action', 'formaction', 'ping'],
      RETURN_DOM_FRAGMENT: true,
    }) as DocumentFragment;
    // Remote pictures are often trackers; nothing is loaded from the internet.
    let n = 0;
    clean.querySelectorAll('img').forEach((img) => {
      const src = img.getAttribute('src') ?? '';
      if (!src.startsWith('data:')) {
        n++;
        const w = Number(img.getAttribute('width')) || 0;
        const h = Number(img.getAttribute('height')) || 0;
        if ((w && w <= 2) || (h && h <= 2)) img.remove();
        else {
          img.removeAttribute('src');
          img.setAttribute('data-blocked', '');
        }
      }
    });
    clean.querySelectorAll('[background]').forEach((el) => el.removeAttribute('background'));
    blocked = n;
    return clean;
  }

  const SHADOW_CSS = `
    :host { all: initial; display: block; }
    .paper { contain: layout paint; position: relative; overflow-x: auto; background: #fff; color: #1a1a1a; padding: 18px 20px; font: 14px/1.55 -apple-system, BlinkMacSystemFont, "Segoe UI", Inter, sans-serif; overflow-wrap: anywhere; border-radius: 10px; }
    img { max-width: 100%; height: auto; }
    img[data-blocked] { display: inline-block; min-width: 24px; min-height: 16px; background: #f1f1f3; border: 1px dashed #d4d4d8; }
    table { max-width: 100% !important; }
    a { color: #2563eb; cursor: pointer; }
    pre { white-space: pre-wrap; }
    blockquote { margin: 0 0 0 4px; padding-left: 12px; border-left: 3px solid #d4d4d8; color: #52525b; }
  `;

  $effect(() => {
    if (!host || !open) return;
    const root = host.shadowRoot ?? host.attachShadow({ mode: 'open' });
    root.replaceChildren();
    const style = document.createElement('style');
    style.textContent = SHADOW_CSS;
    const paper = document.createElement('div');
    paper.className = 'paper';
    if (!plain && open.html) paper.append(sanitize(open.html));
    else {
      blocked = 0;
      const pre = document.createElement('div');
      pre.style.whiteSpace = 'pre-wrap';
      pre.textContent = open.text || '(This message has no text.)';
      paper.append(pre);
    }
    root.append(style, paper);
  });

  function onBodyClick(e: MouseEvent) {
    const a = (e.composedPath() as HTMLElement[]).find((el) => el instanceof HTMLAnchorElement) as HTMLAnchorElement | undefined;
    if (!a) return;
    e.preventDefault();
    const href = a.getAttribute('href') ?? '';
    if (/^(https?:|mailto:)/i.test(href)) import('@tauri-apps/plugin-opener').then((m) => m.openUrl(href));
  }

  function when(ms: number) {
    if (!ms) return '';
    const d = new Date(ms);
    const now = new Date();
    if (d.toDateString() === now.toDateString()) return d.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
    if (now.getTime() - d.getTime() < 6 * 864e5) return d.toLocaleDateString(undefined, { weekday: 'short' });
    return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', year: d.getFullYear() === now.getFullYear() ? undefined : 'numeric' });
  }

  function onKey(e: KeyboardEvent) {
    if (!list.length || (e.target as HTMLElement)?.closest('input, textarea, select, [contenteditable]')) return;
    if (e.metaKey || e.ctrlKey || e.altKey) return;
    if (e.key === 'j' || e.key === 'k') {
      const i = list.findIndex((m) => m.uid === openUid);
      const n = list[Math.max(0, Math.min(list.length - 1, i + (e.key === 'j' ? 1 : -1)))];
      if (n && n.uid !== openUid) read(n);
    } else if (e.key === 'u' && open) setFlag({ seen: false });
    else if (e.key === 's' && open) setFlag({ flagged: !open.flagged });
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="email">
  <header class="drag">
    <div class="title no-drag">
      <Mail size={16} />
      {#if folders.length}
        <select class="select fsel" value={folder} onchange={(e) => pickFolder(e.currentTarget.value)} title="Folder">
          {#each folders as f}<option value={f.name}>{f.label}</option>{/each}
        </select>
      {:else}
        <strong>{folderLabel}</strong>
      {/if}
      {#if account && unread}<span class="pill accent">{unread} unread</span>{/if}
    </div>
    <span class="spacer"></span>
    {#if account && hasPassword}
      <form class="find no-drag" onsubmit={search}>
        <Search size={14} />
        <input placeholder="Search mail" bind:value={query} spellcheck="false" />
        {#if activeQuery}<button type="button" class="icon-btn sm" title="Clear search" onclick={clearSearch}><X size={13} /></button>{/if}
      </form>
      <button class="icon-btn no-drag" title="Refresh" onclick={() => refresh()} disabled={loading}><RefreshCw size={15} class={loading ? 'spin' : ''} /></button>
    {/if}
    <button class="icon-btn no-drag" title="Email settings" onclick={() => store.navigate({ kind: 'settings', tab: 'email' })}><Settings size={16} /></button>
  </header>

  {#if store.emailAccount === undefined}
    <div class="center"><Loader size={18} class="spin" /></div>
  {:else if !account || hasPassword === false}
    <div class="center">
      <div class="setup">
        <Mail size={36} strokeWidth={1.3} />
        <h2>{account ? 'Enter your email password' : 'Read your email in RAD'}</h2>
        <p>
          {account
            ? `This computer doesn’t have the password for ${account.email} yet.`
            : 'Connect Gmail, iCloud, Outlook or any IMAP account. Mail stays with your provider — RAD only shows it, and the AI can summarize it into notes or answer questions about it.'}
        </p>
        <button class="btn primary" onclick={() => store.navigate({ kind: 'settings', tab: 'email' })}><Settings size={14} /> {account ? 'Open email settings' : 'Set up email'}</button>
      </div>
    </div>
  {:else}
    <div class="main">
      <div class="list">
        {#if activeQuery}
          <div class="sinfo">{total} result{total === 1 ? '' : 's'} for “{activeQuery}” in {folderLabel}</div>
        {/if}
        {#if error}
          <div class="err">{error}<button class="btn sm" onclick={() => refresh()}>Try again</button></div>
        {:else if loading && !list.length}
          <div class="err"><Loader size={16} class="spin" /></div>
        {:else}
          {#each list as m (m.uid)}
            <button class="item" class:unread={!m.seen} class:sel={m.uid === openUid} onclick={() => read(m)}>
              <span class="dot"></span>
              <span class="lines">
                <span class="l1">
                  <span class="from truncate">{m.from_name || m.from_addr || '(unknown sender)'}</span>
                  {#if m.attachments}<Paperclip size={12} />{/if}
                  {#if m.flagged}<Star size={12} class="star" />{/if}
                  <span class="date">{when(m.date)}</span>
                </span>
                <span class="subj truncate">{m.subject}</span>
              </span>
            </button>
          {:else}
            <div class="err muted">{activeQuery ? 'No messages match.' : 'This folder is empty.'}</div>
          {/each}
          {#if next != null}
            <button class="btn sm ghost more" onclick={loadMore} disabled={more}>{#if more}<Loader size={13} class="spin" />{/if} Load older messages</button>
          {/if}
        {/if}
      </div>

      <section class="reader">
        {#if readError}
          <div class="center"><div class="err">{readError}</div></div>
        {:else if open}
          <div class="scroll">
            <div class="mhead">
              <h1 class="selectable">{open.subject}</h1>
              <div class="meta">
                <div class="who selectable">
                  <div><span class="k">From</span> {open.from}</div>
                  {#if open.to}<div><span class="k">To</span> {open.to}</div>{/if}
                  {#if open.cc}<div><span class="k">Cc</span> {open.cc}</div>{/if}
                  <div class="muted">{open.date ? new Date(open.date).toLocaleString(undefined, { dateStyle: 'full', timeStyle: 'short' }) : ''}</div>
                </div>
                <div class="acts">
                  <button class="icon-btn" title={open.flagged ? 'Unflag (S)' : 'Flag (S)'} class:active={open.flagged} onclick={() => setFlag({ flagged: !open!.flagged })}><Star size={16} /></button>
                  <button class="icon-btn" title="Mark as unread (U)" onclick={() => setFlag({ seen: false })}><MailOpen size={16} /></button>
                  {#if open.html}
                    <button class="icon-btn" title={plain ? 'Show formatted' : 'Show plain text'} onclick={() => (plain = !plain)}>{#if plain}<Code size={16} />{:else}<FileText size={16} />{/if}</button>
                  {/if}
                </div>
              </div>
              {#if open.attachments.length}
                <div class="atts">
                  {#each open.attachments as a}<span class="att" title={a.mime}><Paperclip size={12} /> {a.name} <span class="muted">{formatBytes(a.size)}</span></span>{/each}
                </div>
              {/if}
              {#if blocked && !plain}
                <div class="blocked"><ImageOff size={13} /> {blocked} picture{blocked === 1 ? '' : 's'} from the internet blocked for privacy.</div>
              {/if}
            </div>
            <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
            <div class="body" bind:this={host} onclick={onBodyClick}></div>
          </div>
          {#key `${folder}:${open.uid}`}<EmailChat message={open} {folder} />{/key}
        {:else if reading}
          <div class="center"><Loader size={18} class="spin" /></div>
        {:else}
          <div class="center muted">
            <div class="setup">
              <Inbox size={30} strokeWidth={1.3} />
              <p>Select a message to read it. Opening it marks it as read.<br /><span class="kbd">J</span> / <span class="kbd">K</span> next / previous · <span class="kbd">U</span> unread · <span class="kbd">S</span> flag</p>
            </div>
          </div>
        {/if}
      </section>
    </div>
  {/if}
</div>

<style>
  .email {
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
  .title {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .title :global(svg) {
    color: var(--accent);
    flex: none;
  }
  .fsel {
    width: auto;
    max-width: 240px;
    height: 30px;
    font-weight: 600;
  }
  .find {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 30px;
    padding: 0 6px 0 10px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--surface-2);
    color: var(--text-3);
  }
  .find input {
    width: 180px;
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
  .list {
    flex: none;
    width: 340px;
    overflow-y: auto;
    border-right: 1px solid var(--border);
    padding: 6px;
  }
  .sinfo {
    padding: 6px 10px 8px;
    font-size: 12px;
    color: var(--text-3);
  }
  .item {
    display: flex;
    gap: 8px;
    width: 100%;
    padding: 8px 10px 9px 6px;
    border: none;
    border-radius: var(--r-sm);
    background: transparent;
    color: var(--text-2);
    text-align: left;
  }
  .item:hover {
    background: var(--surface-hover);
  }
  .item.sel {
    background: var(--surface-active);
  }
  .dot {
    flex: none;
    width: 7px;
    height: 7px;
    margin-top: 6px;
    border-radius: 50%;
  }
  .item.unread .dot {
    background: var(--accent);
  }
  .lines {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .l1 {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text-3);
  }
  .from {
    flex: 1;
    min-width: 0;
    font-size: 13.5px;
    color: var(--text-2);
  }
  .item.unread .from {
    color: var(--text);
    font-weight: 650;
  }
  .item.unread .subj {
    color: var(--text);
    font-weight: 550;
  }
  .l1 :global(.star) {
    color: var(--warning, #f5a524);
    fill: currentColor;
  }
  .date {
    flex: none;
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
  }
  .subj {
    font-size: 12.5px;
  }
  .more {
    width: 100%;
    margin-top: 4px;
    justify-content: center;
  }
  .err {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    padding: 30px 16px;
    color: var(--text-3);
    font-size: 13px;
    text-align: center;
  }
  .reader {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 18px 22px 24px;
  }
  .mhead h1 {
    margin: 0 0 12px;
    font-size: 19px;
    font-weight: 650;
    line-height: 1.3;
  }
  .meta {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    margin-bottom: 12px;
  }
  .who {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: 13px;
    color: var(--text-2);
    overflow-wrap: anywhere;
  }
  .k {
    display: inline-block;
    width: 38px;
    color: var(--text-4);
  }
  .acts {
    display: flex;
    gap: 2px;
  }
  .acts :global(.icon-btn.active svg) {
    color: var(--warning, #f5a524);
    fill: currentColor;
  }
  .atts {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: 12px;
  }
  .att {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 3px 9px;
    border: 1px solid var(--border);
    border-radius: 999px;
    font-size: 12px;
    color: var(--text-2);
  }
  .blocked {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 10px;
    font-size: 12px;
    color: var(--text-3);
  }
  .body {
    border: 1px solid var(--border);
    border-radius: 10px;
  }
  .center {
    flex: 1;
    min-height: 0;
    display: grid;
    place-items: center;
  }
  .setup {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    max-width: 420px;
    padding: 20px;
    text-align: center;
    color: var(--text-3);
  }
  .setup :global(svg) {
    color: var(--accent);
  }
  .setup h2 {
    margin: 4px 0 0;
    font-size: 17px;
    color: var(--text);
  }
  .setup p {
    margin: 0 0 6px;
    font-size: 13.5px;
    line-height: 1.55;
  }
</style>
