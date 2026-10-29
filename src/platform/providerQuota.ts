import type { AgentQuota } from "./chatApi.ts";

export function quotaCacheKey(provider: { id: string; kind: string; brand?: string | null; baseUrl: string; currency?: string | null; quotaUrls?: { balanceUrl?: string | null; spendingUrl?: string | null } | null }): string {
  // Do not key account balances by transient revisions, model, or pricing.
  return `android-tool.chat.quota.v3.${JSON.stringify([provider.id, provider.kind, provider.brand,
    provider.baseUrl, provider.currency, provider.quotaUrls?.balanceUrl, provider.quotaUrls?.spendingUrl])}`;
}

export function hasQuotaAmounts(quota: AgentQuota | null | undefined): boolean {
  return quota?.status === "available" && [quota.remainingAmount, quota.remainingTokens, quota.usedAmount, quota.totalAmount]
    .some((value) => typeof value === "number" && Number.isFinite(value));
}

export function supportsAutomaticQuotaRefresh(quota: AgentQuota | null | undefined): boolean {
  return !["unsupported", "notConfigured", "not_configured"].includes(quota?.status || "");
}

export function quotaBalancePresentation(quota: AgentQuota | null, phase: "idle" | "checking" | "done" | "error",
  error?: string, checkedAt?: number | null) {
  const balance = formatQuotaBalance(quota);
  const known = balance !== "未上报";
  const updated = known && checkedAt ? `最近更新：${new Date(checkedAt).toLocaleTimeString("zh-CN")}。` : "";
  let detail = known ? "点击刷新账户余额。" : "供应商尚未返回余额数据，点击可重新查询。";
  if (phase === "checking") detail = known ? "正在后台更新，当前保留上次余额。" : "正在查询账户余额。";
  else if (phase === "error" || quota?.status === "error") detail = `${known ? "更新失败，当前保留上次余额。" : "余额查询失败。"}${error || quota?.message || "点击可重试。"}`;
  else if (quota?.status === "unsupported") detail = "供应商未提供余额查询，已停止自动查询；点击可手动重试。";
  else if (["notConfigured", "not_configured"].includes(quota?.status || "")) detail = "尚未配置余额查询，请检查供应商设置。";
  return { text: known ? balance : "—", detail: updated + detail };
}

export function readQuotaCache(storage: Pick<Storage, "getItem">, key: string): { checkedAt: number; quota: AgentQuota } | null {
  try {
    const parsed = JSON.parse(storage.getItem(key) || "null");
    return parsed && hasQuotaAmounts(parsed.quota) && typeof parsed.checkedAt === "number"
      && Number.isFinite(parsed.checkedAt) && parsed.checkedAt > 0 ? parsed : null;
  } catch { return null; }
}

export function formatQuotaAmount(value: number | null | undefined, unit?: string | null): string {
  if (value == null || !Number.isFinite(value)) return "未上报";
  const amount = value > 0 && value < 0.000001 ? "<0.000001" : value.toLocaleString("zh-CN", { maximumFractionDigits: 6 });
  return unit?.trim() ? `${amount} ${unit.trim()}` : amount;
}

export function formatQuotaBalance(quota: AgentQuota | null): string {
  if (quota?.remainingTokens != null) return formatQuotaAmount(quota.remainingTokens, "tokens");
  return formatQuotaAmount(quota?.remainingAmount, quota?.unit || quota?.currency);
}

export function formatQuotaSpent(quota: AgentQuota | null): string {
  const used = quota?.usedAmount ?? (quota?.totalAmount != null && quota.remainingAmount != null
    ? Math.max(0, quota.totalAmount - quota.remainingAmount) : undefined);
  return formatQuotaAmount(used, quota?.unit || quota?.currency);
}
