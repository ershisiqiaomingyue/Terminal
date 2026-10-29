import type { AgentQuota } from "../../platform/chatApi.ts";

export interface TurnQuotaBaseline {
  sessionId: string;
  turnId: string;
  providerKey: string;
  source: string;
  currency: string;
  used: number;
  measure?: "balance";
  checkedAt: number;
}

export interface TurnQuotaChange extends TurnQuotaBaseline { amount?: number; settledAt?: number }

export function quotaBaseline(sessionId: string, turnId: string, providerKey: string, quota: AgentQuota | null,
  checkedAt: number | null, startedAt: number): TurnQuotaBaseline | null {
  const measure = quota?.usedAmount == null ? "balance" : undefined;
  const used = measure ? quota?.remainingAmount : quota?.usedAmount;
  if (quota?.status !== "available" || !quota.source || used == null || !Number.isFinite(used) || used < 0
    || !checkedAt || checkedAt > startedAt || startedAt - checkedAt > 30_000) return null;
  return { sessionId, turnId, providerKey, source: quota.source, currency: quota.unit || quota.currency || "额度",
    used, checkedAt, ...(measure ? { measure } : {}) };
}

export function settleQuotaChange(baseline: TurnQuotaBaseline, providerKey: string, quota: AgentQuota,
  checkedAt: number, finishedAt: number): (TurnQuotaChange & { amount: number; settledAt: number }) | null {
  const used = baseline.measure === "balance" ? quota.remainingAmount : quota.usedAmount;
  const delta = used == null ? NaN : baseline.measure === "balance" ? baseline.used - used : used - baseline.used;
  if (providerKey !== baseline.providerKey || quota.status !== "available" || quota.source !== baseline.source
    || (quota.unit || quota.currency || "额度") !== baseline.currency || checkedAt < finishedAt || checkedAt > finishedAt + 60_000
    || used == null || !Number.isFinite(used) || delta < 0) return null;
  return { ...baseline, amount: Math.round(delta * 1e12) / 1e12, settledAt: checkedAt };
}

export function readTurnQuotaChanges(raw: string | null): TurnQuotaChange[] {
  try {
    const values: unknown = JSON.parse(raw || "[]");
    if (!Array.isArray(values)) return [];
    return values.filter((value): value is TurnQuotaChange => value && typeof value === "object"
      && ["sessionId", "turnId", "providerKey", "source", "currency"].every((key) => typeof value[key] === "string")
      && Number.isFinite(value.used) && value.used >= 0 && Number.isFinite(value.checkedAt) && value.checkedAt > 0
      && (value.measure == null || value.measure === "balance")
      && (value.amount == null || Number.isFinite(value.amount) && value.amount >= 0)).slice(-200);
  } catch { return []; }
}
