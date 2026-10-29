import { CompactionProgress } from "@/components/ui";
import type { ContextCompaction } from "./contextCompaction";

/**
 * Context-compaction line inside the turn's execution timeline.
 *
 * The visual is the library `CompactionProgress`; this file only turns the
 * product's compaction record into the status plus the one sentence that
 * explains it. `variant="row"` keeps it on the same line grid as the tool
 * groups instead of floating as its own framed card.
 */
export function ChatCompactionProgress({ compaction }: { compaction: ContextCompaction | null }) {
  if (!compaction) return null;

  const running = compaction.status === "running";
  const complete = compaction.status === "completed";
  const progress = complete ? 100 : compaction.progress;

  const detail = complete && compaction.before != null && compaction.after != null
    ? `${compaction.before.toLocaleString()} → ${compaction.after.toLocaleString()} Token`
    : running ? [
      compaction.processedMessages != null ? `已整理 ${compaction.processedMessages.toLocaleString()} 条消息` : "",
      compaction.summaryCharacters ? `摘要已生成 ${compaction.summaryCharacters.toLocaleString()} 字符` : progress == null ? "等待摘要响应" : "",
    ].filter(Boolean).join(" · ") : "";

  return (
    <CompactionProgress
      className="chat-compaction-progress"
      variant="row"
      status={compaction.status}
      progress={progress}
      detail={detail || undefined}
    />
  );
}
