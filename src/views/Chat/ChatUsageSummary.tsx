import { useLayoutEffect, useRef } from "react";
import type { AgentQuota } from "../../platform/chatApi";
import { Tooltip } from "../../components/ui/Tooltip";
import { ChatBillingSummary } from "./ChatBillingSummary";
import { cacheStatAnomaly, formatCacheHitRate, latestRequestCacheHitRate, usageCacheHitRate, usageInputTotal, type TurnPricing } from "./chatUsage";
import { sessionTokenPair, type SessionTokenTotals } from "./tokenUsagePresentation";
import { retainUsagePresentation, type UsagePresentation } from "./usagePresentation";
import type { ChatUsage } from "./types";
import type { TurnQuotaChange } from "./turnQuotaChange";

// Key this component by session ID so retained readings cannot leak between chats.
export function ChatUsageSummary({ usage, turnId, persistedUsage, persistedTurnId, pricing, quotaChange, quota, session, phase, onRefresh, quotaError, quotaCheckedAt }: {
  usage?: ChatUsage; turnId?: string; persistedUsage?: ChatUsage; persistedTurnId?: string;
  pricing?: TurnPricing; quotaChange?: TurnQuotaChange; quota: AgentQuota | null;
  /** Whole-conversation counters; undefined until the session reports any. */
  session?: SessionTokenTotals;
  phase: "idle" | "checking" | "done" | "error"; onRefresh: () => void;
  quotaError?: string; quotaCheckedAt?: number | null;
}) {
  const retained = useRef<UsagePresentation>();
  const saved = retainUsagePresentation(undefined, persistedUsage, persistedTurnId);
  const seed: UsagePresentation = {
    input: retained.current?.input ?? saved.input,
    output: retained.current?.output ?? saved.output,
    reasoning: retained.current?.reasoning ?? saved.reasoning,
    speed: retained.current?.speed ?? saved.speed,
    cache: retained.current?.cache ?? saved.cache,
    costText: retained.current?.costText ?? saved.costText,
  };
  const display = retainUsagePresentation(seed, usage, turnId, pricing, quotaChange);
  // Commit only rendered readings; speculative/abandoned renders cannot replace them.
  useLayoutEffect(() => { retained.current = display; });
  const cacheUsage = display.cache?.usage;
  const cacheRate = formatCacheHitRate(latestRequestCacheHitRate(cacheUsage));
  // P9-C：口径错配（cached > input）不再伪装成「未上报」——明确标为读数异常。
  const cacheAnomaly = cacheStatAnomaly(cacheUsage?.latestRequestUsage);
  const cacheRateText = cacheAnomaly && cacheRate === "未上报" ? "读数异常" : cacheRate;
  const previousTurn = display.cache?.turnId && turnId && display.cache.turnId !== turnId;
  const latestInput = usageInputTotal(cacheUsage?.latestRequestUsage);
  const sessionPair = sessionTokenPair(session);
  // 只有缓存命中带悬浮窗：其余各项直接显示，不给悬停行为。
  const cacheDetails = `最近一次模型请求${previousTurn ? "（上一轮，等待新上报）" : ""}：${cacheRate}\n缓存读取：${cacheUsage?.latestRequestUsage?.cacheRead?.toLocaleString("zh-CN") ?? "未上报"} tokens / 总输入：${latestInput?.toLocaleString("zh-CN") ?? "未上报"} tokens\n${previousTurn ? "上一轮" : "本轮"}累计缓存命中：${formatCacheHitRate(usageCacheHitRate(cacheUsage))}${cacheAnomaly ? "\n读数异常：缓存读取大于总输入，口径自相矛盾，已忽略该读数" : ""}`;
  return <div className="composer-usage-summary" aria-label="会话用量">
    {sessionPair ? <>
      <span>会话输入 <b>{sessionPair.input}</b></span>
      <span>会话输出 <b>{sessionPair.output}</b></span>
    </> : null}
    <span>输出速度 <b>{display.speed == null ? "—" : `${Math.round(display.speed)} token/s`}</b></span>
    <Tooltip className="composer-usage-item composer-cache-stat" content={cacheDetails}>缓存命中 <b>{cacheRateText === "未上报" ? "—" : cacheRateText}</b></Tooltip>
    <ChatBillingSummary quota={quota} phase={phase} onRefresh={onRefresh} costText={display.costText} quotaError={quotaError} quotaCheckedAt={quotaCheckedAt} />
  </div>;
}
