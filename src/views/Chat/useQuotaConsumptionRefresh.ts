import { useEffect, useRef } from "react";
import type { ChatUsage } from "./types";

/** Billing updates at request boundaries, not at every streamed token. */
export function quotaConsumptionKey(usage?: ChatUsage): string {
  if (!usage || ![usage.input, usage.output, usage.totalCost, usage.inputCost, usage.outputCost].some(value => value != null)) return "";
  return JSON.stringify([usage.input, usage.output, usage.cacheRead, usage.cacheCreation,
    usage.totalCost, usage.inputCost, usage.outputCost, usage.currency]);
}

export function useQuotaConsumptionRefresh({ enabled, accountKey, turnKey, consumptionKey, terminalKey, refresh }: {
  enabled: boolean; accountKey: string; turnKey: string; consumptionKey: string; terminalKey: string;
  refresh: (force: boolean) => Promise<void>;
}) {
  const lastRefresh = useRef({ accountKey: "", at: 0 });
  // Intermediate model/tool rounds share one throttled read; streaming updates
  // must not restart a three-query settlement sequence on every usage event.
  useEffect(() => {
    if (!enabled || !turnKey || !consumptionKey || terminalKey) return;
    const previous = lastRefresh.current;
    const delay = previous.accountKey === accountKey
      ? Math.max(1_200, 10_000 - (Date.now() - previous.at)) : 1_200;
    const timer = setTimeout(() => {
      lastRefresh.current = { accountKey, at: Date.now() };
      void refresh(true);
    }, delay);
    return () => clearTimeout(timer);
  }, [enabled, accountKey, turnKey, consumptionKey, terminalKey, refresh]);

  useEffect(() => {
    if (!enabled || !turnKey || !terminalKey) return;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    const run = async (attempt: number) => {
      if (cancelled) return;
      lastRefresh.current = { accountKey, at: Date.now() };
      await refresh(true);
      // Remote accounting can lag behind the final usage SSE. Queries are
      // sequential, bounded, and cancelled on a new turn/account or unmount.
      if (!cancelled && attempt < 2) timer = setTimeout(() => void run(attempt + 1), attempt === 0 ? 2_000 : 6_000);
    };
    timer = setTimeout(() => void run(0), 800);
    return () => { cancelled = true; clearTimeout(timer); };
  }, [enabled, accountKey, turnKey, terminalKey, refresh]);
}
