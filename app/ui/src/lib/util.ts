import type { Anchor, Document } from './types';

export function formatBytes(n: number) {
  if (n < 1024) return `${n} B`;
  const u = ['KB', 'MB', 'GB', 'TB'];
  let i = -1;
  do {
    n /= 1024;
    i++;
  } while (n >= 1024 && i < u.length - 1);
  return `${n < 10 ? n.toFixed(1) : Math.round(n)} ${u[i]}`;
}

export function timeAgo(ms: number) {
  const s = (Date.now() - ms) / 1000;
  if (s < 45) return 'just now';
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  if (s < 86400) return `${Math.round(s / 3600)}h ago`;
  if (s < 86400 * 7) return `${Math.round(s / 86400)}d ago`;
  return new Date(ms).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: s > 86400 * 300 ? 'numeric' : undefined });
}

export function dayBucket(ms: number) {
  const d = new Date(ms);
  const today = new Date();
  const start = new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime();
  if (ms >= start) return 'Today';
  if (ms >= start - 86400000) return 'Yesterday';
  if (ms >= start - 86400000 * 7) return 'Previous 7 days';
  if (ms >= start - 86400000 * 30) return 'Previous 30 days';
  return d.toLocaleDateString(undefined, { month: 'long', year: 'numeric' });
}

export function anchorLabel(a: Anchor) {
  const parts: string[] = [];
  if (a.file) parts.push(a.file);
  if (a.page) parts.push(`p. ${a.page}`);
  if (a.slide) parts.push(`slide ${a.slide}`);
  if (a.sheet) parts.push(a.sheet);
  if (a.time != null) parts.push(formatTime(a.time));
  if (a.heading) parts.push(a.heading);
  return parts.join(' · ');
}

export function formatTime(t: number) {
  const s = Math.max(0, Math.floor(t));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const ss = String(s % 60).padStart(2, '0');
  return h ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`;
}

export type DocKind = 'note' | 'pdf' | 'doc' | 'sheet' | 'slides' | 'image' | 'audio' | 'video' | 'code' | 'web' | 'mail' | 'archive' | 'text' | 'file';

export function docKind(d: Pick<Document, 'mime' | 'original_name' | 'source_kind'>): DocKind {
  if (d.source_kind === 'note') return 'note';
  const m = d.mime;
  const ext = d.original_name.split('.').pop()?.toLowerCase() ?? '';
  if (m === 'application/pdf' || ext === 'pdf') return 'pdf';
  if (m.startsWith('image/')) return 'image';
  if (m.startsWith('audio/') || ['mp3', 'm4a', 'wav', 'flac', 'ogg', 'opus', 'aac'].includes(ext)) return 'audio';
  if (m.startsWith('video/') || ['mp4', 'mov', 'mkv', 'webm', 'avi', 'm4v'].includes(ext)) return 'video';
  if (['doc', 'docx', 'odt', 'rtf', 'pages'].includes(ext)) return 'doc';
  if (['xls', 'xlsx', 'ods', 'csv', 'tsv', 'numbers'].includes(ext)) return 'sheet';
  if (['ppt', 'pptx', 'odp', 'key'].includes(ext)) return 'slides';
  if (['html', 'htm', 'xhtml', 'epub'].includes(ext)) return 'web';
  if (['eml', 'mbox'].includes(ext)) return 'mail';
  if (['zip'].includes(ext)) return 'archive';
  if (['md', 'markdown', 'txt', 'log'].includes(ext)) return 'text';
  if (m.startsWith('text/') || m.includes('json') || m.includes('xml')) return 'code';
  return 'file';
}

export const kindColor: Record<DocKind, string> = {
  note: '#f5a524', pdf: '#e5484d', doc: '#3b82f6', sheet: '#2f9e67', slides: '#f97316', image: '#a855f7',
  audio: '#ec4899', video: '#06b6d4', code: '#64748b', web: '#0ea5e9', mail: '#8b5cf6', archive: '#a16207', text: '#6b7280', file: '#6b7280',
};

export function greeting() {
  const h = new Date().getHours();
  if (h < 5) return 'Up late';
  if (h < 12) return 'Good morning';
  if (h < 18) return 'Good afternoon';
  return 'Good evening';
}

export function debounce<A extends unknown[]>(fn: (...a: A) => void, ms: number) {
  let t: ReturnType<typeof setTimeout> | undefined;
  return (...a: A) => {
    clearTimeout(t);
    t = setTimeout(() => fn(...a), ms);
  };
}

export const PROJECT_COLORS = ['#7c6cf2', '#3b82f6', '#06b6d4', '#2f9e67', '#84cc16', '#f5a524', '#f97316', '#e5484d', '#ec4899', '#a855f7', '#64748b'];

export function passphraseStrength(p: string): { score: number; label: string } {
  if (!p) return { score: 0, label: '' };
  let pool = 0;
  if (/[a-z]/.test(p)) pool += 26;
  if (/[A-Z]/.test(p)) pool += 26;
  if (/\d/.test(p)) pool += 10;
  if (/[^A-Za-z0-9]/.test(p)) pool += 33;
  const words = p.trim().split(/\s+/).length;
  let bits = p.length * Math.log2(Math.max(pool, 2));
  if (/(.)\1{2,}/.test(p)) bits *= 0.7;
  if (words >= 4) bits = Math.max(bits, words * 11);
  const score = bits < 40 ? 1 : bits < 60 ? 2 : bits < 80 ? 3 : 4;
  return { score, label: ['', 'Weak', 'Fair', 'Strong', 'Excellent'][score] };
}
