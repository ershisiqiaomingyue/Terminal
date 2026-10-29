/**
 * Per-session composer drafts.
 *
 * The composer draft belongs to the conversation it was typed in: switching
 * conversations must not carry it over, and coming back must restore it.
 *
 * Drafts deliberately do NOT live on `ChatSession`. A keystroke would then
 * mutate the session state, and the two persistence effects that watch it
 * (the local chat cache and the native history write) would serialize every
 * conversation and write the whole history record on each typing pause. This
 * store keeps the same per-session behaviour with one small debounced write.
 */

const DRAFTS_KEY = "android-tool.chat.drafts.v1";
const DRAFT_FLUSH_DELAY_MS = 400;

function readStoredDrafts(): Record<string, string> {
  if (typeof window === "undefined") return {};
  try {
    const value: unknown = JSON.parse(window.localStorage.getItem(DRAFTS_KEY) || "{}");
    if (!value || typeof value !== "object") return {};
    return Object.fromEntries(
      Object.entries(value as Record<string, unknown>)
        .filter((entry): entry is [string, string] => typeof entry[1] === "string" && entry[1].trim().length > 0),
    );
  } catch {
    return {};
  }
}

let drafts: Record<string, string> = readStoredDrafts();
let flushTimer: number | null = null;

function flushDrafts() {
  flushTimer = null;
  // Optional cache: a full quota must not take the composer down with it.
  try {
    window.localStorage.setItem(DRAFTS_KEY, JSON.stringify(drafts));
  } catch { /* Drafts are best effort. */ }
}

function scheduleFlush() {
  if (typeof window === "undefined" || flushTimer != null) return;
  flushTimer = window.setTimeout(flushDrafts, DRAFT_FLUSH_DELAY_MS);
}

/** The draft stored for a session, or "" when it has none. */
export function readSessionDraft(sessionId: string): string {
  if (!sessionId) return "";
  return drafts[sessionId] || "";
}

/** Store a session's draft. An empty draft drops the entry entirely. */
export function writeSessionDraft(sessionId: string, draft: string) {
  if (!sessionId) return;
  if (!draft.trim()) {
    if (!(sessionId in drafts)) return;
    delete drafts[sessionId];
  } else {
    if (drafts[sessionId] === draft) return;
    drafts[sessionId] = draft;
  }
  scheduleFlush();
}

/** Drop a deleted conversation's draft so the store cannot grow forever. */
export function pruneSessionDraft(sessionId: string) {
  if (!sessionId || !(sessionId in drafts)) return;
  delete drafts[sessionId];
  scheduleFlush();
}
