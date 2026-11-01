import { useEffect, useMemo, useRef, useState } from "react";
import { Button } from "rain-ui";
import { NoticeQueueRow } from "rain-ui";
import { useConfirmAction } from "rain-ui";
import { ComposerNoticeSlot } from "./ComposerNoticeDock";
import { AttachmentImage } from "./AttachmentImage";
import type { AgentImageAttachment, QueuedThreadArtifact } from "@/platform/chatApi";
import { classifyDropped, SourceIcon } from "./chatSources";

interface ChatQueuePanelProps {
  items: Array<{ id: string; content: string; images?: AgentImageAttachment[]; artifacts?: QueuedThreadArtifact[] }>;
  onPreview?: (image: { name: string; dataUrl: string }) => void;
  busy?: Record<string, "guide" | "edit" | "remove" | "reorder">;
  onGuide: (queueId: string) => void;
  onEdit: (item: ChatQueuePanelProps["items"][number]) => void;
  onRemove: (queueId: string) => Promise<void>;
  onReorder: (queueIds: string[]) => Promise<boolean>;
}

export function ChatQueuePanel({ items, busy = {}, onGuide, onEdit, onRemove, onReorder, onPreview }: ChatQueuePanelProps) {
  const { confirmAction, confirmationDialog } = useConfirmAction();
  const [orderedIds, setOrderedIds] = useState<string[]>([]);
  const [draggingId, setDraggingId] = useState("");
  const pointerDrag = useRef<{ id: string; pointerId: number; startX: number; startY: number; moved: boolean } | null>(null);
  const displayItems = useMemo(() => {
    if (!orderedIds.length) return items;
    const byId = new Map(items.map(item => [item.id, item]));
    return [...orderedIds.map(id => byId.get(id)).filter((item): item is typeof items[number] => Boolean(item)), ...items.filter(item => !orderedIds.includes(item.id))];
  }, [items, orderedIds]);
  const sourceOrder = items.map(item => item.id).join("\u0000");
  useEffect(() => setOrderedIds([]), [sourceOrder]);
  const moveItemRef = useRef<(fromId: string, toId: string, after: boolean) => Promise<void>>(async () => undefined);
  moveItemRef.current = async (fromId, toId, after) => {
    const index = displayItems.findIndex(item => item.id === fromId);
    const targetIndex = displayItems.findIndex(item => item.id === toId);
    if (index < 0 || targetIndex < 0 || index === targetIndex) return;
    const next = [...displayItems];
    const [moved] = next.splice(index, 1);
    const adjustedTarget = next.findIndex(item => item.id === toId);
    next.splice(adjustedTarget + (after ? 1 : 0), 0, moved);
    const ids = next.map(item => item.id);
    setOrderedIds(ids);
    if (!await onReorder(ids)) setOrderedIds([]);
  };
  useEffect(() => {
    const findTarget = (event: PointerEvent) => document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>(".ui-notice-queue-row[data-queue-id]");
    const handleMove = (event: PointerEvent) => {
      const drag = pointerDrag.current;
      if (!drag || drag.pointerId !== event.pointerId) return;
      if (Math.hypot(event.clientX - drag.startX, event.clientY - drag.startY) >= 4) drag.moved = true;
    };
    const handleUp = (event: PointerEvent) => {
      const drag = pointerDrag.current;
      if (!drag || drag.pointerId !== event.pointerId) return;
      const target = findTarget(event);
      const targetId = target?.dataset.queueId;
      if (drag.moved && target && targetId && targetId !== drag.id) {
        const box = target.getBoundingClientRect();
        void moveItemRef.current(drag.id, targetId, event.clientY > box.top + box.height / 2);
      }
      pointerDrag.current = null;
      setDraggingId("");
    };
    const handleCancel = (event: PointerEvent) => {
      if (pointerDrag.current?.pointerId !== event.pointerId) return;
      pointerDrag.current = null;
      setDraggingId("");
    };
    window.addEventListener("pointermove", handleMove);
    window.addEventListener("pointerup", handleUp);
    window.addEventListener("pointercancel", handleCancel);
    return () => {
      window.removeEventListener("pointermove", handleMove);
      window.removeEventListener("pointerup", handleUp);
      window.removeEventListener("pointercancel", handleCancel);
    };
  }, []);
  if (!items.length) return null;
  const single = displayItems.length === 1;
  // 单条：抬头就是「一条追加消息等待中：<正文>」，动作按钮挂在抬头上，下面不再重复一行；
  // 多条：抬头报一句「N 条追加消息等待中」+ 展开箭头，展开后才是 01/02 行列表。
  // 删到只剩一条时这里会自动退回单条形态。
  // 单条与多条渲染的是同一套行：单条就是「一行」，只是不提行号、不提拖拽柄；多条在
  // 抬头多一句「N 条追加消息等待中」+ 展开箭头。这样三个动作按钮的位置永远和展开后
  // 的行一致——不用再为单条另起一套卡头对齐。
  const headingSummary = single ? undefined : (
    <span className="ui-notice-queue-single-label">{displayItems.length} 条追加消息等待中</span>
  );
  return <ComposerNoticeSlot id="queue" kind="queue" label="" summary="" summaryContent={headingSummary} inline collapsible={!single} defaultExpanded>
    {confirmationDialog}
    <section className={`ui-notice-queue-list${single ? " is-single" : ""}`} aria-label="等待发送的追加消息" aria-live="polite">
      {displayItems.map((item, index) => {
        const action = busy[item.id];
        const imagePaths = new Set((item.images || []).map((image) => image.path?.toLowerCase()).filter(Boolean));
        const imageNames = new Set((item.images || []).map((image) => image.name.toLowerCase()));
        const otherArtifacts = (item.artifacts || []).filter((artifact) => !imagePaths.has(artifact.path.toLowerCase())
          && !(artifact.kind !== "directory" && imageNames.has(artifact.name.toLowerCase())));
        const attachmentCount = (item.images?.length || 0) + otherArtifacts.length;
        const attachmentPreview = attachmentCount ? <span className="ui-notice-queue-preview" aria-label={`${attachmentCount} 个附件`}>
          {item.images?.slice(0, 2).map((image, imageIndex) => <AttachmentImage key={`image:${image.path || image.name}:${imageIndex}`} name={image.name} imagePath={image.path || undefined} dataUrl={image.dataUrl} mediaType={image.mediaType} className="ui-notice-queue-thumbnail" onPreview={onPreview} />)}
          {otherArtifacts.slice(0, Math.max(0, 2 - (item.images?.length || 0))).map((artifact, artifactIndex) => {
            const kind = classifyDropped(artifact.name, artifact.kind === "directory") || (artifact.kind === "directory" ? "directory" : "document");
            return <span className="ui-notice-queue-file" key={`artifact:${artifact.path}:${artifactIndex}`} title={`${artifact.name}${artifact.size == null ? "" : ` · ${artifact.size} B`}`}>
              <SourceIcon name={artifact.name} kind={kind} /><span>{artifact.name}</span>
            </span>;
          })}
          {attachmentCount > 2 && <small>+{attachmentCount - 2}</small>}
        </span> : undefined;
        return <NoticeQueueRow key={item.id} queueId={item.id} content={item.content} busy={action}
          dragging={draggingId === item.id}
          index={index}
          single={single}
          preview={attachmentPreview}
          onGuide={() => onGuide(item.id)} onEdit={() => onEdit(item)}
          onMove={(direction) => {
            const target = displayItems[index + direction];
            if (target) void moveItemRef.current(item.id, target.id, direction > 0);
          }}
          onPointerDown={(event) => {
            if (event.button !== 0 || displayItems.length < 2 || action) return;
            event.preventDefault();
            pointerDrag.current = { id: item.id, pointerId: event.pointerId, startX: event.clientX, startY: event.clientY, moved: false };
            setDraggingId(item.id);
          }}
          onRemove={() => confirmAction({ title: "删除排队消息", message: `确认删除尚未发送的消息「${item.content.slice(0, 160)}${item.content.length > 160 ? "…" : ""}」？当前任务会继续执行。`, onConfirm: () => onRemove(item.id) })}
        />;
      })}
    </section>
  </ComposerNoticeSlot>;
}
