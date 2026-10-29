import type { AppFitAddon, AppTerminal } from "./terminalAdapter.ts";
import { getAppMonoFontStack, subscribeAppMonoFont } from "./font.ts";

/** Fit the screen and PTY to the same grid, within the native backend limits.
 * The terminal's parent must be a padding-free content box, not the outer card.
 */
export function fitTerminalGrid(terminal: AppTerminal, fit: AppFitAddon) {
  const parent = terminal.element?.parentElement;
  if (!parent || !parent.clientWidth || !parent.clientHeight) return undefined;
  const dimensions = fit.proposeDimensions();
  if (!dimensions || !Number.isFinite(dimensions.cols) || !Number.isFinite(dimensions.rows)) return undefined;
  // 内核的 `proposeDimensions()` 已经替滚动条预留了 15px（ghostty `addons/fit.ts:188`
  // 的 `DEFAULT_SCROLLBAR_WIDTH`）。自研内核的滚动条已改成 DOM 自绘（8px，贴面板右缘），
  // 15px 的预留足够，**再**减一列就是双重预留，画布右缘会离宿主右框空出一条纯空白。
  // xterm 的 fit 不替滚动条预留，且它的 DOM 渲染器要一格余量吸收字形取整，所以只在
  // xterm 上保留那一列。
  const spare = terminal.rendererType ? 0 : 1;
  const cols = Math.max(20, Math.min(500, dimensions.cols - spare));
  // Leave one row below the cursor so the last line is comfortably readable.
  const rows = Math.max(5, Math.min(300, dimensions.rows - 1));
  if (terminal.cols !== cols || terminal.rows !== rows) terminal.resize(cols, rows);
  return { cols, rows };
}

/** Re-measure cells after the selected web font has actually loaded. xterm's
 * cached fallback metrics can differ from the glyphs the DOM now paints.
 */
export function watchTerminalFont(terminal: AppTerminal, scheduleFit: () => void) {
  let disposed = false;
  let revision = 0;
  const loadFont = async (family: string) => {
    const current = ++revision;
    try { await document.fonts?.load(`${terminal.options.fontSize || 12}px ${family}`); }
    catch { /* Missing fonts continue to use the configured fallback stack. */ }
    if (disposed || current !== revision) return;
    // A semantically identical CSS family forces xterm's public option-change
    // path to refresh both character measurements and the DOM glyph cache.
    terminal.options.fontFamily = terminal.options.fontFamily === family ? `${family} ` : family;
    scheduleFit();
  };
  const unsubscribe = subscribeAppMonoFont(font => { void loadFont(getAppMonoFontStack(font)); });
  void loadFont(getAppMonoFontStack());
  return () => { disposed = true; unsubscribe(); };
}
