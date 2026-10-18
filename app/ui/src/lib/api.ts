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
    kind: 'local' | 'ssh' | 'sandbox' | 'container' | 'rymsh'; host?: string; user?: string; port?: number; identity_file?: string;
    sandbox?: string; container?: string; script?: { code: string; lang: string }; cols: number; rows: number;
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

  /** Where other programs reach RAD's API (and the file that announces it). */
  dbApiInfo: () => call<{ url: string | null; discovery_file: string | null; local: boolean }>('db_api_info'),
  /** Games (ggez): build to check, build and run (streams {phase} / {o}), stop. */
  gameBuild: (id: string, files: { path: string; content: string }[]) => call<{ ok: boolean; output: string }>('game_build', { id, files }),
  gameRun(id: string, files: { path: string; content: string }[], assets: { path: string; data: string }[], onEvent: (e: { phase?: string; code?: number | null; o?: string; build_failed?: boolean }) => void) {
    const ch = new Channel<{ phase?: string; code?: number | null; o?: string; build_failed?: boolean }>();
    ch.onmessage = onEvent;
    return call<{ ok: boolean }>('game_run', { id, files, assets, onEvent: ch });
  },
  /** Reads files picked or dropped from Finder (to add as game assets): base64, at most 10 MB each. */
  gameReadFiles: (paths: string[]) => call<{ name: string; size: number; data?: string; error?: string }[]>('game_read_files', { paths }),
  /** ZX Spectrum: connects the view (messages: picture bytes, or {size, palette} / {status}). */
  zxAttach(model: '48' | '128', onMsg: (m: ArrayBuffer | Record<string, any>) => void) {
    const ch = new Channel<ArrayBuffer | Record<string, any>>();
    ch.onmessage = onMsg;
    return call<void>('zx_attach', { model, onFrame: ch });
  },
  zxKeys: (keys: number) => call<void>('zx_keys', { keys }),
  zxJoystick: (bits: number) => call<void>('zx_joystick', { bits }),
  /** Loads a tape (TZX/TAP: reset + LOAD "") or a snapshot (Z80/SNA); data is base64. */
  zxLoad: (name: string, data: string) => call<{ kind: 'tape' | 'snapshot'; names?: string[] }>('zx_load', { name, data }),
  zxSaveState: () => call<string>('zx_save_state'),
  zxControl: (action: string, value?: number) => call<void>('zx_control', { action, value: value ?? null }),
  zxPowerOff: () => call<void>('zx_power_off'),
  /** The live machine: memory (base64, 64K), pokes, cheats (held values), board statistics. */
  zxMemory: () => call<string>('zx_memory'),
  zxPoke: (addr: number, value: number) => call<void>('zx_poke', { addr, value }),
  zxLock: (addr: number, value: number | null) => call<void>('zx_lock', { addr, value }),
  zxStats: (on: boolean) => call<void>('zx_stats', { on }),
  /** The Spectrum lab: analyse a game, list and export its code, patch it. */
  zxlabOpen: (id: string, name: string, data: string, seconds?: number) => call<T.LabSummary>('zxlab_open', { id, name, data, seconds: seconds ?? null }),
  zxlabListing: (id: string, from: number, to: number, notes: T.LabNotes) => call<T.LabLine[]>('zxlab_listing', { id, from, to, notes }),
  zxlabXrefs: (id: string, addr: number, notes: T.LabNotes) => call<{ addr: number; text: string }[]>('zxlab_xrefs', { id, addr, notes }),
  zxlabMemory: (id: string, from: number, len: number) => call<string>('zxlab_memory', { id, from, len }),
  zxlabTapeBlock: (id: string, index: number) => call<string>('zxlab_tape_block', { id, index }),
  zxlabSource: (id: string, from: number, to: number, notes: T.LabNotes, title: string) => call<{ source: string; verified: boolean; note?: string }>('zxlab_source', { id, from, to, notes, title }),
  zxlabAssemblePatch: (id: string, addr: number, code: string, notes: T.LabNotes) =>
    call<{ ok: boolean; bytes?: string; end?: number; len?: number; splits?: number | null; messages: string }>('zxlab_assemble_patch', { id, addr, code, notes }),
  zxlabBuild: (id: string, patches: { addr: number; bytes: string }[], check?: boolean) =>
    call<{ file: { name: string; data: string }; check?: { crashed: boolean; screen: string[]; report: { code: string; message: string; line: number; statement: number } | null } }>('zxlab_build', { id, patches, check: check ?? null }),
  zxlabClose: (id: string) => call<void>('zxlab_close', { id }),
  /** Which Spectrum languages can be built here (BASIC always; assembler when sjasmplus is built). */
  zxTools: () => call<{ basic: boolean; asm: boolean }>('zx_tools'),
  /** Builds a Spectrum project ('zx-basic' | 'zx-asm'). */
  zxBuild: (kind: string, name: string, files: { path: string; content: string }[]) =>
    call<{ ok: boolean; output: string; errors: { file: string; line: number | null; message: string; basic_line?: number | null; kind?: string }[]; file?: { name: string; data: string } }>('zx_build', { kind, name, files }),
  /** Runs a built program on a hidden Spectrum: the screen as text, the BASIC report, a crash. */
  zxCheck: (name: string, data: string, seconds?: number) =>
    call<{ started: boolean; crashed: boolean; report: { code: string; message: string; line: number; statement: number } | null; screen: string[] }>('zx_check', { name, data, seconds: seconds ?? null }),
  /** Commodore 64: connects the view (messages: picture bytes, or {size, palette} / {status}). */
  c64Attach(onMsg: (m: ArrayBuffer | Record<string, any>) => void) {
    const ch = new Channel<ArrayBuffer | Record<string, any>>();
    ch.onmessage = onMsg;
    return call<void>('c64_attach', { onFrame: ch });
  },
  /** Keys held: their places in the keyboard matrix (row × 8 + column; 64 is RESTORE). */
  c64Keys: (keys: number[]) => call<void>('c64_keys', { keys }),
  c64Joystick: (bits: number) => call<void>('c64_joystick', { bits }),
  c64JoystickPort: (port: 1 | 2) => call<void>('c64_joystick_port', { port }),
  /** Loads a program (PRG/P00/T64), tape (TAP), disk (D64), cartridge (CRT) or save (C64S); base64. */
  c64Load: (name: string, data: string) => call<{ kind: 'program' | 'tape' | 'disk' | 'cartridge' | 'snapshot'; names?: string[] }>('c64_load', { name, data }),
  c64InsertDisk: (name: string, data: string) => call<string[]>('c64_insert_disk', { name, data }),
  c64SaveState: () => call<string>('c64_save_state'),
  c64Control: (action: string, value?: number) => call<void>('c64_control', { action, value: value ?? null }),
  c64Memory: () => call<string>('c64_memory'),
  c64Poke: (addr: number, value: number) => call<void>('c64_poke', { addr, value }),
  c64Lock: (addr: number, value: number | null) => call<void>('c64_lock', { addr, value }),
  c64PowerOff: () => call<void>('c64_power_off'),
  /** Uses the user's ROMs (base64), or the free Open ROMs when null. */
  c64SetRoms: (chars: string | null, basic: string | null, kernal: string | null) => call<'open' | 'commodore'>('c64_set_roms', { chars, basic, kernal }),
  c64ReadRoms: (paths: string[]) => call<{ roms: { chars?: string; basic?: string; kernal?: string }; skipped: string[] }>('c64_read_roms', { paths }),
  c64FindRoms: () => call<string[]>('c64_find_roms'),
  /** Which C64 languages can be built here (BASIC always; assembler when 64tass is built) and which ROMs run. */
  c64Tools: () => call<{ basic: boolean; asm: boolean; roms: 'open' | 'commodore' }>('c64_tools'),
  /** Builds a C64 project ('c64-basic' | 'c64-asm') into a PRG. */
  c64Build: (kind: string, name: string, files: { path: string; content: string }[]) =>
    call<{ ok: boolean; output: string; errors: { file: string; line: number | null; message: string; basic_line?: number | null; kind?: string }[]; file?: { name: string; data: string } }>('c64_build', { kind, name, files }),
  /** Runs a built program on a hidden C64: the screen as text, a BASIC error, a crash (JAM). */
  c64Check: (name: string, data: string, seconds?: number, basic?: boolean) =>
    call<{ needs_roms?: boolean; roms: 'open' | 'commodore'; started?: boolean; crashed?: boolean; error?: { message: string; line: number | null } | null; screen?: string[]; border?: number }>('c64_check', { name, data, seconds: seconds ?? null, basic: basic ?? null }),
  /** The C64 lab: the same as the Spectrum lab's, for C64 games (6502, 64tass). */
  c64labOpen: (id: string, name: string, data: string, seconds?: number) => call<T.LabSummary>('c64lab_open', { id, name, data, seconds: seconds ?? null }),
  c64labListing: (id: string, from: number, to: number, notes: T.LabNotes) => call<T.LabLine[]>('c64lab_listing', { id, from, to, notes }),
  c64labXrefs: (id: string, addr: number, notes: T.LabNotes) => call<{ addr: number; text: string }[]>('c64lab_xrefs', { id, addr, notes }),
  c64labMemory: (id: string, from: number, len: number) => call<string>('c64lab_memory', { id, from, len }),
  c64labSource: (id: string, from: number, to: number, notes: T.LabNotes, title: string) => call<{ source: string; verified: boolean; note?: string }>('c64lab_source', { id, from, to, notes, title }),
  c64labAssemblePatch: (id: string, addr: number, code: string, notes: T.LabNotes) =>
    call<{ ok: boolean; bytes?: string; end?: number; len?: number; splits?: number | null; messages: string }>('c64lab_assemble_patch', { id, addr, code, notes }),
  c64labBuild: (id: string, patches: { addr: number; bytes: string }[], check?: boolean) =>
    call<{ file: { name: string; data: string }; check?: { crashed: boolean; screen: string[]; error: { message: string; line: number | null } | null } }>('c64lab_build', { id, patches, check: check ?? null }),
  c64labClose: (id: string) => call<void>('c64lab_close', { id }),
  gameStop: (id: string) => call<void>('game_stop', { id }),
  /** Searches the local ggez 0.10 docs (guides, examples, API source). */
  gameDocs: (query: string, limit?: number) =>
    call<{ available: boolean; results: { file: string; line: number; text: string }[] }>('game_docs', { query, limit: limit ?? null }),
  gameStatus: () => call<{ running: string[]; building: string[]; rust: boolean }>('game_status'),
  sidexStatus: () => call<T.SidexStatus>('sidex_status'),
  /** Opens SideX (the code editor), on `folder` if given. */
  sidexOpen: (folder?: string) => call<void>('sidex_open', { folder: folder ?? null }),
  sidexForget: (folder?: string) => call<void>('sidex_forget', { folder: folder ?? null }),

  containerStatus: () => call<T.ContainerStatus>('container_status'),
  containerCreate: (kind: T.ContainerKindId, name?: string) => call<T.ContainerInfo>('container_create', { args: { kind, name: name ?? null } }),
  containerStart: (id: string) => call<void>('container_start', { id }),
  containerStop: (id: string) => call<void>('container_stop', { id }),
  containerReset: (id: string) => call<void>('container_reset', { id }),
  containerDelete: (id: string) => call<void>('container_delete', { id }),
  containerUpdate: (args: { id: string; name?: string; cpus?: number; mem_mb?: number; network?: boolean; ports?: number[] }) => call<T.ContainerInfo>('container_update', { args }),
  /** Runs a shell command in a container and returns its output; `cwd` carries over like a shell's. */
  containerExec: (id: string, command: string, cwd?: string | null, timeoutSecs?: number) =>
    call<T.ExecResult>('container_exec', { id, command, cwd: cwd ?? null, timeoutSecs: timeoutSecs ?? null }),
  /** Runs a command, streaming `{o}` / `{e}` text; resolves to the exit code (null if cancelled). */
  containerRun(id: string, command: string, runId: string, onEvent: (e: { o?: string; e?: string }) => void) {
    const ch = new Channel<{ o?: string; e?: string }>();
    ch.onmessage = onEvent;
    return call<number | null>('container_run', { id, command, runId, onEvent: ch });
  },
  containerCancel: (runId: string) => call<void>('container_cancel', { runId }),
  containerRead: (id: string, path: string) => call<string>('container_read', { id, path }),
  containerWrite: (id: string, path: string, data: string) => call<void>('container_write', { id, path, data }),
  containerList: (id: string, path: string) => call<T.ContainerEntry[]>('container_list', { id, path }),
  containerSharedDir: (id: string) => call<string>('container_shared_dir', { id }),

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
