/**
 * 终端内容重置（换页签、重连、池复用前清屏）。
 *
 * 为什么不能用自研内核的 `terminal.reset()`：
 * 内核的 `reset()`（`lib/terminal.ts:927-945`）会先
 * `wasmTerm.free()` 再 `createTerminal()` 换掉底层 WASM 句柄。但 `InputHandler` 与
 * `SelectionManager` 是在 `open()` 时用当时的 `wasmTerm` 构造的（`terminal.ts:516`、
 * `:554`），它们内部捕获的是旧句柄，`reset()` 之后每次鼠标事件都会调用已释放的句柄，
 * 抛出内核句柄失效异常（`failed with result -2`）
 * （`abi.ts` 里 `INVALID_VALUE = -2`），表现是控制台刷错、选区与鼠标追踪失效。
 *
 * 改用等价的 ANSI 软重置序列：退出备用屏幕、关掉各鼠标追踪模式与括号粘贴、复位 SGR 与
 * 自动换行、清屏 + 清回滚缓冲 + 光标归位。实测（2026-10-23，headless Edge 走应用真实模块图）
 * 与 `reset()` 效果一致（回滚缓冲 0、`baseY` 0、首行为空），且**不产生任何异常**，
 * 之后鼠标选区和 `getSelection()` 都正常。
 *
 * xterm.js 的 `reset()` 没有这个问题，保持原样。
 */

/** 退出备用屏幕 + 关闭鼠标追踪/括号粘贴/光标隐藏/自动换行 + 复位 SGR + 清屏清回滚 + 归位。 */
export const SOFT_RESET_SEQUENCE =
  "\u001b[?1049l\u001b[?47l" + // 退出备用屏幕
  "\u001b[?1000l\u001b[?1002l\u001b[?1003l" + // 关闭 X10/按钮/任意位置鼠标追踪
  "\u001b[?1005l\u001b[?1006l\u001b[?1015l" + // 关闭 UTF-8 / SGR / urxvt 鼠标编码
  "\u001b[?2004l" + // 关闭括号粘贴
  "\u001b[?1l" + // 光标键恢复普通模式
  "\u001b[?25h\u001b[?7h" + // 显示光标、开启自动换行
  "\u001b[0m" + // 复位 SGR
  "\u001b[2J\u001b[3J\u001b[H"; // 清屏、清回滚缓冲、光标归位

/** 需要重置内容的最小终端接口。 */
export interface ResettableTerminal {
  write(data: string): void;
  reset(): void;
  clear(): void;
  /** 只有自研内核提供（`lib/terminal.ts:688`），用来识别内核类型。 */
  setPaused?: (paused: boolean, options: { releaseRenderer?: boolean }) => void;
}

/**
 * 清屏但不动终端模式（对应"清屏"按钮）。
 *
 * 自研内核的 `clear()` 只发 `ESC[2J ESC[H`，不会清回滚缓冲，向上滚动还能看到旧内容；
 * 这里补一个 `ESC[3J` 把回滚缓冲也清掉。不重置鼠标追踪等模式，避免打断正在跑的
 * 全屏程序。xterm.js 的 `clear()` 已经包含回滚缓冲清理，保持原样。
 */
export function clearTerminalContent(terminal: ResettableTerminal): void {
  if (typeof terminal.setPaused === "function") {
    terminal.write("\u001b[2J\u001b[3J\u001b[H");
    return;
  }
  terminal.clear();
}

/**
 * 清空终端内容。
 *
 * 自研内核走 ANSI 软重置（避开 `reset()` 换句柄导致的 `-2` 异常），xterm.js 继续用
 * `reset()`。内核类型通过 `setPaused` 是否存在判断，调用方不需要传引擎名。
 */
export function resetTerminalContent(terminal: ResettableTerminal): void {
  if (typeof terminal.setPaused === "function") {
    terminal.write(SOFT_RESET_SEQUENCE);
    return;
  }
  terminal.reset();
}
