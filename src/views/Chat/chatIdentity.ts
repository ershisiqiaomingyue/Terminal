import type { ChatSession } from "./types";

export function workspaceKey(path: string) {
  const normalized = path.replace(/\\/g, "/").replace(/\/+$/, "");
  return /^(?:[A-Za-z]:\/|\/\/)/.test(normalized) ? normalized.toLowerCase() : normalized;
}

/** Identity comes from the stored id, never from a title or message text. */
export function uniqueChatSessions(value: unknown): ChatSession[] {
  if (!Array.isArray(value)) return [];
  const sessions = new Map<string, ChatSession>();
  for (const item of value) {
    if (!item || typeof item.id !== "string" || !item.id || typeof item.title !== "string" || !Array.isArray(item.messages)) continue;
    const session = item as ChatSession;
    const previous = sessions.get(session.id);
    if (!previous) { sessions.set(session.id, session); continue; }
    const [older, newer] = previous.updatedAt > session.updatedAt ? [session, previous] : [previous, session];
    sessions.set(session.id, { ...older, ...newer, messages: newer.messages.length ? newer.messages : older.messages });
  }
  return [...sessions.values()];
}

export function uniqueWorkspaces<T extends { path: string; name: string }>(entries: T[]): T[] {
  const seen = new Set<string>();
  return entries.filter((entry) => {
    if (!entry || typeof entry.path !== "string" || typeof entry.name !== "string") return false;
    const key = workspaceKey(entry.path);
    if (!key || seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

/** First migration only: recover project membership from durable conversation metadata. */
export function recoverWorkspaceEntries(
  entries: Array<{ path: string; name: string }>,
  histories: Array<{ metadata?: Record<string, unknown> | null }>,
) {
  const recovered = histories.flatMap(({ metadata }) => {
    const path = metadata?.scope === "workspace" && typeof metadata.workspacePath === "string"
      ? metadata.workspacePath.trim() : "";
    return path ? [{ path, name: path.split(/[\\/]/).filter(Boolean).pop() || path }] : [];
  });
  return uniqueWorkspaces([...entries, ...recovered]);
}

