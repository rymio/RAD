import { Marked, type Tokens } from 'marked';
import DOMPurify from 'dompurify';
import hljs from 'highlight.js/lib/common';

const marked = new Marked({
  gfm: true,
  breaks: false,
  renderer: {
    code({ text, lang }: Tokens.Code) {
      const language = (lang || '').split(/\s/)[0];
      let html: string;
      try {
        html = language && hljs.getLanguage(language) ? hljs.highlight(text, { language }).value : hljs.highlightAuto(text).value;
      } catch {
        html = escapeHtml(text);
      }
      const label = language ? `<span class="code-lang">${escapeHtml(language)}</span>` : '';
      return `<pre>${label}<button class="copy-code" data-copy title="Copy">⧉</button><code class="hljs">${html}</code></pre>`;
    },
  },
});

export function escapeHtml(s: string) {
  return s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]!);
}

/** Markdown → safe HTML, with [n] citations turned into clickable pills. */
export function renderMarkdown(src: string, maxCite = 0): string {
  // Strip <think> blocks from reasoning models.
  src = src.replace(/<think>[\s\S]*?<\/think>/g, '').replace(/<think>[\s\S]*$/, '');
  const raw = marked.parse(src, { async: false }) as string;
  const clean = DOMPurify.sanitize(raw, { ADD_ATTR: ['data-copy', 'target'], FORBID_TAGS: ['style', 'iframe', 'form', 'input'] });
  if (!maxCite) return clean;
  // Replace citations outside code.
  return clean
    .split(/(<pre[\s\S]*?<\/pre>|<code[\s\S]*?<\/code>)/g)
    .map((part) =>
      part.startsWith('<pre') || part.startsWith('<code')
        ? part
        : part.replace(/\[(\d{1,2})\]/g, (m, n) => (Number(n) >= 1 && Number(n) <= maxCite ? `<button class="cite" data-cite="${n}">${n}</button>` : m)),
    )
    .join('');
}
