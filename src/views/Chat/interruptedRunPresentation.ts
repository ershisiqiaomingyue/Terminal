import type { AgentRunSummary } from "@/platform/chatApi";
import type { ChatMessage } from "./types";

/** Only the last conversation turn can offer resume; later messages supersede old prompts. */
export function resumableInterruptedTurnId(messages: ChatMessage[], runs: AgentRunSummary[] = []) {
  const latest = messages.at(-1);
  if (!latest?.turnId) return undefined;
  if (messages.some(message => message.pending)) return undefined;
  return runs.some(run => run.turnId === latest.turnId && run.status === "interrupted" && run.finishedAt == null)
    ? latest.turnId : undefined;
}
