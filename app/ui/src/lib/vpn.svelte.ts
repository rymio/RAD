import { listen } from '@tauri-apps/api/event';
import { rad } from './api';
import { store } from './state.svelte';
import type { VpnInfo } from './types';

/** RAD's VPN, shared by the top-bar switch and Settings → VPN. */
class VpnState {
  info = $state<VpnInfo | null>(null);
  busy = $state(false);
  private timer: ReturnType<typeof setInterval> | null = null;
  private started = false;

  /** 'off' | 'connecting' (on, no handshake yet) | 'on' | 'error' */
  get state() {
    const i = this.info;
    if (!i?.on) return 'off';
    if (i.status?.up) return 'on';
    return i.status?.last_error ? 'error' : 'connecting';
  }

  start() {
    if (this.started) return;
    this.started = true;
    this.refresh();
    listen<boolean>('vpn-changed', () => this.refresh());
  }

  async refresh() {
    try {
      this.info = await rad.vpnGet();
    } catch {
      /* not available */
    }
    // While on, keep the status (handshake, traffic) fresh.
    if (this.info?.on && !this.timer) this.timer = setInterval(() => this.refresh(), 3000);
    if (!this.info?.on && this.timer) {
      clearInterval(this.timer);
      this.timer = null;
    }
  }

  async set(on: boolean) {
    if (this.busy) return;
    if (on && !this.info?.configured) {
      store.navigate({ kind: 'settings', tab: 'vpn' });
      return;
    }
    this.busy = true;
    try {
      this.info = await rad.vpnSet(on);
      this.refresh();
    } catch (e) {
      store.error(e, on ? 'The VPN didn’t start' : 'Couldn’t turn the VPN off');
    } finally {
      this.busy = false;
    }
  }
}

export const vpn = new VpnState();

export function formatAgo(secs: number | null | undefined) {
  if (secs == null) return 'never';
  if (secs < 60) return `${secs}s ago`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
  return `${Math.floor(secs / 3600)}h ago`;
}
