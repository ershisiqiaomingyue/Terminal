import type { AppTerminal, TerminalEngine } from "./terminalAdapter.ts";

type ClipboardTerminal = Pick<AppTerminal, "attachCustomKeyEventHandler" | "hasSelection" | "getSelection" | "paste">;
type ClipboardOptions = {
  getSessionId: () => string | undefined;
  onError: (message: string) => void;
  engine?: TerminalEngine;
};

/** Copy never interrupts a selected command; paste uses the terminal's bracketed-paste handling. */
export function bindTerminalClipboard(terminal: ClipboardTerminal, options: ClipboardOptions) {
  let disposed = false;
  let pastes: Promise<void> = Promise.resolve();
  // 两个内核对 attachCustomKeyEventHandler 的返回值定义相反：xterm.js 返回 false 表示“已消费”，
  // 自研内核返回 true 表示“已消费并阻止默认行为”。返回值必须按内核区分，否则自研内核下会把所有按键吞掉。
  const consumedResult = options.engine === "core";
  const passthroughResult = !consumedResult;
  const fail = (action: string, error: unknown) => {
    if (!disposed) options.onError(`${action}失败：${error instanceof Error ? error.message : String(error)}`);
  };
  terminal.attachCustomKeyEventHandler((event) => {
    if (disposed || !event.ctrlKey || event.altKey || event.metaKey || event.isComposing) return passthroughResult;
    const key = event.key.toLowerCase();
    const copy = key === "c" && (event.shiftKey || terminal.hasSelection());
    const paste = key === "v";
    if (!copy && !paste) return passthroughResult;
    // Prevent browser paste and kernel key processing from both sending the same text.
    event.preventDefault();
    if (event.type !== "keydown" || event.repeat) return consumedResult;
    if (copy) {
      const selection = terminal.getSelection();
      if (selection) {
        void (async () => {
          try { await navigator.clipboard.writeText(selection); }
          catch (error) { fail("复制", error); }
        })();
      }
    } else {
      const sessionId = options.getSessionId();
      if (!sessionId) return consumedResult;
      // Read within the key event's user activation, then preserve paste ordering.
      const text = (async () => {
        try { return await navigator.clipboard.readText(); }
        catch (error) { fail("粘贴", error); return null; }
      })();
      pastes = pastes.then(async () => {
        const value = await text;
        if (disposed || value === null) return;
        if (sessionId !== options.getSessionId()) {
          options.onError("终端会话已切换，本次粘贴已取消。");
          return;
        }
        terminal.paste(value);
      }).catch((error) => fail("粘贴", error));
    }
    return consumedResult;
  });
  return () => { disposed = true; };
}
