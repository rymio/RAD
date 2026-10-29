<script lang="ts">
  // Settings → AI API server: shares the models RAD runs with other programs and computers in
  // the OpenAI, Anthropic and Ollama dialects (the server is crates/rad-server/src/aiserver).
  import { onMount } from 'svelte';
  import { ask } from '@tauri-apps/plugin-dialog';
  import { api } from '../lib/api';
  import { store } from '../lib/state.svelte';
  import { formatBytes, timeAgo } from '../lib/util';
  import { Loader, Plus, Trash2, Copy, X, CircleAlert, KeyRound } from '@lucide/svelte';

  interface Usage { requests: number; errors: number; tokens_in: number; tokens_out: number; tokens_per_second: number | null; last_used: number | null }
  interface Config { enabled: boolean; port: number; lan: boolean; require_key: boolean; share_providers: boolean; hidden: string[] }
  interface Status {
    config: Config;
    running: boolean;
    error: string | null;
    urls: string[];
    since: number | null;
    requests: number;
    active: number;
    apis: Record<string, number>;
    totals: Usage;
    keys: { id: string; name: string; prefix: string; created_at: number; usage: Usage }[];
    models: { name: string; kind: 'chat' | 'embedding'; owned_by: string; builtin: boolean; shared: boolean; loaded: boolean; size: number | null; parameters: string | null; usage: Usage }[];
  }

  let st = $state<Status | null>(null);
  let saving = $state(false);
  let port = $state('');
  let keyName = $state('');
  let newKey = $state<string | null>(null);
  let tick = $state(Date.now());

  const cfg = $derived(st?.config);
  const url = $derived(st?.urls.at(-1) ?? `http://127.0.0.1:${cfg?.port ?? 11435}`);
  const exampleKey = $derived(cfg?.require_key ? (newKey ?? 'radai_…') : 'none');
  const firstChat = $derived(st?.models.find((m) => m.shared && m.kind === 'chat')?.name ?? 'MODEL');

  async function refresh() {
    try {
      st = await api.get<Status>('/v1/ai-server');
      if (!port) port = String(st.config.port);
    } catch (e) {
      store.error(e, 'Couldn’t load the AI API server');
    }
  }

  async function save(patch: Partial<Config>) {
    if (!cfg) return;
    saving = true;
    try {
      st = await api.put<Status>('/v1/ai-server', { ...cfg, ...patch });
      port = String(st.config.port);
      if (st.error) store.toast('error', 'The AI API server couldn’t start', st.error);
    } catch (e) {
      store.error(e, 'Couldn’t change the AI API server');
      port = String(cfg.port);
    } finally {
      saving = false;
    }
  }

  function savePort() {
    const p = Number(port);
    if (!cfg || p === cfg.port) return;
    if (!Number.isInteger(p) || p < 1024 || p > 65535) {
      store.toast('error', 'Choose a port from 1024 to 65535');
      port = String(cfg.port);
      return;
    }
    save({ port: p });
  }

  async function setLan(on: boolean) {
    if (on && !(await ask('Other computers on your network will be able to reach the AI API server. Keep “Require an API key” on unless you trust everyone on this network.', { title: 'Share on the local network', kind: 'warning' }))) return;
    save({ lan: on });
  }

  async function setRequireKey(on: boolean) {
    if (!on && !(await ask(`Without keys, any program${cfg?.lan ? ' on your network' : ' on this computer'} can use your models${cfg?.share_providers ? ' — including the paid ones from your other providers' : ''}. Web pages on other sites are still turned away.`, { title: 'Allow use without a key', kind: 'warning' }))) return;
    save({ require_key: on });
  }

  function share(name: string, on: boolean) {
    if (!cfg) return;
    save({ hidden: on ? cfg.hidden.filter((h) => h !== name) : [...cfg.hidden, name] });
  }

  async function createKey(e: Event) {
    e.preventDefault();
    const name = keyName.trim();
    if (!name) return;
    try {
      const r = await api.post<{ key: string }>('/v1/ai-server/keys', { name });
      newKey = r.key;
      keyName = '';
      await refresh();
    } catch (err) {
      store.error(err, 'Couldn’t create the key');
    }
  }

  async function deleteKey(k: Status['keys'][number]) {
    if (!(await ask(`Delete the API key “${k.name}”? Programs using it lose access at once.`, { title: 'Delete API key', kind: 'warning' }))) return;
    await api.del(`/v1/ai-server/keys/${k.id}`).catch((e) => store.error(e));
    refresh();
  }

  function copy(text: string) {
    navigator.clipboard.writeText(text).then(() => store.toast('success', 'Copied', undefined, 1500));
  }

  const num = (n: number) => n.toLocaleString('en-GB');
  function uptime(since: number) {
    const s = Math.max(0, Math.floor((tick - since) / 1000));
    if (s < 60) return `${s} s`;
    if (s < 3600) return `${Math.floor(s / 60)} min`;
    if (s < 86400) return `${Math.floor(s / 3600)} h ${Math.floor(s / 60) % 60} min`;
    return `${Math.floor(s / 86400)} d ${Math.floor(s / 3600) % 24} h`;
  }

  onMount(() => {
    refresh();
    // The statistics move while programs use the server.
    const t = setInterval(() => {
      tick = Date.now();
      if (st?.running && !saving) refresh();
    }, 3000);
    return () => clearInterval(t);
  });
</script>

<h2>AI API server</h2>
<p class="lead">
  Share the models RAD runs with other programs and computers. They connect as if RAD were <strong>OpenAI</strong>, <strong>Anthropic</strong> or <strong>Ollama</strong>, so most AI tools work with it as they are.
  It shares your downloaded built-in models and, if you choose, the models of your other providers. It runs while RAD is unlocked.
</p>

{#if st && cfg}
  <div class="card prov">
    <div class="ph">
      <div class="pn spacer">
        <div class="st">
          AI API server
          {#if st.running}<span class="pill success">Running</span>{:else if cfg.enabled && st.error}<span class="pill danger">Not running</span>{:else}<span class="pill">Off</span>{/if}
          {#if st.active}<span class="pill accent">{st.active} answering</span>{/if}
        </div>
        <div class="sd">
          {#if st.running}Listening on port {cfg.port}{cfg.lan ? ' on your network' : ' (this computer only)'}{#if st.since} · up {uptime(st.since)}{/if}{:else}Programs can’t reach your models while it’s off.{/if}
        </div>
      </div>
      {#if saving}<Loader size={15} class="spin" />{/if}
      <button class="switch" class:on={cfg.enabled} disabled={saving} onclick={() => save({ enabled: !cfg.enabled })} aria-label="AI API server"></button>
    </div>
    {#if cfg.enabled && st.error}
      <div class="test bad"><CircleAlert size={13} /> {st.error}</div>
    {/if}
    {#if st.running}
      {#each st.urls as u}
        <div class="kv"><code class="selectable">{u}</code><button class="btn sm" onclick={() => copy(u)}><Copy size={13} /> Copy</button></div>
      {/each}
    {/if}
  </div>

  <div class="group">
    <div class="srow">
      <div><div class="st">Port</div><div class="sd">11435 by default. Use 11434 for tools that only look for Ollama there (if Ollama itself isn’t running).</div></div>
      <input class="input port" type="number" min="1024" max="65535" bind:value={port} onchange={savePort} onkeydown={(e) => e.key === 'Enter' && savePort()} disabled={saving} />
    </div>
    <div class="srow">
      <div><div class="st">Share on the local network</div><div class="sd">Other computers on your network can connect. Off: only programs on this computer.</div></div>
      <button class="switch" class:on={cfg.lan} disabled={saving} onclick={() => setLan(!cfg.lan)} aria-label="Share on the local network"></button>
    </div>
    <div class="srow">
      <div><div class="st">Require an API key</div><div class="sd">Programs send a key made below. Turn off for tools that can’t send one (many Ollama clients).</div></div>
      <button class="switch" class:on={cfg.require_key} disabled={saving} onclick={() => setRequireKey(!cfg.require_key)} aria-label="Require an API key"></button>
    </div>
    <div class="srow">
      <div><div class="st">Also share my other providers’ models</div><div class="sd">Ollama, OpenAI, Anthropic… set up under Models, named like “provider/model”. Their use counts against your accounts with them.</div></div>
      <button class="switch" class:on={cfg.share_providers} disabled={saving} onclick={() => save({ share_providers: !cfg.share_providers })} aria-label="Share other providers' models"></button>
    </div>
  </div>

  <h3>Statistics {#if st.since}<span class="sd inline">since {new Date(st.since).toLocaleString('en-GB', { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' })}</span>{/if}</h3>
  <div class="stats">
    <div class="stat"><span class="n">{num(st.requests)}</span><span class="l">requests</span></div>
    <div class="stat"><span class="n">{num(st.totals.tokens_in)}</span><span class="l">tokens in</span></div>
    <div class="stat"><span class="n">{num(st.totals.tokens_out)}</span><span class="l">tokens out</span></div>
    <div class="stat"><span class="n">{st.totals.tokens_per_second ?? '—'}</span><span class="l">tokens / s</span></div>
    <div class="stat"><span class="n">{num(st.totals.errors)}</span><span class="l">errors</span></div>
  </div>
  {#if Object.keys(st.apis).length}
    <p class="sd apis">By API: {Object.entries(st.apis).map(([k, v]) => `${{ openai: 'OpenAI', anthropic: 'Anthropic', ollama: 'Ollama' }[k] ?? k} ${num(v)}`).join(' · ')}</p>
  {/if}

  <h3>Models</h3>
  <div class="group">
    {#each st.models as m (m.name)}
      <div class="srow model">
        <div class="spacer">
          <div class="st">
            <span class="mono">{m.name}</span>
            <span class="pill">{m.kind === 'embedding' ? 'Embeddings' : 'Chat'}</span>
            {#if m.loaded}<span class="pill success">Loaded</span>{/if}
            {#if !m.builtin}<span class="pill warning">{m.owned_by}</span>{/if}
          </div>
          <div class="sd">
            {[m.parameters && `${m.parameters} parameters`, m.size && formatBytes(m.size)].filter(Boolean).join(' · ') || (m.builtin ? 'Built-in' : m.owned_by)}
            {#if m.usage.requests}
              · {num(m.usage.requests)} request{m.usage.requests === 1 ? '' : 's'} · {num(m.usage.tokens_out)} tokens out{#if m.usage.tokens_per_second} · {m.usage.tokens_per_second} tok/s{/if}{#if m.usage.errors} · {num(m.usage.errors)} failed{/if}{#if m.usage.last_used} · {timeAgo(m.usage.last_used)}{/if}
            {/if}
          </div>
        </div>
        <button class="switch" class:on={m.shared} disabled={saving} onclick={() => share(m.name, !m.shared)} aria-label="Share {m.name}" title={m.shared ? 'Shared' : 'Not shared'}></button>
      </div>
    {:else}
      <div class="srow"><div class="sd">No models to share yet — download a built-in model under Models{cfg.share_providers ? ', or check your other providers' : ''}.</div></div>
    {/each}
  </div>

  <h3>API keys</h3>
  {#if newKey}
    <div class="newkey">
      <div><strong>New key</strong> — copy it now, it isn’t shown again.</div>
      <div class="kv"><code class="selectable">{newKey}</code><button class="btn sm" onclick={() => copy(newKey!)}><Copy size={13} /> Copy</button><button class="icon-btn sm" onclick={() => (newKey = null)} aria-label="Dismiss"><X size={13} /></button></div>
    </div>
  {/if}
  <div class="group">
    <form class="srow" onsubmit={createKey}>
      <input class="input spacer" placeholder="Who or what will use it, e.g. Laptop, Open WebUI" bind:value={keyName} />
      <button class="btn sm" disabled={!keyName.trim()}><Plus size={13} /> Create key</button>
    </form>
    {#each st.keys as k (k.id)}
      <div class="srow">
        <KeyRound size={15} />
        <div class="spacer">
          <div class="st">{k.name} <code>{k.prefix}…</code></div>
          <div class="sd">Made {timeAgo(k.created_at)}{#if k.usage.requests} · {num(k.usage.requests)} request{k.usage.requests === 1 ? '' : 's'} · {num(k.usage.tokens_in + k.usage.tokens_out)} tokens · last {timeAgo(k.usage.last_used!)}{:else} · not used since the server started{/if}</div>
        </div>
        <button class="icon-btn" onclick={() => deleteKey(k)} aria-label="Delete key"><Trash2 size={15} /></button>
      </div>
    {/each}
  </div>

  <h3>Connecting</h3>
  <div class="group help">
    <div class="srow"><div><div class="st">OpenAI</div><div class="sd">Base URL <code>{url}/v1</code>, API key <code>{exampleKey}</code> · chat, completions, embeddings, models</div></div></div>
    <div class="srow"><div><div class="st">Anthropic</div><div class="sd">Base URL <code>{url}</code>, API key <code>{exampleKey}</code> · messages (streaming too), token counting, models</div></div></div>
    <div class="srow"><div><div class="st">Ollama</div><div class="sd">Host <code>{url}</code> (e.g. <code>OLLAMA_HOST={url.replace(/^http:\/\//, '')}</code>) · chat, generate, embed, tags, ps, show</div></div></div>
    <div class="srow"><div><div class="st">Statistics</div><div class="sd"><code>{url}/rad/stats</code> — the shared models, which are loaded, and how they’ve been used</div></div></div>
    <div class="srow">
      <code class="selectable example">curl {url}/v1/chat/completions{cfg.require_key ? ` -H "Authorization: Bearer ${exampleKey}"` : ''} -d '{JSON.stringify({ model: firstChat, messages: [{ role: 'user', content: 'Hello!' }] })}'</code>
    </div>
  </div>
  <p class="sd">Built-in models answer one request at a time; others wait their turn. Images work with providers’ vision models, not with built-in ones. Statistics start again whenever the server does.</p>
{:else}
  <Loader size={16} class="spin" />
{/if}

<style>
  .port {
    width: 100px;
    flex: none;
  }
  .kv {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 8px;
  }
  .kv code {
    flex: 1;
    min-width: 0;
    overflow-wrap: anywhere;
    font-size: 12px;
  }
  .newkey {
    padding: 10px 12px;
    margin-bottom: 10px;
    border: 1px solid var(--success);
    border-radius: var(--r-sm);
    background: var(--success-soft);
    font-size: 13px;
  }
  .stats {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(110px, 1fr));
    gap: 8px;
  }
  .stat {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: var(--r);
    background: var(--surface);
  }
  .stat .n {
    font-size: 18px;
    font-weight: 650;
    font-variant-numeric: tabular-nums;
  }
  .stat .l {
    font-size: 12px;
    color: var(--text-3);
  }
  .apis {
    margin: 8px 2px 0;
  }
  .inline {
    display: inline;
    font-weight: 400;
    margin-left: 6px;
  }
  .mono {
    font-family: var(--mono);
    font-size: 12.5px;
  }
  .model .st {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }
  .help .sd code {
    font-size: 11.5px;
  }
  .example {
    font-size: 11.5px;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }
</style>
