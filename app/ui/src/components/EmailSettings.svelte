<script lang="ts">
  import { onMount } from 'svelte';
  import { rad } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import type { EmailAccount } from '../lib/types';
  import { Check, CircleAlert, Loader, Trash2, Mail, ShieldCheck } from '@lucide/svelte';

  type Preset = { id: string; label: string; host: string; port: number; security: 'ssl' | 'starttls'; domains: string[]; help?: string };
  const PRESETS: Preset[] = [
    { id: 'gmail', label: 'Gmail', host: 'imap.gmail.com', port: 993, security: 'ssl', domains: ['gmail.com', 'googlemail.com'], help: 'Turn on 2-Step Verification, then create an app password at myaccount.google.com/apppasswords.' },
    { id: 'icloud', label: 'iCloud', host: 'imap.mail.me.com', port: 993, security: 'ssl', domains: ['icloud.com', 'me.com', 'mac.com'], help: 'Create an app-specific password at account.apple.com → Sign-In and Security. The username is the part before @.' },
    { id: 'outlook', label: 'Outlook', host: 'outlook.office365.com', port: 993, security: 'ssl', domains: ['outlook.com', 'hotmail.com', 'live.com', 'msn.com'], help: 'Use an app password from account.microsoft.com → Security. Some work accounts don’t allow IMAP sign-in.' },
    { id: 'yahoo', label: 'Yahoo', host: 'imap.mail.yahoo.com', port: 993, security: 'ssl', domains: ['yahoo.com', 'ymail.com'], help: 'Generate an app password under Account security in your Yahoo account.' },
    { id: 'fastmail', label: 'Fastmail', host: 'imap.fastmail.com', port: 993, security: 'ssl', domains: ['fastmail.com', 'fastmail.fm'], help: 'Create an app password under Settings → Privacy & Security.' },
    { id: 'other', label: 'Other', host: '', port: 993, security: 'ssl', domains: [] },
  ];

  const saved = $derived(store.emailAccount ?? null);
  let email = $state('');
  let host = $state('');
  let port = $state(993);
  let security = $state<'ssl' | 'starttls'>('ssl');
  let username = $state('');
  let password = $state('');
  let preset = $state('other');
  let userTouched = $state(false);
  let busy = $state(false);
  let result = $state<{ ok: boolean; text: string } | null>(null);
  let hasPassword = $state(false);

  const help = $derived(PRESETS.find((p) => p.id === preset)?.help);
  const draft = $derived<EmailAccount>({ email: email.trim(), host: host.trim(), port: +port || 993, security, username: (username || email).trim() });
  const valid = $derived(!!draft.email && !!draft.host && !!draft.username);
  const changed = $derived(!saved || JSON.stringify({ email: saved.email, host: saved.host, port: saved.port, security: saved.security, username: saved.username }) !== JSON.stringify(draft));

  function fill(a: EmailAccount | null) {
    email = a?.email ?? '';
    host = a?.host ?? '';
    port = a?.port ?? 993;
    security = a?.security ?? 'ssl';
    username = a?.username ?? '';
    userTouched = !!a && a.username !== a.email;
    preset = PRESETS.find((p) => p.host === host)?.id ?? 'other';
  }

  onMount(async () => {
    if (store.emailAccount === undefined) await store.loadAppData('email_account');
    fill(store.emailAccount ?? null);
    if (store.emailAccount) hasPassword = await rad.mailHasPassword(store.emailAccount).catch(() => false);
  });

  function pickPreset(id: string) {
    preset = id;
    const p = PRESETS.find((x) => x.id === id)!;
    if (p.id !== 'other') {
      host = p.host;
      port = p.port;
      security = p.security;
    }
  }

  function onEmail() {
    const domain = email.split('@')[1]?.toLowerCase().trim();
    const p = PRESETS.find((x) => domain && x.domains.includes(domain));
    if (p) pickPreset(p.id);
    else if (domain && preset === 'other' && !host) host = `imap.${domain}`;
    if (!userTouched) username = preset === 'icloud' ? email.split('@')[0] : email;
  }

  async function save(e: Event) {
    e.preventDefault();
    if (!valid || busy) return;
    busy = true;
    result = null;
    try {
      const acc = draft;
      if (password) {
        const n = await rad.mailTest(acc, password);
        await rad.mailSetPassword(acc, password);
        result = { ok: true, text: `Connected — ${n} folder${n === 1 ? '' : 's'} found.` };
      } else if (!(await rad.mailHasPassword(acc))) {
        result = { ok: false, text: 'Enter the password (or app password) for this account.' };
        return;
      } else {
        result = { ok: true, text: 'Saved.' };
      }
      if (saved && saved.host + saved.username + saved.port !== acc.host + acc.username + acc.port) await rad.mailSetPassword(saved, null);
      await store.saveEmailAccount(acc);
      hasPassword = true;
      password = '';
    } catch (err: any) {
      result = { ok: false, text: err?.message ?? String(err) };
    } finally {
      busy = false;
    }
  }

  async function remove() {
    if (!saved) return;
    try {
      await rad.mailSetPassword(saved, null);
      await store.saveEmailAccount(null);
      fill(null);
      password = '';
      hasPassword = false;
      result = null;
      store.toast('info', 'Email account removed', 'Its password was deleted from this computer.');
    } catch (e) {
      store.error(e, 'Couldn’t remove the account');
    }
  }
</script>

<h2>Email</h2>
<p class="lead">
  Read your mail in RAD and ask the AI about it. Messages are <strong>not downloaded into your vault</strong> — they’re fetched from your provider over
  an encrypted connection when you look at them, and opening one marks it as read there too. Pictures from the internet are blocked.
</p>

{#if saved && !hasPassword}
  <div class="test bad"><CircleAlert size={13} /> This computer doesn’t have the password for {saved.email} yet — enter it below.</div>
{/if}

<form onsubmit={save}>
  <div class="presets">
    {#each PRESETS as p}
      <button type="button" class="preset" class:on={preset === p.id} onclick={() => pickPreset(p.id)}>{p.label}</button>
    {/each}
  </div>

  <div class="group">
    <div class="srow">
      <div><div class="st">Email address</div></div>
      <input class="input w320" type="email" bind:value={email} oninput={onEmail} placeholder="you@example.com" spellcheck="false" />
    </div>
    <div class="srow">
      <div><div class="st">Password</div><div class="sd">{help ?? 'Many providers need an app password instead of your normal one.'} Stored in this computer’s keychain only.</div></div>
      <input class="input w320" type="password" bind:value={password} placeholder={hasPassword ? '•••••••• (saved — leave empty to keep)' : 'App password'} autocomplete="off" />
    </div>
  </div>

  <h3>Incoming mail (IMAP)</h3>
  <div class="group">
    <div class="srow">
      <div><div class="st">Server</div></div>
      <input class="input w320 mono" bind:value={host} oninput={() => (preset = PRESETS.find((p) => p.host === host)?.id ?? 'other')} placeholder="imap.example.com" spellcheck="false" />
    </div>
    <div class="srow">
      <div><div class="st">Port & security</div></div>
      <div class="row" style="gap:8px">
        <input class="input mono" style="width:90px" type="number" bind:value={port} />
        <select class="select" style="width:222px" bind:value={security} onchange={() => (port = security === 'ssl' ? 993 : 143)}>
          <option value="ssl">SSL/TLS</option>
          <option value="starttls">STARTTLS</option>
        </select>
      </div>
    </div>
    <div class="srow">
      <div><div class="st">Username</div><div class="sd">Usually your full email address.</div></div>
      <input class="input w320" bind:value={username} oninput={() => (userTouched = true)} placeholder={email || 'you@example.com'} spellcheck="false" />
    </div>
  </div>

  {#if result}
    <div class="test" class:bad={!result.ok}>{#if result.ok}<Check size={13} />{:else}<CircleAlert size={13} />{/if} {result.text}</div>
  {/if}

  <div class="row actions">
    {#if saved}
      <button type="button" class="btn ghost danger" onclick={remove}><Trash2 size={14} /> Remove account</button>
    {/if}
    <span class="spacer"></span>
    {#if saved}
      <button type="button" class="btn" onclick={() => store.navigate({ kind: 'email' })}><Mail size={14} /> Open Email</button>
    {/if}
    <button class="btn primary" disabled={!valid || busy || (!changed && !password)}>
      {#if busy}<Loader size={14} class="spin" />{:else}<ShieldCheck size={14} />{/if}
      {password ? 'Connect & save' : 'Save'}
    </button>
  </div>
</form>

<style>
  .presets {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: 14px;
  }
  .preset {
    height: 30px;
    padding: 0 12px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--surface);
    color: var(--text-2);
    font-size: 13px;
  }
  .preset:hover {
    border-color: var(--border-strong);
  }
  .preset.on {
    border-color: var(--accent);
    background: color-mix(in oklab, var(--accent) 12%, var(--surface));
    color: var(--text);
    font-weight: 550;
  }
  .w320 {
    width: 320px;
  }
  .mono {
    font-family: var(--font-mono, ui-monospace, monospace);
    font-size: 12.5px;
  }
  .test {
    margin: 0 0 14px;
  }
  .actions {
    gap: 8px;
    margin-top: 6px;
  }
</style>
