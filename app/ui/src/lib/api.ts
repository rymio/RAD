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

  browserStart: (width: number, height: number, scale: number, url?: string) =>
    call<T.BrowserState | null>('browser_start', { width, height, scale, url: url ?? null }),
  browserStop: () => call<void>('browser_stop'),
  browserSend: (cmd: Record<string, unknown>) => call<void>('browser_send', { cmd }),
  /** Changed rows of the page: `[w, h, y0, rows]` (u32 LE), then RGBA. */
  browserFrame: () => invoke<ArrayBuffer>('browser_frame'),
  browserEval: (script: string) => call<unknown>('browser_eval', { script }),
  browserScrape: (maxChars?: number) => call<T.PageScrape>('browser_scrape', { maxChars: maxChars ?? null }),
  browserSnapshot: (text: boolean, image: boolean) => call<T.BrowserSnapshot>('browser_snapshot', { text, image }),
  browserSaveScreenshot: (projectId?: T.Id) => call<T.UploadResponse>('browser_save_screenshot', { projectId: projectId ?? null }),
  browserSavePage: (projectId?: T.Id) => call<T.UploadResponse>('browser_save_page', { projectId: projectId ?? null }),

  termOpen: (args: { kind: 'local' | 'ssh'; host?: string; user?: string; port?: number; identity_file?: string; cols: number; rows: number }) =>
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

  fsPlaces: () => call<T.FsPlace[]>('fs_places'),
  fsList: (path?: string) => call<T.FsListing>('fs_list', { path: path ?? null }),
  fsPreview: (path: string) => call<T.FsPreview>('fs_preview', { path }),
  fsOpen: (path: string) => call<void>('fs_open', { path }),
  fsReveal: (path: string) => call<void>('fs_reveal', { path }),
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
