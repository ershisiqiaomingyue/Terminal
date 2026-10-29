import { useEffect, useState } from "react";
import { NoticePermissionContent, NoticeQuestionContent } from "@/components/ui/NoticeContent";
import { ComposerNoticeSlot } from "./ComposerNoticeDock";
interface ChatPermissionBarProps {
  requestKey: string;
  slotId?: string;
  toolLabel: string;
  input?: Record<string, unknown>;
  environmentLabel?: string;
  question?: { question: string; options: string[] };
  instruction: string;
  busy?: boolean;
  /** N2：等待到期时刻（epoch 毫秒）——超时后本次申请被自动拒绝（合成 Deny）。 */
  expiresAtMs?: number | null;
  onInstructionChange: (value: string) => void;
  onResolve: (decision: "once" | "session" | "deny" | "other") => void;
}

/** N2：权限/询问等待的剩余时间倒计时，每秒递减；无截止数据时不显示。 */
function PermissionCountdown({ expiresAtMs }: { expiresAtMs?: number | null }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (expiresAtMs == null) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [expiresAtMs]);
  if (expiresAtMs == null) return null;
  const remaining = Math.max(0, expiresAtMs - now);
  const minutes = Math.floor(remaining / 60_000);
  const seconds = Math.floor((remaining % 60_000) / 1_000);
  return <div style={{ fontSize: 12, opacity: 0.75, marginTop: 4 }} title="超时后本次申请将被自动拒绝（合成 Deny），需要时可重新发起">
    剩余 {minutes}:{String(seconds).padStart(2, "0")}
  </div>;
}

export function ChatPermissionBar({ requestKey, slotId, toolLabel, input, environmentLabel, question, instruction, busy = false, expiresAtMs, onInstructionChange, onResolve }: ChatPermissionBarProps) {
  return <ComposerNoticeSlot id={slotId || (question ? "question" : "permission")} kind={question ? "question" : "permission"} label={question ? "AI 询问" : "权限申请"} summary={question ? `需要回答 · ${question.question}` : `需要批准 · ${toolLabel}`} state="pending" attention autoOpenKey={requestKey}>
    <PermissionCountdown expiresAtMs={expiresAtMs} />
    {question ? <NoticeQuestionContent options={question.options} value={instruction} busy={busy} onChange={onInstructionChange} onAnswer={() => onResolve("other")} onSkip={() => onResolve("deny")} />
      : <NoticePermissionContent command={input ? typeof input.command === "string" ? input.command : JSON.stringify(input, null, 2) : undefined}
          environment={environmentLabel} instruction={instruction} busy={busy} onInstructionChange={onInstructionChange} onResolve={onResolve} />}
  </ComposerNoticeSlot>;
}
