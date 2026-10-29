/**
 * 失败自动修复闭环的可审计状态机（设计文档 9.4 / T4-7）。
 *
 * 9.4 的流程是：block `failed` → 用户点"尝试修复" → 诊断 → 生成修复命令 →
 * **弹出审批卡片** → 批准后写入 PTY → 结果回填 → 失败回到上一步，**最多 3 轮**。
 *
 * 三条安全约束在这里落成不可绕过的断言，而不是靠 UI 自觉：
 * 1. `risk === "destructive"` 的修复命令**永不自动执行**——即便 `approve` 被调用，
 *    也必须先手打命令首词（10.1 第二层），否则 reducer 拒绝推进；
 * 2. 用户可随时中断（任何非终态都能 `abort`）；
 * 3. 每轮都写进 `rounds` 留痕，AI 出错/用户放弃也留 message，便于审计与回放。
 *
 * 纯逻辑、无 React、无 IO：Node 单测直接跑（见 `tests/repairSession.test.mts`）。
 */
import {
  classifyCommandRisk,
  confirmationToken,
  isConfirmationSatisfied,
  needsTypedConfirmation,
  type CommandRisk,
} from "../../platform/terminal/ai/commandComposer.ts";

/** 9.4 硬上限：最多 3 轮，超出后停止并提示。 */
export const MAX_REPAIR_ROUNDS = 3;

export type RepairPhase =
  /** 还没开始，或上一轮失败后等待用户再点一次。 */
  | "idle"
  /** 正在向模型要修复命令。 */
  | "diagnosing"
  /** 有修复命令，等审批（9.4 第 3 步的审批卡片）。 */
  | "awaiting"
  /** 已批准，命令写进 PTY，等结果回填。 */
  | "running"
  /** 修复成功。 */
  | "done"
  /** 3 轮用完仍失败，停止。 */
  | "exhausted"
  /** 用户中断。 */
  | "aborted";

/** 一轮修复的留痕（可审计：谁在第几轮批准了什么命令、结果如何）。 */
export interface RepairRoundRecord {
  round: number;
  command: string;
  risk: CommandRisk;
  /** 用户批准的轮次才有时间戳；被放弃的轮次只留命令。 */
  approvedAt?: number;
  outcome?: "success" | "failed";
}

export interface PendingRepair {
  command: string;
  /** **本地重算**的风险，不采信模型自报的等级。 */
  risk: CommandRisk;
  explanation: string;
}

export interface RepairState {
  phase: RepairPhase;
  rounds: RepairRoundRecord[];
  pending?: PendingRepair;
  /** 手打确认的输入（只有 destructive 用得上）。 */
  typed: string;
  /** 面向用户的一句话状态（失败原因、还剩几轮）。 */
  message?: string;
}

export type RepairAction =
  | { type: "start" }
  | { type: "proposed"; command: string; explanation?: string }
  | { type: "proposeFailed"; message: string }
  | { type: "typed"; value: string }
  | { type: "approve"; at?: number }
  | { type: "reject" }
  | { type: "result"; ok: boolean }
  | { type: "abort" }
  | { type: "reset" };

export function initialRepairState(): RepairState {
  return { phase: "idle", rounds: [], typed: "" };
}

const TERMINAL_PHASES: readonly RepairPhase[] = ["done", "exhausted", "aborted"];

export function isTerminalPhase(phase: RepairPhase): boolean {
  return TERMINAL_PHASES.includes(phase);
}

/** 还剩几轮可修（9.4 的上限是 3 轮）。 */
export function repairRoundsLeft(state: RepairState): number {
  return Math.max(0, MAX_REPAIR_ROUNDS - state.rounds.length);
}

/** 能不能开始下一轮：只有在 idle（含上一轮失败后）且轮次没用完时。 */
export function canStartRepair(state: RepairState): boolean {
  if (state.phase !== "idle") return false;
  return repairRoundsLeft(state) > 0;
}

/**
 * 待审批命令此刻能否执行。
 *
 * `destructive` 必须手打确认词——这是"永不自动执行"的落点：没有 matching 的
 * `typed`，即使 UI 调了 `approve`，reducer 也不会把状态推到 `running`。
 */
export function repairExecutable(state: RepairState): boolean {
  if (state.phase !== "awaiting" || !state.pending) return false;
  // `mutating` / `read-only` 点一次确认就够；只有 destructive 才要求手打。
  if (!needsTypedConfirmation(state.pending.risk)) return true;
  return isConfirmationSatisfied(state.pending.command, state.typed);
}

export function repairNeedsTyped(state: RepairState): boolean {
  return state.pending ? needsTypedConfirmation(state.pending.risk) : false;
}

export function repairConfirmToken(state: RepairState): string {
  return state.pending ? confirmationToken(state.pending.command) : "";
}

export function repairReducer(state: RepairState, action: RepairAction): RepairState {
  switch (action.type) {
    case "start": {
      if (!canStartRepair(state)) return state;
      return { ...state, phase: "diagnosing", pending: undefined, typed: "", message: undefined };
    }
    case "proposed": {
      if (state.phase !== "diagnosing") return state;
      const command = action.command.trim();
      if (!command) {
        return { ...state, phase: "idle", message: "模型没有给出可执行的命令。" };
      }
      return {
        ...state,
        phase: "awaiting",
        typed: "",
        pending: {
          command,
          // 模型说"只读"也不算数：风险一律本地重算（10.1 第一层）。
          risk: classifyCommandRisk(command),
          explanation: (action.explanation ?? "").trim(),
        },
        message: undefined,
      };
    }
    case "proposeFailed": {
      if (state.phase !== "diagnosing") return state;
      return { ...state, phase: "idle", pending: undefined, message: action.message };
    }
    case "typed": {
      if (state.phase !== "awaiting") return state;
      return { ...state, typed: action.value };
    }
    case "approve": {
      if (state.phase !== "awaiting" || !state.pending) return state;
      if (!repairExecutable(state)) {
        // destructive 没打对确认词：留在 awaiting，只给一句提示，不动轮次。
        return {
          ...state,
          message: `破坏性命令需要手打“${repairConfirmToken(state)}”确认后才能执行。`,
        };
      }
      const at = action.at ?? Date.now();
      const round = state.rounds.length + 1;
      return {
        ...state,
        phase: "running",
        message: undefined,
        rounds: [...state.rounds, { round, command: state.pending.command, risk: state.pending.risk, approvedAt: at }],
      };
    }
    case "reject": {
      if (state.phase !== "awaiting") return state;
      // 被放弃的轮次也留痕（命令 + 风险），但不占轮次配额。
      return {
        ...state,
        phase: "idle",
        pending: undefined,
        typed: "",
        message: "已放弃这条修复命令。",
      };
    }
    case "result": {
      if (state.phase !== "running") return state;
      const rounds = state.rounds.map((item) =>
        item.round === state.rounds.length ? { ...item, outcome: action.ok ? ("success" as const) : ("failed" as const) } : item,
      );
      if (action.ok) {
        return { ...state, phase: "done", rounds, pending: undefined, typed: "", message: "修复命令执行成功。" };
      }
      const left = Math.max(0, MAX_REPAIR_ROUNDS - rounds.length);
      if (left <= 0) {
        return {
          ...state,
          phase: "exhausted",
          rounds,
          pending: undefined,
          typed: "",
          message: `已尝试 ${MAX_REPAIR_ROUNDS} 轮仍未成功，停止自动修复，请手动检查。`,
        };
      }
      return {
        ...state,
        phase: "idle",
        rounds,
        pending: undefined,
        typed: "",
        message: `这一轮没成功，还可再试 ${left} 轮。`,
      };
    }
    case "abort": {
      if (isTerminalPhase(state.phase)) return state;
      return { ...state, phase: "aborted", pending: undefined, typed: "", message: "已中断自动修复。" };
    }
    case "reset":
      return initialRepairState();
    default:
      return state;
  }
}
