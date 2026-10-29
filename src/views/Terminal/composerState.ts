/**
 * CommandComposer 的纯逻辑层（开发方案 9.3 / T4-1）。
 *
 * 单独成文件的原因：面板本体是 tsx，Node 单测跑不了；而"编辑草稿后风险要重新
 * 分级""destructive 必须手打命令首词才放行"这类判定是最不能靠人肉复核的部分，
 * 必须能被 `node --experimental-strip-types --test` 直接覆盖。
 *
 * 安全要点（10.1 第二层）：用户**编辑**草稿后风险必须按编辑后的文本重新算。
 * 模型给的是 `read-only`、用户把它改成 `rm -rf /sdcard` 的情况，只信模型等于
 * 把防线交给用户的手速。
 */
import {
  classifyCommandRisk,
  confirmationToken,
  isConfirmationSatisfied,
  maxRisk,
  needsConfirmClick,
  needsTypedConfirmation,
} from "../../platform/terminal/ai/commandComposer.ts";
import type { CommandDraft, CommandRisk } from "../../platform/terminal/ai/commandComposer.ts";

export type ComposerPhase = "idle" | "drafting" | "ready" | "error";

export interface ComposerMeta {
  modelId: string;
  durationMs: number;
  requestBytes: number;
}

export interface ComposerState {
  /** 第 1 行的自然语言（含 `#` 前缀之前的原文，`#` 由调用方剥）。 */
  nl: string;
  phase: ComposerPhase;
  /** 模型给出的完整命令序列。 */
  commands: string[];
  /** 第 2 行可编辑草稿，永远等于当前正在确认的那条（初始 = commands[0]）。 */
  editable: string;
  explanation: string;
  /** 当前草稿的最终风险：`max(模型判定, 本地分级)`，编辑后会重算。 */
  risk: CommandRisk;
  /** 模型原始判定，作为**下限**保留。 */
  modelRisk?: CommandRisk;
  needsRoot: boolean;
  riskEscalated: boolean;
  /** 多命令逐条确认（9.3.3）时正在处理第几条。 */
  index: number;
  /** `mutating` 需要的显式点击确认。 */
  confirmed: boolean;
  /** `destructive` 需要的手打确认文本。 */
  typed: string;
  error: string;
  meta: ComposerMeta | null;
  /** 全部命令都处理完了。 */
  done: boolean;
}

export type ComposerAction =
  | { type: "nl"; value: string }
  | { type: "start" }
  | { type: "success"; draft: CommandDraft; meta: ComposerMeta }
  | { type: "failure"; message: string }
  | { type: "edit"; value: string }
  | { type: "confirm" }
  | { type: "typed"; value: string }
  /** 当前这条已执行，推进到下一条（没有下一条则标记 done）。 */
  | { type: "advance" }
  | { type: "reset" };

export function initialComposerState(): ComposerState {
  return {
    nl: "",
    phase: "idle",
    commands: [],
    editable: "",
    explanation: "",
    risk: "read-only",
    modelRisk: undefined,
    needsRoot: false,
    riskEscalated: false,
    index: 0,
    confirmed: false,
    typed: "",
    error: "",
    meta: null,
    done: false,
  };
}

/**
 * 重新计算草稿风险。
 *
 * `modelRisk` 作为**下限**而不是等值：用户把一条被判 destructive 的命令改简单了，
 * 仍然要求确认——多要一次确认只损失一次点击，少要一次可能删库。
 */
export function composerRisk(editable: string, modelRisk?: CommandRisk): CommandRisk {
  const local = classifyCommandRisk(editable);
  return modelRisk ? maxRisk(modelRisk, local) : local;
}

export function composerReducer(state: ComposerState, action: ComposerAction): ComposerState {
  switch (action.type) {
    case "nl":
      // 生成中不接受改词，避免"看到的输入"和"正在跑的请求"不一致。
      if (state.phase === "drafting") return state;
      return { ...state, nl: action.value };
    case "start":
      return { ...state, phase: "drafting", error: "", done: false };
    case "success": {
      const commands = action.draft.commands;
      const first = commands[0] ?? "";
      return {
        ...state,
        phase: "ready",
        commands,
        editable: first,
        explanation: action.draft.explanation,
        risk: composerRisk(first, action.draft.risk),
        modelRisk: action.draft.risk,
        needsRoot: action.draft.needsRoot,
        riskEscalated: action.draft.riskEscalated,
        index: 0,
        confirmed: false,
        typed: "",
        error: "",
        meta: action.meta,
        done: false,
      };
    }
    case "failure":
      return { ...state, phase: "error", error: action.message, commands: [], editable: "" };
    case "edit":
      // 改一个字风险就可能变，确认状态必须作废（否则"确认过一次"能顶掉新命令）。
      return {
        ...state,
        editable: action.value,
        risk: composerRisk(action.value, state.modelRisk),
        confirmed: false,
        typed: "",
      };
    case "confirm":
      return { ...state, confirmed: true };
    case "typed":
      return { ...state, typed: action.value };
    case "advance":
      return afterExecute(state);
    case "reset":
      return initialComposerState();
    default:
      return state;
  }
}

export type ExecuteGate = { ok: true } | { ok: false; reason: string };

/**
 * 能否执行当前草稿（10.1 第二层）。
 *
 * - `read-only`：直接放行；
 * - `mutating`：需要一次显式点击；
 * - `destructive`：需要手打命令首词（`rm -rf x` 要打 `rm`）。
 */
export function executeGate(state: ComposerState): ExecuteGate {
  if (state.phase !== "ready") return { ok: false, reason: "草稿还没生成好" };
  if (state.done) return { ok: false, reason: "这条草稿已经处理完了" };
  const command = state.editable.trim();
  if (!command) return { ok: false, reason: "命令为空" };
  if (needsTypedConfirmation(state.risk)) {
    if (!isConfirmationSatisfied(command, state.typed)) {
      return { ok: false, reason: `这条命令风险为 destructive，请手打 "${confirmationToken(command)}" 确认` };
    }
    return { ok: true };
  }
  if (needsConfirmClick(state.risk) && !state.confirmed) {
    return { ok: false, reason: "这条命令会改变系统状态，请先点确认" };
  }
  return { ok: true };
}

/** 还有几条没处理（不含当前这条）。 */
export function remainingCount(state: ComposerState): number {
  return Math.max(0, state.commands.length - state.index - 1);
}

/**
 * 当前这条执行完之后的状态。
 *
 * 有剩余命令就换到下一行并**重置确认态**（9.3.3 逐条确认：上一条的确认不能
 * 顺延到下一条），没有剩余则标记 done。
 */
export function afterExecute(state: ComposerState): ComposerState {
  const nextIndex = state.index + 1;
  if (nextIndex >= state.commands.length) {
    return { ...state, done: true, confirmed: false, typed: "" };
  }
  const next = state.commands[nextIndex] ?? "";
  return {
    ...state,
    index: nextIndex,
    editable: next,
    risk: composerRisk(next, state.modelRisk),
    confirmed: false,
    typed: "",
  };
}

/** 面板底部的操作提示（9.3.1 原文四键）。 */
export function composerHint(state: ComposerState): string {
  if (state.phase === "drafting") return "[Esc 取消] 正在生成命令…";
  if (state.phase === "error") return "[Enter 重新生成] [Esc 关闭]";
  if (state.phase !== "ready") return "[Enter 生成命令] [Esc 关闭]";
  const gate = executeGate(state);
  return gate.ok
    ? "[Enter 执行] [Tab 只插入不执行] [Esc 丢弃]"
    : `[Tab 只插入不执行] [Esc 丢弃] — ${gate.reason}`;
}

/** 当前草稿是否需要用户额外确认（决定是否渲染确认区）。 */
export function needsAttention(state: ComposerState): boolean {
  return state.phase === "ready" && !state.done && state.risk !== "read-only" && !executeGate(state).ok;
}
