import { formatQuotaAmount } from "../../platform/providerQuota.ts";
import { estimateTurnCost, latestRequestCacheHitRate, outputSpeedForDisplay, usageCacheHitRate, usageInputTotal, type TurnPricing } from "./chatUsage.ts";
import type { ChatUsage } from "./types.ts";
import type { TurnQuotaChange } from "./turnQuotaChange.ts";

export function turnCostForDisplay(usage?: ChatUsage, pricing?: TurnPricing, quotaChange?: TurnQuotaChange): string | undefined {
  const cost = estimateTurnCost(usage, pricing);
  const reported = usage?.totalCost != null || (usage?.inputCost != null && usage?.outputCost != null);
  if (!reported && quotaChange?.amount != null) return `≈ ${formatQuotaAmount(quotaChange.amount, quotaChange.currency)}`;
  if (cost.amount == null) return undefined;
  return `${reported ? "" : "≈ "}${formatQuotaAmount(cost.amount, cost.currency)}${cost.partial ? "+" : ""}`;
}

export interface UsagePresentation {
  input?: number;
  output?: number;
  /** Provider-reported reasoning tokens. Left undefined when nothing reported them. */
  reasoning?: number;
  speed?: number;
  cache?: { usage: ChatUsage; turnId?: string };
  costText?: string;
}

// Display memory only: never feed retained values into billing, persistence or
// session totals. Cache counters and timing ratios are replaced atomically.
export function retainUsagePresentation(previous: UsagePresentation | undefined, usage?: ChatUsage,
  turnId?: string, pricing?: TurnPricing, quotaChange?: TurnQuotaChange): UsagePresentation {
  const input = usageInputTotal(usage);
  const speed = outputSpeedForDisplay(usage);
  const cacheReported = usage?.latestRequestUsage !== undefined;
  const legacyCache = !previous?.cache && usage && (usage.cacheRead != null || usage.cacheHitRate != null);
  // A report that carries no cache counters proves nothing: keeping the last
  // measurable ratio beats blanking the footer. The tooltip already labels a
  // reading of the previous turn as "上一轮，等待新上报".
  const cacheMeasurable = latestRequestCacheHitRate(usage) != null || usageCacheHitRate(usage) != null;
  return {
    input: input ?? previous?.input,
    output: usage?.output ?? previous?.output,
    reasoning: usage?.reasoning ?? previous?.reasoning,
    speed: speed?.rate ?? previous?.speed,
    cache: (cacheMeasurable || !previous?.cache) && (cacheReported || legacyCache) && usage
      ? { usage, turnId } : previous?.cache,
    costText: turnCostForDisplay(usage, pricing, quotaChange) ?? previous?.costText,
  };
}
