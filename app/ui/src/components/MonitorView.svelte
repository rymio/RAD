<script lang="ts">
  // Web Monitoring: the websites the owner watches, with their status, IP addresses and
  // certificates, a thumbnail (click: open in Chromium), SSH to the server and notes for logins.
  // Checking and Gilgamesh's announcements live in lib/monitor.svelte.ts.
  import { onMount } from 'svelte';
  import { ask } from '@tauri-apps/plugin-dialog';
  import { store } from '../lib/state.svelte';
  import { activity } from '../lib/gilgamesh/observe.svelte';
  import { monitor, problems, siteName, hostOf, uptime, CERT_WARN_DAYS, type Site, type SshDetails } from '../lib/monitor.svelte';
  import { timeAgo } from '../lib/util';
  import Modal from './Modal.svelte';
  import { Activity, Plus, RefreshCw, Loader, Globe, Server, Pencil, Trash2, Eye, EyeOff, Copy, KeyRound, ShieldCheck, ShieldAlert, ImageOff, Volume2, VolumeX, ExternalLink, Camera } from '@lucide/svelte';

  let adding = $state('');
  let busy = $state(false);
  let editing = $state<{ id: string; url: string; name: string; notes: string; sshOn: boolean; ssh: { host: string; user: string; port: number; identity_file: string } } | null>(null);
  let editOpen = $state(false);
  let shown = $state<Set<string>>(new Set());

  const m = monitor;
  const online = $derived(m.sites.filter((s) => s.check?.online).length);
  const offline = $derived(m.sites.filter((s) => s.check && !s.check.online).length);
  const certWarnings = $derived(m.sites.filter((s) => { const c = s.check?.certificate; return c && !c.error && (c.days_left! <= CERT_WARN_DAYS || c.problem); }).length);

  // Gilgamesh sees what's on screen here.
  $effect(() => {
    const bad = m.sites.flatMap(problems);
    activity.seen('monitor', m.sites.length ? `${m.sites.length} monitored websites; ${bad.length ? `problems: ${bad.join('; ')}` : 'all fine'}` : 'no websites added yet');
    return () => activity.seen('monitor', null);
  });

  onMount(() => {
    m.load().catch((e) => store.error(e, 'Couldn’t load Web Monitoring'));
  });

  async function add(e: Event) {
    e.preventDefault();
    const url = adding.trim();
    if (!url) return;
    busy = true;
    try {
      adding = '';
      await m.add(url);
    } catch (err) {
      store.error(err, 'Couldn’t add the website');
    } finally {
      busy = false;
    }
  }

  function openChromium(s: Site) {
    store.pendingChromiumUrl = s.check?.final_url || s.url;
    store.navigate({ kind: 'chromium' });
  }

  function ssh(d: SshDetails) {
    store.runAction({ kind: 'ssh', host: d.host, user: d.user, port: d.port, identity_file: d.identity_file });
  }

  function edit(s: Site) {
    editing = {
      id: s.id, url: s.url, name: s.name, notes: s.notes, sshOn: !!s.ssh,
      ssh: { host: s.ssh?.host ?? hostOf(s.url), user: s.ssh?.user ?? '', port: s.ssh?.port ?? 22, identity_file: s.ssh?.identity_file ?? '' },
    };
    editOpen = true;
  }

  async function saveEdit(e: Event) {
    e.preventDefault();
    const x = editing;
    if (!x || !x.url.trim()) return;
    const h = x.ssh;
    const sshd: SshDetails | null = x.sshOn && h.host.trim()
      ? { host: h.host.trim(), user: h.user.trim() || undefined, port: h.port && Number(h.port) !== 22 ? Number(h.port) : undefined, identity_file: h.identity_file.trim() || undefined }
      : null;
    const url = x.url.trim().includes('://') ? x.url.trim() : `https://${x.url.trim()}`;
    editOpen = false;
    await m.update(x.id, { url, name: x.name.trim(), notes: x.notes, ssh: sshd });
  }

  async function pickKey() {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const f = await open({ multiple: false, directory: false, title: 'Choose a private key' });
    if (typeof f === 'string' && editing) editing.ssh.identity_file = f;
  }

  async function remove(s: Site) {
    if (!(await ask(`Stop monitoring ${siteName(s)}? Its SSH details and notes are deleted too.`, { title: 'Remove website', kind: 'warning' }))) return;
    await m.remove(s.id);
  }

  function toggleNotes(id: string) {
    const n = new Set(shown);
    if (n.has(id)) n.delete(id);
    else n.add(id);
    shown = n;
  }

  function copy(text: string) {
    navigator.clipboard.writeText(text).then(() => store.toast('success', 'Copied', undefined, 1500));
  }

  const sshLabel = (d: SshDetails) => `${d.user ? d.user + '@' : ''}${d.host}${d.port ? ':' + d.port : ''}`;
  const day = (ms: number) => new Date(ms).toLocaleDateString('en-GB', { day: 'numeric', month: 'short', year: 'numeric' });
  const at = (ms: number) => new Date(ms).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit' });

  function certState(s: Site): 'ok' | 'warn' | 'bad' | 'none' {
    const c = s.check?.certificate;
    if (!c) return 'none';
    if (c.error || c.days_left! < 0 || c.problem) return 'bad';
    return c.days_left! <= CERT_WARN_DAYS ? 'warn' : 'ok';
  }
</script>

<div class="mon">
  <header class="drag">
    <h1>Web Monitoring</h1>
    {#if m.sites.length}
      <span class="pill success">{online} online</span>
      {#if offline}<span class="pill danger">{offline} offline</span>{/if}
      {#if certWarnings}<span class="pill warning">{certWarnings} certificate{certWarnings === 1 ? '' : 's'}</span>{/if}
    {/if}
    <span class="spacer"></span>
    {#if m.lastRun}<span class="when no-drag" title="Checked when RAD starts and every 3 hours">Checked {timeAgo(m.lastRun)} · next {at(m.nextRun)}</span>{/if}
    <button class="icon-btn no-drag" class:active={m.voice} title={m.voice ? 'Gilgamesh tells you about problems out loud (click to keep quiet)' : 'Gilgamesh keeps quiet (click to have him tell you)'} onclick={() => m.setVoice(!m.voice)}>
      {#if m.voice}<Volume2 size={16} />{:else}<VolumeX size={16} />{/if}
    </button>
    <button class="btn sm no-drag" disabled={!m.sites.length || m.checking.size > 0} onclick={() => m.run('manual')}>
      {#if m.checking.size}<Loader size={13} class="spin" /> Checking…{:else}<RefreshCw size={13} /> Check now{/if}
    </button>
  </header>

  <form class="addbar" onsubmit={add}>
    <Globe size={16} />
    <input class="input" placeholder="Add a website, e.g. https://booksia.com.au" bind:value={adding} spellcheck="false" autocomplete="off" />
    <button class="btn sm primary" disabled={!adding.trim() || busy}>{#if busy}<Loader size={13} class="spin" />{:else}<Plus size={13} />{/if} Add</button>
  </form>

  {#if !m.loaded}
    <div class="center"><Loader size={20} class="spin" /></div>
  {:else if !m.sites.length}
    <div class="center">
      <div class="empty">
        <div class="glyph"><Activity size={28} /></div>
        <h2>Watch your websites</h2>
        <p>Add a website to see whether it’s up, its IP address and when its certificate expires. Gilgamesh checks them when RAD starts and every three hours, and tells you out loud when something is wrong. Keep SSH details and logins with each site.</p>
      </div>
    </div>
  {:else}
    <div class="grid">
      {#each m.sites as s (s.id)}
        {@const c = s.check}
        {@const cs = certState(s)}
        {@const up = uptime(s)}
        <article class="card" class:down={c && !c.online}>
          <button class="thumb" title="Open in Chromium" onclick={() => openChromium(s)}>
            {#if m.thumbs[s.id]}
              <img src={m.thumbs[s.id]} alt={siteName(s)} />
            {:else if m.thumbing.has(s.id)}
              <span class="ph"><Loader size={18} class="spin" /> Taking a picture…</span>
            {:else}
              <span class="ph"><ImageOff size={18} /> No picture yet</span>
            {/if}
            <span class="open"><ExternalLink size={13} /> Open in Chromium</span>
          </button>

          <div class="body">
            <div class="title">
              <span class="dot" data-s={!c ? 'none' : c.online ? 'up' : 'down'}></span>
              <strong class="truncate" title={c?.title ?? ''}>{siteName(s)}</strong>
              {#if m.checking.has(s.id)}<Loader size={13} class="spin" />{:else if c}<span class="pill" class:success={c.online} class:danger={!c.online}>{c.online ? 'Online' : 'Offline'}</span>{/if}
            </div>
            <a class="url truncate" href={s.url} onclick={(e) => (e.preventDefault(), openChromium(s))}>{s.url}</a>

            {#if c}
              <dl>
                <dt>Answer</dt>
                <dd>{#if c.status}HTTP {c.status}{#if c.ms} · {c.ms} ms{/if}{:else}—{/if}{#if c.final_url && hostOf(c.final_url) !== hostOf(s.url)} · goes to {hostOf(c.final_url)}{/if}</dd>
                <dt>IP</dt>
                <dd class="ips">{#each c.ips as ip}<button class="ip" title="Copy" onclick={() => copy(ip)}>{ip}</button>{:else}—{/each}</dd>
                <dt>Certificate</dt>
                <dd class="cert" data-s={cs}>
                  {#if !c.certificate}No HTTPS
                  {:else if c.certificate.error}<ShieldAlert size={13} /> {c.certificate.error}
                  {:else}
                    {#if cs === 'ok'}<ShieldCheck size={13} />{:else}<ShieldAlert size={13} />{/if}
                    {c.certificate.days_left! < 0 ? 'Expired' : 'Expires'} {day(c.certificate.expires!)}
                    <span class="days">({c.certificate.days_left! < 0 ? `${-c.certificate.days_left!} day${c.certificate.days_left === -1 ? '' : 's'} ago` : c.certificate.days_left === 0 ? 'today' : `${c.certificate.days_left} days`})</span>
                    {#if c.certificate.problem && c.certificate.days_left! >= 0}<div class="problem">{c.certificate.problem}</div>{/if}
                    <div class="sub">{c.certificate.issuer}{#if c.certificate.protocol} · {c.certificate.protocol}{/if}</div>
                  {/if}
                </dd>
                {#if c.error && c.online}<dt></dt><dd class="sub">{c.error}</dd>{/if}
                {#if !c.online && c.error}<dt>Problem</dt><dd class="problem">{c.error}</dd>{/if}
              </dl>
              <div class="hist" title={up !== null ? `Up ${up}% of the last ${s.history.length} checks` : ''}>
                {#each s.history as h}<i class:off={!h.online} title="{new Date(h.at).toLocaleString('en-GB')}: {h.online ? `online${h.ms ? `, ${h.ms} ms` : ''}` : 'offline'}"></i>{/each}
                <span class="sub">{up}% · checked {timeAgo(c.checked_at)}</span>
              </div>
            {:else if !m.checking.has(s.id)}
              <p class="sub">Not checked yet.</p>
            {/if}

            {#if s.notes}
              <div class="notes">
                <div class="nh">
                  <KeyRound size={13} /> <span class="spacer">Notes</span>
                  <button class="icon-btn sm" title={shown.has(s.id) ? 'Hide' : 'Show'} onclick={() => toggleNotes(s.id)}>{#if shown.has(s.id)}<EyeOff size={13} />{:else}<Eye size={13} />{/if}</button>
                  <button class="icon-btn sm" title="Copy" onclick={() => copy(s.notes)}><Copy size={13} /></button>
                </div>
                <pre class="selectable" class:hidden={!shown.has(s.id)}>{s.notes}</pre>
              </div>
            {/if}

            <div class="acts">
              {#if s.ssh}
                <button class="btn sm" title="Open an SSH connection in the Terminal" onclick={() => ssh(s.ssh!)}><Server size={13} /> {sshLabel(s.ssh)}</button>
              {/if}
              <span class="spacer"></span>
              <button class="icon-btn sm" title="Check now" disabled={m.checking.has(s.id)} onclick={() => m.check(s.id)}><RefreshCw size={14} /></button>
              <button class="icon-btn sm" title="Take a new picture" disabled={m.thumbing.has(s.id)} onclick={() => m.thumb(s.id)}><Camera size={14} /></button>
              <button class="icon-btn sm" title="Edit: name, address, SSH, notes" onclick={() => edit(s)}><Pencil size={14} /></button>
              <button class="icon-btn sm" title="Remove" onclick={() => remove(s)}><Trash2 size={14} /></button>
            </div>
          </div>
        </article>
      {/each}
    </div>
  {/if}
</div>

<Modal bind:open={editOpen} title={editing ? `Edit ${editing.name || hostOf(editing.url)}` : ''} width={500}>
  {#if editing}
    <form onsubmit={saveEdit}>
      <div class="field">
        <label class="label" for="mu">Address</label>
        <input id="mu" class="input" bind:value={editing.url} spellcheck="false" autocomplete="off" />
      </div>
      <div class="field">
        <label class="label" for="mn">Name <span class="muted">(optional)</span></label>
        <input id="mn" class="input" bind:value={editing.name} placeholder={hostOf(editing.url)} />
      </div>
      <label class="check">
        <input type="checkbox" bind:checked={editing.sshOn} />
        <span>SSH to the server <small>A button on the card opens the connection in the Terminal.</small></span>
      </label>
      {#if editing.sshOn}
        <div class="row" style="gap:10px">
          <div class="field" style="flex:1">
            <label class="label" for="mh">Host</label>
            <input id="mh" class="input" bind:value={editing.ssh.host} spellcheck="false" autocomplete="off" />
          </div>
          <div class="field" style="width:120px">
            <label class="label" for="mus">User</label>
            <input id="mus" class="input" bind:value={editing.ssh.user} placeholder="default" spellcheck="false" autocomplete="off" />
          </div>
          <div class="field" style="width:80px">
            <label class="label" for="mp">Port</label>
            <input id="mp" class="input" type="number" min="1" max="65535" bind:value={editing.ssh.port} />
          </div>
        </div>
        <div class="field">
          <label class="label" for="mk">Private key <span class="muted">(optional)</span></label>
          <div class="row" style="gap:8px">
            <input id="mk" class="input" style="flex:1" bind:value={editing.ssh.identity_file} placeholder="~/.ssh/id_ed25519" spellcheck="false" />
            <button type="button" class="btn sm" onclick={pickKey}>Choose…</button>
          </div>
        </div>
      {/if}
      <div class="field">
        <label class="label" for="mo">Notes <span class="muted">(logins, hosting, anything)</span></label>
        <textarea id="mo" class="input notes-input" rows="6" bind:value={editing.notes} spellcheck="false" placeholder={'Admin: https://booksia.com.au/wp-admin\nUser: …\nPassword: …'}></textarea>
        <div class="hint">Kept in your encrypted vault, hidden on the card until you show them, and left out of search and chat answers.</div>
      </div>
      <div class="row" style="justify-content:flex-end; gap:8px">
        <button type="button" class="btn ghost" onclick={() => (editOpen = false)}>Cancel</button>
        <button class="btn primary" disabled={!editing.url.trim()}>Save</button>
      </div>
    </form>
  {/if}
</Modal>

<style>
  .mon {
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
    margin: 0 4px 0 0;
    font-size: 15px;
    font-weight: 650;
  }
  .when {
    font-size: 12px;
    color: var(--text-3);
  }
  .addbar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 16px;
    color: var(--text-3);
    border-bottom: 1px solid var(--border);
  }
  .addbar .input {
    flex: 1;
    max-width: 560px;
  }
  .center {
    flex: 1;
    display: grid;
    place-items: center;
    padding: 24px;
  }
  .empty {
    max-width: 440px;
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
  .grid {
    flex: 1;
    overflow: auto;
    padding: 16px;
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
    gap: 14px;
    align-content: start;
  }
  .card {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--r-lg);
    background: var(--surface);
    overflow: hidden;
    box-shadow: var(--shadow-sm);
  }
  .card.down {
    border-color: color-mix(in oklab, var(--danger) 45%, var(--border));
  }
  .thumb {
    position: relative;
    aspect-ratio: 16 / 10;
    padding: 0;
    border: none;
    border-bottom: 1px solid var(--border);
    background: var(--surface-3);
    cursor: pointer;
    overflow: hidden;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    object-position: top;
    display: block;
  }
  .ph {
    position: absolute;
    inset: 0;
    display: flex;
    gap: 8px;
    align-items: center;
    justify-content: center;
    font-size: 12.5px;
    color: var(--text-3);
  }
  .open {
    position: absolute;
    right: 8px;
    bottom: 8px;
    display: inline-flex;
    gap: 5px;
    align-items: center;
    padding: 4px 9px;
    border-radius: 99px;
    font-size: 11.5px;
    background: var(--glass);
    color: var(--text);
    opacity: 0;
    transition: opacity var(--t);
  }
  .thumb:hover .open,
  .thumb:focus-visible .open {
    opacity: 1;
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 12px 14px 10px;
  }
  .title {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 14px;
  }
  .title strong {
    flex: 1;
    min-width: 0;
  }
  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    flex: none;
    background: var(--text-4);
  }
  .dot[data-s='up'] {
    background: var(--success);
  }
  .dot[data-s='down'] {
    background: var(--danger);
    animation: pulse 1.2s infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
  .url {
    font-size: 12px;
    color: var(--text-3);
    text-decoration: none;
  }
  .url:hover {
    color: var(--accent);
  }
  dl {
    display: grid;
    grid-template-columns: 78px 1fr;
    gap: 5px 8px;
    margin: 4px 0 0;
    font-size: 12.5px;
  }
  dt {
    color: var(--text-3);
  }
  dd {
    margin: 0;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .ips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }
  .ip {
    border: 1px solid var(--border);
    background: var(--surface-2);
    border-radius: var(--r-xs);
    padding: 0 6px;
    font: inherit;
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--text);
    cursor: copy;
  }
  .cert :global(svg) {
    vertical-align: -2px;
  }
  .cert[data-s='ok'] :global(svg) {
    color: var(--success);
  }
  .cert[data-s='warn'] {
    color: var(--warning);
  }
  .cert[data-s='bad'] {
    color: var(--danger);
  }
  .days {
    color: inherit;
    opacity: 0.85;
  }
  .problem {
    color: var(--danger);
  }
  .sub {
    font-size: 11.5px;
    color: var(--text-3);
  }
  .hist {
    display: flex;
    align-items: center;
    gap: 2px;
    margin-top: 2px;
  }
  .hist i {
    width: 5px;
    height: 14px;
    border-radius: 1.5px;
    background: var(--success);
    opacity: 0.8;
  }
  .hist i.off {
    background: var(--danger);
  }
  .hist .sub {
    margin-left: 6px;
  }
  .notes {
    border: 1px solid var(--border);
    border-radius: var(--r-sm);
    background: var(--surface-2);
    padding: 4px 8px 6px;
  }
  .nh {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-2);
  }
  .notes pre {
    margin: 4px 0 0;
    font-family: var(--mono);
    font-size: 11.5px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    max-height: 160px;
    overflow: auto;
  }
  .notes pre.hidden {
    filter: blur(5px);
    user-select: none;
    pointer-events: none;
    max-height: 3.2em;
    overflow: hidden;
  }
  .acts {
    display: flex;
    align-items: center;
    gap: 4px;
    margin-top: 4px;
  }
  .check {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    font-size: 13px;
    margin: 4px 0 10px;
    cursor: pointer;
  }
  .check span {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .check small {
    color: var(--text-3);
    font-size: 12px;
  }
  .notes-input {
    font-family: var(--mono);
    font-size: 12px;
    resize: vertical;
  }
  .hint {
    font-size: 12px;
    color: var(--text-3);
    margin-top: 4px;
  }
</style>
