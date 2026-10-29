import { useCallback, useEffect, useState } from "react";
import type { AgentQuota } from "@/platform/chatApi";
import { quotaBaseline, readTurnQuotaChanges, settleQuotaChange } from "./turnQuotaChange";

const STORAGE_KEY = "android-tool.chat.turn-quota-changes.v1";

export function useTurnQuotaChange({ sessionId, turnId, providerKey, quota, checkedAt, finishedAt }: {
  sessionId: string; turnId?: string; providerKey: string; quota: AgentQuota | null;
  checkedAt: number | null; finishedAt?: number | null;
}) {
  const [records, setRecords] = useState(() => {
    try { return readTurnQuotaChanges(window.localStorage.getItem(STORAGE_KEY)); } catch { return []; }
  });
  useEffect(() => {
    try { window.localStorage.setItem(STORAGE_KEY, JSON.stringify(records)); } catch { /* Billing stays usable without storage. */ }
  }, [records]);
  const begin = useCallback((requestSessionId: string, requestTurnId: string, startedAt: number,
    snapshot = quota, snapshotCheckedAt = checkedAt) => {
    const baseline = quotaBaseline(requestSessionId, requestTurnId, providerKey, snapshot, snapshotCheckedAt, startedAt);
    if (!baseline) return;
    setRecords((previous) => previous.some((item) => item.sessionId === requestSessionId && item.turnId === requestTurnId)
      ? previous : [...previous, baseline].slice(-200));
  }, [providerKey, quota, checkedAt]);
  useEffect(() => {
    if (!quota || !checkedAt || !finishedAt || !turnId) return;
    setRecords((previous) => {
      const index = previous.findIndex((item) => item.sessionId === sessionId && item.turnId === turnId);
      if (index < 0) return previous;
      const change = settleQuotaChange(previous[index], providerKey, quota, checkedAt, finishedAt);
      const previousAmount = previous[index].amount;
      if (!change || (previous[index].settledAt ?? 0) >= checkedAt
        || (previousAmount != null && change.amount <= previousAmount)) return previous;
      return previous.map((item, position) => position === index ? change : item);
    });
  }, [sessionId, turnId, providerKey, quota, checkedAt, finishedAt]);
  return { begin, change: records.find((item) => item.sessionId === sessionId && item.turnId === turnId && item.amount != null) };
}
