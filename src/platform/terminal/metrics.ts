/**
 * 终端度量埋点（开发方案 11.3 / 任务 T3-7）。
 *
 * 只做**进程内聚合**：计数器和延迟样本都留在内存里，由设置面板或调试入口读
 * `terminalMetricsSnapshot()`。不上报、不落盘——11.3 要的是"本地可观测"，用来
 * 判断阶段 3 的两个出口指标（请求体 P95 ≤ 16 KB、首 token P50 ≤ 1.5 s）是否达标。
 *
 * 三条约束：
 * 1. **零依赖**：本文件不 import 任何平台模块，Node 单测可以直直接载。
 * 2. **不抛异常**：埋点坏掉绝不能影响终端和 AI 主流程，所有入参都做了兜底。
 * 3. **有界内存**：延迟样本只留最近 `MAX_SAMPLES` 条，长时间跑终端不会涨内存。
 */

/** 内核渲染后端（11.1 的 `rendererType`）。 */
export type TerminalRendererType = "webgl" | "canvas" | "unknown";

/** 一次 AI 调用的结果，由服务层在结束时回报。 */
export interface AiCallRecord {
  mode: "explain" | "diagnose";
  ok: boolean;
  /** 端到端耗时（ms），含提示词组装与网络。 */
  durationMs: number;
  /** 实际发出的请求体字节数；失败在前置阶段时缺省。 */
  requestBytes?: number;
}

export interface TerminalMetricsSnapshot {
  coreLoadMs: number | null;
  rendererType: TerminalRendererType;
  pausedCount: number;
  pauseDurationMs: number;
  blocksCreated: number;
  blocksFailed: number;
  blocksInterrupted: number;
  blockSplitHeuristicMisses: number;
  aiExplainCount: number;
  aiDiagnoseCount: number;
  aiErrorCount: number;
  /** 失败次数 / 总次数；没有样本时为 0，避免出现 NaN。 */
  aiErrorRate: number;
  aiLatencyP50: number | null;
  aiLatencyP95: number | null;
  aiRequestBytesP50: number | null;
  aiRequestBytesP95: number | null;
}

/** 延迟与字节样本上限：512 条足以算 P50/P95，又不会让内存随会话时长线性增长。 */
export const MAX_SAMPLES = 512;

interface MetricsState {
  coreLoadMs: number | null;
  rendererType: TerminalRendererType;
  pausedCount: number;
  pauseDurationMs: number;
  blocksCreated: number;
  blocksFailed: number;
  blocksInterrupted: number;
  blockSplitHeuristicMisses: number;
  aiExplainCount: number;
  aiDiagnoseCount: number;
  aiErrorCount: number;
  aiLatencies: number[];
  aiRequestBytes: number[];
}

function emptyState(): MetricsState {
  return {
    coreLoadMs: null,
    rendererType: "unknown",
    pausedCount: 0,
    pauseDurationMs: 0,
    blocksCreated: 0,
    blocksFailed: 0,
    blocksInterrupted: 0,
    blockSplitHeuristicMisses: 0,
    aiExplainCount: 0,
    aiDiagnoseCount: 0,
    aiErrorCount: 0,
    aiLatencies: [],
    aiRequestBytes: [],
  };
}

let state: MetricsState = emptyState();

/** wasm 初始化耗时（内核创建时记一次，后续覆盖为最新值）。 */
export function recordCoreLoad(durationMs: number): void {
  if (!Number.isFinite(durationMs) || durationMs < 0) return;
  state.coreLoadMs = Math.round(durationMs);
}

export function setTerminalRendererType(renderer: TerminalRendererType): void {
  state.rendererType = renderer;
}

/** 一次暂停渲染（后台页签省 GPU）：记次数与累计时长。 */
export function recordRenderPause(durationMs: number): void {
  state.pausedCount += 1;
  if (Number.isFinite(durationMs) && durationMs > 0) state.pauseDurationMs += Math.round(durationMs);
}

/** 块收尾时按最终状态记数；`running` 之类未收尾状态不计数。 */
export function recordBlockFinished(status: string): void {
  if (status === "failed") state.blocksFailed += 1;
  else if (status === "interrupted") state.blocksInterrupted += 1;
}

export function recordBlockCreated(): void {
  state.blocksCreated += 1;
}

/**
 * 用户手动纠正切块（把两条被误合并的命令拆开、或把误拆的合并）。
 * 用来调提示符正则——记的是"启发式没猜对"的次数，不是块数。
 */
export function recordBlockSplitMiss(): void {
  state.blockSplitHeuristicMisses += 1;
}

/** 服务层在每次真实请求结束后回报一次。前置拒绝（没模型、块不可用）不该走这里。 */
export function recordAiCall(record: AiCallRecord): void {
  if (record.mode === "diagnose") state.aiDiagnoseCount += 1;
  else state.aiExplainCount += 1;
  if (!record.ok) state.aiErrorCount += 1;
  pushSample(state.aiLatencies, record.durationMs);
  if (typeof record.requestBytes === "number") pushSample(state.aiRequestBytes, record.requestBytes);
}

function pushSample(list: number[], value: number): void {
  if (!Number.isFinite(value) || value < 0) return;
  list.push(value);
  // 超出上限时丢掉最旧的样本：既保内存，又让统计偏向最近的行为。
  if (list.length > MAX_SAMPLES) list.splice(0, list.length - MAX_SAMPLES);
}

/**
 * 最近秩百分位（nearest-rank）：取排序后第 `ceil(p/100 * n)` 个样本。
 * 样本不足时返回 `null`，让调用方区分"没有数据"和"测得 0"。
 */
export function percentile(samples: readonly number[], p: number): number | null {
  if (samples.length === 0) return null;
  const clamped = Math.min(100, Math.max(0, p));
  const sorted = [...samples].sort((a, b) => a - b);
  const rank = Math.ceil((clamped / 100) * sorted.length);
  const index = Math.min(sorted.length - 1, Math.max(0, rank - 1));
  return sorted[index];
}

/** 读一份快照（拷贝，外部改不影响内部状态）。 */
export function terminalMetricsSnapshot(): TerminalMetricsSnapshot {
  const totalAi = state.aiExplainCount + state.aiDiagnoseCount;
  return {
    coreLoadMs: state.coreLoadMs,
    rendererType: state.rendererType,
    pausedCount: state.pausedCount,
    pauseDurationMs: state.pauseDurationMs,
    blocksCreated: state.blocksCreated,
    blocksFailed: state.blocksFailed,
    blocksInterrupted: state.blocksInterrupted,
    blockSplitHeuristicMisses: state.blockSplitHeuristicMisses,
    aiExplainCount: state.aiExplainCount,
    aiDiagnoseCount: state.aiDiagnoseCount,
    aiErrorCount: state.aiErrorCount,
    aiErrorRate: totalAi === 0 ? 0 : state.aiErrorCount / totalAi,
    aiLatencyP50: percentile(state.aiLatencies, 50),
    aiLatencyP95: percentile(state.aiLatencies, 95),
    aiRequestBytesP50: percentile(state.aiRequestBytes, 50),
    aiRequestBytesP95: percentile(state.aiRequestBytes, 95),
  };
}

/** 清空（单测与"重置统计"按钮用）。 */
export function resetTerminalMetrics(): void {
  state = emptyState();
}

/** 导出成可粘贴的一行 JSON，方便用户反馈问题时附上。 */
export function exportTerminalMetrics(): string {
  return JSON.stringify(terminalMetricsSnapshot());
}
