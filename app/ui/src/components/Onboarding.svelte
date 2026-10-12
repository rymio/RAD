<script lang="ts">
  import { store } from '../lib/state.svelte';
  import { rad } from '../lib/api';
  import { passphraseStrength } from '../lib/util';
  import Logo from './Logo.svelte';
  import RecoveryKit from './RecoveryKit.svelte';
  import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';
  import { documentDir, join } from '@tauri-apps/api/path';
  import { ArrowRight, Server, KeyRound, Lock, Loader, ChevronLeft, Eye, EyeOff, Laptop, FilePlus2, FolderOpen, Clock } from '@lucide/svelte';

  let url = $state(store.config?.server_url ?? 'http://');
  let code = $state('');
  let deviceName = $state(store.config?.device_name ?? '');
  let pass = $state('');
  let pass2 = $state('');
  let show = $state(false);
  let busy = $state(false);
  let err = $state('');
  let confirmed = $state(false);
  // Standalone mode is chosen here; server mode needs an address first.
  let choice = $state<'pick' | 'server'>(store.config?.server_url ? 'server' : 'pick');

  const strength = $derived(passphraseStrength(pass));
  const steps = $derived(store.isLocal ? (['connect', 'create', 'recovery-kit'] as const) : (['connect', 'pair', 'create', 'recovery-kit'] as const));
  const stepLabels = $derived(store.isLocal ? ['Vault file', 'Passphrase', 'Recovery'] : ['Location', 'Pair', 'Vault', 'Recovery']);
  const stepIndex = $derived(Math.max(0, (steps as readonly string[]).indexOf(store.phase)));

  async function run(fn: () => Promise<void>) {
    busy = true;
    err = '';
    try {
      await fn();
    } catch (e: any) {
      err = e?.message ?? String(e);
    } finally {
      busy = false;
    }
  }

  async function openLocal(path: string) {
    await run(async () => {
      store.status = await rad.startLocal(path);
      store.config = await rad.config();
      store.decidePhase();
    });
  }

  async function createLocal() {
    let def = 'Personal.radvault';
    try {
      def = await join(await documentDir(), 'RAD', 'Personal.radvault');
    } catch {}
    const path = await saveDialog({ title: 'Create a new RAD vault', defaultPath: def, filters: [{ name: 'RAD vault', extensions: ['radvault'] }] });
    if (path) await openLocal(path);
  }

  async function openExisting() {
    const path = await openDialog({ title: 'Open a RAD vault', multiple: false, filters: [{ name: 'RAD vault', extensions: ['radvault'] }] });
    if (typeof path === 'string') await openLocal(path);
  }

  const recent = $derived(store.config?.recent_vaults ?? []);

  const connect = () =>
    run(async () => {
      store.status = await rad.connect(url);
      store.config = await rad.config();
      store.decidePhase();
    });

  const pair = () =>
    run(async () => {
      store.status = await rad.pair(code, deviceName);
      store.config = await rad.config();
      store.decidePhase();
    });

  const create = () =>
    run(async () => {
      if (pass.length < 10) throw new Error('Use at least 10 characters — a short sentence works great.');
      if (pass !== pass2) throw new Error('The passphrases don’t match.');
      store.pendingRecoveryKey = await rad.initVault(pass);
      pass = pass2 = '';
      store.phase = 'recovery-kit';
    });

  async function finish() {
    store.pendingRecoveryKey = null;
    store.status = await rad.status();
    await store.enterApp();
  }

  function formatCode(v: string) {
    const raw = v.toUpperCase().replace(/[^A-Z0-9]/g, '').slice(0, 8);
    return raw.length > 4 ? `${raw.slice(0, 4)}-${raw.slice(4)}` : raw;
  }
</script>

<div class="wrap drag">
  <div class="aura"></div>
  <div class="card no-drag fade-up">
    <div class="brand">
      <Logo size={40} />
      <div>
        <div class="name">RAD</div>
        <div class="tag">Your private, encrypted second brain</div>
      </div>
    </div>

    <div class="steps">
      {#each stepLabels as s, i}
        <div class="step" class:done={i < stepIndex} class:active={i === stepIndex}><span>{i + 1}</span>{s}</div>
      {/each}
    </div>

    {#if store.phase === 'connect' && choice === 'pick'}
      <h1>Where should your vault live?</h1>
      <p class="lead">Everything RAD knows is kept in one encrypted vault file. Keep it on this computer, or on a RAD server so all your devices share it.</p>
      <div class="modes">
        <div class="mode">
          <div class="mi"><Laptop size={20} /></div>
          <div class="mt">Standalone — this computer</div>
          <div class="md">No server needed. The vault is a file you choose (local disk, external or USB drive). All features work offline with local models.</div>
          <div class="row">
            <button class="btn primary" disabled={busy} onclick={createLocal}><FilePlus2 size={15} /> Create vault…</button>
            <button class="btn" disabled={busy} onclick={openExisting}><FolderOpen size={15} /> Open…</button>
          </div>
        </div>
        <button class="mode click" onclick={() => (choice = 'server')}>
          <div class="mi"><Server size={20} /></div>
          <div class="mt">Connect to a RAD server</div>
          <div class="md">The vault lives on your home server, NAS or VPS. Mac, Windows and Linux all see the same data, live.</div>
          <span class="go">Continue <ArrowRight size={14} /></span>
        </button>
      </div>
      {#if recent.length}
        <div class="recent">
          <div class="section-title">Recent vaults</div>
          {#each recent as r}
            <button class="rv" disabled={busy} onclick={() => openLocal(r)} title={r}><Clock size={13} /><span class="truncate">{r}</span></button>
          {/each}
        </div>
      {/if}
      {#if err}<div class="err shake" style="margin-top:14px">{err}</div>{/if}
      {#if busy}<div class="muted" style="margin-top:12px;display:flex;gap:8px;align-items:center"><Loader size={14} class="spin" /> Opening vault…</div>{/if}
    {:else if store.phase === 'connect'}
      <button class="back btn ghost sm" onclick={() => (choice = 'pick')}><ChevronLeft size={14} /> Back</button>
      <h1>Connect to your RAD server</h1>
      <p class="lead">Your vault lives on a server you own — at home, on a NAS, or reachable over Tailscale — so every computer sees the same data.</p>
      <form onsubmit={(e) => (e.preventDefault(), connect())}>
        <div class="field">
          <label class="label" for="url">Server address</label>
          <div class="input-ic">
            <Server size={16} />
            <input id="url" class="input lg" bind:value={url} placeholder="http://192.168.1.20:8765" autocomplete="off" spellcheck="false" />
          </div>
          <div class="hint">Start the server with <code>docker compose up -d</code> or <code>rad-server</code> — it prints its address and a pairing code.</div>
        </div>
        {#if err}<div class="err shake">{err}</div>{/if}
        <button class="btn primary lg block" disabled={busy || url.length < 8}>
          {#if busy}<Loader size={16} class="spin" />{/if} Connect <ArrowRight size={16} />
        </button>
      </form>
    {:else if store.phase === 'pair'}
      <button class="back btn ghost sm" onclick={() => (store.phase = 'connect')}><ChevronLeft size={14} /> {store.status?.name}</button>
      <h1>Pair this computer</h1>
      <p class="lead">Enter the pairing code shown in the server's console, or create one on another device in <strong>Settings → Devices</strong>.</p>
      <form onsubmit={(e) => (e.preventDefault(), pair())}>
        <div class="field">
          <label class="label" for="code">Pairing code</label>
          <input id="code" class="input lg mono code" value={code} oninput={(e) => (code = formatCode(e.currentTarget.value))} placeholder="ABCD-EF23" autocomplete="off" spellcheck="false" />
        </div>
        <div class="field">
          <label class="label" for="dn">Device name</label>
          <input id="dn" class="input" bind:value={deviceName} />
        </div>
        {#if err}<div class="err shake">{err}</div>{/if}
        <button class="btn primary lg block" disabled={busy || code.length < 9}>
          {#if busy}<Loader size={16} class="spin" />{/if} Pair device <ArrowRight size={16} />
        </button>
      </form>
    {:else if store.phase === 'create'}
      {#if store.isLocal}<button class="back btn ghost sm" onclick={async () => { await rad.forgetServer(); store.config = await rad.config(); store.phase = 'connect'; choice = 'pick'; }}><ChevronLeft size={14} /> Choose another location</button>{/if}
      <h1>Create your vault</h1>
      <p class="lead">
        Everything you add — documents, notes, screenshots, recordings and chats — is stored in one encrypted file
        {#if store.isLocal}<strong class="path">{store.config?.local_vault}</strong>{:else}on <strong>{store.status?.name}</strong>{/if}.
        Choose a passphrase you'll remember.
      </p>
      <form onsubmit={(e) => (e.preventDefault(), create())}>
        <div class="field">
          <label class="label" for="p1">Passphrase</label>
          <div class="input-ic">
            <KeyRound size={16} />
            <input id="p1" class="input lg" type={show ? 'text' : 'password'} bind:value={pass} placeholder="e.g. purple-tiger eats 3 lemons at dawn" autocomplete="new-password" />
            <button type="button" class="icon-btn eye" onclick={() => (show = !show)}>{#if show}<EyeOff size={16} />{:else}<Eye size={16} />{/if}</button>
          </div>
          <div class="meter s{strength.score}"><span></span><span></span><span></span><span></span></div>
          <div class="hint">{strength.label ? `${strength.label} · ` : ''}At least 10 characters. A few random words is long and memorable.</div>
        </div>
        <div class="field">
          <label class="label" for="p2">Repeat passphrase</label>
          <input id="p2" class="input lg" type={show ? 'text' : 'password'} bind:value={pass2} autocomplete="new-password" />
        </div>
        {#if err}<div class="err shake">{err}</div>{/if}
        <button class="btn primary lg block" disabled={busy || !pass || !pass2}>
          {#if busy}<Loader size={16} class="spin" /> Encrypting…{:else}<Lock size={16} /> Create encrypted vault{/if}
        </button>
      </form>
    {:else if store.phase === 'recovery-kit' && store.pendingRecoveryKey}
      <h1>Save your recovery kit</h1>
      <p class="lead">Your vault is ready and encrypted. Keep this key safe — you'll need it if you ever forget your passphrase.</p>
      <RecoveryKit recoveryKey={store.pendingRecoveryKey} bind:confirmed />
      <button class="btn primary lg block" style="margin-top:18px" disabled={!confirmed} onclick={finish}>Open RAD <ArrowRight size={16} /></button>
    {/if}
  </div>
</div>

<style>
  .wrap {
    position: relative;
    height: 100%;
    display: grid;
    place-items: center;
    background: var(--bg);
    overflow: auto;
    padding: 40px 20px;
  }
  .aura {
    position: absolute;
    inset: -20%;
    background:
      radial-gradient(40% 35% at 25% 25%, color-mix(in oklab, var(--accent) 26%, transparent), transparent 70%),
      radial-gradient(35% 30% at 80% 70%, color-mix(in oklab, #06b6d4 18%, transparent), transparent 70%),
      radial-gradient(30% 30% at 60% 20%, color-mix(in oklab, #ec4899 12%, transparent), transparent 70%);
    filter: blur(20px);
    animation: drift 18s ease-in-out infinite alternate;
    pointer-events: none;
  }
  @keyframes drift {
    to {
      transform: translate3d(3%, -2%, 0) scale(1.05);
    }
  }
  .card {
    position: relative;
    width: 480px;
    max-width: 100%;
    padding: 30px 32px 32px;
    border-radius: 24px;
    background: var(--glass);
    backdrop-filter: blur(30px) saturate(1.5);
    -webkit-backdrop-filter: blur(30px) saturate(1.5);
    border: 1px solid var(--border-strong);
    box-shadow: var(--shadow-lg);
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 22px;
  }
  .name {
    font-weight: 750;
    font-size: 18px;
    letter-spacing: 0.02em;
  }
  .tag {
    color: var(--text-3);
    font-size: 12.5px;
  }
  .steps {
    display: flex;
    gap: 6px;
    margin-bottom: 22px;
  }
  .step {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11.5px;
    color: var(--text-4);
    font-weight: 550;
    padding-top: 8px;
    border-top: 2px solid var(--border);
    transition: all var(--t) var(--ease);
  }
  .step span {
    font-variant-numeric: tabular-nums;
  }
  .step.active {
    color: var(--accent);
    border-color: var(--accent);
  }
  .step.done {
    color: var(--text-2);
    border-color: color-mix(in oklab, var(--accent) 45%, transparent);
  }
  h1 {
    font-size: 22px;
    letter-spacing: -0.02em;
    margin: 0 0 8px;
    font-weight: 700;
  }
  .lead {
    color: var(--text-2);
    margin: 0 0 22px;
    font-size: 14px;
    line-height: 1.55;
  }
  .input-ic {
    position: relative;
  }
  .input-ic > :global(svg) {
    position: absolute;
    left: 15px;
    top: 15px;
    color: var(--text-3);
  }
  .input-ic .input {
    padding-left: 40px;
  }
  .eye {
    position: absolute;
    right: 8px;
    top: 8px;
  }
  .code {
    text-align: center;
    font-size: 22px;
    letter-spacing: 0.25em;
  }
  code {
    font-family: var(--mono);
    font-size: 0.92em;
    background: var(--surface-3);
    padding: 1px 5px;
    border-radius: 5px;
  }
  .meter {
    display: flex;
    gap: 4px;
    margin-top: 8px;
  }
  .meter span {
    flex: 1;
    height: 4px;
    border-radius: 2px;
    background: var(--surface-3);
    transition: background var(--t);
  }
  .s1 span:nth-child(-n + 1) {
    background: var(--danger);
  }
  .s2 span:nth-child(-n + 2) {
    background: var(--warning);
  }
  .s3 span:nth-child(-n + 3) {
    background: var(--success);
  }
  .s4 span {
    background: var(--success);
  }
  .err {
    margin-bottom: 14px;
    padding: 10px 12px;
    border-radius: var(--r-sm);
    background: var(--danger-soft);
    color: var(--danger);
    font-size: 13px;
  }
  .back {
    margin: -6px 0 8px -8px;
  }
  .modes {
    display: grid;
    gap: 10px;
  }
  .mode {
    display: block;
    text-align: left;
    padding: 16px 18px;
    border-radius: var(--r-lg);
    border: 1px solid var(--border-strong);
    background: var(--surface);
    transition: border-color var(--t), transform var(--t) var(--ease), box-shadow var(--t);
  }
  .mode.click:hover {
    border-color: var(--accent-ring);
    transform: translateY(-1px);
    box-shadow: var(--shadow);
  }
  .mi {
    width: 36px;
    height: 36px;
    border-radius: 11px;
    display: grid;
    place-items: center;
    color: var(--accent);
    background: var(--accent-soft);
    margin-bottom: 10px;
  }
  .mt {
    font-weight: 650;
    font-size: 15px;
    margin-bottom: 4px;
  }
  .md {
    color: var(--text-2);
    font-size: 13px;
    line-height: 1.5;
    margin-bottom: 12px;
  }
  .go {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--accent);
    font-weight: 600;
    font-size: 13px;
  }
  .recent {
    margin-top: 18px;
  }
  .rv {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 32px;
    margin-top: 4px;
    padding: 0 10px;
    border: none;
    border-radius: var(--r-sm);
    background: transparent;
    color: var(--text-2);
    font-size: 12.5px;
    font-family: var(--mono);
    text-align: left;
  }
  .rv:hover {
    background: var(--surface-hover);
    color: var(--text);
  }
  .path {
    font-family: var(--mono);
    font-size: 12.5px;
    font-weight: 500;
    word-break: break-all;
  }
</style>
