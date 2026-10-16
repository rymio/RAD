import { invoke, Channel } from '@tauri-apps/api/core';
import type * as T from './types';

export class RadError extends Error {
  code: string;
  constructor(code: string, message: string) {
    super(message);
    this.code = code;
  }
}

async function call<R>(cmd: string, args?: Record<string, unknown>): Promise<R> {
  try {
    return await invoke<R>(cmd, args);
  } catch (e: any) {
    if (e && typeof e === 'object' && 'code' in e) throw new RadError(e.code, e.message);
    throw new RadError('error', String(e));
  }
}

const req = <R>(method: string, path: string, body?: unknown) => call<R>('api', { method, path, body: body ?? null });

export const api = {
  get: <R>(p: string) => req<R>('GET', p),
  post: <R>(p: string, b?: unknown) => req<R>('POST', p, b ?? {}),
  put: <R>(p: string, b: unknown) => req<R>('PUT', p, b),
  patch: <R>(p: string, b: unknown) => req<R>('PATCH', p, b),
  del: (p: string) => req<null>('DELETE', p),
};

export const rad = {
  config: () => call<T.ClientConfig>('get_config'),
  setConfig: (config: T.ClientConfig) => call<T.ClientConfig>('set_config', { config }),
  platform: () => call<string>('platform'),
  connect: (url: string) => call<T.ServerStatus>('connect', { url }),
  startLocal: (path: string) => call<T.ServerStatus>('start_local', { path }),
  status: () => call<T.ServerStatus>('status'),
  pair: (code: string, deviceName?: string) => call<T.ServerStatus>('pair', { code, deviceName }),
  forgetServer: () => call<void>('forget_server'),
  initVault: (passphrase: string) => call<string>('init_vault', { passphrase }),
  unlock: (passphrase: string) => call<void>('unlock', { passphrase }),
  unlockRecovery: (recoveryKey: string, newPassphrase: string) => call<string>('unlock_recovery', { recoveryKey, newPassphrase }),
  lock: (server = false) => call<void>('lock', { server }),
  activity: () => call<void>('activity'),
  openMain: (route?: T.Route) => call<void>('open_main', { route: route ?? null }),
  hideQuick: () => call<void>('hide_quick'),
  toggleQuick: () => call<void>('toggle_quick_cmd'),

  chatSend(chatId: T.Id, request: Record<string, unknown>, onEvent: (e: T.ChatStreamEvent) => void) {
    const ch = new Channel<T.ChatStreamEvent>();
    ch.onmessage = onEvent;
    return call<void>('chat_send', { chatId, request, onEvent: ch });
  },
  chatStop: (chatId: T.Id) => call<void>('chat_stop', { chatId }),
  pullModel(providerId: T.Id, model: string, onEvent: (e: any) => void) {
    const ch = new Channel<any>();
    ch.onmessage = onEvent;
    return call<void>('pull_model', { providerId, model, onEvent: ch });
  },

  uploadPaths: (projectId: T.Id, paths: string[], source = 'upload') =>
    call<Array<T.UploadResponse | { error: string }>>('upload_paths', { args: { project_id: projectId, paths, source } }),
  async uploadBytes(projectId: T.Id, name: string, data: Uint8Array | ArrayBuffer, source = 'paste') {
    try {
      return await invoke<T.UploadResponse>('upload_raw', data instanceof Uint8Array ? data : new Uint8Array(data), {
        headers: { 'x-project-id': String(projectId), 'x-filename': encodeURIComponent(name), 'x-source': source },
      });
    } catch (e: any) {
      throw new RadError(e?.code ?? 'error', e?.message ?? String(e));
    }
  },
  captureScreenshot: (projectId?: T.Id) => call<T.UploadResponse | null>('capture_screenshot', { projectId: projectId ?? null }),
  exportDocument: (documentId: T.Id, dest: string) => call<void>('export_document', { documentId, dest }),

  /** `engine`: 'servo' (Web Browser), 'chromium' (CEF) or 'tor' (Servo over Arti); each has its own process and tabs. */
  browserStart: (engine: T.BrowserEngine, width: number, height: number, scale: number, url?: string) =>
    call<T.BrowserState | null>('browser_start', { engine, width, height, scale, url: url ?? null }),
  browserStop: (engine: T.BrowserEngine) => call<void>('browser_stop', { engine }),
  browserSend: (engine: T.BrowserEngine, cmd: Record<string, unknown>) => call<void>('browser_send', { engine, cmd }),
  /** Raw bytes: [w, h, y0, rows] (u32 LE) then RGBA rows. */
  browserFrame: (engine: T.BrowserEngine) => invoke<ArrayBuffer>('browser_frame', { engine }),
  browserEval: (engine: T.BrowserEngine, script: string) => call<unknown>('browser_eval', { engine, script }),
  browserScrape: (engine: T.BrowserEngine, maxChars?: number) => call<T.PageScrape>('browser_scrape', { engine, maxChars: maxChars ?? null }),
  browserSnapshot: (engine: T.BrowserEngine, text: boolean, image: boolean) => call<T.BrowserSnapshot>('browser_snapshot', { engine, text, image }),
  browserSaveScreenshot: (engine: T.BrowserEngine, projectId?: T.Id) =>
    call<T.UploadResponse>('browser_save_screenshot', { engine, projectId: projectId ?? null }),
  browserSavePage: (engine: T.BrowserEngine, projectId?: T.Id) => call<T.UploadResponse>('browser_save_page', { engine, projectId: projectId ?? null }),

  termOpen: (args: {
    kind: 'local' | 'ssh' | 'sandbox' | 'rymsh'; host?: string; user?: string; port?: number; identity_file?: string;
    sandbox?: string; script?: { code: string; lang: string }; cols: number; rows: number;
  }) =>
    call<T.TermInfo>('term_open', { args }),
  termList: () => call<T.TermInfo[]>('term_list'),
  /** Streams output: ArrayBuffer chunks, or `{ exit }` when the session ends. Replays scrollback first. */
  termAttach(id: number, onData: (d: ArrayBuffer | { exit: string }) => void) {
    const ch = new Channel<ArrayBuffer | { exit: string }>();
    ch.onmessage = onData;
    return call<void>('term_attach', { id, onData: ch });
  },
  termDetach: (id: number) => call<void>('term_detach', { id }),
  termWrite: (id: number, data: string) => call<void>('term_write', { id, data }),
  termResize: (id: number, cols: number, rows: number) => call<void>('term_resize', { id, cols, rows }),
  termClose: (id: number) => call<void>('term_close', { id }),

  /** Runs a command in a sandbox terminal's shell (visibly) and returns what it printed. */
  sandboxExec: (id: number, command: string, timeoutSecs?: number) =>
    call<{ exit_code: number | null; output: string; truncated: boolean; timed_out: boolean }>('sandbox_exec', { id, command, timeoutSecs: timeoutSecs ?? null }),
  /** One model reply for a conversation the app keeps itself (nothing is stored). */
  assist: (system: string, messages: { role: 'user' | 'assistant'; content: string }[]) =>
    req<{ content: string; model: string }>('POST', '/v1/assist', { system, messages }),
  /** The terminal agent on an SSH server or this computer: runs one command (shown in the terminal unless `quiet`). */
  agentExec: (id: number, command: string, timeoutSecs?: number, quiet = false) =>
    call<{ exit_code: number | null; output: string; truncated: boolean; timed_out: boolean; cancelled: boolean; cwd: string | null }>('agent_exec', { id, command, timeoutSecs: timeoutSecs ?? null, quiet }),
  agentCancel: (id: number) => call<void>('agent_cancel', { id }),
  /** Kept in memory for that terminal only; `null` forgets it. */
  agentSetSudo: (id: number, password: string | null) => call<void>('agent_set_sudo', { id, password }),
  /** Copies local files to the server (into `dest`, the agent's folder, or home); returns where. */
  sshUpload: (id: number, paths: string[], dest?: string) => call<string>('ssh_upload', { id, paths, dest: dest ?? null }),
  /** Copies a file from the server into Downloads; returns the local path. */
  sshDownload: (id: number, remote: string) => call<string>('ssh_download', { id, remote }),
  sandboxStatus: () => call<T.SandboxStatus>('sandbox_status'),
  sandboxCreate: (args: { name: string; distro: string; cpus: number; mem_mb: number; disk_gb: number; network: boolean }) =>
    call<T.SandboxInfo>('sandbox_create', { args }),
  sandboxReset: (id: string) => call<void>('sandbox_reset', { id }),
  sandboxDelete: (id: string) => call<void>('sandbox_delete', { id }),
  sandboxUpdate: (args: { id: string; name?: string; cpus?: number; mem_mb?: number; network?: boolean }) => call<T.SandboxInfo>('sandbox_update', { args }),
  sandboxSend: (id: string, paths: string[]) => call<number>('sandbox_send', { id, paths }),
  sandboxSharedDir: (id: string) => call<string>('sandbox_shared_dir', { id }),

  /** Moves this computer's vault file into `folder` and reopens it there (locked). */
  vaultRelocate: (folder: string) => call<T.ServerStatus>('vault_relocate', { folder }),

  fsPlaces: () => call<T.FsPlace[]>('fs_places'),
  fsList: (path?: string) => call<T.FsListing>('fs_list', { path: path ?? null }),
  fsPreview: (path: string) => call<T.FsPreview>('fs_preview', { path }),
  fsOpen: (path: string) => call<void>('fs_open', { path }),
  fsReveal: (path: string) => call<void>('fs_reveal', { path }),

  /** The Music/Video engine: returns the latest `ready`/`status`/`meta`/`tracks` events when it was already running. */
  mediaStart: (player: T.MediaPlayerKind) => call<Record<string, any> | null>('media_start', { player }),
  mediaStop: (player: T.MediaPlayerKind) => call<void>('media_stop', { player }),
  mediaSend: (player: T.MediaPlayerKind, cmd: Record<string, unknown>) => call<void>('media_send', { player, cmd }),
  /** Raw bytes: [w, h, 0, h] (u32 LE) then RGBA; w = 0 when there's no new picture. */
  mediaFrame: (player: T.MediaPlayerKind) => invoke<ArrayBuffer>('media_frame', { player }),
  /** A loopback address the engine streams a vault file from. */
  rymnetStatus: () => call<T.RymnetStatus>('rymnet_status'),
  rymwinPaste: () => call<void>('rymwin_paste'),
  vpnGet: () => call<T.VpnInfo>('vpn_get'),
  vpnSet: (on: boolean) => call<T.VpnInfo>('vpn_set', { on }),
  vpnSave: (settings: Omit<T.VpnSettings, 'mtu' | 'was_on'> & { preshared_key: string | null }) => call<T.VpnInfo>('vpn_save', { settings }),
  vpnGenerateKey: () => call<T.VpnInfo>('vpn_generate_key'),
  vpnImport: (text: string) => call<T.VpnInfo>('vpn_import', { text }),
  vpnImportFile: (path: string) => call<T.VpnInfo>('vpn_import_file', { path }),
  vpnCheckIp: () => call<{ direct: string | null; vpn: string | null; error: string | null }>('vpn_check_ip'),
  rymnetTempDir: () => call<string>('rymnet_temp_dir'),
  rymnetInitiate: (address?: string) => call<string[]>('rymnet_initiate', { address: address ?? null }),
  rymnetRemovePeer: (name: string) => call<void>('rymnet_remove_peer', { name }),
  rymnetStopDaemon: () => call<void>('rymnet_stop_daemon'),
  rymnetGuide: () => call<string>('rymnet_guide'),
  rymnetExec: (args: string[], timeoutSecs?: number) =>
    call<{ exit_code: number | null; output: string; timed_out: boolean }>('rymnet_exec', { args, timeoutSecs: timeoutSecs ?? null }),
  mediaVaultUrl: (documentId: T.Id) => call<string>('media_vault_url', { documentId }),

  mailTest: (account: T.EmailAccount, password: string) => call<number>('mail_test', { account, password }),
  mailSetPassword: (account: T.EmailAccount, password: string | null) => call<void>('mail_set_password', { account, password }),
  mailHasPassword: (account: T.EmailAccount) => call<boolean>('mail_has_password', { account }),
  mailFolders: (account: T.EmailAccount) => call<T.MailFolder[]>('mail_folders', { account }),
  mailList: (account: T.EmailAccount, folder: string, query: string | null, cursor: number | null) =>
    call<T.MailPage>('mail_list', { account, folder, query, cursor }),
  /** Fetching a message marks it as read on the server. */
  mailRead: (account: T.EmailAccount, folder: string, uid: number) => call<T.MailMessage>('mail_read', { account, folder, uid }),
  mailSetFlags: (account: T.EmailAccount, folder: string, uid: number, flags: { seen?: boolean; flagged?: boolean }) =>
    call<void>('mail_set_flags', { account, folder, uid, seen: flags.seen ?? null, flagged: flags.flagged ?? null }),
};

let platformName = '';
export async function initPlatform() {
  platformName = await rad.platform().catch(() => '');
  return platformName;
}
export const platform = () => platformName;

/** URL for streaming vault content through the app's secure `rad:` protocol. */
export function radUrl(path: string) {
  return platformName === 'windows' ? `http://rad.localhost${path}` : `rad://localhost${path}`;
}
export const contentUrl = (id: T.Id) => radUrl(`/v1/documents/${id}/content`);
export const thumbUrl = (id: T.Id, v?: number) => radUrl(`/v1/documents/${id}/thumb`) + (v ? `?v=${v}` : '');
