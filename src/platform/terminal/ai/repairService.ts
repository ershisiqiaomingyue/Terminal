/**
 * 修复命令的生成服务（设计文档 9.4 第 2 步 / T4-7）。
 *
 * 只做两件事：把"失败块 + 诊断"变成一次模型请求，再把模型输出**解析并加固**
 * 成一条带风险等级的命令。它**不执行任何命令**——执行永远走审批卡片
 * （见 `src/views/Terminal/repairSession.ts` 的 `approve`）。
 */
import type { CommandBlock } from "../blocks/types.ts";
import { parseCommandDraft, type CommandRisk } from "./commandComposer.ts";
import { buildRepairPrompt } from "./repairPrompt.ts";
import { buildBlockAiRequest, resolveModelChoice, type BlockAiDeps, type TerminalModelSelection, type TerminalProviderLike } from "./terminalAiService.ts";
import { MAX_REQUEST_BYTES } from "./prompts.ts";
import type { DeviceContext } from "./deviceContext.ts";

export interface RepairProposal {
  command: string;
  explanation: string;
  /** 风险已按 `max(模型, 本地)` 加固过，调用方直接用。 */
  risk: CommandRisk;
  /** 模型给多了时被丢弃的命令（只采用第一条），UI 需要如实告知。 */
  dropped: string[];
  modelId: string;
  requestBytes: number;
  durationMs: number;
}

export type RepairOutcome =
  | { ok: true; proposal: RepairProposal }
  | { ok: false; message: string };

export interface ProposeRepairOptions {
  /** 上一轮诊断文本（9.4 第 2 步）。 */
  diagnosis: string;
  /** 第几轮，1 起。 */
  attempt?: number;
  previous?: { command: string; outcome: "failed" | "cancelled" }[];
  device?: DeviceContext;
  selection?: TerminalModelSelection;
  signal?: AbortSignal;
}

function describeInvokeError(error: unknown): string {
  const raw = error instanceof Error ? error.message : typeof error === "string" ? error : "";
  const detail = raw.trim();
  if (!detail) return "AI 请求失败，请稍后重试。";
  if (/timeout|timed out/i.test(detail)) return `AI 请求超时，请重试。（${detail}）`;
  if (/401|403|unauthor|forbidden|api key/i.test(detail)) return `模型鉴权失败，请检查供应商配置。（${detail}）`;
  if (/429|rate limit|quota/i.test(detail)) return `模型限流或额度不足，请稍后重试。（${detail}）`;
  return `AI 请求失败：${detail}`;
}

/** 向模型要一条修复命令。失败只返回一句人话，不抛异常。 */
export async function proposeRepair(
  block: CommandBlock,
  deps: BlockAiDeps,
  options: ProposeRepairOptions,
): Promise<RepairOutcome> {
  if (options.signal?.aborted) return { ok: false, message: "已取消。" };

  const now = deps.now ?? (() => Date.now());
  const startedAt = now();
  const newId =
    deps.newId ?? ((prefix: string) => `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`);

  if (!deps.listProviders) return { ok: false, message: "未接入模型供应商列表，无法生成修复命令。" };
  let providers: TerminalProviderLike[] = [];
  try {
    providers = await deps.listProviders();
  } catch {
    providers = [];
  }
  const choice = resolveModelChoice(providers, options.selection);
  if (!choice) return { ok: false, message: "请先在设置中配置模型供应商。" };

  const prompt = buildRepairPrompt({
    block,
    diagnosis: options.diagnosis,
    attempt: options.attempt ?? 1,
    previous: options.previous,
    device: options.device,
  });
  if (prompt.bytes > MAX_REQUEST_BYTES * 2) {
    return {
      ok: false,
      message: `命令或输出过长（约 ${Math.round(prompt.bytes / 1024)} KB），已停止发送。请先缩短输出。`,
    };
  }

  const request = buildBlockAiRequest({
    modelId: choice.modelId,
    supportsReasoning: choice.supportsReasoning,
    prompt,
    sessionId: `terminal-repair-${block.id}`,
    turnId: newId("terminal-repair-turn"),
  });

  let text = "";
  try {
    const response = await deps.invoke<{ text?: string }>("agent_one_shot", { request });
    text = (response?.text ?? "").trim();
  } catch (error) {
    return { ok: false, message: describeInvokeError(error) };
  }
  if (!text) return { ok: false, message: "模型没有返回内容，请重试。" };

  const parsed = parseCommandDraft(text);
  if (!parsed.ok) return { ok: false, message: parsed.error };

  const [command, ...rest] = parsed.draft.commands;
  return {
    ok: true,
    proposal: {
      command,
      explanation: parsed.draft.explanation,
      risk: parsed.draft.risk,
      dropped: rest,
      modelId: choice.modelId,
      requestBytes: prompt.bytes,
      durationMs: Math.max(0, now() - startedAt),
    },
  };
}
