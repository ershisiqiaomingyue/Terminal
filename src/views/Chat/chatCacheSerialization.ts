import type { ChatSession } from "./types";

/** Serialize the optional UI cache in short batches. Native SQLite remains
 * authoritative; streaming/navigation can cancel obsolete work between batches. */
export async function serializeChatCache(
  sessions: ChatSession[],
  cancelled: () => boolean,
  yieldToUi: () => Promise<void> = () => new Promise(resolve => setTimeout(resolve, 0)),
  now: () => number = () => performance.now(),
): Promise<string | null> {
  const parts: string[] = ["["];
  let deadline = now() + 4;
  for (let index = 0; index < sessions.length; index++) {
    if (cancelled()) return null;
    const { messages, ...metadata } = sessions[index];
    if (index) parts.push(",");
    parts.push(JSON.stringify(metadata).slice(0, -1), ',"messages":[');
    for (let messageIndex = 0; messageIndex < messages.length; messageIndex++) {
      if (cancelled()) return null;
      if (messageIndex) parts.push(",");
      parts.push(JSON.stringify(messages[messageIndex]));
      if (now() >= deadline) {
        await yieldToUi();
        deadline = now() + 4;
      }
    }
    parts.push("]}");
    if (now() >= deadline) {
      await yieldToUi();
      deadline = now() + 4;
    }
  }
  parts.push("]");
  return cancelled() ? null : parts.join("");
}
