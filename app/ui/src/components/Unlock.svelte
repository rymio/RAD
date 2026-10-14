<script lang="ts">
  import { store } from '../lib/state.svelte';
  import { rad } from '../lib/api';
  import Logo from './Logo.svelte';
  import RecoveryKit from './RecoveryKit.svelte';
  import { Lock, Loader, ArrowRight, LifeBuoy, Server, ChevronLeft, HardDrive, FolderOpen } from '@lucide/svelte';

  let { compact = false }: { compact?: boolean } = $props();

  let pass = $state('');
  let busy = $state(false);
  let err = $state('');
  let shake = $state(false);
  let mode = $state<'pass' | 'recover' | 'kit'>('pass');
  let rkey = $state('');
  let newPass = $state('');
  let newKit = $state('');
  let confirmed = $state(false);
  let input: HTMLInputElement | undefined = $state();

  $effect(() => {
    input?.focus();
  });

  async function unlock(e: Event) {
    e.preventDefault();
    if (!pass) return;
    busy = true;
    err = '';
    try {
      await rad.unlock(pass);
      pass = '';
      if (!compact) {
        store.status = await rad.status();
        await store.enterApp();
      }
    } catch (e: any) {
      err = e?.code === 'wrong_passphrase' ? 'That passphrase isn’t right.' : (e?.message ?? String(e));
      shake = false;
      requestAnimationFrame(() => (shake = true));
      pass = '';
    } finally {
      busy = false;
    }
  }

  async function recover(e: Event) {
    e.preventDefault();
    busy = true;
    err = '';
    try {
      if (newPass.length < 10) throw new Error('Choose a new passphrase of at least 10 characters.');
      newKit = await rad.unlockRecovery(rkey, newPass);
      rkey = newPass = '';
      mode = 'kit';
    } catch (e: any) {
      err = e?.message ?? String(e);
    } finally {
      busy = false;
    }
  }

  async function done() {
    store.status = await rad.status();
    await store.enterApp();
  }
</script>

<div class="wrap" class:compact class:drag={!compact}>
  {#if !compact}<div class="aura"></div>{/if}
  <div class="card no-drag fade-up">
    {#if mode === 'pass'}
      <div class="lockmark"><Logo size={compact ? 40 : 56} /><span class="badge"><Lock size={12} /></span></div>
      <h1>{compact ? 'Unlock RAD' : 'Welcome back'}</h1>
      {#if store.isLocal}
        <p class="where" title={store.config?.local_vault}><HardDrive size={13} /> {store.whereLabel} · on this computer</p>
      {:else}
        <p class="where"><Server size={13} /> {store.config?.server_name ?? 'RAD server'} · {store.status?.device?.name ?? store.config?.device_name}</p>
      {/if}
      {#if store.vaultMove}
        <div class="moving"><Loader size={13} class="spin" /> Moving the vault… {store.vaultMove.total ? Math.floor((store.vaultMove.done / store.vaultMove.total) * 100) : 0}%</div>
      {/if}
      <form onsubmit={unlock} class:shake>
        <input bind:this={input} class="input lg pass" type="password" bind:value={pass} placeholder="Passphrase" autocomplete="current-password" disabled={busy || !!store.vaultMove} />
        <button class="btn primary lg go" disabled={busy || !pass || !!store.vaultMove} aria-label="Unlock">
          {#if busy}<Loader size={17} class="spin" />{:else}<ArrowRight size={17} />{/if}
        </button>
      </form>
      {#if err}<div class="err">{err}</div>{/if}
      {#if !compact}
        <div class="links">
          <button class="btn ghost sm" onclick={() => ((mode = 'recover'), (err = ''))}><LifeBuoy size={14} /> Use recovery key</button>
          <button class="btn ghost sm" onclick={async () => { await rad.forgetServer(); store.config = await rad.config(); store.status = null; store.phase = 'connect'; }}>{#if store.isLocal}<FolderOpen size={14} /> Switch vault{:else}<Server size={14} /> Change server{/if}</button>
        </div>
      {/if}
    {:else if mode === 'recover'}
      <button class="btn ghost sm back" onclick={() => (mode = 'pass')}><ChevronLeft size={14} /> Back</button>
      <h1>Recover your vault</h1>
      <p class="lead">Enter the key from your recovery kit and choose a new passphrase. Other devices will need the new passphrase.</p>
      <form class="col" onsubmit={recover}>
        <textarea class="textarea mono" rows="3" bind:value={rkey} placeholder="RAD1-XXXXX-XXXXX-…" spellcheck="false"></textarea>
        <input class="input lg" type="password" bind:value={newPass} placeholder="New passphrase" autocomplete="new-password" />
        {#if err}<div class="err">{err}</div>{/if}
        <button class="btn primary lg block" disabled={busy || !rkey || !newPass}>{#if busy}<Loader size={16} class="spin" />{/if} Recover & set passphrase</button>
      </form>
    {:else}
      <h1>New recovery kit</h1>
      <p class="lead">Your vault was re-keyed. Your old recovery key no longer works — save this new one.</p>
      <RecoveryKit recoveryKey={newKit} bind:confirmed />
      <button class="btn primary lg block" style="margin-top:16px" disabled={!confirmed} onclick={done}>Open RAD</button>
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
    overflow: hidden;
  }
  .wrap.compact {
    background: transparent;
  }
  .aura {
    position: absolute;
    inset: -20%;
    background:
      radial-gradient(35% 35% at 50% 40%, color-mix(in oklab, var(--accent) 24%, transparent), transparent 70%),
      radial-gradient(30% 30% at 70% 75%, color-mix(in oklab, #06b6d4 14%, transparent), transparent 70%);
    filter: blur(24px);
    pointer-events: none;
  }
  .card {
    position: relative;
    width: 420px;
    max-width: calc(100% - 32px);
    padding: 34px 32px 26px;
    border-radius: 26px;
    background: var(--glass);
    backdrop-filter: blur(30px) saturate(1.5);
    -webkit-backdrop-filter: blur(30px) saturate(1.5);
    border: 1px solid var(--border-strong);
    box-shadow: var(--shadow-lg);
    text-align: center;
  }
  .compact .card {
    box-shadow: none;
    background: transparent;
    border: none;
    backdrop-filter: none;
  }
  .lockmark {
    position: relative;
    display: inline-block;
    margin-bottom: 14px;
  }
  .badge {
    position: absolute;
    right: -6px;
    bottom: -4px;
    width: 24px;
    height: 24px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: var(--surface);
    color: var(--text);
    border: 1px solid var(--border-strong);
    box-shadow: var(--shadow);
  }
  h1 {
    margin: 0 0 6px;
    font-size: 22px;
    font-weight: 700;
    letter-spacing: -0.02em;
  }
  .where {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin: 0 0 22px;
    color: var(--text-3);
    font-size: 12.5px;
  }
  .lead {
    color: var(--text-2);
    font-size: 13.5px;
    margin: 0 0 18px;
  }
  form {
    display: flex;
    gap: 8px;
  }
  form.col {
    flex-direction: column;
  }
  .pass {
    flex: 1;
    text-align: left;
  }
  .go {
    width: 46px;
    padding: 0;
    height: 46px;
  }
  .err {
    margin-top: 12px;
    color: var(--danger);
    font-size: 13px;
  }
  .links {
    display: flex;
    justify-content: center;
    gap: 6px;
    margin-top: 22px;
  }
  .back {
    float: left;
    margin: -14px 0 0 -12px;
  }
  textarea {
    text-align: left;
  }
  .moving {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    margin: -4px 0 12px;
    font-size: 13px;
    color: var(--accent);
  }
</style>
