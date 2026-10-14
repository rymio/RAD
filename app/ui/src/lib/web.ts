import type { PageScrape } from './types';

const SEARCH = 'https://html.duckduckgo.com/html/?q=';

/** What the user typed in the address bar → an address: URL, host, or a web search. */
export function toUrl(input: string): string {
  const s = input.trim();
  if (!s) return 'about:blank';
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(s) || /^(about|data|file):/i.test(s)) return s;
  if (/^(localhost|\d{1,3}(\.\d{1,3}){3})(:\d+)?(\/.*)?$/i.test(s)) return `http://${s}`;
  if (!/\s/.test(s) && /^[^\s/?#]+\.[a-z]{2,}(:\d+)?([/?#].*)?$/i.test(s)) return `https://${s}`;
  return SEARCH + encodeURIComponent(s);
}

export function hostOf(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, '');
  } catch {
    return '';
  }
}

/** Questions that are about how the page looks rather than what it says. */
const VISUAL = /\b(see|look|looks|show|shown|visible|screen|screenshot|image|images|picture|photo|chart|graph|diagram|plot|map|color|colour|layout|design|style|icon|logo|button|video|figure|ui|appearance|what'?s on)\b/i;

export type ShotMode = 'auto' | 'always' | 'off';

/** Whether to send a screenshot with a question. In auto mode: when the question is visual or the
 *  page has little text to go on (canvas apps, image galleries, video pages), and a vision model is set. */
export function wantsScreenshot(mode: ShotMode, question: string, page: PageScrape | undefined, hasVision: boolean) {
  if (mode === 'off') return false;
  if (mode === 'always') return true;
  if (!hasVision) return false;
  return VISUAL.test(question) || !page || page.text.trim().length < 400;
}

/** The page as context for the model. */
export function pageContext(page: PageScrape | undefined, withScreenshot: boolean): string {
  const lines = [
    "The user has this web page open in RAD's built-in browser right now. Answer about it when the question is about the page; say so when the page doesn't contain the answer. Links you mention should be full URLs so the user can open them.",
  ];
  if (page) {
    lines.push(`URL: ${page.url}`, `TITLE: ${page.title}`);
    if (page.description) lines.push(`DESCRIPTION: ${page.description}`);
    if (page.page_height) lines.push(`SCROLL POSITION: ${page.scroll_y}px of ${page.page_height}px`);
    if (page.selection.trim()) lines.push(`TEXT THE USER SELECTED:\n${page.selection.trim()}`);
    lines.push(`PAGE TEXT:\n${page.text || '(no text — the page may be graphical)'}`);
    if (page.links.length) lines.push('LINKS ON THE PAGE:\n' + page.links.slice(0, 80).map((l) => `- ${l.text} → ${l.href}`).join('\n'));
  }
  if (withScreenshot) lines.push('A screenshot of the visible part of the page is attached.');
  return lines.join('\n\n');
}
