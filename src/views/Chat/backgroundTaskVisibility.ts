type Task = { sessionId: string; turnId?: string; status: string; finishedAt?: number | null };
export type BackgroundTaskTurn = { id: string; startedAt?: number };

export function visibleBackgroundTasks<T extends Task>(tasks: T[], sessionId: string, turn?: BackgroundTaskTurn): T[] {
  return tasks.filter(task => task.sessionId === sessionId && (!turn?.id || task.status === "running"
    || task.turnId === turn.id
    || (turn.startedAt != null && task.finishedAt != null && task.finishedAt >= turn.startedAt)));
}
