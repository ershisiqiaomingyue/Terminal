import type { AgentThreadSnapshot } from "@/platform/chatApi";

/** Which kind of interaction the run is blocked on while it waits for the user. */
export type ConversationPending = "question" | "approval" | null;

export type ConversationRunStatus = { revision: number; phase: "running" | "waiting" | null; pending: ConversationPending };

function pendingOf(snapshot: Pick<AgentThreadSnapshot, "pendingPermission">): ConversationPending {
  const permission = snapshot.pendingPermission;
  if (!permission) return null;
  return permission.toolName === "ask_user" ? "question" : "approval";
}

export function acceptConversationRunStatus(
  current: ConversationRunStatus | undefined,
  snapshot: Pick<AgentThreadSnapshot, "revision" | "status" | "activeTurn" | "pendingPermission">,
): ConversationRunStatus | undefined {
  if (!Number.isSafeInteger(snapshot.revision) || snapshot.revision < 0 || (current && snapshot.revision <= current.revision)) return current;
  const active = snapshot.activeTurn?.status === "running";
  const phase = active ? snapshot.status === "waiting_for_permission" ? "waiting" : "running" : null;
  return { revision: snapshot.revision, phase, pending: phase === "waiting" ? pendingOf(snapshot) : null };
}
