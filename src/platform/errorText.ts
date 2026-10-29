/**
 * 错误文本归一化。
 *
 * 后端命令的 `Err` 类型并不统一：绝大多数是 `String`，但有些命令（如
 * `discover_agent_models`）返回的是 serde 结构体（`AgentCommandError { code, message, retryable }`）。
 * 这类值到达前端是普通对象，直接 `String(reason)` 会得到「[object Object]」，
 * 提示里看不到真实原因。
 *
 * 所以按「Error → 带 message 的对象 → 字符串 → JSON → 兜底文案」的顺序逐级取可读文本。
 * 这个模块刻意不依赖 `runtime.ts`（那里会引入 Tauri API），
 * 这样单元测试与 Node 环境可以直接引用。
 */

const FALLBACK_ERROR = "操作失败，请重试。";

const MESSAGE_KEYS = ["message", "error", "detail", "reason", "msg", "description"] as const;

export function formatErrorText(reason: unknown): string {
  if (reason == null) return FALLBACK_ERROR;
  if (reason instanceof Error) return reason.message || reason.name || FALLBACK_ERROR;
  if (typeof reason === "string") return reason.trim() || FALLBACK_ERROR;
  if (typeof reason === "number" || typeof reason === "boolean") return String(reason);
  if (typeof reason === "object") {
    const record = reason as Record<string, unknown>;
    for (const key of MESSAGE_KEYS) {
      const value = record[key];
      if (typeof value === "string" && value.trim()) return value.trim();
      // 只下探一层，够覆盖 { error: { message } } 这种形态。
      if (value && typeof value === "object") {
        const nested = (value as Record<string, unknown>).message;
        if (typeof nested === "string" && nested.trim()) return nested.trim();
      }
    }
    try {
      const json = JSON.stringify(reason);
      if (json && json !== "{}" && json !== "[]") return json;
    } catch {
      // 循环引用等无法序列化的情况，落到兜底文案。
    }
    return FALLBACK_ERROR;
  }
  return FALLBACK_ERROR;
}
