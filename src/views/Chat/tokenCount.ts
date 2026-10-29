/** Compact display only; callers retain the exact count in their tooltip. */
export function formatTokenCount(value: number | null | undefined): string {
  if (value == null || !Number.isFinite(value)) return "未知";
  const count = Math.max(0, Math.round(value));
  if (count >= 1_000_000) return `${Number((count / 1_000_000).toFixed(1))}M`;
  if (count >= 1_000) return `${Number((count / 1_000).toFixed(1))}K`;
  return count.toLocaleString("zh-CN");
}
