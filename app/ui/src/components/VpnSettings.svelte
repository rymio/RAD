<script lang="ts">
  import { open as openDialog, ask } from '@tauri-apps/plugin-dialog';
  import { rad } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import { vpn, formatAgo } from '../lib/vpn.svelte';
  import { formatBytes } from '../lib/util';
  import type { VpnScopes } from '../lib/types';
  import { Copy, KeyRound, FileUp, Save, Loader, Globe, Check } from '@lucide/svelte';

  let endpoint = $state('');
  let serverKey = $state('');
  let psk = $state('');
  let address = $state('');
  let dns = $state('');
  let keepalive = $state<number | null>(25);
  let scopes = $state<VpnScopes>({ services: true, browsers: true, email: true, ssh: true, rymnet: true });
  let autoConnect = $state(false);
  let saving = $state(false);
  let pasteOpen = $state(false);
  let pasted = $state('');
  let ipCheck = $state<{ direct: string | null; vpn: string | null; error: string | null } | null>(null);
  let checking = $state(false);
  let loaded = false;

  $effect(() => {
    vpn.start();
    if (!vpn.info || loaded) return;
    loaded = true;
    fill();
  });

  function fill() {
    const s = vpn.info!.settings;
    endpoint = s.endpoint;
    serverKey = s.peer_public_key;
    address = s.address.join(', ');
    dns = s.dns.join(', ');
    keepalive = s.keepalive ?? 25;
    scopes = { ...s.scopes };
    autoConnect = s.auto_connect;
    psk = '';
  }

  const list = (s: string) => s.split(',').map((x) => x.trim()).filter(Boolean);

  async function save() {
    saving = true;
    try {
      vpn.info = await rad.vpnSave({
        endpoint, peer_public_key: serverKey, address: list(address), dns: list(dns), keepalive,
        scopes, auto_connect: autoConnect, preshared_key: psk.trim() ? psk.trim() : null,
      });
      psk = '';
      store.toast('success', 'VPN settings saved', vpn.info.on ? 'Applied to the running VPN.' : undefined);
    } catch (e) {
      store.error(e, 'Couldn’t save the VPN settings');
    } finally {
      saving = false;
    }
  }

  async function newKey() {
    if (vpn.info?.public_key && !(await ask('This replaces this device’s key. The server needs the new public key before the VPN works again. Continue?', { title: 'New VPN key', kind: 'warning' }))) return;
    try {
      vpn.info = await rad.vpnGenerateKey();
    } catch (e) {
      store.error(e, 'Couldn’t create a key');
    }
  }

  function imported() {
    fill();
    pasteOpen = false;
    pasted = '';
    store.toast('success', 'WireGuard config imported', 'Keys are in your keychain. Turn the VPN on from the top bar.');
  }

  async function importText(text: string) {
    try {
      vpn.info = await rad.vpnImport(text);
      imported();
    } catch (e) {
      store.error(e, 'Couldn’t import');
    }
  }

  async function importFile() {
    const f = await openDialog({ filters: [{ name: 'WireGuard config', extensions: ['conf'] }, { name: 'All files', extensions: ['*'] }] });
    if (!f) return;
    try {
      vpn.info = await rad.vpnImportFile(String(f));
      imported();
    } catch (e) {
      store.error(e, 'Couldn’t import');
    }
  }

  async function checkIp() {
    checking = true;
    try {
      ipCheck = await rad.vpnCheckIp();
    } catch (e) {
      store.error(e, 'IP check failed');
    } finally {
      checking = false;
    }
  }

  const copy = (t: string) => navigator.clipboard.writeText(t).then(() => store.toast('success', 'Copied'));
  const firstAddress = $derived(list(address)[0] ?? '10.66.0.2/32');
  const peerSnippet = $derived(
    vpn.info?.public_key
      ? `[Peer]\n# ${store.config?.device_name ?? 'RAD'}\nPublicKey = ${vpn.info.public_key}\nPresharedKey = <output of: wg genpsk>\nAllowedIPs = ${firstAddress}`
      : '',
  );

  const SCOPES: { k: keyof VpnScopes; t: string; d: string }[] = [
    { k: 'services', t: 'RAD & AI', d: 'Your RAD server, AI providers (OpenAI, Anthropic…), model and voice downloads. This computer and your local network stay direct.' },
    { k: 'browsers', t: 'Chromium', d: 'Every page and name lookup. Chromium restarts when the VPN changes. The Servo browser can’t use a proxy and isn’t covered.' },
    { k: 'email', t: 'Email', d: 'Your IMAP mail server.' },
    { k: 'ssh', t: 'SSH terminals', d: 'New SSH sessions, with their agent and file transfers. On Linux this needs netcat-openbsd.' },
    { k: 'rymnet', t: 'Rymnet', d: 'Peer connections from RYMWIN, rymsh, rym and the daemon. Rymnet programs already running pick it up when they next start.' },
  ];
</script>

<h2>VPN</h2>
<p class="intro">
  RAD’s own WireGuard tunnel to your server. It runs inside RAD — no administrator rights, nothing to install — and only the RAD traffic you choose
  below goes through it. While it’s on, that traffic never falls back to the open network: if the server doesn’t answer, those connections wait or fail.
</p>

<div class="group">
  <div class="srow">
    <div>
      <div class="st">VPN</div>
      <div class="sd">
        {#if !vpn.info?.configured}Set up this device’s key and your server below first.
        {:else if vpn.info.on}
          {vpn.state === 'on' ? 'Connected' : vpn.state === 'error' ? 'Problem' : 'Connecting…'} · handshake {formatAgo(vpn.info.status?.handshake_ago_secs)} ·
          ↑ {formatBytes(vpn.info.status?.tx_bytes ?? 0)} ↓ {formatBytes(vpn.info.status?.rx_bytes ?? 0)}
        {:else}Off. Use the VPN switch in the top bar, or this one.{/if}
      </div>
      {#if vpn.info?.status?.last_error && vpn.state !== 'on'}<div class="err">{vpn.info.status.last_error}</div>{/if}
    </div>
    <button class="switch" class:on={vpn.info?.on} disabled={vpn.busy || !vpn.info?.configured} aria-label="VPN on or off" onclick={() => vpn.set(!vpn.info?.on)}></button>
  </div>
  <div class="srow">
    <div><div class="st">Connect when RAD starts</div><div class="sd">If the VPN was on when you quit RAD.</div></div>
    <button class="switch" class:on={autoConnect} aria-label="Connect when RAD starts" onclick={() => { autoConnect = !autoConnect; save(); }}></button>
  </div>
  <div class="srow">
    <div>
      <div class="st">What the internet sees</div>
      <div class="sd">
        {#if ipCheck}
          Without VPN: <b class="mono">{ipCheck.direct ?? 'unknown'}</b>
          {#if vpn.info?.on} · Through VPN: <b class="mono">{ipCheck.vpn ?? 'failed'}</b>{#if ipCheck.vpn && ipCheck.vpn !== ipCheck.direct}&nbsp;<Check size={12} class="ok" />{/if}{/if}
          {#if ipCheck.error}<div class="err">{ipCheck.error}</div>{/if}
        {:else}Checks your public IP address, directly and through the VPN (api.ipify.org).{/if}
      </div>
    </div>
    <button class="btn sm" disabled={checking} onclick={checkIp}>{#if checking}<Loader size={13} class="spin" />{:else}<Globe size={13} />{/if} Check</button>
  </div>
</div>

<h3>1 · This device’s key</h3>
<div class="group">
  <div class="srow">
    <div class="grow">
      <div class="st">Public key</div>
      {#if vpn.info?.public_key}
        <div class="key mono selectable">{vpn.info.public_key}</div>
        <div class="sd">Add it as a [Peer] on your server. The private key never leaves this computer — it’s in your keychain.</div>
      {:else}
        <div class="sd">Create a key here, or import a WireGuard config that already has one.</div>
      {/if}
    </div>
    <div class="acts">
      {#if vpn.info?.public_key}<button class="btn sm ghost" onclick={() => copy(vpn.info!.public_key!)}><Copy size={13} /> Copy</button>{/if}
      <button class="btn sm" onclick={newKey}><KeyRound size={13} /> {vpn.info?.public_key ? 'New key' : 'Create key'}</button>
    </div>
  </div>
  {#if peerSnippet}
    <div class="snip">
      <div class="sd">On the server, add this to <code>/etc/wireguard/wg0.conf</code>, or run <code class="selectable">sudo wg set wg0 peer {vpn.info?.public_key} allowed-ips {firstAddress} &amp;&amp; sudo wg-quick save wg0</code></div>
      <pre class="selectable">{peerSnippet}</pre>
      <button class="btn sm ghost" onclick={() => copy(peerSnippet)}><Copy size={13} /> Copy</button>
    </div>
  {/if}
</div>

<h3>2 · Your server</h3>
<div class="group form">
  <div class="imports">
    <button class="btn sm" onclick={importFile}><FileUp size={13} /> Import .conf file</button>
    <button class="btn sm ghost" onclick={() => (pasteOpen = !pasteOpen)}>Paste config</button>
    <span class="sd">A wg-quick file from your server fills in everything below, keys included.</span>
  </div>
  {#if pasteOpen}
    <textarea class="textarea mono" rows="8" bind:value={pasted} placeholder={'[Interface]\nPrivateKey = …\nAddress = 10.66.0.2/32\nDNS = 10.66.0.1\n\n[Peer]\nPublicKey = …\nEndpoint = vpn.example.com:51820'}></textarea>
    <div><button class="btn sm primary" disabled={!pasted.trim()} onclick={() => importText(pasted)}>Import</button></div>
  {/if}
  <label class="field"><span class="label">Server endpoint</span><input class="input mono" bind:value={endpoint} placeholder="vpn.example.com:51820" spellcheck="false" /></label>
  <label class="field"><span class="label">Server public key</span><input class="input mono" bind:value={serverKey} placeholder="contents of /etc/wireguard/server.pub" spellcheck="false" /></label>
  <label class="field"><span class="label">Preshared key <span class="opt">(optional)</span></span><input class="input mono" type="password" bind:value={psk} placeholder={vpn.info?.has_psk ? '•••••••• set — type to replace' : 'from wg genpsk'} autocomplete="off" /></label>
  <div class="three">
    <label class="field"><span class="label">This device’s tunnel address</span><input class="input mono" bind:value={address} placeholder="10.66.0.2/32" spellcheck="false" /></label>
    <label class="field"><span class="label">DNS through the tunnel</span><input class="input mono" bind:value={dns} placeholder="10.66.0.1" spellcheck="false" /></label>
    <label class="field"><span class="label">Keepalive (s)</span><input class="input mono" type="number" min="0" max="600" bind:value={keepalive} /></label>
  </div>
</div>

<h3>3 · What goes through the VPN</h3>
<div class="group">
  {#each SCOPES as s (s.k)}
    <div class="srow">
      <div><div class="st">{s.t}</div><div class="sd">{s.d}</div></div>
      <button class="switch" class:on={scopes[s.k]} aria-label={s.t} onclick={() => (scopes[s.k] = !scopes[s.k])}></button>
    </div>
  {/each}
</div>

<div class="save">
  <button class="btn primary" disabled={saving} onclick={save}>{#if saving}<Loader size={14} class="spin" />{:else}<Save size={14} />{/if} Save</button>
</div>

<style>
  .intro {
    margin: -4px 0 16px;
    color: var(--text-2);
    font-size: 13px;
    line-height: 1.55;
  }
  h3 {
    margin: 22px 0 8px;
    font-size: 13px;
    font-weight: 650;
    color: var(--text-2);
  }
  .grow {
    flex: 1;
    min-width: 0;
  }
  .key {
    margin: 4px 0;
    font-size: 12px;
    word-break: break-all;
  }
  .mono {
    font-family: var(--mono);
  }
  .acts {
    display: flex;
    gap: 6px;
    flex: none;
  }
  .snip {
    padding: 10px 14px 12px;
    border-top: 1px solid var(--border);
  }
  .snip code {
    word-break: break-all;
  }
  .snip pre {
    margin: 6px 0;
    padding: 8px 10px;
    border-radius: var(--r-sm);
    background: var(--code-bg);
    font-family: var(--mono);
    font-size: 11.5px;
    white-space: pre-wrap;
    word-break: break-all;
  }
  .form {
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .imports {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
  }
  .field {
    display: flex;
    flex-direction: column;
    margin: 0;
  }
  .three {
    display: grid;
    grid-template-columns: 1fr 1fr 110px;
    gap: 10px;
  }
  .opt {
    font-weight: 400;
    color: var(--text-3);
  }
  .err {
    margin-top: 4px;
    color: var(--danger);
    font-size: 12px;
    word-break: break-word;
  }
  .save {
    display: flex;
    justify-content: flex-end;
    margin: 16px 0 8px;
  }
  .sd :global(.ok) {
    color: var(--success);
    vertical-align: -2px;
  }
  @media (max-width: 700px) {
    .three {
      grid-template-columns: 1fr;
    }
  }
</style>
