export type AssistantTextEvent = { type?: string; text?: unknown; sequence?: unknown; eventId?: unknown; history_result?: unknown; paragraph_start?: unknown; origin?: unknown };

export function appendAssistantStreamText(current: string, next: string) {
  return `${current}${next}`;
}

export function mergeSnapshotText(events: AssistantTextEvent[]) {
  const segments = new Map<unknown, { text: string; paragraphStart: boolean }>();
  [...events]
    .sort((left, right) => Number(left.sequence ?? 0) - Number(right.sequence ?? 0))
    .filter((event) => event.type === "snapshot_text" && typeof event.text === "string")
    .forEach((event) => {
      // A durable item is a complete segment, not a provider delta. Only
      // revisions of the same item replace text; equal neighbouring text is real.
      const id = event.eventId ?? event;
      segments.set(id, { text: String(event.text), paragraphStart: event.paragraph_start === true || segments.get(id)?.paragraphStart === true });
    });
  let text = "";
  for (const segment of segments.values()) {
    if (segment.paragraphStart && text && segment.text) text += "\n\n";
    text += segment.text;
  }
  return text;
}

export function isAssistantProcessBoundary(event: AssistantTextEvent) {
  return (event.type === "reasoning_delta" && event.origin !== "runtime_fallback" && typeof event.text === "string" && Boolean(event.text.trim())) || event.type === "command_output" || event.type?.startsWith("tool_call_") === true;
}

export function projectAssistantSnapshotText(events: AssistantTextEvent[], retainPartialResult = false) {
  const ordered = [...events].sort((left, right) => Number(left.sequence ?? 0) - Number(right.sequence ?? 0));
  const snapshots = ordered.filter((event) => event.type === "snapshot_text" && typeof event.text === "string");
  let lastToolIndex = -1;
  for (let index = ordered.length - 1; index >= 0; index -= 1) {
    if (!isAssistantProcessBoundary(ordered[index])) continue;
    lastToolIndex = index;
    break;
  }
  const hasHistoryClassification = snapshots.some((event) => typeof event.history_result === "boolean");
  let resultEvents = hasHistoryClassification
    ? snapshots.filter((event) => event.history_result === true)
    : lastToolIndex < 0
    ? snapshots
    : ordered.slice(lastToolIndex + 1).filter((event) => event.type === "snapshot_text" && typeof event.text === "string");

  // A terminal turn can stop at a tool boundary without a final answer.
  // Keep the partial text visible when its execution process collapses.
  if (retainPartialResult && !hasHistoryClassification && resultEvents.length === 0) resultEvents = snapshots;

  const resultSet = new Set(resultEvents);
  const executionEvents = snapshots.filter((event) => !resultSet.has(event));
  return {
    executionEvents,
    resultEvents,
    resultText: mergeSnapshotText(resultEvents),
  };
}
