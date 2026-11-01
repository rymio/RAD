// Colab notebooks: the cells as the UI holds them, and Jupyter's .ipynb format (nbformat 4) they
// are kept in, so notebooks from Google Colab or Jupyter open here and these open there.

import katex from 'katex';
import 'katex/dist/katex.min.css';
import DOMPurify from 'dompurify';
import { renderMarkdown } from '../markdown';

export type Lang = 'python' | 'rust';

export interface CompileError {
  message: string;
  code?: string | null;
  line?: number | null;
  column?: number | null;
  help?: string[];
  hint?: string | null;
  report?: string;
  rendered?: string;
}

export type Output =
  | { kind: 'stream'; name: 'stdout' | 'stderr'; text: string }
  /** Cargo building the crates (Rust): shown folded, not saved. */
  | { kind: 'build'; text: string }
  /** `display(…)` along the way, or the cell's value at the end (`result`). */
  | { kind: 'data'; data: Record<string, string>; result?: boolean }
  | { kind: 'error'; errKind: string; ename?: string; message: string; traceback?: string; errors?: CompileError[]; line?: number | null };

export type CellState = 'idle' | 'queued' | 'running' | 'ok' | 'error';

export interface Cell {
  id: string;
  type: 'code' | 'markdown';
  source: string;
  outputs: Output[];
  count: number | null;
  state: CellState;
  ms?: number;
}

export interface Notebook {
  id: string | null;
  lang: Lang;
  title: string;
  cells: Cell[];
  /** The rest of the file's metadata, kept as it was. */
  metadata: Record<string, any>;
}

export const cellId = () => Math.random().toString(36).slice(2, 10);

export function newCell(type: Cell['type'] = 'code', source = ''): Cell {
  return { id: cellId(), type, source, outputs: [], count: null, state: 'idle' };
}

const text = (v: unknown): string => (Array.isArray(v) ? v.join('') : typeof v === 'string' ? v : '');
/** nbformat keeps multi-line strings as a list of lines. */
const lines = (s: string): string[] => s.split(/(?<=\n)/);

export function fromIpynb(id: string, nb: any): Notebook {
  const md = nb?.metadata ?? {};
  const langName = String(md.kernelspec?.language ?? md.language_info?.name ?? md.kernelspec?.name ?? 'python').toLowerCase();
  const cells: Cell[] = (nb?.cells ?? [])
    .filter((c: any) => c.cell_type === 'code' || c.cell_type === 'markdown' || c.cell_type === 'raw')
    .map((c: any): Cell => {
      const outputs: Output[] = (c.outputs ?? []).flatMap((o: any): Output[] => {
        if (o.output_type === 'stream') return [{ kind: 'stream', name: o.name === 'stderr' ? 'stderr' : 'stdout', text: text(o.text) }];
        if (o.output_type === 'display_data' || o.output_type === 'execute_result') {
          const data: Record<string, string> = {};
          for (const [k, v] of Object.entries(o.data ?? {})) data[k] = typeof v === 'string' || Array.isArray(v) ? text(v) : JSON.stringify(v);
          return [{ kind: 'data', data, result: o.output_type === 'execute_result' }];
        }
        if (o.output_type === 'error') return [{ kind: 'error', errKind: 'exception', ename: o.ename, message: o.evalue ?? '', traceback: stripAnsi(text((o.traceback ?? []).map((l: string) => (l.endsWith('\n') ? l : l + '\n')))) }];
        return [];
      });
      const failed = outputs.some((o) => o.kind === 'error');
      return { id: typeof c.id === 'string' ? c.id : cellId(), type: c.cell_type === 'code' ? 'code' : 'markdown', source: text(c.source), outputs, count: c.execution_count ?? null, state: c.cell_type !== 'code' ? 'idle' : failed ? 'error' : c.execution_count ? 'ok' : 'idle' };
    });
  return { id, lang: langName === 'rust' ? 'rust' : 'python', title: md.rad?.title ?? String(md.colab?.name ?? 'Notebook').replace(/\.ipynb$/, ''), cells, metadata: md };
}

export function toIpynb(nb: Notebook): any {
  const kernelspec = nb.lang === 'rust' ? { name: 'rust', display_name: 'Rust', language: 'rust' } : { name: 'python3', display_name: 'Python 3', language: 'python' };
  const language_info = nb.lang === 'rust' ? { name: 'rust', file_extension: '.rs', mimetype: 'text/rust' } : { name: 'python', file_extension: '.py', mimetype: 'text/x-python' };
  return {
    nbformat: 4,
    nbformat_minor: 5,
    metadata: { ...nb.metadata, kernelspec, language_info, rad: { ...(nb.metadata.rad ?? {}), title: nb.title } },
    cells: nb.cells.map((c) => {
      if (c.type === 'markdown') return { cell_type: 'markdown', id: c.id, metadata: {}, source: lines(c.source) };
      return {
        cell_type: 'code',
        id: c.id,
        metadata: {},
        execution_count: c.count,
        source: lines(c.source),
        outputs: c.outputs.flatMap((o): any[] => {
          if (o.kind === 'stream') return [{ output_type: 'stream', name: o.name, text: lines(o.text) }];
          if (o.kind === 'data') return [o.result ? { output_type: 'execute_result', execution_count: c.count, data: o.data, metadata: {} } : { output_type: 'display_data', data: o.data, metadata: {} }];
          if (o.kind === 'error') return [{ output_type: 'error', ename: o.ename ?? o.errKind, evalue: o.message, traceback: (errorText(o) || o.message).split('\n') }];
          return [];
        }),
      };
    }),
  };
}

/** An error as text: the traceback, or the compiler's report for each error. */
export function errorText(o: Extract<Output, { kind: 'error' }>): string {
  if (o.errors?.length) return o.errors.map((e) => (e.report?.trim() || e.rendered?.trim() || e.message) + (e.hint ? `\n${e.hint}` : '')).join('\n\n');
  return o.traceback?.trim() || o.message;
}

export function stripAnsi(s: string): string {
  return s.replace(/\x1b\[[0-9;]*[A-Za-z]/g, '');
}

// ---------------------------------------------------------------- maths in markdown

function tex(src: string, displayMode: boolean): string {
  return katex.renderToString(src, { displayMode, throwOnError: false, output: 'html', trust: false, strict: 'ignore' });
}

/** Markdown with LaTeX maths ($…$, $$…$$, \(…\), \[…\]) typeset by KaTeX. */
export function renderMath(src: string, maxCite = 0): string {
  const code: string[] = [];
  const math: string[] = [];
  const keep = (html: string) => `\u0000M${math.push(html) - 1}\u0000`;
  // Code is left alone.
  let s = src.replace(/```[\s\S]*?(```|$)|`[^`\n]+`/g, (m) => `\u0000C${code.push(m) - 1}\u0000`);
  s = s
    .replace(/\$\$([\s\S]+?)\$\$/g, (_, t) => keep(tex(t, true)))
    .replace(/\\\[([\s\S]+?)\\\]/g, (_, t) => keep(tex(t, true)))
    .replace(/\\\(([\s\S]+?)\\\)/g, (_, t) => keep(tex(t, false)))
    // $x$ but not prices: no space just inside the dollars, no digit straight after.
    .replace(/(^|[^\\$\w])\$(?=\S)([^$\n]+?)(?<=\S)\$(?!\d)/g, (_, pre, t) => pre + keep(tex(t, false)));
  s = s.replace(/\u0000C(\d+)\u0000/g, (_, i) => code[+i]);
  // The placeholders go through markdown as plain text, then become the typeset maths.
  const html = renderMarkdown(s.replace(/\u0000M(\d+)\u0000/g, '@@RADMATH$1@@'), maxCite);
  return html.replace(/@@RADMATH(\d+)@@/g, (_, i) => math[+i] ?? '');
}

/** HTML a cell produced (pandas tables, plotters charts), without scripts or anything active. */
export function safeHtml(html: string): string {
  return DOMPurify.sanitize(html, { USE_PROFILES: { html: true, svg: true }, FORBID_TAGS: ['style', 'iframe', 'form', 'input', 'script', 'object', 'embed'], FORBID_ATTR: ['srcdoc'] });
}

export function safeSvg(svg: string): string {
  return DOMPurify.sanitize(svg, { USE_PROFILES: { svg: true, svgFilters: true } });
}
