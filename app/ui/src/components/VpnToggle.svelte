<script lang="ts">
  import { onMount } from 'svelte';
  import { store } from '../lib/state.svelte';
  import { vpn, formatAgo } from '../lib/vpn.svelte';
  import { formatBytes } from '../lib/util';
  import { ShieldCheck, ShieldOff, ShieldAlert, Loader, Settings2 } from '@lucide/svelte';

  let open = $state(false);
  let root: HTMLDivElement | undefined = $state();

  onMount(() => {
    vpn.start();
    const away = (e: MouseEvent) => {
      if (open && root && !root.contains(e.target as Node)) open = false;
    };
    window.addEventListener('mousedown', away);
    return () => window.removeEventListener('mousedown', away);
  });

  const st = $derived(vpn.state);
  const label = $derived(st === 'on' ? 'VPN on' : st === 'connecting' ? 'Connecting…' : st === 'error' ? 'VPN problem' : 'VPN off');
  const scopes = $derived(vpn.info?.settings.scopes);
  const covered = $derived(
    scopes
      ? [scopes.services && 'RAD & AI', scopes.browsers && 'Chromium', scopes.email && 'Email', scopes.ssh && 'SSH', scopes.rymnet && 'Rymnet'].filter(Boolean).join(' · ')
      : '',
  );

  function settings() {
    open = false;
    store.navigate({ kind: 'settings', tab: 'vpn' });
  }
</script>

<div class="vpn no-drag" bind:this={root}>
  <button class="pill {st}" title="{label} — click for details" onclick={() => (open = !open)}>
    {#if vpn.busy || st === 'connecting'}<Loader size={12} class="spin" />
    {:else if st === 'on'}<ShieldCheck size={12} />
    {:else if st === 'error'}<ShieldAlert size={12} />
    {:else}<ShieldOff size={12} />{/if}
    VPN
  </button>

  {#if open}
    <div class="pop">
      <div class="head">
        <span class="title">{label}</span>
        <button
          class="switch"
          class:on={vpn.info?.on}
          disabled={vpn.busy}
          aria-label="VPN on or off"
          title={vpn.info?.configured ? '' : 'Set up the VPN first'}
          onclick={() => vpn.set(!vpn.info?.on)}
        ></button>
      </div>
      {#if !vpn.info?.configured}
        <p class="hint">Not set up yet. Add your WireGuard server in Settings → VPN.</p>
      {:else}
        <dl>
          <dt>Server</dt><dd class="mono truncate">{vpn.info.settings.endpoint}</dd>
          {#if vpn.info.on}
            <dt>Handshake</dt><dd>{formatAgo(vpn.info.status?.handshake_ago_secs)}</dd>
            <dt>Traffic</dt><dd>↑ {formatBytes(vpn.info.status?.tx_bytes ?? 0)} · ↓ {formatBytes(vpn.info.status?.rx_bytes ?? 0)}</dd>
          {/if}
          <dt>Covers</dt><dd>{covered || 'nothing selected'}</dd>
        </dl>
        {#if vpn.info.status?.last_error && st !== 'on'}<p class="err">{vpn.info.status.last_error}</p>{/if}
        {#if st === 'connecting'}<p class="hint">Waiting for the server to answer. Covered connections wait too — nothing goes out unprotected.</p>{/if}
      {/if}
      <button class="btn sm ghost set" onclick={settings}><Settings2 size={13} /> VPN settings</button>
    </div>
  {/if}
</div>

<style>
  .vpn {
    position: relative;
    flex: none;
  }
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 22px;
    padding: 0 8px;
    border-radius: 999px;
    border: 1px solid var(--border);
    background: var(--surface-2);
    color: var(--text-3);
    font-size: 11px;
    font-weight: 650;
    letter-spacing: 0.03em;
  }
  .pill.on {
    background: var(--success-soft);
    border-color: transparent;
    color: var(--success);
  }
  .pill.connecting {
    background: var(--warning-soft);
    border-color: transparent;
    color: var(--warning);
  }
  .pill.error {
    background: var(--danger-soft);
    border-color: transparent;
    color: var(--danger);
  }
  .pop {
    position: absolute;
    z-index: 80;
    top: calc(100% + 6px);
    left: 0;
    width: 280px;
    padding: 12px;
    border-radius: var(--r);
    border: 1px solid var(--border);
    background: var(--surface);
    box-shadow: var(--shadow-lg);
    font-size: 12.5px;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 6px;
  }
  .title {
    font-weight: 650;
    font-size: 13px;
  }
  dl {
    display: grid;
    grid-template-columns: 74px 1fr;
    gap: 4px 8px;
    margin: 8px 0;
  }
  dt {
    color: var(--text-3);
  }
  dd {
    margin: 0;
    min-width: 0;
  }
  .mono {
    font-family: var(--mono);
    font-size: 11.5px;
  }
  .hint {
    margin: 6px 0;
    color: var(--text-3);
    line-height: 1.45;
  }
  .err {
    margin: 6px 0;
    color: var(--danger);
    word-break: break-word;
  }
  .set {
    width: 100%;
    margin-top: 4px;
  }
</style>
