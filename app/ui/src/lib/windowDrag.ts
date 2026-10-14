import { getCurrentWindow } from '@tauri-apps/api/window';

/** Things inside a `.drag` area that should be clicked, not dragged. */
const INTERACTIVE = 'button, a, input, select, textarea, label, [contenteditable]:not([contenteditable="false"]), [role="tab"], [role="button"], [tabindex]:not([tabindex="-1"]), .no-drag';

/**
 * Moves the window when the user drags any `.drag` area (headers, tab strips).
 *
 * This asks the window to start a native drag, rather than relying on WebKit's
 * `-webkit-app-region` CSS. WebKit keeps that drag map as a cache which can go stale after
 * layout changes (e.g. the terminal resizing), leaving the header undraggable.
 */
export function installWindowDrag() {
  const win = getCurrentWindow();
  document.addEventListener('mousedown', (e) => {
    if (e.button !== 0) return;
    const t = e.target as HTMLElement | null;
    if (!t?.closest('.drag') || t.closest(INTERACTIVE)) return;
    e.preventDefault();
    // Double-click zooms the window, like a native title bar.
    if (e.detail === 2) win.toggleMaximize().catch(() => {});
    else win.startDragging().catch(() => {});
  });
}
