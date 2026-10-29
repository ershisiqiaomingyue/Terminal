import React from "react";
import "./task-step-list.css";

/** Status of one task step, mirroring the card-showcase.html spec. */
export type TaskStepStatus = "done" | "running" | "pending" | "error" | "cancelled";

export interface TaskStep {
  /** Main step text; plain words and chip entries can be mixed. */
  content: React.ReactNode;
  status: TaskStepStatus;
  /** Optional trailing chip, e.g. a target app or assignee. */
  chip?: React.ReactNode;
}

export interface TaskStepListProps {
  steps: TaskStep[];
  /** Compact rows, matching the vertical compression used in the chat page. */
  compact?: boolean;
  onStepClick?: (index: number) => void;
  className?: string;
}

const STATUS_LABEL: Record<TaskStepStatus, string> = {
  done: "已完成",
  running: "执行中",
  pending: "待处理",
  error: "执行失败",
  cancelled: "已停止",
};

export const TaskStepStatusIcon: React.FC<{ status: TaskStepStatus; className?: string }> = ({ status, className }) => (
  <span className={["task-step-icon", status, className].filter(Boolean).join(" ")} aria-hidden="true">
    {status === "done" && <svg viewBox="0 0 24 24"><path d="M20 6L9 17l-5-5" /></svg>}
    {(status === "error" || status === "cancelled") && <svg viewBox="0 0 24 24"><path d="m7 7 10 10M17 7 7 17" /></svg>}
    {status === "running" && <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5"><circle cx="12" cy="12" r="9" strokeDasharray="32" strokeLinecap="round" /></svg>}
  </span>
);

/**
 * Vertical task checklist: done = solid ring with a check, running = spinning
 * dash arc, pending = single hollow ring. Exactly the geometry from the
 * design showcase (20px circle, no inner circle on pending).
 */
export const TaskStepList: React.FC<TaskStepListProps> = ({
  steps,
  compact = false,
  onStepClick,
  className,
}) => (
  <div className={["task-step-list", compact && "is-compact", className].filter(Boolean).join(" ")} role="list">
    {steps.map((step, index) => (
      <div
        key={index}
        role="listitem"
        aria-label={`${STATUS_LABEL[step.status]}：${typeof step.content === "string" ? step.content : ""}`}
        className={["task-step-row", `status-${step.status}`, onStepClick && "is-clickable"]
          .filter(Boolean)
          .join(" ")}
        onClick={onStepClick ? () => onStepClick(index) : undefined}
      >
        <TaskStepStatusIcon status={step.status} />
        <span className="task-step-content">
          {step.content}
          {step.chip && <span className="task-step-chip">{step.chip}</span>}
        </span>
      </div>
    ))}
  </div>
);

export default TaskStepList;
