export type ReasoningEffort = "low" | "medium" | "high" | "xhigh" | "max";

export interface ModelCapability {
  model: string;
  provider?: string | null;
  contextWindow?: number | null;
  supportsReasoning?: boolean | null;
  reasoningEfforts?: string[];
  reasoningBudget?: boolean;
  source?: string | null;
}

export function findModelCapability(records: ModelCapability[], modelId: string, brand?: string | null) {
  const id = modelId.trim().toLowerCase();
  const eligible = records.filter((record) => !record.provider || record.provider.toLowerCase() === brand?.toLowerCase());
  const exact = eligible.find((record) => record.model.toLowerCase() === id);
  if (exact) return exact;
  const basename = id.split("/").at(-1) || id;
  return eligible.find((record) => {
    if (record.model.includes("/")) return false;
    const name = record.model.toLowerCase();
    if (name === basename) return true;
    const suffix = basename.startsWith(`${name}-`) || basename.startsWith(`${name}:`) ? basename.slice(name.length + 1) : "";
    return /^(latest|preview|free|online|\d{8}|\d{4}|\d{4}-\d{2}-\d{2})$/.test(suffix);
  });
}

export function selectableReasoningEfforts(record: ModelCapability, protocol: string): ReasoningEffort[] {
  if (!record.supportsReasoning) return [];
  if (record.reasoningEfforts?.length) {
    return record.reasoningEfforts.filter((value): value is ReasoningEffort => ["low", "medium", "high", "xhigh", "max"].includes(value));
  }
  return protocol === "anthropic" && record.reasoningBudget ? ["low", "medium", "high", "max"] : [];
}

export function effectiveReasoningEffort(model: { reasoningEfforts?: ReasoningEffort[]; supportsReasoning?: boolean | null } | undefined, effort: ReasoningEffort) {
  if (model?.reasoningEfforts) return model.reasoningEfforts.includes(effort) ? effort
    : model.reasoningEfforts.find((value) => value === "medium") || model.reasoningEfforts[0] || null;
  return model?.supportsReasoning ? effort : null;
}

/**
 * 设置面板「上下文窗口」输入框的默认预填值，与 Rust 端
 * crates/agent-core/src/provider_runtime.rs 的 PREFILLED_CONTEXT_WINDOW 保持一致。
 *
 * 它只是输入框的初始提示，不是用户声明：旧版本保存供应商时会把它原样写进
 * provider / model 层级，所以持久化值等于它时必须按「未声明」处理。
 */
export const PREFILLED_CONTEXT_WINDOW = 65_536;

/** 持久化窗口是否由用户显式声明（等于预填值、或等于已知窗口都不算声明）。 */
export function isDeclaredContextWindow(persisted?: number | null, known?: number | null) {
  return Boolean(persisted && persisted > 0 && persisted !== PREFILLED_CONTEXT_WINDOW && persisted !== known);
}

/**
 * 与 Rust 端 resolved_model_context_window 同一条优先级：
 * 用户显式声明 > 内置/目录已知窗口 > 未声明（后端对未知模型按 64K 保守估算）。
 * 面板与模型行只展示这里的返回值，避免显示后端不会使用的数字。
 */
export function resolveContextWindow(persisted?: number | null, known?: number | null) {
  if (isDeclaredContextWindow(persisted, known)) return persisted as number;
  return known && known > 0 ? known : null;
}
