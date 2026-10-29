import { getHistorySession, listHistorySummaries } from "../../platform/chatApi";
import { isTauri } from "../../platform/runtime";
import { readChatNavSnapshot, readChatSessions, readDeletedChatSessionIds } from "./chatNavStore";
import { configureHistoryReadAhead, conversationHistoryReadAhead } from "./historyReadAhead";
import { loadSessions } from "./sessionHistory";

configureHistoryReadAhead(getHistorySession);

export async function prefetchConversation(id: string) {
  if (!id || !isTauri() || readDeletedChatSessionIds().has(id)) return Promise.resolve(null);
  // Navigation warms the transcript only. Execution pages belong to mounted,
  // expanded rows; a long conversation must not fan out hundreds of reads.
  return conversationHistoryReadAhead.prefetch(id).catch(() => null);
}

/** Prepare the selected/recent transcript before showing the app. */
export async function warmConversationHistory() {
  const sessions = readChatSessions();
  const saved = readChatNavSnapshot().sessionId;
  const selected = saved && (!sessions.length || sessions.some(session => session.id === saved && !session.archived))
    ? saved : [...sessions].filter(session => !session.archived).sort((a, b) => b.updatedAt - a.updatedAt)[0]?.id;
  if (selected) await prefetchConversation(selected);
  else if (isTauri()) {
    const summaries = await listHistorySummaries().catch(() => []);
    const available = summaries.filter(summary => !summary.archived && summary.metadata?.scope !== "collaboration_agent");
    const recent = available.find(summary => summary.metadata?.scope !== "workspace") || available[0];
    if (recent) await prefetchConversation(recent.id);
  }
  loadSessions();
}
