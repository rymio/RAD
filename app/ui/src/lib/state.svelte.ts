import { listen, emitTo } from '@tauri-apps/api/event';
import { api, rad as bridge, RadError } from './api';
import type * as T from './types';
import { debounce } from './util';
import { speech } from './speech.svelte';

export type Phase = 'boot' | 'connect' | 'pair' | 'create' | 'recovery-kit' | 'unlock' | 'app';

export interface Draft {
  user: T.Message | null;
  content: string;
  citations: T.Citation[];
  status: string;
  error: string | null;
}

export interface Toast {
  id: number;
  kind: 'info' | 'success' | 'error';
  title: string;
  body?: string;
}

class Store {
  phase = $state<Phase>('boot');
  status = $state<T.ServerStatus | null>(null);
  config = $state<T.ClientConfig | null>(null);
  projects = $state<T.Project[]>([]);
  chats = $state<T.Chat[]>([]);
  settings = $state<T.Settings | null>(null);
  providers = $state<T.Provider[]>([]);
  health = $state<T.ProviderHealth[]>([]);
  memories = $state<T.Memory[]>([]);
  jobs = $state<T.JobSummary>({ queued: 0, running: 0, failed: 0 });
  route = $state<T.Route>({ kind: 'chat', id: null });
  online = $state(true);
  toasts = $state<Toast[]>([]);
  pendingRecoveryKey = $state<string | null>(null);
  paletteOpen = $state(false);
  aboutOpen = $state(false);
  /** Bumped on remote changes so open views refetch. */
  rev = $state({ docs: {} as Record<number, number>, chat: {} as Record<number, number>, any: 0 });
  /** Live document status updates. */
  docStatus = $state<Record<number, { status: T.DocStatus; error: string | null }>>({});
  uploading = $state<{ name: string; index: number; count: number; phase: string; sent?: number; total?: number } | null>(null);
  streamingChats = $state<Record<number, boolean>>({});
  messages = $state<Record<number, T.Message[]>>({});
  drafts = $state<Record<number, Draft>>({});
  /** The conversation docked under the web browser. */
  /** The conversation beside each browser engine. */
  browserChatIds = $state<Record<T.BrowserEngine, number | null>>({ servo: null, chromium: null, tor: null, rymwin: null });
  /** Kept in the vault: browser favourites and SSH hosts. */
  bookmarks = $state<T.Bookmark[]>([]);
  sshHosts = $state<T.SshHost[]>([]);
  /** undefined until loaded from the vault; null when no account is set up. */
  emailAccount = $state<T.EmailAccount | null | undefined>(undefined);
  /** The vault file is being moved to another folder (bytes copied so far). */
  vaultMove = $state<{ done: number; total: number } | null>(null);
  /** Requests for views that may not be open yet (from agent actions, Files…). */
  pendingTerminal = $state<T.TerminalRequest | null>(null);
  /** A program for the ZX Spectrum view to load (from a Games project): name, base64 data, title. */
  pendingZx = $state<{ name: string; data: string; title: string } | null>(null);
  /** A program for the C64 view to load (from a Games project): name, base64 data, title. */
  pendingC64 = $state<{ name: string; data: string; title: string } | null>(null);
  /** Linux sandboxes can run here (engine present). */
  sandboxReady = $state(false);
  pendingBrowserUrl = $state<string | null>(null);
  /** Files on their way into RYMWIN (from Files or the vault), dropped once it's open. */
  pendingRymwinDrop = $state<string[] | null>(null);
  pendingAttachments = $state<T.Document[]>([]);
  private quick = false;

  private toastId = 0;
  private listening = false;

  toast(kind: Toast['kind'], title: string, body?: string, ms = 4200) {
    const id = ++this.toastId;
    this.toasts = [...this.toasts, { id, kind, title, body }];
    setTimeout(() => (this.toasts = this.toasts.filter((t) => t.id !== id)), ms);
  }

  error(e: unknown, title = 'Something went wrong') {
    const msg = e instanceof RadError ? e.message : e instanceof Error ? e.message : String(e);
    this.toast('error', title, msg, 6500);
  }

  navigate(r: T.Route) {
    this.route = r;
  }

  get projectMap() {
    return new Map(this.projects.map((p) => [p.id, p]));
  }

  get isLocal() {
    return this.config?.mode === 'local';
  }

  /** Short description of where the vault lives, for headers and the unlock screen. */
  get whereLabel() {
    if (this.isLocal) {
      const p = this.config?.local_vault ?? '';
      return p.split(/[\\/]/).pop() ?? 'Vault';
    }
    return this.config?.server_name ?? 'RAD server';
  }

  /** `quick`: the Quick Ask window only reads state; the main window owns the connection. */
  async boot(quick = false) {
    this.quick = quick;
    this.config = await bridge.config();
    this.applyTheme();
    this.listen();
    if (quick) {
      this.status = await bridge.status().catch(() => null);
      return;
    }
    try {
      if (this.isLocal && this.config.local_vault) {
        this.status = await bridge.startLocal(this.config.local_vault);
      } else if (this.config.server_url) {
        this.status = await bridge.connect(this.config.server_url);
      } else {
        this.phase = 'connect';
        return;
      }
      this.config = await bridge.config();
      this.online = true;
      this.decidePhase();
    } catch (e) {
      this.online = false;
      this.phase = 'connect';
      this.error(e, this.isLocal ? 'Could not open your vault' : 'Could not reach your RAD server');
    }
  }

  decidePhase() {
    const s = this.status;
    if (!s) return (this.phase = 'connect');
    if (!s.device) return (this.phase = 'pair');
    if (!s.initialized) return (this.phase = 'create');
    if (s.session_valid) return this.enterApp();
    this.phase = 'unlock';
  }

  async enterApp() {
    this.phase = 'app';
    await this.loadAll();
  }

  async loadAll() {
    const [projects, chats, settings, providers, memories, health, jobs] = await Promise.all([
      api.get<T.Project[]>('/v1/projects'),
      api.get<T.Chat[]>('/v1/chats'),
      api.get<T.Settings>('/v1/settings'),
      api.get<T.Provider[]>('/v1/providers'),
      api.get<T.Memory[]>('/v1/memories'),
      api.get<T.ProviderHealth[]>('/v1/health/providers'),
      api.get<T.JobSummary>('/v1/jobs'),
    ]).catch((e) => {
      this.error(e, 'Loading your vault failed');
      return [[], [], null, [], [], [], this.jobs] as const;
    });
    this.projects = projects as T.Project[];
    this.chats = chats as T.Chat[];
    this.settings = settings as T.Settings | null;
    this.providers = providers as T.Provider[];
    this.memories = memories as T.Memory[];
    this.health = health as T.ProviderHealth[];
    this.jobs = jobs as T.JobSummary;
    this.loadAppData('bookmarks');
    this.loadAppData('ssh_hosts');
    this.loadAppData('email_account');
    bridge.sandboxStatus().then((s) => {
      this.sandboxReady = s.supported && s.engine;
      document.documentElement.classList.toggle('sandbox-ok', this.sandboxReady);
    }).catch(() => {});
  }

  /** "Run" on a code block: runs it in a Linux sandbox, in the Terminal. */
  runInSandbox(code: string, lang: string) {
    this.pendingTerminal = { kind: 'sandbox_run', code, lang };
    this.route = { kind: 'terminal' };
  }

  async loadAppData(key: 'bookmarks' | 'ssh_hosts' | 'email_account') {
    const v = await api.get<any>(`/v1/app-data/${key}`).catch(() => null);
    if (key === 'bookmarks') this.bookmarks = Array.isArray(v) ? v : [];
    else if (key === 'email_account') this.emailAccount = v && typeof v === 'object' && v.host ? v : null;
    else this.sshHosts = Array.isArray(v) ? v : [];
  }

  async saveEmailAccount(acc: T.EmailAccount | null) {
    await api.put('/v1/app-data/email_account', acc);
    this.emailAccount = acc;
  }

  async saveBookmarks(list: T.Bookmark[]) {
    this.bookmarks = list;
    await api.put('/v1/app-data/bookmarks', list).catch((e) => this.error(e, 'Couldn’t save favourites'));
  }

  /** Remembers an SSH host for the Terminal's quick-connect list (most recent first). */
  async rememberSshHost(h: T.SshHost) {
    const same = (x: T.SshHost) => x.host === h.host && (x.user ?? '') === (h.user ?? '') && (x.port ?? 22) === (h.port ?? 22);
    const list = [{ ...h, last_used: Date.now() }, ...this.sshHosts.filter((x) => !same(x))].slice(0, 30);
    this.sshHosts = list;
    await api.put('/v1/app-data/ssh_hosts', list).catch(() => {});
  }

  /** Carries out what the assistant asked for. Quick Ask hands it to the main window. */
  async runAction(a: T.AgentAction) {
    if (this.quick) {
      await bridge.openMain();
      await emitTo('main', 'rad-agent-action', a);
      return;
    }
    if (a.kind === 'open_url' && a.url) {
      this.pendingBrowserUrl = a.url;
      this.route = { kind: 'browser' };
    } else if (a.kind === 'ssh' || a.kind === 'terminal') {
      this.pendingTerminal = a;
      this.route = { kind: 'terminal' };
    }
  }

  get sidebarCollapsed() {
    return !!this.config?.sidebar_collapsed;
  }

  toggleSidebar() {
    this.saveConfig({ sidebar_collapsed: !this.sidebarCollapsed });
  }

  reloadProjects = debounce(async () => {
    this.projects = await api.get<T.Project[]>('/v1/projects').catch(() => this.projects);
  }, 150);
  reloadChats = debounce(async () => {
    this.chats = await api.get<T.Chat[]>('/v1/chats').catch(() => this.chats);
  }, 150);
  reloadProviders = debounce(async () => {
    this.providers = await api.get<T.Provider[]>('/v1/providers').catch(() => this.providers);
  }, 150);
  reloadSettings = debounce(async () => {
    this.settings = await api.get<T.Settings>('/v1/settings').catch(() => this.settings);
  }, 150);
  reloadMemories = debounce(async () => {
    this.memories = await api.get<T.Memory[]>('/v1/memories').catch(() => this.memories);
  }, 150);
  reloadJobs = debounce(async () => {
    this.jobs = await api.get<T.JobSummary>('/v1/jobs').catch(() => this.jobs);
  }, 400);

  handleEvent(ev: T.RadEvent) {
    switch (ev.type) {
      case 'change':
        this.rev.any++;
        switch (ev.entity) {
          case 'project':
            this.reloadProjects();
            break;
          case 'document':
            this.reloadProjects();
            this.reloadJobs();
            for (const k of Object.keys(this.rev.docs)) this.rev.docs[+k]++;
            break;
          case 'chat':
            this.reloadChats();
            // Another device changed this chat: refresh it unless we're streaming into it.
            if (!this.streamingChats[ev.entity_id] && !this.drafts[ev.entity_id] && this.messages[ev.entity_id]) {
              this.loadChat(ev.entity_id).catch(() => {});
            }
            break;
          case 'provider':
            this.reloadProviders();
            break;
          case 'settings':
            this.reloadSettings();
            break;
          case 'memory':
            this.reloadMemories();
            break;
          case 'app_data':
            if (ev.op === 'bookmarks' || ev.op === 'ssh_hosts') this.loadAppData(ev.op);
            break;
        }
        break;
      case 'document_status':
        this.docStatus[ev.document_id] = { status: ev.status, error: ev.error };
        this.reloadJobs();
        break;
      case 'provider_health':
        this.health = ev.health;
        break;
    }
  }

  private async listen() {
    if (this.listening) return;
    this.listening = true;
    await listen<T.RadEvent>('rad-event', (e) => this.handleEvent(e.payload));
    await listen('rad-locked', () => {
      if (this.phase === 'app' || this.phase === 'recovery-kit') {
        this.phase = 'unlock';
        this.projects = [];
        this.chats = [];
        this.memories = [];
        this.route = { kind: 'chat', id: null };
      }
    });
    await listen<boolean>('rad-connection', (e) => (this.online = e.payload));
    await listen<T.AgentAction>('rad-agent-action', (e) => this.runAction(e.payload));
    await listen('rad-resync', () => {
      if (this.phase === 'app') this.loadAll();
    });
    await listen<T.ClientConfig>('rad-config', (e) => {
      this.config = e.payload;
      this.applyTheme();
    });
    await listen<{ done: number; total: number }>('rad-vault-move', (e) => {
      this.vaultMove = e.payload.done >= e.payload.total ? null : e.payload;
    });
    await listen<any>('rad-upload-progress', (e) => {
      this.uploading = e.payload.phase === 'done' ? null : e.payload;
    });
    await listen<any>('rad-navigate', (e) => {
      const r = e.payload;
      if (r === 'settings') this.route = { kind: 'settings' };
      else if (r && typeof r === 'object' && 'kind' in r) this.route = r;
    });
    await listen('rad-unlocked', async () => {
      // Another window unlocked (e.g. Quick Ask) — follow along.
      if (this.phase === 'unlock') {
        this.status = await bridge.status().catch(() => this.status);
        await this.enterApp();
      }
    });
    // Report activity for the idle auto-lock (throttled).
    let last = 0;
    const ping = () => {
      const now = Date.now();
      if (now - last > 20000) {
        last = now;
        bridge.activity().catch(() => {});
      }
    };
    for (const ev of ['mousemove', 'keydown', 'mousedown', 'wheel']) window.addEventListener(ev, ping, { passive: true });
    window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => this.applyTheme());
  }

  applyTheme() {
    const c = this.config;
    const root = document.documentElement;
    const pref = c?.theme ?? 'system';
    const dark = window.matchMedia('(prefers-color-scheme: dark)').matches;
    root.dataset.theme = pref === 'system' ? (dark ? 'dark' : 'light') : pref;
    if (c?.accent) root.style.setProperty('--accent', c.accent);
    root.classList.toggle('sb-collapsed', !!c?.sidebar_collapsed);
  }

  async saveConfig(patch: Partial<T.ClientConfig>) {
    if (!this.config) return;
    this.config = await bridge.setConfig({ ...this.config, ...patch });
    this.applyTheme();
  }

  async lock(server = false) {
    await bridge.lock(server);
  }

  // ------------------------------------------------------------ helpers used across views

  async newChat(scope: T.Id[] = []) {
    const chat = await api.post<T.Chat>('/v1/chats', { scope: { projects: scope } });
    this.chats = [chat, ...this.chats.filter((c) => c.id !== chat.id)];
    this.route = { kind: 'chat', id: chat.id };
    return chat;
  }

  /** Project used for chat attachments and screenshots when no project is in focus. */
  async inboxProject(): Promise<T.Project> {
    const preferred = this.config?.screenshot_project_id;
    const p = this.projects.find((p) => p.id === preferred) ?? this.projects.find((p) => p.name === 'Inbox');
    if (p) return p;
    const created = await api.post<T.Project>('/v1/projects', { name: 'Inbox', color: '#64748b', icon: 'inbox', description: 'Pasted images, screenshots and chat attachments' });
    this.projects = [...this.projects, created];
    return created;
  }

  async loadChat(id: T.Id) {
    const r = await api.get<T.ChatWithMessages>(`/v1/chats/${id}`);
    this.messages[id] = r.messages;
    const i = this.chats.findIndex((c) => c.id === id);
    if (i >= 0) this.chats[i] = r.chat;
    else this.chats = [r.chat, ...this.chats];
    return r;
  }

  /** Sends a message and streams the answer into `drafts[chatId]`. */
  async sendMessage(
    chatId: T.Id,
    req: {
      content: string; attachments?: T.Id[]; model?: T.ModelRef | null; truncate_from?: T.Id; regenerate?: boolean;
      /** For this answer only, not stored: e.g. the page open in the browser. */
      context?: string; images?: T.InlineImage[];
    },
  ) {
    const list = this.messages[chatId] ?? [];
    if (req.truncate_from) this.messages[chatId] = list.filter((m) => m.id < req.truncate_from!);
    if (req.regenerate) {
      while (this.messages[chatId]?.length && this.messages[chatId].at(-1)!.role !== 'user') this.messages[chatId].pop();
      const last = this.messages[chatId]?.pop();
      this.drafts[chatId] = { user: last ?? null, content: '', citations: [], status: 'Thinking again…', error: null };
    } else {
      const optimistic: T.Message = {
        id: -Date.now(), chat_id: chatId, role: 'user', content: req.content, attachments: [], citations: [],
        model: null, error: null, tokens_in: null, tokens_out: null, created_at: Date.now(),
      };
      this.drafts[chatId] = { user: optimistic, content: '', citations: [], status: 'Sending…', error: null };
    }
    this.streamingChats[chatId] = true;
    let final: T.Message | null = null;
    try {
      await bridge.chatSend(chatId, { ...req, model: req.model ?? null }, (ev) => {
        const d = this.drafts[chatId];
        if (!d) return;
        switch (ev.type) {
          case 'user_message':
            d.user = ev.message;
            break;
          case 'status':
            d.status = ev.text;
            break;
          case 'sources':
            d.citations = ev.citations;
            break;
          case 'delta':
            d.content += ev.text;
            d.status = '';
            break;
          case 'action':
            this.runAction(ev.action);
            break;
          case 'done':
            final = ev.message;
            break;
          case 'error':
            d.error = ev.message;
            break;
          case 'stopped':
            d.status = '';
            break;
        }
      });
    } catch (e) {
      const d = this.drafts[chatId];
      if (d) d.error = e instanceof Error ? e.message : String(e);
    } finally {
      this.streamingChats[chatId] = false;
    }
    // Replace the draft with what the server stored (or reload if the stream was cut).
    const d = this.drafts[chatId];
    if (final && d?.user) {
      this.messages[chatId] = [...(this.messages[chatId] ?? []), d.user, final];
      delete this.drafts[chatId];
      const answer = final as T.Message;
      if (this.config?.tts_enabled && this.config.tts_auto_read && !answer.error && answer.content.trim()) {
        speech.speak(`m${answer.id}`, answer.content, this.config).catch((e) => this.error(e, 'Couldn’t read the answer aloud'));
      }
    } else {
      await this.loadChat(chatId).catch(() => {});
      if (this.drafts[chatId] && !this.drafts[chatId].error) delete this.drafts[chatId];
    }
    this.reloadChats();
  }

  stopChat(chatId: T.Id) {
    bridge.chatStop(chatId).catch(() => {});
  }

  modelLabel(m: T.ModelRef | null | undefined) {
    if (!m) return 'No model';
    const short = m.model.split('/').pop() ?? m.model;
    return short.replace(/:latest$/, '');
  }

  providerHealth(id: T.Id) {
    return this.health.find((h) => h.provider_id === id);
  }
}

export const store = new Store();
