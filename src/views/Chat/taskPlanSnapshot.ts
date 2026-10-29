export type TaskPlanStep = {
  id: string;
  title: string;
  status: "pending" | "running" | "done" | "error" | "cancelled";
};

export type TaskPlanSnapshot = {
  goal: string;
  steps: TaskPlanStep[];
};

type EventLike = Record<string, unknown>;

const STEP_STATUSES = new Set<TaskPlanStep["status"]>(["pending", "running", "done", "error", "cancelled"]);

export function taskPlanSnapshotFromEvents(events: EventLike[]): TaskPlanSnapshot | null {
  let snapshot: TaskPlanSnapshot | null = null;
  for (const event of events) {
    if (event.type !== "task_plan") continue;
    const goal = typeof event.goal === "string" ? event.goal.trim() : "";
    const rawSteps = Array.isArray(event.steps) ? event.steps : [];
    const steps = rawSteps.flatMap((step, index) => {
      if (!step || typeof step !== "object" || Array.isArray(step)) return [];
      const value = step as Record<string, unknown>;
      const title = typeof value.title === "string" ? value.title.trim() : "";
      const status = typeof value.status === "string" && STEP_STATUSES.has(value.status as TaskPlanStep["status"])
        ? value.status as TaskPlanStep["status"]
        : "pending";
      return title ? [{ id: typeof value.id === "string" && value.id ? value.id : `step-${index}`, title, status }] : [];
    });
    if (goal || steps.length) snapshot = { goal, steps };
  }
  return snapshot;
}

export function taskPlanSnapshotFromThreadItems(items: EventLike[]): TaskPlanSnapshot | null {
  const plans = items.filter((item) => item.kind === "task_plan");
  const latest = plans.at(-1);
  if (!latest) return null;
  return taskPlanSnapshotFromEvents([{ type: "task_plan", goal: latest.goal, steps: latest.steps }]);
}

/** The composer only owns the plan of a currently executing turn. */
export function activeComposerTaskPlan(snapshot: { activeTurn?: { status: string } | null; status: string; items: EventLike[] } | null) {
  if (!snapshot?.activeTurn || snapshot.activeTurn.status !== "running") return null;
  return taskPlanSnapshotFromThreadItems(snapshot.items);
}
