import { memo, useId, useRef, type KeyboardEvent, type RefObject, type UIEvent, type WheelEvent } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { Card } from "@/components/ui/Card";
import { reasoningPreviewText } from "./reasoningPresentation";
import { MarkdownMessage } from "./MarkdownMessage";
import { useStreamingScrollFollow } from "./useStreamingScrollFollow";

/**
 * 展开后的思考正文：只是一个受控的滚动区，跟随逻辑全在父组件里。
 *
 * 之所以把 hook 提到父组件：折叠时这段正文会整体卸载，如果状态跟着它走，
 * 「读者已经接管」的记忆会随卸载一起消失；更要紧的是「回到最新」按钮画在
 * 滚动区外面，它得知道当前是否还在跟随。
 */
const ReasoningDetail = memo(function ReasoningDetail({ detailId, content, active, detailRef, following, onScroll, onWheel, onKeyDown }: {
  detailId: string;
  content: string;
  active: boolean;
  detailRef: RefObject<HTMLDivElement>;
  following: boolean;
  onScroll: (event: UIEvent<HTMLElement>) => void;
  onWheel: (event: WheelEvent<HTMLElement>) => void;
  onKeyDown: (event: KeyboardEvent<HTMLElement>) => void;
}) {
  return <div id={detailId} ref={detailRef} className="agent-reasoning-detail" role="region" aria-label="思考详情"
    tabIndex={0} data-following={following ? "true" : "false"}
    onScroll={onScroll} onWheel={onWheel} onKeyDown={onKeyDown}>
    <MarkdownMessage content={content} streaming={active} />
  </div>;
});

/**
 * Each reasoning entry owns its disclosure independently of the turn and tools.
 *
 * The expanded body sits on the library Card (tone="muted", the second tone in
 * the gallery) so the thought stream reads as its own inset panel instead of
 * floating text under the summary row.
 *
 * While the model is still thinking the card follows its own tail, so the newest
 * sentence stays visible without the reader chasing it. Scrolling up (wheel,
 * scrollbar, keyboard) hands control back: the card freezes where the reader put
 * it and only resumes once they scroll back down to the bottom — or press the
 * floating 回到最新 chip, which is the only way back from the keyboard.
 */
export const ReasoningPreview = memo(function ReasoningPreview({ content, active, expanded, onToggle, label }: {
  content: string;
  active: boolean;
  expanded: boolean;
  onToggle: (anchor: HTMLElement) => void;
  label?: string;
}) {
  const detailId = useId();
  const detailRef = useRef<HTMLDivElement>(null);
  // 只有还在生成时才跟随：回看一段已经结束的思考，读者是要从头读的，
  // 一展开就甩到末尾会把最想看的开头藏起来。
  // `expanded` 作为 armed 传入：折叠会卸载滚动区，重新展开要从干净状态起算。
  const follow = useStreamingScrollFollow(content, detailRef, active, expanded);
  const Chevron = expanded ? ChevronDown : ChevronRight;
  const caughtUp = !active || follow.following;
  return <div className="agent-reasoning-entry">
    <button type="button" className={`agent-reasoning-preview${active ? " is-active" : ""}`}
      aria-label={expanded ? "收起思考过程" : "展开思考过程"} aria-expanded={expanded} aria-controls={detailId} onClick={(event) => onToggle(event.currentTarget)}>
      <span>{reasoningPreviewText(content, active, label)}</span><Chevron size={13} aria-hidden="true" />
    </button>
    {expanded && <Card tone="muted" className="agent-reasoning-card">
      <ReasoningDetail detailId={detailId} content={content} active={active}
        detailRef={detailRef} following={follow.following} onScroll={follow.onScroll}
        onWheel={follow.onWheel} onKeyDown={follow.onKeyDown} />
      {!caughtUp && <button type="button" className="agent-reasoning-jump" onClick={follow.resume}>
        回到最新
      </button>}
    </Card>}
  </div>;
});
