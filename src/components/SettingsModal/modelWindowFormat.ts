/** Shared context-window formatting so every badge renders identically. */
export function formatContextWindow(value?: number | null) {
  if (!value || value <= 0) return "";
  if (value >= 1_000_000) {
    const mega = value / 1_000_000;
    return `${Number.isInteger(mega) ? mega : Number(mega.toFixed(2))}M`;
  }
  if (value >= 1_000) {
    const kilo = value / 1_000;
    return `${Number.isInteger(kilo) ? kilo : kilo.toFixed(1)}K`;
  }
  return String(value);
}

/**
 * 输入框里显示的 token 数：0 / 负数 / 空一律显示为空，表示「自动」。
 * 未填写的上下文窗口运行时按已知窗口或 64K 保守估算，未填写的输出上限
 * 按上下文窗口推算，所以「空」是有意义的状态，不能被 0 顶替。
 */
export function tokenCountInputValue(value?: number | null) {
  return value && value > 0 ? String(value) : "";
}

/** 解析用户填写的 token 数：只收数字，空或 0 视为「自动」（返回 null）。 */
export function parseTokenCount(raw: string): number | null {
  const digits = raw.replace(/[^\d]/g, "").slice(0, 9);
  const value = Number(digits);
  return Number.isFinite(value) && value > 0 ? value : null;
}
