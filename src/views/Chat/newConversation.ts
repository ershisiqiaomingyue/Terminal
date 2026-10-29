import type { ChatSession } from "./types";
import { workspaceKey } from "./chatIdentity.ts";

export function isUnsentConversation(session: ChatSession) {
  return !session.messages.length && !session.historyMessageCount && !session.agentRuns?.length
    && !session.contextCheckpoint && !session.usage && !session.artifacts?.length;
}

export function abandonedDraftIds(sessions: ChatSession[], activeId: string, knownHistory: ReadonlySet<string>) {
  return sessions.filter(session => session.id !== activeId && !session.archived
    && !knownHistory.has(session.id) && isUnsentConversation(session)).map(session => session.id);
}

/** A local draft has no native history yet; missing persisted history is different. */
export function conversationHasHistory(session: ChatSession | null | undefined, knownHistory: boolean) {
  return !session || knownHistory || Boolean(session.messages.length || session.historyMessageCount
    || session.agentRuns?.length || session.contextCheckpoint || session.usage || session.artifacts?.length);
}

export function findEmptyConversation(
  sessions: ChatSession[],
  scope: "workspace" | "standalone",
  workspacePath = "",
  preferredId = "",
  unresolvedHistoryIds: ReadonlySet<string> = new Set(),
) {
  const candidates = sessions.filter((session) => !session.archived
    && (session.scope || "standalone") === scope
    && (scope === "standalone" || workspaceKey(session.workspacePath || "") === workspaceKey(workspacePath))
    && isUnsentConversation(session)
    && !(unresolvedHistoryIds.has(session.id) && session.historyMessageCount === undefined));
  return candidates.find((session) => session.id === preferredId)
    || candidates.reduce<ChatSession | undefined>((latest, session) => !latest || session.updatedAt > latest.updatedAt ? session : latest, undefined);
}
