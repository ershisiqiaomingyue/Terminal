import { useMemo } from "react";
import { Flag } from "lucide-react";
import type { AgentThreadSnapshot } from "@/platform/chatApi";

/** G1/G2：goal 工具事件流里的最新 goal 状态（tool_call "goal" 的输出）。 */
export type LiveGoal = {
  goalId: string;
  objective: string;
  status: string;
  acceptance: string[];
  round: number;
  maxRounds: number;
  tokensUsed?: number;
  tokenBudget?: number | null;
};

const GOAL_TERMINAL_STATUSES = new Set(["complete", "dropped"]);

function inputObject(value: unknown): Record<string, unknown> | null {
  if (typeof value === "string") {
    try { value = JSON.parse(value); } catch { return null; }
  }
  return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : null;
}

/** 从快照的工具调用事件流里恢复最新 goal 状态；没有 goal 活动时返回 null。 */
export function activeGoalFromSnapshot(snapshot: AgentThreadSnapshot | null): LiveGoal | null {
  const items = snapshot?.items ?? [];
  let latest: LiveGoal | null = null;
  for (const item of items) {
    if (item.kind !== "tool_call" || item.name !== "goal") continue;
    const output = inputObject(item.output);
    if (!output) continue;
    const goal = inputObject(output.goal);
    if (!goal || typeof goal.goal_id !== "string") continue;
    const acceptance = Array.isArray(goal.acceptance)
      ? goal.acceptance.filter((value): value is string => typeof value === "string")
      : [];
    latest = {
      goalId: goal.goal_id,
      objective: typeof goal.objective === "string" ? goal.objective : "",
      status: typeof goal.status === "string" ? goal.status : "active",
      acceptance,
      round: typeof goal.round === "number" ? goal.round : 0,
      maxRounds: typeof goal.max_rounds === "number" ? goal.max_rounds : 8,
      tokensUsed: typeof goal.tokens_used === "number" ? goal.tokens_used : undefined,
      tokenBudget: typeof goal.token_budget === "number" ? goal.token_budget : null,
    };
  }
  // 终态徽章保留显示（完成/放弃是重要结局），但 usage_limited 等中间态也算活着。
  return latest;
}

const STATUS_LABELS: Record<string, string> = {
  active: "进行中",
  paused: "已暂停",
  blocked: "受阻",
  usage_limited: "用量受限",
  budget_limited: "预算耗尽",
  complete: "已完成",
  dropped: "已放弃",
};

/** 对话输入区上方的 goal 徽章——跨轮目标的状态锚点（悬停看验收条件与进度）。 */
export function GoalBadge({ goal }: { goal: LiveGoal | null }) {
  const detail = useMemo(() => {
    if (!goal) return "";
    const lines: string[] = [`状态：${STATUS_LABELS[goal.status] ?? goal.status}`, `轮次：${goal.round}/${goal.maxRounds}`];
    if (goal.acceptance.length) lines.push(`验收：${goal.acceptance.join("；")}`);
    if (goal.tokensUsed != null) {
      lines.push(`用量：${goal.tokensUsed.toLocaleString("zh-CN")} tokens${goal.tokenBudget != null ? ` / 预算 ${goal.tokenBudget.toLocaleString("zh-CN")}` : ""}`);
    }
    return lines.join("\n");
  }, [goal]);
  if (!goal) return null;
  const terminal = GOAL_TERMINAL_STATUSES.has(goal.status);
  return <div
    className={`agent-turn-note goal-badge${terminal ? " is-terminal" : ""}`}
    role="status"
    title={detail}
  >
    <Flag size={14} />
    <span>
      <b>{terminal ? "目标已达" : "目标"}：</b>{goal.objective}
      <small className="agent-steer-accepted-hint">{STATUS_LABELS[goal.status] ?? goal.status} · 第 {goal.round} 轮</small>
    </span>
  </div>;
}
