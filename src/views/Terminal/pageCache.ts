/**
 * 终端页在**本窗口内**的页面切换缓存（内存级，不落盘）。
 *
 * 左侧导航切到别的工具页会把整棵终端视图卸载，此前的行为是回来后一切归零
 * （1x1 空面板、页签与布局全丢）——后端会话其实还活着，纯粹是前端状态没了。
 *
 * 做法：卸载时直接复用悬浮窗拆离的快照通道（`captureToolWindowState("shell")`
 * 会把 "shell"（页签/会话序列/回滚缓冲）和 "shell-ui"（布局/绑定/比例）一起
 * 收进一个对象），存进模块级变量；重新挂载时 `restoreToolWindowState` 灌回去，
 * 恢复路径与悬浮窗完全同源（含 journal 补投离开期间的输出），不多写一行
 * 平行逻辑。应用重启自然清空（模块重新加载），维持「重开从 1x1 空面板开始」
 * 的既有定稿。
 */
let saved: Record<string, unknown> | null = null;

/** 视图卸载时存入整份工作区快照（来自 captureToolWindowState）。 */
export function saveTerminalPageState(next: Record<string, unknown>): void {
  saved = next;
}

/** 重新挂载时取出；没有缓存（首次进入 / 应用刚启动）返回 null。 */
export function takeTerminalPageState(): Record<string, unknown> | null {
  return saved;
}
