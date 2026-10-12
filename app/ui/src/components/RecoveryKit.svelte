<script lang="ts">
  import { save } from '@tauri-apps/plugin-dialog';
  import { invoke } from '@tauri-apps/api/core';
  import { Copy, Download, Printer, ShieldAlert, Check } from '@lucide/svelte';
  import { store } from '../lib/state.svelte';

  let { recoveryKey, confirmed = $bindable(false) }: { recoveryKey: string; confirmed?: boolean } = $props();
  let copied = $state(false);

  const groups = $derived(recoveryKey.replace(/^RAD1-/, '').split('-'));
  const kitText = $derived(
    `RAD RECOVERY KIT\n================\n\nServer: ${store.config?.server_name ?? ''} (${store.config?.server_url ?? ''})\nCreated: ${new Date().toLocaleString()}\n\nRecovery key:\n\n    ${recoveryKey}\n\nThis key opens your RAD vault if you forget your passphrase.\nKeep it offline and private (print it, or store it in a password manager).\nAnyone with this key and a copy of the vault file can read your data.\nChanging your passphrase creates a new recovery key; this one then stops working.\n`,
  );

  async function copy() {
    await navigator.clipboard.writeText(recoveryKey);
    copied = true;
    setTimeout(() => (copied = false), 1600);
  }
  async function download() {
    const dest = await save({ defaultPath: 'RAD Recovery Kit.txt', filters: [{ name: 'Text', extensions: ['txt'] }] });
    if (!dest) return;
    try {
      await invoke('save_text_file', { dest, content: kitText });
      store.toast('success', 'Recovery kit saved', 'Move it somewhere offline and safe.');
    } catch (e) {
      store.error(e);
    }
  }
  function print() {
    const w = window.open('', '_blank', 'width=700,height=600');
    if (!w) return;
    w.document.write(`<pre style="font:15px/1.6 ui-monospace,monospace;padding:40px">${kitText.replace(/</g, '&lt;')}</pre>`);
    w.document.close();
    w.print();
  }
</script>

<div class="kit">
  <div class="warn">
    <ShieldAlert size={18} />
    <div>
      <strong>This is the only way back in if you forget your passphrase.</strong>
      RAD can't reset it for you — your vault is encrypted with your key alone.
    </div>
  </div>
  <div class="key selectable">
    <span class="prefix">RAD1</span>
    {#each groups as g, i}<span class="g" style="animation-delay:{i * 25}ms">{g}</span>{/each}
  </div>
  <div class="actions">
    <button class="btn" onclick={copy}>{#if copied}<Check size={15} /> Copied{:else}<Copy size={15} /> Copy{/if}</button>
    <button class="btn" onclick={download}><Download size={15} /> Save as file</button>
    <button class="btn" onclick={print}><Printer size={15} /> Print</button>
  </div>
  <label class="confirm">
    <input type="checkbox" bind:checked={confirmed} />
    <span>I've stored my recovery kit somewhere safe</span>
  </label>
</div>

<style>
  .warn {
    display: flex;
    gap: 12px;
    padding: 12px 14px;
    border-radius: var(--r);
    background: var(--warning-soft);
    color: var(--text);
    font-size: 13px;
    line-height: 1.5;
  }
  .warn :global(svg) {
    color: var(--warning);
    flex: none;
    margin-top: 1px;
  }
  .warn strong {
    display: block;
  }
  .key {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    justify-content: center;
    margin: 18px 0 14px;
    padding: 18px;
    border-radius: var(--r-lg);
    background: var(--surface-2);
    border: 1px dashed var(--border-strong);
    font-family: var(--mono);
    font-size: 16px;
    letter-spacing: 0.06em;
  }
  .g,
  .prefix {
    padding: 4px 7px;
    border-radius: 7px;
    background: var(--surface);
    border: 1px solid var(--border);
    animation: fade-up 300ms var(--ease) both;
  }
  .prefix {
    color: var(--accent);
    font-weight: 600;
  }
  .actions {
    display: flex;
    gap: 8px;
    justify-content: center;
  }
  .confirm {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-top: 18px;
    font-size: 13.5px;
    color: var(--text-2);
  }
  .confirm input {
    width: 16px;
    height: 16px;
    accent-color: var(--accent);
  }
</style>
