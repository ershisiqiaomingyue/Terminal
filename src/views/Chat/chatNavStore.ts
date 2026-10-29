import type { ChatSession } from "./types";
import { uniqueChatSessions, uniqueWorkspaces } from "./chatIdentity.ts";
import { cacheBrowserValue } from "../../platform/browserStorage.ts";

export const CHAT_NAV_EVENT = "android-tool-chat-nav";
export const CHAT_SESSIONS_STORAGE_KEY = "android-tool.chat.sessions.v1";
export const CHAT_WORKSPACES_STORAGE_KEY = "android-tool.chat.workspaces.v1";
const CHAT_NAV_SNAPSHOT_KEY = "android-tool.chat.last-nav.v1";
const DELETED_SESSIONS_KEY = "android-tool.chat.deleted-sessions.v1";

export function readDeletedChatSessionIds(): Set<string> {
  try {
    const value: unknown = JSON.parse(window.localStorage.getItem(DELETED_SESSIONS_KEY) || "[]");
    return new Set(Array.isArray(value) ? value.filter((id): id is string => typeof id === "string") : []);
  } catch { return new Set(); }
}

/** Retain deletion across reloads so old WebView caches cannot revive a chat. */
export function markChatSessionDeleted(id: string) {
  const deleted = readDeletedChatSessionIds();
  deleted.add(id);
  cacheBrowserValue(DELETED_SESSIONS_KEY, JSON.stringify([...deleted]));
  if (chatSessionsSnapshot) chatSessionsSnapshot = chatSessionsSnapshot.filter(session => session.id !== id);
}

export interface WorkspaceEntry {
  path: string;
  name: string;
}

export interface ChatNavSnapshot {
  sessionId: string;
  scope: "workspace" | "standalone";
  workspacePath: string;
}

export interface ChatNavDetail {
  type: string;
  id?: string;
  path?: string;
  name?: string;
  draft?: string;
  /** 重命名会话时的新标题。 */
  title?: string;
  /** 置顶/取消置顶的目标状态。 */
  next?: boolean;
}

const EMPTY_NAV_SNAPSHOT: ChatNavSnapshot = { sessionId: "", scope: "standalone", workspacePath: "" };

function readStoredChatNavSnapshot(): ChatNavSnapshot {
  if (typeof window === "undefined") return EMPTY_NAV_SNAPSHOT;
  try {
    const value = JSON.parse(window.localStorage.getItem(CHAT_NAV_SNAPSHOT_KEY) || "{}") as Partial<ChatNavSnapshot>;
    return {
      sessionId: typeof value.sessionId === "string" ? value.sessionId : "",
      scope: value.scope === "workspace" ? "workspace" : "standalone",
      workspacePath: typeof value.workspacePath === "string" ? value.workspacePath : "",
    };
  } catch {
    return EMPTY_NAV_SNAPSHOT;
  }
}

let chatNavSnapshot = readStoredChatNavSnapshot();
let chatSessionsSnapshot: ChatSession[] | undefined;
let pendingDetails: ChatNavDetail[] = [];

export function readChatNavSnapshot() {
  return chatNavSnapshot;
}

export function writeChatNavSnapshot(snapshot: ChatNavSnapshot) {
  chatNavSnapshot = snapshot;
  if (typeof window === "undefined") return;
  cacheBrowserValue(CHAT_NAV_SNAPSHOT_KEY, JSON.stringify(snapshot));
}

export function readChatSessions(): ChatSession[] {
  const deleted = readDeletedChatSessionIds();
  if (chatSessionsSnapshot) return chatSessionsSnapshot.filter(session => !deleted.has(session.id));
  if (typeof window === "undefined") return [];
  try {
    const raw = window.localStorage.getItem(CHAT_SESSIONS_STORAGE_KEY);
    chatSessionsSnapshot = uniqueChatSessions(raw ? JSON.parse(raw) : []).filter(session => !deleted.has(session.id));
    return chatSessionsSnapshot;
  } catch {
    return [];
  }
}

/** Navigation follows committed UI state; disk persistence may be deferred while streaming. */
export function publishChatSessions(sessions: ChatSession[]) {
  chatSessionsSnapshot = sessions;
  if (typeof window !== "undefined") window.dispatchEvent(new Event(CHAT_NAV_EVENT));
}

export function readWorkspaces(): WorkspaceEntry[] {
  if (typeof window === "undefined") return [];
  try {
    const raw = window.localStorage.getItem(CHAT_WORKSPACES_STORAGE_KEY);
    const entries: unknown = raw ? JSON.parse(raw) : [];
    return Array.isArray(entries) ? uniqueWorkspaces(entries) : [];
  } catch {
    return [];
  }
}

/** Queues commands until the lazily mounted Chat view installs its listener. */
export function dispatchChatNav(detail: ChatNavDetail) {
  pendingDetails.push(detail);
  window.dispatchEvent(new CustomEvent<ChatNavDetail>(CHAT_NAV_EVENT, { detail }));
}

export function acknowledgeChatNavDetail(detail: ChatNavDetail) {
  const index = pendingDetails.indexOf(detail);
  if (index >= 0) pendingDetails.splice(index, 1);
}

export function takePendingChatNavDetails() {
  const pending = pendingDetails;
  pendingDetails = [];
  return pending;
}
