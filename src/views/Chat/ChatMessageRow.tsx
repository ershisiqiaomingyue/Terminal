import React, { useMemo, useState } from "react";
import { Check, CircleAlert, Copy, Download, Pencil } from "lucide-react";
import { type AgentEventPayload, type AgentRunSummary } from "@/platform/chatApi";
import { Button } from "rain-ui";
import { Spinner } from "rain-ui";
import { MarkdownMessage } from "./MarkdownMessage";
import { AgentActivityTimeline } from "./AgentActivityTimeline";
import { ToolActivityStatus } from "./ToolActivityStatus";
import { hasTimelineTools } from "./activityState";
import { MessageFileChanges } from "./ChatFileChanges";
import type { FilePreviewOrigin } from "./filePreviewDiff";
import { RECOVERY_INTERRUPTED_MESSAGE } from "./historyRecovery";
import { prefetchHistoryProcess, useHistoryProcess } from "./useHistoryProcess";
import { historyProcessEventsForRange, historyProcessTiming } from "./historyProcess";
import { pendingMessageState } from "./threadSnapshotRuntime";
import { type ThinkingCopy } from "@/platform/thinkingCopy";
import { timelineStateKey } from "./timelineState";
import { visibleMessageAttachments } from "./chatAttachments";
import { toolEnvironmentLabel } from "./toolIdentity";
import type { ChatMessage } from "./types";
import { isTauri } from "@/platform/runtime";
import { assistantTerminalNotice } from "./chatErrors";
import { shouldShowEmptyReplyNotice } from "./emptyReplyNotice";
import {
  messageCopyText,
  localizeToolIds,
  assistantResultText,
  toolStepTitle,
  AnalysisReport,
  formatHistoryTime,
} from "./chatMessagePresentation";
import { messageAttachmentKey, SourceIcon } from "./chatSources";
import { AttachmentImage } from "./AttachmentImage";
// 分叉沿用侧栏菜单里那颗分支图标，两处指的是同一件事，就不该长得不一样。
import { BranchIcon } from "@/components/Sidebar/navIcons";

export const EMPTY_AGENT_EVENTS: AgentEventPayload[] = [];

export type ChatMessageRowProps = {
  thinkingCopy: ThinkingCopy;
  sessionId: string;
  item: ChatMessage;
  index: number;
  showPersistentMessageActions: boolean;
  interruptedRun?: AgentRunSummary;
  renderedActivity: string;
  timelineExpanded?: boolean;
  fileChanges?: AgentEventPayload;
  workspacePath: string;
  copied: boolean;
  ownsTurnActivity: boolean;
  turnPending: boolean;
  turnStartedAt?: number;
  turnFinishedAt?: number;
  turnEvents?: AgentEventPayload[];
  onTimelineExpandedChange: (messageId: string, expanded: boolean) => void;
  onLayoutChange: (anchor?: HTMLElement) => void;
  onPreviewAttachment: (attachment: { name: string; dataUrl: string }) => void;
  onPathMenu: (path: string, x: number, y: number) => void;
  onOpenAttachment: (path: string, origin?: FilePreviewOrigin) => void;
  onCopy: (messageId: string, content: string) => void;
  onEdit: (message: ChatMessage) => void;
  /** 从这条对话分叉出一个副本（对话级操作，行上给个入口省得回侧栏）。 */
  onFork?: () => void;
};

export const ChatMessageRow = React.memo(function ChatMessageRow({
  thinkingCopy,
  sessionId,
  item,
  index,
  showPersistentMessageActions,
  interruptedRun,
  renderedActivity,
  timelineExpanded,
  fileChanges,
  workspacePath,
  copied,
  ownsTurnActivity,
  turnPending,
  turnStartedAt,
  turnFinishedAt,
  turnEvents,
  onTimelineExpandedChange,
  onLayoutChange,
  onPreviewAttachment,
  onPathMenu,
  onOpenAttachment,
  onCopy,
  onEdit,
  onFork,
}: ChatMessageRowProps) {
  const [liveStatusHost, setLiveStatusHost] = useState<HTMLDivElement | null>(null);
  const processOpen = timelineExpanded ?? turnPending;
  const historical = item.role === "assistant" && !turnPending && Boolean(item.turnId) && isTauri();
  const bodyEvents = item.events || EMPTY_AGENT_EVENTS;
  const historyDetailsAvailable = ownsTurnActivity && historical && bodyEvents.some((event) => event.has_execution_details === true);
  // Later segments subscribe to the owner's lazy history cache too. They have
  // no terminal marker of their own, but still own their range of process items.
  const pagedHistory = historical && (turnEvents || bodyEvents).some((event) => event.has_execution_details === true);
  const linkedDetails = ownsTurnActivity && (item.turnSegmentCount || 0) > 1;
  const loadHistoryDetails = historyDetailsAvailable;
  const process = useHistoryProcess(sessionId, item.turnId, processOpen, pagedHistory, loadHistoryDetails);
  const processEvents = process.events
    ? historyProcessEventsForRange(process.events, item.processStart, item.processEnd)
    : null;
  // History pages contain execution details and may not include the final reply.
  // Keep the message result independent of disclosure loading and pagination.
  // Complete live transcripts must not be replaced by a partial history page.
  const events = pagedHistory && processEvents ? processEvents : bodyEvents;
  const copyText = messageCopyText(item);
  const answer = interruptedRun && item.content === RECOVERY_INTERRUPTED_MESSAGE
    ? ""
    : localizeToolIds(assistantResultText(item));
  const timelineOwnsText = !pagedHistory && bodyEvents.some((event) =>
    ["snapshot_text", "text_delta"].includes(event.type || "") && typeof event.text === "string" && event.text.trim() && event.origin !== "runtime_fallback");
  const toolsVisible = hasTimelineTools(events) || timelineOwnsText;
  const terminalNotice = assistantTerminalNotice(item, events);
  const body = terminalNotice?.contentIsDiagnostic || (timelineOwnsText && processOpen) ? "" : answer;
  const currentActivityEvents = turnEvents || item.events || events;
  const currentState = useMemo<ReturnType<typeof pendingMessageState>>(() => item.pending
    ? pendingMessageState(currentActivityEvents, renderedActivity, item.turnId || item.id, thinkingCopy)
    : { phase: "waiting", activity: { text: "" } }, [item.pending, currentActivityEvents, renderedActivity, item.turnId, item.id, thinkingCopy]);
  const currentActivity = currentState.activity;
  const activityText = currentActivity.toolName
    ? [`正在${toolStepTitle(currentActivity.toolName)}`, toolEnvironmentLabel(currentActivity.toolName), currentActivity.detail].filter(Boolean).join(" · ")
    : localizeToolIds(currentActivity.text);
  const showActivity = Boolean(item.pending) && currentState.phase !== "writing";
  const resolvedFileChanges = fileChanges;
  const reportEvent = bodyEvents.find((event) => event.type === "final_report") || [...events].reverse().find((event) => event.type === "completed" && Boolean(event.report && typeof event.report === "object"));
  const attachments = visibleMessageAttachments(item.attachments, workspacePath);
  const hasResult = Boolean(body || attachments?.length || reportEvent);
  const processTiming = historyProcessTiming(process.events || events);
  const effectiveStartedAt = processTiming.startedAt || turnStartedAt || item.startedAt;
  const effectiveFinishedAt = processTiming.finishedAt || turnFinishedAt || item.finishedAt;
  const historyControls = loadHistoryDetails && historical && (process.loading || process.error || process.hasMore)
    ? <div className="history-process-controls">
      {process.loading && <span role="status"><Spinner size={13} className="is-inline" />正在读取执行过程…</span>}
      {process.error && <span role="alert">执行过程读取失败。</span>}
      {(process.hasMore || process.error) && <Button type="button" variant="secondary" icon={<Download size={13} />} disabled={process.loading} onClick={() => void process.load()}>{process.error ? "重试" : "加载更多执行过程"}</Button>}
    </div> : null;

  return <article data-index={index} data-message-id={item.id} className={`message-row ${item.role}${item.pending ? " is-streaming" : ""}${item.error ? " is-error" : ""}`}>
    <div className="message-body">
      {item.role === "assistant" && <AgentActivityTimeline liveStatusHost={liveStatusHost} liveStatusLabel={activityText} events={events} startedAt={ownsTurnActivity ? effectiveStartedAt : undefined} finishedAt={ownsTurnActivity ? effectiveFinishedAt : undefined} pending={ownsTurnActivity ? turnPending : Boolean(item.pending)} failed={item.error} paused={Boolean(interruptedRun)} expanded={processOpen} includeResultText={timelineOwnsText} historyAvailable={historyDetailsAvailable} showSummary={ownsTurnActivity} linkedDetails={linkedDetails} onDetailsIntent={historyDetailsAvailable && item.turnId ? () => { void prefetchHistoryProcess(sessionId, item.turnId!); } : undefined} onExpandedChange={(expanded) => onTimelineExpandedChange(timelineStateKey(item), expanded)} onLayoutChange={onLayoutChange} onPathMenu={onPathMenu} />}
      {processOpen && historyControls}
      {item.role === "assistant" && !hasResult
        ? interruptedRun || terminalNotice
          ? null
          : shouldShowEmptyReplyNotice({ pending: Boolean(item.pending), turnPending, turnSegmentIndex: item.turnSegmentIndex, turnSegmentCount: item.turnSegmentCount, hasAnswer: Boolean(answer), hasTools: toolsVisible })
            ? <div className="message-content"><MarkdownMessage content="模型没有返回内容。历史上下文仍在，这次请求停在等待模型响应，没有读取文件。请再发一次。" /></div>
            : null
        : !hasResult
          ? null
          : <div className="message-content">
            {attachments?.length ? <div className="message-attachment-list">{attachments.map((attachment, attachmentIndex) => attachment.kind === "image"
              ? <span key={messageAttachmentKey(attachment, attachmentIndex)} className="message-image-attachment">
                <AttachmentImage {...attachment} onPreview={onPreviewAttachment} /><span>{attachment.name}</span></span>
              : <span key={messageAttachmentKey(attachment, attachmentIndex)} className="message-file-attachment" title={attachment.path}><SourceIcon name={attachment.name} kind={attachment.kind} /><span>{attachment.name}</span></span>)}</div> : null}
            <AnalysisReport event={reportEvent} />
            {body ? <MarkdownMessage content={body} streaming={item.pending} onPathMenu={onPathMenu} /> : null}
          </div>}
      {terminalNotice && <div className="message-terminal-diagnostic" role={terminalNotice.failed ? "alert" : "status"}><CircleAlert size={16} aria-hidden="true" /><span>{terminalNotice.text}</span></div>}
      {item.role === "assistant" && !turnPending && <MessageFileChanges event={resolvedFileChanges} onOpen={onOpenAttachment} workspacePath={workspacePath} onLayoutChange={() => onLayoutChange()} />}
      {(item.role === "assistant" || (!item.pending && copyText)) && <div className="message-footer">
      {item.role === "assistant" && item.pending && (
        <div className={`message-processing-status${showActivity ? "" : " is-hidden"}`} data-phase={currentState.phase} role={showActivity ? "status" : undefined} aria-live={showActivity ? "polite" : "off"} aria-hidden={!showActivity}>
          <ToolActivityStatus state={currentState.phase === "permission" ? "pending" : "running"} />
          <div className="message-live-process" ref={setLiveStatusHost} />
          <span className="message-live-fallback">{activityText}</span>
        </div>
      )}
      {!item.pending && copyText && <div className={`message-actions${showPersistentMessageActions ? " persistent" : ""}`}>
        <Button type="button" variant="bare" title={copied ? "已复制" : "复制消息"} onClick={() => onCopy(item.id, copyText)}>
          {copied ? <Check size={13} /> : <Copy size={13} />}
        </Button>
        {item.role === "user" ? (
          /* 用户自己发出去的消息只留「复制 + 编辑」：分叉是从某轮回答往后开一支，时间
             戳对刚敲出去的话也没有信息量，这两样都归助手那侧。 */
          <Button type="button" variant="bare" title="编辑并从此处继续" aria-label="编辑并从此处继续" onClick={() => onEdit(item)}>
            <Pencil size={13} />
          </Button>
        ) : (
          <>
            {onFork && <Button type="button" variant="bare" title="分叉对话" aria-label="分叉对话" onClick={onFork}>
              <BranchIcon size={14} />
            </Button>}
            <time className="message-actions-time">{formatHistoryTime(item.createdAt)}</time>
          </>
        )}
      </div>}
      </div>}
    </div>
  </article>;
});
