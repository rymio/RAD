<script lang="ts">
  import { store } from '../lib/state.svelte';
  import { api } from '../lib/api';
  import type { ModelInfo, ModelRef, Id } from '../lib/types';
  import { Cpu, Check, Loader, ChevronDown } from '@lucide/svelte';

  let {
    value,
    onchange,
    allowDefault = true,
    filter = '',
    compact = true,
    placeholder = 'Choose a model',
  }: {
    value: ModelRef | null;
    onchange: (m: ModelRef | null) => void;
    allowDefault?: boolean;
    filter?: 'embed' | 'chat' | '';
    compact?: boolean;
    placeholder?: string;
  } = $props();

  let open = $state(false);
  let loading = $state(false);
  let models = $state<Record<Id, ModelInfo[] | string>>({});
  let q = $state('');

  async function load() {
    loading = true;
    await Promise.all(
      store.providers
        .filter((p) => p.enabled)
        .map(async (p) => {
          try {
            models[p.id] = await api.get<ModelInfo[]>(`/v1/providers/${p.id}/models`);
          } catch (e: any) {
            models[p.id] = e?.message ?? 'unreachable';
          }
        }),
    );
    loading = false;
  }

  function toggle() {
    open = !open;
    if (open) load();
  }

  const isEmbed = (id: string) => /embed|bge|e5-|gte-|minilm|nomic-embed|mxbai/i.test(id);
  const isAudio = (id: string) => /whisper|transcri|speech|stt/i.test(id);
  function visible(list: ModelInfo[]) {
    return list.filter((m) => {
      if (q && !m.id.toLowerCase().includes(q.toLowerCase())) return false;
      if (filter === 'embed') return isEmbed(m.id);
      if (filter === 'chat') return !isEmbed(m.id) && !isAudio(m.id);
      return true;
    });
  }

  const defaultLabel = $derived(store.modelLabel(store.settings?.chat_model));
  const label = $derived(value ? store.modelLabel(value) : allowDefault ? `Default · ${defaultLabel}` : placeholder);
</script>

<div class="wrap">
  {#if compact}
    <button class="chip" onclick={toggle} title="Model"><Cpu size={13} /> {label}</button>
  {:else}
    <button class="select-like" onclick={toggle}><Cpu size={14} /><span class="spacer truncate">{label}</span><ChevronDown size={14} /></button>
  {/if}
  {#if open}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="scrim" onclick={() => (open = false)}></div>
    <div class="pop" class:wide={!compact}>
      <!-- svelte-ignore a11y_autofocus -->
      <input class="input q" placeholder="Filter models…" bind:value={q} autofocus />
      <div class="list">
        {#if allowDefault}
          <button class="opt" onclick={() => ((open = false), onchange(null))}>
            <span class="spacer">Use default <span class="muted">({defaultLabel})</span></span>{#if !value}<Check size={14} />{/if}
          </button>
        {/if}
        {#each store.providers.filter((p) => p.enabled) as p (p.id)}
          <div class="ph"><span class="dot {store.providerHealth(p.id)?.ok === false ? 'bad' : 'ok'}"></span>{p.name}</div>
          {#if typeof models[p.id] === 'string'}
            <div class="perr">{models[p.id]}</div>
          {:else if models[p.id]}
            {#each visible(models[p.id] as ModelInfo[]) as m (m.id)}
              <button class="opt" onclick={() => ((open = false), onchange({ provider_id: p.id, model: m.id }))}>
                <span class="spacer truncate mono">{m.id}</span>
                {#if m.parameter_size}<span class="muted sz">{m.parameter_size}</span>{/if}
                {#if value?.provider_id === p.id && value?.model === m.id}<Check size={14} />{/if}
              </button>
            {:else}
              <div class="perr">No matching models</div>
            {/each}
          {:else}
            <div class="perr"><Loader size={12} class="spin" /> Loading…</div>
          {/if}
        {:else}
          <div class="perr">Add a provider in Settings → Models.</div>
        {/each}
        {#if loading}<div class="perr"><Loader size={12} class="spin" /></div>{/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .wrap {
    position: relative;
  }
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 90;
  }
  .select-like {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    height: 36px;
    padding: 0 10px 0 12px;
    border-radius: var(--r-sm);
    border: 1px solid var(--border-strong);
    background: var(--surface);
    font-size: 13.5px;
    text-align: left;
    color: var(--text);
  }
  .select-like :global(svg) {
    color: var(--text-3);
  }
  .pop {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 100;
    width: 320px;
    padding: 6px;
    border-radius: var(--r);
    background: var(--glass);
    backdrop-filter: blur(28px) saturate(1.6);
    -webkit-backdrop-filter: blur(28px) saturate(1.6);
    border: 1px solid var(--border-strong);
    box-shadow: var(--shadow-lg);
    animation: pop 140ms var(--ease) both;
  }
  .pop.wide {
    left: 0;
    right: auto;
    width: 100%;
    min-width: 320px;
  }
  .q {
    height: 32px;
    margin-bottom: 6px;
  }
  .list {
    max-height: 340px;
    overflow: auto;
  }
  .ph {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 8px 4px;
    font-size: 11px;
    font-weight: 650;
    color: var(--text-3);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .ph .dot {
    width: 6px;
    height: 6px;
    box-shadow: none;
  }
  .opt {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    min-height: 30px;
    padding: 0 8px;
    border: none;
    border-radius: var(--r-xs);
    background: transparent;
    font-size: 12.5px;
    text-align: left;
  }
  .opt:hover {
    background: var(--surface-hover);
  }
  .opt :global(svg) {
    color: var(--accent);
  }
  .mono {
    font-family: var(--mono);
    font-size: 12px;
  }
  .sz {
    font-size: 11px;
  }
  .perr {
    display: flex;
    gap: 6px;
    align-items: center;
    padding: 4px 8px 6px;
    font-size: 12px;
    color: var(--text-3);
  }
</style>
