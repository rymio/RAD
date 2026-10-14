<script lang="ts">
  import { onMount } from 'svelte';
  import { ask } from '@tauri-apps/plugin-dialog';
  import { revealItemInDir } from '@tauri-apps/plugin-opener';
  import { isEnabled as autoIsEnabled, enable as autoEnable, disable as autoDisable } from '@tauri-apps/plugin-autostart';
  import { store } from '../lib/state.svelte';
  import { api, rad } from '../lib/api';
  import { formatBytes, timeAgo, PROJECT_COLORS } from '../lib/util';
  import type { Device, LocalModel, LocalModels, ModelRef, Provider, ProviderKind, Settings as S, VaultInfo, VaultSharing } from '../lib/types';
  import ModelPicker from './ModelPicker.svelte';
  import Modal from './Modal.svelte';
  import RecoveryKit from './RecoveryKit.svelte';
  import VoiceSettings from './VoiceSettings.svelte';
  import EmailSettings from './EmailSettings.svelte';
  import {
    SlidersHorizontal, Palette, Cpu, Search, Brain, MonitorSmartphone, ShieldCheck, Server, Plus, Trash2, Plug, Loader,
    Check, Download, KeyRound, HardDriveDownload, Lock, Wrench, Sun, Moon, Monitor, Circle, CircleAlert, FolderOpen, AudioLines, Mail, FolderInput, Users,
  } from '@lucide/svelte';

  const tabs = [
    { id: 'general', label: 'General', icon: SlidersHorizontal },
    { id: 'appearance', label: 'Appearance', icon: Palette },
    { id: 'models', label: 'Models', icon: Cpu },
    { id: 'voice', label: 'Voice', icon: AudioLines },
    { id: 'email', label: 'Email', icon: Mail },
    { id: 'retrieval', label: 'Answers & search', icon: Search },
    { id: 'memories', label: 'Memories', icon: Brain },
    { id: 'devices', label: 'Devices', icon: MonitorSmartphone },
    { id: 'vault', label: 'Vault & security', icon: ShieldCheck },
    { id: 'server', label: 'Storage', icon: Server },
  ] as const;
  const visibleTabs = $derived(tabs.filter((t) => !(store.isLocal && t.id === 'devices')));
  type Tab = (typeof tabs)[number]['id'];

  let tab = $state<Tab>(((store.route.kind === 'settings' && store.route.tab) as Tab) || 'general');
  const isMac = document.documentElement.dataset.os === 'macos';

  // ---------------------------------------------------------------- general
  let autostart = $state(false);
  onMount(async () => {
    autostart = await autoIsEnabled().catch(() => false);
  });
  async function setAutostart(v: boolean) {
    try {
      v ? await autoEnable() : await autoDisable();
      autostart = v;
    } catch (e) {
      store.error(e);
    }
  }
  const cfg = $derived(store.config!);

  // ---------------------------------------------------------------- settings (server-side)
  let s = $state<S | null>(store.settings ? { ...store.settings } : null);
  $effect(() => {
    if (store.settings && !s) s = { ...store.settings };
  });
  let savingS = $state(false);
  async function saveSettings(patch: Partial<S>) {
    if (!s) return;
    s = { ...s, ...patch };
    savingS = true;
    try {
      store.settings = await api.put<S>('/v1/settings', s);
      s = { ...store.settings };
    } catch (e) {
      store.error(e, 'Saving settings failed');
    } finally {
      savingS = false;
    }
  }

  // ---------------------------------------------------------------- providers
  const presets: { name: string; kind: ProviderKind; url: string; key: boolean; hint?: string }[] = [
    { name: 'Ollama', kind: 'ollama', url: 'http://localhost:11434', key: false, hint: 'Your Ollama server. Use its LAN address if it runs on another machine.' },
    { name: 'LM Studio', kind: 'openai', url: 'http://localhost:1234/v1', key: false, hint: 'Enable “Serve on local network” in LM Studio if it runs on another machine.' },
    { name: 'OpenAI', kind: 'openai', url: 'https://api.openai.com/v1', key: true },
    { name: 'Anthropic', kind: 'anthropic', url: 'https://api.anthropic.com', key: true },
    { name: 'OpenRouter', kind: 'openai', url: 'https://openrouter.ai/api/v1', key: true },
    { name: 'Groq', kind: 'openai', url: 'https://api.groq.com/openai/v1', key: true, hint: 'Fast and inexpensive Whisper transcription.' },
    { name: 'Google Gemini', kind: 'openai', url: 'https://generativelanguage.googleapis.com/v1beta/openai', key: true },
    { name: 'Mistral', kind: 'openai', url: 'https://api.mistral.ai/v1', key: true },
    { name: 'Custom (OpenAI-compatible)', kind: 'openai', url: 'http://', key: false, hint: 'vLLM, llama.cpp server, LocalAI, Speaches/faster-whisper…' },
  ];
  let addOpen = $state(false);
  let np = $state({ name: '', kind: 'ollama' as ProviderKind, base_url: '', api_key: '', preset: 0 });
  let testing = $state<Record<number, string>>({});
  let pullModel = $state('');
  let pulling = $state<{ provider: number; status: string; pct: number } | null>(null);

  function pickPreset(i: number) {
    const p = presets[i];
    np = { name: p.name, kind: p.kind, base_url: p.url, api_key: '', preset: i };
  }
  async function addProvider(e: Event) {
    e.preventDefault();
    try {
      const p = await api.post<Provider>('/v1/providers', { name: np.name, kind: np.kind, base_url: np.base_url, api_key: np.api_key || undefined });
      store.providers = [...store.providers, p];
      addOpen = false;
      test(p);
    } catch (err) {
      store.error(err, 'Adding provider failed');
    }
  }
  async function test(p: Provider) {
    testing[p.id] = 'Testing…';
    try {
      const r = await api.post<{ ok: boolean; latency_ms: number | null; models: number; error: string | null }>(`/v1/providers/${p.id}/test`);
      testing[p.id] = r.ok ? `Connected · ${r.models} models · ${r.latency_ms} ms` : `✕ ${r.error}`;
    } catch (e: any) {
      testing[p.id] = `✕ ${e?.message ?? e}`;
    }
  }
  async function toggleProvider(p: Provider) {
    await api.patch(`/v1/providers/${p.id}`, { enabled: !p.enabled }).catch((e) => store.error(e));
    store.reloadProviders();
  }
  async function setKey(p: Provider) {
    const key = prompt(`API key for ${p.name} (leave empty to remove)`);
    if (key === null) return;
    await api.patch(`/v1/providers/${p.id}`, { api_key: key }).catch((e) => store.error(e));
    store.reloadProviders();
  }
  async function removeProvider(p: Provider) {
    if (!(await ask(`Remove ${p.name}? Chats and settings using its models fall back to defaults.`, { title: 'Remove provider', kind: 'warning' }))) return;
    await api.del(`/v1/providers/${p.id}`).catch((e) => store.error(e));
    store.reloadProviders();
    store.reloadSettings();
  }
  // Built-in (llama.cpp) models: refreshed every second while a download runs.
  let local = $state<LocalModels | null>(null);
  let localTimer: ReturnType<typeof setTimeout> | undefined;
  async function loadLocal() {
    clearTimeout(localTimer);
    try {
      local = await api.get<LocalModels>('/v1/local-models');
    } catch (e) {
      store.error(e, 'Loading built-in models failed');
      return;
    }
    if (local.models.some((m) => m.downloaded !== null)) localTimer = setTimeout(loadLocal, 1000);
  }
  $effect(() => {
    if (tab === 'models') loadLocal();
    return () => clearTimeout(localTimer);
  });
  async function downloadLocal(m: LocalModel) {
    try {
      await api.post(`/v1/local-models/${m.id}/download`);
      loadLocal();
    } catch (e) {
      store.error(e, `Can't download ${m.name}`);
    }
  }
  async function removeLocal(m: LocalModel) {
    if (!(await ask(`Delete ${m.name} (${formatBytes(m.bytes)})? You can download it again any time.`, { title: 'Delete model', kind: 'warning' }))) return;
    await api.del(`/v1/local-models/${m.id}`).catch((e) => store.error(e));
    loadLocal();
    s = await api.get<S>('/v1/settings');
    store.settings = { ...s };
  }
  const usedForChat = (m: LocalModel) => !!local && s?.chat_model?.provider_id === local.provider_id && s?.chat_model?.model === m.id;
  async function useForChat(m: LocalModel) {
    if (!local?.provider_id) return;
    await saveSettings({ chat_model: { provider_id: local.provider_id, model: m.id } });
    store.toast('success', `${m.name} now answers your chats`, 'Runs on this machine with llama.cpp — no other program needed.');
  }
  const fitLabel = { ok: 'Fits', tight: 'Tight fit', too_big: 'Too big for this machine' } as const;
  const fitClass = { ok: 'success', tight: 'warning', too_big: 'danger' } as const;

  async function pull(p: Provider) {
    if (!pullModel.trim()) return;
    pulling = { provider: p.id, status: 'Starting…', pct: 0 };
    try {
      await rad.pullModel(p.id, pullModel.trim(), (ev) => {
        if (ev.error) throw new Error(ev.error);
        pulling = { provider: p.id, status: ev.status ?? '', pct: ev.total ? Math.round((ev.completed / ev.total) * 100) : (pulling?.pct ?? 0) };
      });
      store.toast('success', `Pulled ${pullModel}`);
      pullModel = '';
    } catch (e) {
      store.error(e, 'Pull failed');
    } finally {
      pulling = null;
    }
  }

  // ---------------------------------------------------------------- memories
  let newMemory = $state('');
  async function addMemory(e: Event) {
    e.preventDefault();
    if (!newMemory.trim()) return;
    await api.post('/v1/memories', { text: newMemory.trim() }).catch((err) => store.error(err));
    newMemory = '';
    store.reloadMemories();
  }

  // ---------------------------------------------------------------- devices
  let devices = $state<Device[]>([]);
  let code = $state<{ code: string; expires_at: number } | null>(null);
  async function loadDevices() {
    devices = await api.get<Device[]>('/v1/devices').catch(() => []);
  }
  async function newCode() {
    code = await api.post<{ code: string; expires_at: number }>('/v1/devices/pairing-code', { ttl_minutes: 10 }).catch((e) => (store.error(e), null));
  }
  async function revoke(d: Device) {
    if (!(await ask(`Revoke “${d.name}”? It will need a new pairing code to connect again.`, { title: 'Revoke device', kind: 'warning' }))) return;
    await api.del(`/v1/devices/${d.id}`).catch((e) => store.error(e));
    loadDevices();
  }

  // ---------------------------------------------------------------- vault
  let info = $state<VaultInfo | null>(null);
  let vaultBusy = $state('');
  let passOpen = $state(false);
  let cur = $state('');
  let nw = $state('');
  let nw2 = $state('');
  let newKit = $state<string | null>(null);
  let kitConfirmed = $state(false);
  async function loadInfo() {
    info = await api.get<VaultInfo>('/v1/vault/info').catch(() => null);
  }
  async function vaultAction(kind: 'backup' | 'verify' | 'compact') {
    vaultBusy = kind;
    try {
      const r: any = await api.post(`/v1/vault/${kind}`);
      if (kind === 'backup') store.toast('success', 'Backup written', `${r.file} · ${formatBytes(r.bytes)}`);
      if (kind === 'verify') store.toast(r.ok ? 'success' : 'error', r.ok ? 'Vault is healthy' : 'Problems found', r.messages.join('\n'));
      if (kind === 'compact') store.toast('success', 'Vault compacted', `${r.removed_blobs} unused blobs removed`);
      loadInfo();
    } catch (e) {
      store.error(e);
    } finally {
      vaultBusy = '';
    }
  }
  async function changePass(e: Event) {
    e.preventDefault();
    if (nw !== nw2) return store.toast('error', 'The new passphrases don’t match');
    vaultBusy = 'pass';
    try {
      const r = await api.post<{ recovery_key: string }>('/v1/vault/passphrase', { current: cur, new: nw });
      cur = nw = nw2 = '';
      passOpen = false;
      newKit = r.recovery_key;
    } catch (err) {
      store.error(err, 'Changing passphrase failed');
    } finally {
      vaultBusy = '';
    }
  }
  async function lockServer() {
    if (!(await ask('Lock the vault on the server? Every device is signed out and background indexing pauses until someone unlocks again.', { title: 'Lock server vault', kind: 'warning' }))) return;
    store.lock(true);
  }

  $effect(() => {
    if (tab === 'devices') loadDevices();
    if (tab === 'vault') loadInfo();
  });

  async function revealVault() {
    if (!cfg?.local_vault) return;
    const { revealItemInDir } = await import('@tauri-apps/plugin-opener');
    revealItemInDir(cfg.local_vault).catch((e) => store.error(e));
  }

  // ---- where the vault lives, and sharing it between computers
  let sharing = $state<VaultSharing | null>(null);
  let sharingBusy = $state(false);
  let moveTo = $state<string | null>(null);

  async function loadSharing() {
    sharing = await api.get<VaultSharing>('/v1/vault/sharing').catch(() => null);
  }
  $effect(() => {
    if (tab !== 'server' || !store.isLocal) return;
    loadSharing();
    const t = setInterval(loadSharing, 10_000);
    return () => clearInterval(t);
  });

  async function setSharing(on: boolean) {
    sharingBusy = true;
    try {
      sharing = await api.put<VaultSharing>('/v1/vault/sharing', { shared: on });
      store.toast('success', on ? 'Sharing is on' : 'Sharing is off', on ? 'Other computers can now open this vault at the same time.' : 'Only one computer at a time can open this vault.');
    } catch (e) {
      store.error(e, 'Couldn’t change sharing');
    } finally {
      sharingBusy = false;
    }
  }

  async function pickMoveFolder() {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const d = await open({ directory: true, title: 'Move the vault to…' });
    if (typeof d === 'string') moveTo = d;
  }

  async function moveVault() {
    const folder = moveTo;
    moveTo = null;
    if (!folder) return;
    store.vaultMove = { done: 0, total: 1 };
    try {
      await rad.vaultRelocate(folder);
      store.config = await rad.config();
      store.toast('success', 'Vault moved', `It’s now in ${folder}. Unlock it to carry on.`);
    } catch (e) {
      store.error(e, 'Couldn’t move the vault');
    } finally {
      store.vaultMove = null;
    }
  }

  async function switchVault() {
    if (!(await ask('Lock this vault and choose another one?', { title: 'Switch vault' }))) return;
    await rad.forgetServer();
    store.config = await rad.config();
    store.status = null;
    store.phase = 'connect';
  }

  async function disconnect() {
    if (!(await ask('Disconnect from this server? This device forgets its pairing; your vault stays safe on the server.', { title: 'Disconnect', kind: 'warning' }))) return;
    await rad.forgetServer();
    store.status = null;
    store.phase = 'connect';
  }

  const themes = [
    { id: 'system', label: 'System', icon: Monitor },
    { id: 'light', label: 'Light', icon: Sun },
    { id: 'dark', label: 'Dark', icon: Moon },
    { id: 'oled', label: 'Midnight', icon: Circle },
  ];

  const roles: { key: keyof S; label: string; hint: string; filter: 'chat' | 'embed' | '' }[] = [
    { key: 'chat_model', label: 'Chat', hint: 'Answers your questions. Bigger models give better answers.', filter: 'chat' },
    { key: 'embedding_model', label: 'Embeddings', hint: 'Powers semantic search. The built-in model works offline; changing it re-indexes your vault in the background.', filter: 'embed' },
    { key: 'vision_model', label: 'Vision', hint: 'Reads screenshots and images (e.g. qwen2.5vl, gemma 3/4, llama3.2-vision, GPT-4o, Claude).', filter: 'chat' },
    { key: 'transcription_model', label: 'Transcription', hint: 'Turns MP3/MP4 into searchable text (whisper-1, whisper-large-v3…). OpenAI-compatible providers only.', filter: '' },
    { key: 'rewrite_model', label: 'Query rewriting', hint: 'Optional small, fast model that turns follow-ups into standalone searches. Defaults to the chat model.', filter: 'chat' },
  ];
</script>

<div class="settings">
  <header class="drag"><h1>Settings</h1></header>
  <div class="body">
    <nav>
      {#each visibleTabs as t}
        <button class:on={tab === t.id} onclick={() => (tab = t.id)}><t.icon size={15} /> {t.label}</button>
      {/each}
    </nav>

    <section class="pane">
      {#if tab === 'general' && cfg}
        <h2>General</h2>
        <div class="group">
          <div class="srow">
            <div><div class="st">Device name</div><div class="sd">How this computer appears in Devices.</div></div>
            <input class="input w240" value={cfg.device_name} onchange={(e) => store.saveConfig({ device_name: e.currentTarget.value })} />
          </div>
          <div class="srow">
            <div><div class="st">Lock after inactivity</div><div class="sd">RAD asks for your passphrase again after this much idle time.</div></div>
            <select class="select w240" value={String(cfg.auto_lock_minutes)} onchange={(e) => store.saveConfig({ auto_lock_minutes: +e.currentTarget.value })}>
              {#each [[5, '5 minutes'], [15, '15 minutes'], [30, '30 minutes'], [60, '1 hour'], [240, '4 hours'], [0, 'Never']] as [v, l]}<option value={String(v)}>{l}</option>{/each}
            </select>
          </div>
          <div class="srow">
            <div><div class="st">Lock when the computer sleeps</div></div>
            <button class="switch" class:on={cfg.lock_on_sleep} onclick={() => store.saveConfig({ lock_on_sleep: !cfg.lock_on_sleep })} aria-label="Lock on sleep"></button>
          </div>
        </div>
        <div class="group">
          <div class="srow">
            <div><div class="st">Launch at login</div><div class="sd">Starts quietly in the {isMac ? 'menu bar' : 'system tray'}, locked.</div></div>
            <button class="switch" class:on={autostart} onclick={() => setAutostart(!autostart)} aria-label="Launch at login"></button>
          </div>
          <div class="srow">
            <div><div class="st">Keep running when the window closes</div><div class="sd">RAD stays in the {isMac ? 'menu bar' : 'tray'} so Quick Ask and screenshots are one keystroke away.</div></div>
            <button class="switch" class:on={cfg.close_to_tray} onclick={() => store.saveConfig({ close_to_tray: !cfg.close_to_tray })} aria-label="Close to tray"></button>
          </div>
          {#if isMac}
            <div class="srow">
              <div><div class="st">Hide Dock icon</div><div class="sd">Run RAD from the menu bar only.</div></div>
              <button class="switch" class:on={cfg.hide_dock_icon} onclick={() => store.saveConfig({ hide_dock_icon: !cfg.hide_dock_icon })} aria-label="Hide dock icon"></button>
            </div>
          {/if}
        </div>
        <h3>Shortcuts</h3>
        <div class="group">
          {#each [['hotkey_quick_ask', 'Quick Ask', 'Floating ask-anything window'], ['hotkey_screenshot', 'Screenshot to RAD', 'Capture straight into your vault'], ['hotkey_lock', 'Lock RAD', '']] as [k, l, d]}
            <div class="srow">
              <div><div class="st">{l}</div>{#if d}<div class="sd">{d}</div>{/if}</div>
              <input class="input w240 mono" value={(cfg as any)[k]} onchange={(e) => store.saveConfig({ [k]: e.currentTarget.value } as any)} placeholder="e.g. Alt+Space" />
            </div>
          {/each}
          <div class="srow">
            <div><div class="st">Screenshots go to</div><div class="sd">Used by the global shortcut and the tray menu.</div></div>
            <select class="select w240" value={String(cfg.screenshot_project_id ?? '')} onchange={(e) => store.saveConfig({ screenshot_project_id: e.currentTarget.value ? +e.currentTarget.value : null })}>
              <option value="">Inbox</option>
              {#each store.projects.filter((p) => !p.archived) as p}<option value={String(p.id)}>{p.name}</option>{/each}
            </select>
          </div>
        </div>
      {:else if tab === 'appearance' && cfg}
        <h2>Appearance</h2>
        <div class="themes">
          {#each themes as t}
            <button class="theme" class:on={cfg.theme === t.id} onclick={() => store.saveConfig({ theme: t.id })}>
              <div class="tprev {t.id}"><span></span><span></span><span></span></div>
              <div class="tl"><t.icon size={14} /> {t.label}</div>
            </button>
          {/each}
        </div>
        <h3>Accent color</h3>
        <div class="swatches">
          {#each PROJECT_COLORS.slice(0, 10) as c}
            <button class="sw" class:sel={cfg.accent === c} style="background:{c}" onclick={() => store.saveConfig({ accent: c })} aria-label={c}></button>
          {/each}
        </div>
      {:else if tab === 'models'}
        <h2>Models</h2>
        {#if store.isLocal}
          <p class="lead">Standalone mode — RAD talks to these providers directly from this computer, so <code>localhost</code> is this machine (e.g. Ollama or LM Studio running here). Keys are stored encrypted inside your vault.</p>
        {:else}
          <p class="lead">RAD's server talks to these providers, so <code>localhost</code> means the server machine. Keys are stored encrypted inside your vault and are never sent to devices.</p>
        {/if}
        <div class="provs">
          {#each store.providers as p (p.id)}
            {@const h = store.providerHealth(p.id)}
            <div class="card prov" class:off={!p.enabled}>
              <div class="ph">
                <span class="dot {!p.enabled ? '' : h ? (h.ok ? 'ok' : 'bad') : 'warn'}"></span>
                {#if p.kind === 'builtin'}
                  <div class="pn"><div class="st">{p.name} <span class="pill">built-in</span></div><div class="sd">Runs the built-in models below {store.isLocal ? 'on this computer' : 'on the server'} — no other program needed.</div></div>
                {:else}
                  <div class="pn"><div class="st">{p.name} <span class="pill">{p.kind === 'openai' ? 'OpenAI-compatible' : p.kind}</span></div><div class="sd mono">{p.base_url}</div></div>
                {/if}
                <span class="spacer"></span>
                <button class="btn sm" onclick={() => test(p)}><Plug size={13} /> Test</button>
                {#if p.kind !== 'builtin'}
                  <button class="btn sm" onclick={() => setKey(p)}><KeyRound size={13} /> {p.has_api_key ? 'Key ✓' : 'API key'}</button>
                {/if}
                <button class="switch" class:on={p.enabled} onclick={() => toggleProvider(p)} aria-label="Enabled"></button>
                {#if p.kind !== 'builtin'}
                  <button class="icon-btn" onclick={() => removeProvider(p)} aria-label="Remove"><Trash2 size={15} /></button>
                {/if}
              </div>
              {#if testing[p.id]}<div class="test" class:bad={testing[p.id].startsWith('✕')}>{testing[p.id]}</div>{/if}
              {#if h && !h.ok && h.error}<div class="test bad">{h.error}</div>{/if}
              {#if p.kind === 'ollama'}
                <div class="pull">
                  <input class="input mono" placeholder="Pull a model, e.g. qwen2.5:7b or nomic-embed-text" bind:value={pullModel} disabled={!!pulling} />
                  <button class="btn" onclick={() => pull(p)} disabled={!!pulling || !pullModel.trim()}><Download size={14} /> Pull</button>
                </div>
                {#if pulling?.provider === p.id}
                  <div class="progress"><div style="width:{pulling.pct}%"></div></div>
                  <div class="sd">{pulling.status} {pulling.pct ? `· ${pulling.pct}%` : ''}</div>
                {/if}
              {/if}
            </div>
          {/each}
          <button class="add card" onclick={() => ((addOpen = true), pickPreset(0))}><Plus size={16} /> Add provider</button>
        </div>

        <h3>Built-in models <span class="pill">llama.cpp</span></h3>
        {#if local}
          <p class="lead">
            Light models that run inside RAD, {store.isLocal ? 'on this computer' : 'on the server'} — offline, nothing else to install.
            Memory: {formatBytes(local.total_memory)}. Kept in <span class="mono selectable">{local.dir}</span>
            {#if store.isLocal}<button class="btn sm" onclick={() => revealItemInDir(local!.dir)}><FolderOpen size={13} /> Open folder</button>{/if}
          </p>
          <div class="provs">
            {#each local.models as m (m.id)}
              {@const pct = m.downloaded !== null ? Math.floor((m.downloaded / m.bytes) * 100) : 0}
              <div class="card prov">
                <div class="ph">
                  <div class="pn spacer">
                    <div class="st">
                      {m.name}
                      <span class="pill">{m.kind === 'chat' ? 'Chat' : 'Embeddings'}</span>
                      {#if m.recommended && m.kind === 'chat'}<span class="pill accent">Recommended</span>{/if}
                      {#if usedForChat(m)}<span class="pill success"><Check size={10} /> Answers your chats</span>{/if}
                    </div>
                    <div class="sd">{m.description}</div>
                    <div class="sd">
                      {m.parameters} · {formatBytes(m.bytes)} · needs ~{formatBytes(m.memory_needed)} memory ·
                      <span class="pill {fitClass[m.fit]}">{fitLabel[m.fit]}</span> · {m.license}
                    </div>
                  </div>
                  {#if m.downloaded !== null}
                    <span class="sd">{pct}%</span>
                  {:else if !m.installed}
                    <button class="btn sm primary" disabled={m.fit === 'too_big'} onclick={() => downloadLocal(m)}><Download size={13} /> Download</button>
                  {:else}
                    {#if m.kind === 'chat' && !usedForChat(m)}
                      <button class="btn sm" onclick={() => useForChat(m)}>Use for chat</button>
                    {/if}
                    {#if m.bundled}
                      <span class="pill">Included</span>
                    {:else}
                      <button class="icon-btn" onclick={() => removeLocal(m)} aria-label="Delete model"><Trash2 size={15} /></button>
                    {/if}
                  {/if}
                </div>
                {#if m.downloaded !== null}
                  <div class="progress"><div style="width:{pct}%"></div></div>
                {/if}
                {#if m.error}<div class="test bad">{m.error}</div>{/if}
              </div>
            {/each}
          </div>
        {/if}

          <div class="provs">
            <button class="card prov tts" onclick={() => (tab = 'voice')}>
              <div class="ph">
                <div class="pn spacer">
                  <div class="st">MioTTS-0.1B <span class="pill">Text to speech</span></div>
                  <div class="sd">Reads answers aloud on this computer — English and Japanese, preset or cloned voices. Set up under Voice.</div>
                </div>
                <AudioLines size={16} />
              </div>
            </button>
          </div>

        <h3>Model roles</h3>
        {#if s}
          <div class="group">
            {#each roles as r}
              <div class="srow">
                <div><div class="st">{r.label}</div><div class="sd">{r.hint}</div></div>
                <div class="w320">
                  <ModelPicker value={(s as any)[r.key] as ModelRef | null} onchange={(m) => saveSettings({ [r.key]: m } as any)} allowDefault={r.key === 'rewrite_model'} filter={r.filter} compact={false} placeholder="Not set" />
                </div>
              </div>
            {/each}
          </div>
        {/if}
      {:else if tab === 'voice'}
        <VoiceSettings />
      {:else if tab === 'email'}
        <EmailSettings />
      {:else if tab === 'retrieval' && s}
        <h2>Answers & search</h2>
        <div class="group">
          <div class="field col">
            <label class="label" for="persona">Persona</label>
            <textarea id="persona" class="textarea" rows="3" value={s.persona} onchange={(e) => saveSettings({ persona: e.currentTarget.value })}></textarea>
            <div class="hint">How RAD should behave. Memories and project instructions are added automatically.</div>
          </div>
          <div class="srow">
            <div><div class="st">Sources per answer</div><div class="sd">More sources = more context, slower answers.</div></div>
            <input class="input w120" type="number" min="1" max="30" value={s.top_k} onchange={(e) => saveSettings({ top_k: +e.currentTarget.value })} />
          </div>
          <div class="srow">
            <div><div class="st">Context budget</div><div class="sd">Maximum characters of your documents sent with each question.</div></div>
            <input class="input w120" type="number" step="1000" value={s.context_chars} onchange={(e) => saveSettings({ context_chars: +e.currentTarget.value })} />
          </div>
          <div class="srow">
            <div><div class="st">Rewrite follow-up questions</div><div class="sd">Resolves “and what about that?” before searching.</div></div>
            <button class="switch" class:on={s.query_rewrite} onclick={() => saveSettings({ query_rewrite: !s!.query_rewrite })} aria-label="Query rewrite"></button>
          </div>
          <div class="srow">
            <div><div class="st">Remember earlier conversations</div><div class="sd">Search your past chats too and cite them, so RAD recalls what you discussed before. Long chats are also summarised so they don't forget their beginning.</div></div>
            <button class="switch" class:on={s.chat_recall} onclick={() => saveSettings({ chat_recall: !s!.chat_recall })} aria-label="Remember earlier conversations"></button>
          </div>
          <div class="srow">
            <div><div class="st">Creativity</div><div class="sd">Temperature {s.temperature.toFixed(1)} — lower is more factual.</div></div>
            <input class="w240" type="range" min="0" max="1.5" step="0.1" value={s.temperature} onchange={(e) => saveSettings({ temperature: +e.currentTarget.value })} />
          </div>
          <div class="srow">
            <div><div class="st">Max answer length</div><div class="sd">Tokens.</div></div>
            <input class="input w120" type="number" step="256" value={s.max_output_tokens} onchange={(e) => saveSettings({ max_output_tokens: +e.currentTarget.value })} />
          </div>
        </div>
        <h3>Indexing</h3>
        <div class="group">
          <div class="srow">
            <div><div class="st">Chunk size</div><div class="sd">Characters per indexed passage. Applies to new or re-indexed files.</div></div>
            <input class="input w120" type="number" step="100" value={s.chunk_chars} onchange={(e) => saveSettings({ chunk_chars: +e.currentTarget.value })} />
          </div>
          <div class="srow">
            <div><div class="st">Overlap</div></div>
            <input class="input w120" type="number" step="50" value={s.chunk_overlap} onchange={(e) => saveSettings({ chunk_overlap: +e.currentTarget.value })} />
          </div>
        </div>
      {:else if tab === 'memories'}
        <h2>Memories</h2>
        <p class="lead">Facts RAD always keeps in mind. Say <em>“Remember that…”</em> in any chat, or add them here.</p>
        <form class="addmem" onsubmit={addMemory}>
          <input class="input" bind:value={newMemory} placeholder="e.g. My partner's name is Anna; her birthday is 14 March" />
          <button class="btn primary" disabled={!newMemory.trim()}><Plus size={14} /> Add</button>
        </form>
        <div class="group">
          {#each store.memories as m (m.id)}
            <div class="srow mem">
              <button class="switch" class:on={m.enabled} onclick={async () => { await api.patch(`/v1/memories/${m.id}`, { enabled: !m.enabled }); store.reloadMemories(); }} aria-label="Enabled"></button>
              <div class="spacer selectable" class:dim={!m.enabled}>{m.text}<div class="sd">{timeAgo(m.created_at)}</div></div>
              <button class="icon-btn" onclick={async () => { await api.del(`/v1/memories/${m.id}`); store.reloadMemories(); }} aria-label="Delete"><Trash2 size={15} /></button>
            </div>
          {:else}
            <div class="srow"><span class="muted">No memories yet.</span></div>
          {/each}
        </div>
      {:else if tab === 'devices'}
        <h2>Devices</h2>
        <p class="lead">Every paired computer can reach your vault, but each one still needs your passphrase to unlock.</p>
        <div class="group">
          {#each devices as d (d.id)}
            <div class="srow">
              <MonitorSmartphone size={18} />
              <div class="spacer"><div class="st">{d.name} {#if d.id === store.status?.device?.id}<span class="pill accent">This device</span>{/if} {#if d.revoked}<span class="pill danger">Revoked</span>{/if}</div><div class="sd">{d.os} · paired {timeAgo(d.created_at)} · last seen {timeAgo(d.last_seen)}</div></div>
              {#if !d.revoked && d.id !== store.status?.device?.id}<button class="btn sm danger" onclick={() => revoke(d)}>Revoke</button>{/if}
            </div>
          {/each}
        </div>
        <div class="pair card">
          <div class="spacer"><div class="st">Add another device</div><div class="sd">Install RAD on your other computer, enter this server's address, then this code.</div></div>
          {#if code}
            <div class="code mono selectable">{code.code}</div>
          {/if}
          <button class="btn primary" onclick={newCode}><Plus size={14} /> {code ? 'New code' : 'Create pairing code'}</button>
        </div>
      {:else if tab === 'vault'}
        <h2>Vault & security</h2>
        {#if info}
          <div class="stats">
            <div class="stat"><div class="v">{formatBytes(info.file_bytes)}</div><div class="k">Vault file</div></div>
            <div class="stat"><div class="v">{info.documents}</div><div class="k">Files</div></div>
            <div class="stat"><div class="v">{info.notes}</div><div class="k">Notes</div></div>
            <div class="stat"><div class="v">{info.chunks}</div><div class="k">Passages</div></div>
            <div class="stat"><div class="v">{info.chunks ? Math.round((info.embedded_chunks / info.chunks) * 100) : 0}%</div><div class="k">Semantic index</div></div>
          </div>
          <div class="group">
            <div class="srow"><div><div class="st">Encrypted vault</div><div class="sd mono selectable">{info.path}</div></div><span class="pill success"><Lock size={10} /> AES-256 · SQLCipher</span></div>
            <div class="srow"><div><div class="st">Embedding model</div><div class="sd">{info.embedding_model ?? 'Not set — keyword search only'}</div></div></div>
          </div>
        {/if}
        <div class="group">
          <div class="srow">
            <div><div class="st">Passphrase</div><div class="sd">Changing it re-encrypts the vault and issues a new recovery kit.</div></div>
            <button class="btn" onclick={() => (passOpen = true)}><KeyRound size={14} /> Change passphrase</button>
          </div>
          <div class="srow">
            {#if store.isLocal}
              <div><div class="st">Lock vault</div><div class="sd">Removes the key from memory. Background indexing pauses until you unlock.</div></div>
              <button class="btn danger" onclick={() => store.lock(true)}><Lock size={14} /> Lock now</button>
            {:else}
              <div><div class="st">Lock the server vault</div><div class="sd">Signs out every device and removes the key from the server's memory.</div></div>
              <button class="btn danger" onclick={lockServer}><Lock size={14} /> Lock everywhere</button>
            {/if}
          </div>
          {#if s}
            <div class="srow">
              <div><div class="st">{store.isLocal ? 'Vault auto-lock' : 'Server auto-lock'}</div><div class="sd">{store.isLocal ? 'Also lock the vault itself after this long without use (the app lock above locks it too).' : 'Lock the server vault when no device has used it for a while (pauses indexing).'}</div></div>
              <select class="select w240" value={String(s.server_auto_lock_minutes)} onchange={(e) => saveSettings({ server_auto_lock_minutes: +e.currentTarget.value })}>
                {#each [[0, 'Never'], [60, '1 hour'], [240, '4 hours'], [720, '12 hours'], [1440, '24 hours']] as [v, l]}<option value={String(v)}>{l}</option>{/each}
              </select>
            </div>
          {/if}
        </div>
        <h3>Backups</h3>
        <div class="group">
          {#if s}
            <div class="srow">
              <div><div class="st">Daily encrypted backup</div><div class="sd">{info?.backup_dir ?? ''} · backups are encrypted and safe to sync to the cloud.</div></div>
              <button class="switch" class:on={s.backup_daily} onclick={() => saveSettings({ backup_daily: !s!.backup_daily })} aria-label="Daily backup"></button>
            </div>
            <div class="srow">
              <div><div class="st">Keep</div></div>
              <input class="input w120" type="number" min="1" max="100" value={s.backup_keep} onchange={(e) => saveSettings({ backup_keep: +e.currentTarget.value })} />
            </div>
          {/if}
          {#each info?.backups.slice(0, 5) ?? [] as b}
            <div class="srow"><HardDriveDownload size={16} /><div class="spacer"><div class="st mono">{b.file}</div><div class="sd">{formatBytes(b.bytes)} · {timeAgo(b.created_at)}</div></div></div>
          {/each}
          <div class="srow btns">
            <button class="btn" onclick={() => vaultAction('backup')} disabled={!!vaultBusy}>{#if vaultBusy === 'backup'}<Loader size={14} class="spin" />{:else}<HardDriveDownload size={14} />{/if} Back up now</button>
            <button class="btn" onclick={() => vaultAction('verify')} disabled={!!vaultBusy}>{#if vaultBusy === 'verify'}<Loader size={14} class="spin" />{:else}<ShieldCheck size={14} />{/if} Verify integrity</button>
            <button class="btn" onclick={() => vaultAction('compact')} disabled={!!vaultBusy}>{#if vaultBusy === 'compact'}<Loader size={14} class="spin" />{:else}<Wrench size={14} />{/if} Compact</button>
          </div>
        </div>
      {:else if tab === 'server' && store.isLocal}
        <h2>Storage</h2>
        <p class="lead">Standalone mode — your vault is a single encrypted file on this computer. No server is involved; RAD runs everything itself.</p>
        <div class="group">
          <div class="srow">
            <div class="spacer"><div class="st">Vault file</div><div class="sd mono selectable">{cfg?.local_vault}</div>{#if sharing}<div class="sd">Location: {sharing.location_label}</div>{/if}</div>
            <button class="btn sm" onclick={revealVault}><FolderOpen size={13} /> Show</button>
            <button class="btn sm" onclick={pickMoveFolder}><FolderInput size={13} /> Move…</button>
          </div>
          <div class="srow">
            <div><div class="st">Switch vault</div><div class="sd">Open or create another vault file, or connect to a RAD server instead. This vault is locked first.</div></div>
            <button class="btn" onclick={switchVault}>Switch…</button>
          </div>
        </div>
        <h3>Moving to a server later</h3>
        <p class="lead">The vault file is the same format a RAD server uses. Copy it to your server (e.g. <code>/var/lib/rad/personal.radvault</code>), start <code>rad-server</code>, then choose <em>Connect to a RAD server</em> here — same passphrase, all your data.</p>
        {#if sharing?.location === 'synced'}
          <div class="test bad">
            <CircleAlert size={13} />
            <span class="spacer">
              Your vault is in <strong>{sharing.location_label}</strong>. Sync services copy the file between computers instead of sharing it, so use it on one
              computer at a time — RAD won’t open it on a second computer while it’s open here. To use it on several computers at once, move it to a network share.
            </span>
          </div>
        {/if}

        <h3>Several computers</h3>
        <div class="group">
          <div class="srow">
            <div class="spacer">
              <div class="st">Use on several computers at once</div>
              <div class="sd">
                {#if sharing?.location === 'synced'}
                  Not available in {sharing.location_label}. Move the vault to a network share first — a NAS, or a shared folder on another computer.
                {:else if sharing?.location === 'network'}
                  Other computers open this file from the share and unlock it with the same passphrase. Changes show up everywhere within seconds; when two save at once, one waits its turn.
                {:else}
                  For other computers to open this vault too, it has to be on a network share they can all reach — a NAS works best. Move it there, then turn this on.
                {/if}
              </div>
            </div>
            <button
              class="switch"
              class:on={sharing?.shared}
              disabled={!sharing || sharingBusy || sharing.location === 'synced'}
              onclick={() => sharing && setSharing(!sharing.shared)}
              aria-label="Use on several computers at once"
            ></button>
          </div>
          {#each sharing?.sessions ?? [] as s (s.machine + s.since)}
            <div class="srow">
              <Users size={16} />
              <div class="spacer">
                <div class="st">{s.machine} {#if s.this}<span class="pill accent">This computer</span>{/if}</div>
                <div class="sd">Open since {timeAgo(s.since)}</div>
              </div>
            </div>
          {/each}
        </div>
        <p class="lead">Backups are safe to keep in a sync folder. To use it on another computer, choose <em>Open…</em> on RAD’s start screen there and pick this file.</p>
      {:else if tab === 'server'}
        <h2>Server</h2>
        <div class="group">
          <div class="srow"><div><div class="st">{store.status?.name ?? cfg?.server_name}</div><div class="sd mono selectable">{cfg?.server_url}</div></div><span class="pill">v{store.status?.version}</span></div>
          <div class="srow"><div><div class="st">This device</div><div class="sd">{store.status?.device?.name}</div></div></div>
          <div class="srow">
            <div><div class="st">Disconnect</div><div class="sd">Forget this server on this computer. Your vault isn't touched.</div></div>
            <button class="btn danger" onclick={disconnect}>Disconnect</button>
          </div>
        </div>
        <h3>Reaching your server from anywhere</h3>
        <p class="lead">Install <strong>Tailscale</strong> on the server and your computers, then connect to <code>http://&lt;server&gt;.your-tailnet.ts.net:8765</code> — or use <code>tailscale serve</code> for HTTPS. No port forwarding needed.</p>
      {/if}
      {#if savingS}<div class="saving"><Loader size={12} class="spin" /> Saving</div>{/if}
    </section>
  </div>
</div>

<Modal bind:open={addOpen} title="Add a model provider" width={560}>
  <div class="presets">
    {#each presets as p, i}
      <button class="preset" class:on={np.preset === i} onclick={() => pickPreset(i)}>{p.name}</button>
    {/each}
  </div>
  <form onsubmit={addProvider}>
    {#if presets[np.preset]?.hint}<div class="hint" style="margin:0 0 14px">{presets[np.preset].hint}</div>{/if}
    <div class="field"><label class="label" for="pn">Name</label><input id="pn" class="input" bind:value={np.name} /></div>
    <div class="field"><label class="label" for="pu">Base URL</label><input id="pu" class="input mono" bind:value={np.base_url} spellcheck="false" /></div>
    <div class="field">
      <label class="label" for="pk">API key {presets[np.preset]?.key ? '' : '(optional)'}</label>
      <input id="pk" class="input mono" type="password" bind:value={np.api_key} placeholder={presets[np.preset]?.key ? 'sk-…' : 'Only if your server requires one'} />
    </div>
    <div class="row" style="justify-content:flex-end"><button type="button" class="btn ghost" onclick={() => (addOpen = false)}>Cancel</button><button class="btn primary" disabled={!np.base_url}>Add & test</button></div>
  </form>
</Modal>

<Modal open={!!moveTo} title="Move the vault" width={480} onclose={() => (moveTo = null)}>
  <p class="lead" style="margin-top:0">RAD locks the vault, moves the file to</p>
  <p class="mono selectable" style="font-size:12.5px;margin:0 0 12px;overflow-wrap:anywhere">{moveTo}</p>
  <p class="lead">and opens it from there — unlock it with your passphrase as usual. Within one disk it takes a moment; to another disk or a network share the file is copied, checked, and only then removed from the old place.</p>
  <div class="row" style="justify-content:flex-end;margin-top:16px">
    <button class="btn ghost" onclick={() => (moveTo = null)}>Cancel</button>
    <button class="btn primary" onclick={moveVault}><FolderInput size={14} /> Move vault</button>
  </div>
</Modal>

<Modal bind:open={passOpen} title="Change passphrase" width={440}>
  <form onsubmit={changePass}>
    <div class="field"><label class="label" for="c1">Current passphrase</label><input id="c1" class="input" type="password" bind:value={cur} autocomplete="current-password" /></div>
    <div class="field"><label class="label" for="c2">New passphrase</label><input id="c2" class="input" type="password" bind:value={nw} autocomplete="new-password" /></div>
    <div class="field"><label class="label" for="c3">Repeat new passphrase</label><input id="c3" class="input" type="password" bind:value={nw2} autocomplete="new-password" /></div>
    <div class="hint" style="margin-bottom:14px"><CircleAlert size={12} /> Other devices will need the new passphrase. Your old recovery kit stops working.</div>
    <div class="row" style="justify-content:flex-end"><button type="button" class="btn ghost" onclick={() => (passOpen = false)}>Cancel</button><button class="btn primary" disabled={!cur || nw.length < 10 || vaultBusy === 'pass'}>{#if vaultBusy === 'pass'}<Loader size={14} class="spin" />{/if} Re-encrypt vault</button></div>
  </form>
</Modal>

<Modal open={!!newKit} title="Your new recovery kit" width={520} onclose={() => (newKit = null)}>
  {#if newKit}
    <RecoveryKit recoveryKey={newKit} bind:confirmed={kitConfirmed} />
    <div class="row" style="justify-content:flex-end;margin-top:16px"><button class="btn primary" disabled={!kitConfirmed} onclick={() => (newKit = null)}><Check size={14} /> Done</button></div>
  {/if}
</Modal>

<style>
  .settings {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  header {
    flex: none;
    height: var(--titlebar-h);
    display: flex;
    align-items: center;
    padding: 0 26px 0 calc(26px + var(--lead));
    border-bottom: 1px solid var(--border);
  }
  header h1 {
    margin: 0;
    font-size: 15px;
    font-weight: 650;
  }
  .body {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  nav {
    width: 220px;
    flex: none;
    padding: 14px 10px;
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  nav button {
    display: flex;
    align-items: center;
    gap: 10px;
    height: 34px;
    padding: 0 12px;
    border: none;
    border-radius: var(--r-sm);
    background: transparent;
    color: var(--text-2);
    font-size: 13.5px;
    text-align: left;
  }
  nav button:hover {
    background: var(--surface-hover);
    color: var(--text);
  }
  nav button.on {
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 600;
  }
  .pane {
    position: relative;
    flex: 1;
    overflow: auto;
    padding: 26px 34px 60px;
    max-width: 920px;
  }
  .pane :global(h2) {
    margin: 0 0 18px;
    font-size: 22px;
    letter-spacing: -0.02em;
  }
  .pane :global(h3) {
    margin: 28px 0 10px;
    font-size: 13px;
    font-weight: 650;
    color: var(--text-2);
  }
  .pane :global(.lead) {
    color: var(--text-2);
    margin: -6px 0 18px;
    line-height: 1.6;
  }
  .pane :global(code) {
    font-family: var(--mono);
    font-size: 0.9em;
    background: var(--surface-3);
    padding: 1px 5px;
    border-radius: 5px;
  }
  .pane :global(.group) {
    border: 1px solid var(--border);
    border-radius: var(--r-lg);
    background: var(--surface);
    margin-bottom: 14px;
  }
  .pane :global(.srow) {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 13px 16px;
    border-bottom: 1px solid var(--border);
  }
  .pane :global(.srow:last-child) {
    border-bottom: none;
  }
  .pane :global(.srow > div:first-child:not(.spacer)) {
    flex: 1;
    min-width: 0;
  }
  .pane :global(.srow svg) {
    color: var(--text-3);
    flex: none;
  }
  .srow.btns {
    gap: 8px;
  }
  .pane :global(.st) {
    font-weight: 550;
    font-size: 13.5px;
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }
  .pane :global(.sd) {
    font-size: 12.5px;
    color: var(--text-3);
    margin-top: 2px;
    line-height: 1.45;
  }
  .mono {
    font-family: var(--mono);
    font-size: 12px;
  }
  .w120 {
    width: 120px;
  }
  .w240 {
    width: 240px;
  }
  .w320 {
    width: 320px;
    flex: none;
  }
  .field.col {
    padding: 14px 16px;
    margin: 0;
    border-bottom: 1px solid var(--border);
  }
  .dim {
    opacity: 0.5;
  }
  .themes {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 12px;
  }
  .theme {
    padding: 8px;
    border-radius: var(--r-lg);
    border: 1px solid var(--border);
    background: var(--surface);
    text-align: left;
    transition: all var(--t-fast);
  }
  .theme.on {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  .tprev {
    height: 76px;
    border-radius: 10px;
    padding: 10px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .tprev span {
    height: 9px;
    border-radius: 4px;
    background: currentColor;
    opacity: 0.25;
  }
  .tprev span:first-child {
    width: 60%;
    opacity: 0.6;
    background: var(--accent);
  }
  .tprev span:last-child {
    width: 80%;
  }
  .tprev.light {
    background: #f4f4f8;
    color: #222;
  }
  .tprev.dark {
    background: #18181e;
    color: #eee;
  }
  .tprev.oled {
    background: #000;
    color: #eee;
  }
  .tprev.system {
    background: linear-gradient(120deg, #f4f4f8 50%, #18181e 50%);
    color: #888;
  }
  .tl {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 8px 4px 2px;
    font-size: 13px;
    font-weight: 550;
  }
  .swatches {
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
  }
  .sw {
    width: 30px;
    height: 30px;
    border-radius: 10px;
    border: 2px solid transparent;
    transition: transform var(--t-fast) var(--ease-spring);
  }
  .sw:hover {
    transform: scale(1.1);
  }
  .sw.sel {
    border-color: var(--surface);
    box-shadow: 0 0 0 2px var(--text);
  }
  .provs {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .tts {
    width: 100%;
    text-align: left;
    color: inherit;
    font: inherit;
  }
  .pane :global(.prov) {
    padding: 14px 14px 14px 16px;
  }
  .prov.off {
    opacity: 0.65;
  }
  .pane :global(.ph) {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .pane :global(.pn) {
    min-width: 0;
  }
  .pane :global(.test) {
    margin-top: 10px;
    font-size: 12.5px;
    color: var(--success);
  }
  .pane :global(.test.bad) {
    color: var(--danger);
  }
  .pull {
    display: flex;
    gap: 8px;
    margin-top: 12px;
  }
  .pane :global(.progress) {
    height: 5px;
    border-radius: 3px;
    background: var(--surface-3);
    margin-top: 10px;
    overflow: hidden;
  }
  .pane :global(.progress div) {
    height: 100%;
    background: var(--accent);
    transition: width 300ms var(--ease);
  }
  .add {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    height: 52px;
    border-style: dashed;
    color: var(--text-2);
    background: transparent;
  }
  .add:hover {
    color: var(--accent);
    border-color: var(--accent);
  }
  .presets {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-bottom: 16px;
  }
  .preset {
    height: 30px;
    padding: 0 12px;
    border-radius: 999px;
    border: 1px solid var(--border-strong);
    background: var(--surface);
    font-size: 12.5px;
  }
  .preset.on {
    background: var(--accent);
    border-color: transparent;
    color: var(--on-accent);
  }
  .addmem {
    display: flex;
    gap: 8px;
    margin-bottom: 14px;
  }
  .mem {
    align-items: flex-start;
  }
  .pair {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 16px;
  }
  .code {
    font-size: 22px;
    letter-spacing: 0.18em;
    padding: 8px 14px;
    border-radius: var(--r);
    background: var(--accent-soft);
    color: var(--accent);
    font-weight: 600;
  }
  .stats {
    display: grid;
    grid-template-columns: repeat(5, 1fr);
    gap: 10px;
    margin-bottom: 14px;
  }
  .stat {
    padding: 14px;
    border-radius: var(--r-lg);
    border: 1px solid var(--border);
    background: var(--surface);
  }
  .stat .v {
    font-size: 20px;
    font-weight: 700;
    letter-spacing: -0.02em;
    font-variant-numeric: tabular-nums;
  }
  .stat .k {
    font-size: 12px;
    color: var(--text-3);
  }
  .saving {
    position: fixed;
    right: 24px;
    top: 64px;
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-3);
  }
</style>
