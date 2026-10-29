/** 顶部菜单栏 → 终端页的动作契约。 */
export const TERMINAL_MENU_EVENT = "terminal:menu";

export type TerminalMenuDetail =
  | { action: "new"; value: "pwsh" | "powershell" | "cmd" }
  | { action: "new-wsl" }
  | { action: "new-ssh" }
  | { action: "new-adb" }
  | { action: "close-current" }
  | { action: "layout"; value: "1x1" | "1x2" | "2v" | "2x2" }
  | { action: "pane"; value: "host" | "git" | "files" };

export function sendTerminalMenu(detail: TerminalMenuDetail): void {
  window.dispatchEvent(new CustomEvent(TERMINAL_MENU_EVENT, { detail }));
}
