import { useEffect, useState } from "react";
import { ProgressRing, type ProgressRingSegment } from "rain-ui";
import { formatTokenCount } from "./tokenCount";

type ContextAnomaly = { system: number; tools: number; total: number };

type Props = {
  tokens?: number | null;
  window?: number | null;
  systemTokens?: number;
  toolTokens?: number;
  /** P8-D：供应商上报的自相矛盾读数（system+tools > total）。没有任何有效读数时显示「数据异常」而非空白。 */
  anomaly?: ContextAnomaly | null;
  /** P8-F：本轮压缩触发点（消息级预算扣固定前缀），tooltip 显示。 */
  measuredTrigger?: number | null;
  /** 读数来源模型（跨模型 fallback 时 UI 标注，防误导）。 */
  readingModelId?: string;
  checking: boolean;
  sending: boolean;
  onRefresh: () => void;
  sessionId: string;
};

/**
 * Composer context-usage control.
 *
 * Thin binding around the library `ProgressRing`: this file keeps only the
 * product wiring (token→percent math, the breakdown rows, refresh on open and
 * the reset when the session changes). Ring geometry, the 70/90 thresholds,
 * the panel and the legend all live in the component.
 */
export function ContextUsageIndicator({ tokens, window: capacity, systemTokens, toolTokens, anomaly, measuredTrigger, readingModelId, checking, sending, onRefresh, sessionId }: Props) {
  // Remount on session change so the panel closes and no stale reading survives.
  const [instanceKey, setInstanceKey] = useState(sessionId);
  useEffect(() => { setInstanceKey(sessionId); }, [sessionId]);

  const percent = capacity && tokens != null
    ? Math.min(100, Math.max(0, Math.round(tokens / capacity * 100)))
    : null;

  // Older runs have no breakdown. Do not invent prompt/tool counts for them.
  const hasBreakdown = tokens != null && systemTokens != null && toolTokens != null && systemTokens + toolTokens <= tokens;
  const toPercent = (value?: number) => (capacity && value != null ? Math.min(100, Math.max(0, value / capacity * 100)) : null);
  const segment = (id: string, label: string, value?: number): ProgressRingSegment => ({
    id, label, percent: toPercent(value), valueText: value == null ? "—" : `~${formatTokenCount(value)}`,
  });

  const segments: ProgressRingSegment[] | undefined = hasBreakdown
    ? [
        segment("system", "系统提示词", systemTokens!),
        segment("tools", "工具定义", toolTokens!),
        segment("messages", "对话消息", tokens! - systemTokens! - toolTokens!),
      ]
    : [
        segment("system", "系统提示词"),
        segment("tools", "工具定义"),
        segment("messages", "对话消息"),
      ];

  // P8-D：读数自相矛盾且没有任何有效读数时明确标出异常，而不是沉默地显示「未知」。
  // label 同时是 ProgressRing 的 title/aria-label，悬停可见矛盾的具体数字。
  const hasAnomaly = tokens == null && anomaly != null;
  // P8-F：触发点与占用各自明确——占用按声明窗口计百分比，压缩按消息级
  // 预算（扣固定前缀）的触发点计，两个分母不同是设计使然。
  // 读数来自其他模型时标注来源（会话视角：换模型后历史 breakdown 仍显示，
  // 但不让用户误以为是当前模型的前缀构成）。
  const triggerText = measuredTrigger != null && capacity != null
    ? `；压缩将在 ~${formatTokenCount(measuredTrigger)}（消息级预算的 ${Math.min(100, Math.round(measuredTrigger / capacity * 100))}%）触发`
    : "";
  const sourceText = readingModelId != null ? `（读数由 ${readingModelId} 上报）` : "";
  const label = hasAnomaly
    ? `上下文读数异常：供应商上报的读数自相矛盾（系统提示词 ${formatTokenCount(anomaly!.system)} + 工具定义 ${formatTokenCount(anomaly!.tools)} > 总量 ${formatTokenCount(anomaly!.total)}），已忽略该读数`
    : percent == null ? "当前上下文占用未知" : `上下文已用 ${percent}%${triggerText}${sourceText}`;
  const valueText = hasAnomaly ? "数据异常" : tokens == null ? "未知" : `~${formatTokenCount(tokens)} / ${formatTokenCount(capacity)}`;

  return (
    <ProgressRing
      key={instanceKey}
      percent={percent}
      label={label}
      valueText={valueText}
      segments={segments}
      busy={checking}
      panelLabel="上下文用量"
      onOpen={() => { if (!sending && !checking) onRefresh(); }}
    />
  );
}
