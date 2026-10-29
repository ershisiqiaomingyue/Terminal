import React from "react";
import type { AgentQuota } from "@/platform/chatApi";
import { quotaBalancePresentation } from "@/platform/providerQuota";
import { type TurnPricing } from "./chatUsage";
import { turnCostForDisplay } from "./usagePresentation";
import type { ChatUsage } from "./types";
import type { TurnQuotaChange } from "./turnQuotaChange";

// 账目两项直接显示，不挂悬浮窗：悬停只在缓存命中上保留。
export function ChatBillingSummary({ quota, phase, onRefresh, usage, pricing, quotaChange, costText: retainedCost, quotaError, quotaCheckedAt }: {
  quota: AgentQuota | null;
  phase: "idle" | "checking" | "done" | "error";
  onRefresh: () => void;
  usage?: ChatUsage;
  pricing?: TurnPricing;
  quotaChange?: TurnQuotaChange;
  costText?: string;
  quotaError?: string;
  quotaCheckedAt?: number | null;
}) {
  const costText = retainedCost ?? turnCostForDisplay(usage, pricing, quotaChange);
  const balance = quotaBalancePresentation(quota, phase, quotaError, quotaCheckedAt);
  return <>
    <button type="button" aria-label="刷新账户余额" aria-busy={phase === "checking"} title={balance.detail} disabled={phase === "checking"} onClick={onRefresh}>
      余额 <b>{balance.text}</b>
    </button>
    <span>本轮消费 <b>{costText ?? "—"}</b></span>
  </>;
}
