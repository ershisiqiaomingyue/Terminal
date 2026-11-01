import { createPortal } from "react-dom";
import { Checkbox } from "rain-ui";
import { useEffect, useRef, useState, type Ref } from "react";
import { BackgroundTaskNotices } from "./BackgroundTasks";
import { ChatPermissionBar } from "./ChatPermissionBar";
import { ChatQueuePanel } from "./ChatQueuePanel";
import { ComposerChangedFiles } from "./ComposerChangedFiles";
import { ComposerTaskList } from "./ChatTaskPanels";
import { ComposerNoticePreview } from "./ComposerNoticeDock";
import { Button } from "rain-ui";
import type { FileChangeEntry } from "rain-ui";
import { MessageFileChanges } from "./ChatFileChanges";
import { FileDiffView } from "rain-ui";
import { parseFileDiff } from "rain-ui";
import { FilePreviewPanel } from "rain-ui";

/** Debug injects fixture data into production adapters, including background tasks. */
export type ChatDebugKey =
  | "permission"
  | "question"
  | "plan"
  | "background"
  | "queue"
  | "files"
  | "replyFiles";

export const CHAT_DEBUG_ITEMS: readonly { key: ChatDebugKey; label: string }[] = [
  { key: "permission", label: "权限申请" },
  { key: "question", label: "AI 询问" },
  { key: "plan", label: "任务编排" },
  { key: "background", label: "后台任务" },
  { key: "queue", label: "追加消息" },
  { key: "files", label: "文件改动" },
  { key: "replyFiles", label: "回复后文件改动（14）" },
];

const NOOP = () => undefined;

const REPLY_FILES: FileChangeEntry[] = [
  { path: "src/components/ui/NoticeContent.tsx", status: "created", addedLines: 80, removedLines: 0 },
  { path: "src/components/ui/ProgressRing.tsx", status: "modified", addedLines: 41, removedLines: 30 },
  { path: "src/components/ui/notice-panel.css", status: "modified", addedLines: 30, removedLines: 14 },
  { path: "src/views/Chat/ContextUsageIndicator.tsx", status: "modified", addedLines: 50, removedLines: 80 },
  { path: "src/views/Chat/legacy-usage.ts", status: "deleted", addedLines: 0, removedLines: 30 },
];

/** Reply preview lives in the message area; its actions only change fixture state. */
export function ChatDebugFileChanges({ workspacePath }: { workspacePath?: string }) {
  const root = useRef<HTMLElement>(null);
  const [undone, setUndone] = useState(false);
  const [preview, setPreview] = useState("");
  useEffect(() => { root.current?.scrollIntoView({ block: "center" }); }, []);
  return <section ref={root} className="chat-debug-reply" aria-label="AI 回复后文件改动调试">
    {undone ? <Button variant="ghost" onClick={() => setUndone(false)}>恢复演示文件</Button> : <MessageFileChanges
      event={{ type: "file_changes", entries: REPLY_FILES }} workspacePath={workspacePath} onOpen={setPreview}
      onUndo={() => { setUndone(true); setPreview(""); }} />}
    {preview && <FilePreviewPanel path={preview} mode="diff" onClose={() => setPreview("")} footerNote="调试示例">
      <FileDiffView {...parseFileDiff(`--- a/${preview}\n+++ b/${preview}\n@@ -1,2 +1,2 @@\n- // Previous component rendering\n+ // Shared component rendering`)} />
    </FilePreviewPanel>}
  </section>;
}

export function ChatDebugPicker({ on, onToggle, position, portalRoot, menuRef }: {
  on: Record<string, boolean>;
  onToggle: (key: string) => void;
  position: { top: number; right: number };
  portalRoot: HTMLElement;
  menuRef?: Ref<HTMLDivElement>;
}) {
  return createPortal(
    <div
      ref={menuRef}
      className="chat-debug-picker"
      role="group"
      aria-label="选择要预览的通知"
      style={{ top: `${position.top}px`, right: `${position.right}px` }}
    >
      <span className="chat-debug-picker-title">组件调试</span>
      {CHAT_DEBUG_ITEMS.map((item) => (
        <Checkbox
          key={item.key}
          checked={Boolean(on[item.key])}
          label={item.label}
          onChange={() => onToggle(item.key)}
        />
      ))}
    </div>,
    portalRoot,
  );
}

export function ChatDebugNotices({ on, onEditQueuedMessage }: { on: Record<string, boolean>; onEditQueuedMessage?: (content: string) => void }) {
  const [permissionDraft, setPermissionDraft] = useState("");
  const [questionDraft, setQuestionDraft] = useState("");
  const [startedAt] = useState(() => Date.now() - 84000);
  const [backgroundStatus, setBackgroundStatus] = useState("running");
  const [queue, setQueue] = useState([
    { id: "q1", content: "顺便把 lint 也跑一遍" },
    { id: "q2", content: "然后把改动提交到 feature 分支" },
  ]);
  return (
    <ComposerNoticePreview>
      {on.permission && (
        <ChatPermissionBar
          requestKey="debug-permission"
          toolLabel="android_adb_action"
          input={{ command: "adb shell pm uninstall com.example.debug" }}
          environmentLabel="Pixel 8 Pro"
          instruction={permissionDraft}
          busy={false}
          onInstructionChange={setPermissionDraft}
          onResolve={NOOP}
        />
      )}

      {/* AI 询问的真实形态就是权限条带 question —— 同一个组件。 */}
      {on.question && (
        <ChatPermissionBar
          requestKey="debug-question"
          slotId="question"
          toolLabel="android_adb_action"
          environmentLabel="Pixel 8 Pro"
          question={{
            question: "要把产物推送到哪台设备？",
            options: ["Pixel 8 Pro", "Redmi K60", "模拟器"],
          }}
          instruction={questionDraft}
          busy={false}
          onInstructionChange={setQuestionDraft}
          onResolve={NOOP}
        />
      )}

      {on.plan && (
        <ComposerTaskList
          active
          goal="把 SystemUI 的改动编译并推送到 Pixel 8 Pro"
          startedAt={startedAt}
          onOpenPlanner={NOOP}
          onCollapse={NOOP}
          onDelete={NOOP}
          steps={[
            { id: "s1", title: "拉取 AOSP 分支", status: "done" },
            { id: "s2", title: "编译 SystemUI 模块", status: "done" },
            { id: "s3", title: "推送并验证产物", status: "running" },
          ]}
        />
      )}

      {on.background && <BackgroundTaskNotices tasks={[{
        id: "debug", sessionId: "debug", command: "ninja SystemUI", title: "编译 SystemUI", status: backgroundStatus,
        exitCode: null, logTail: "> Building SystemUI...\n> [ 42%] Compiling resources", error: null, logPath: "/tmp/debug.log", startedAt,
      }]} onStop={() => setBackgroundStatus("cancelled")} />}

      {on.queue && (
        <ChatQueuePanel
          items={queue}
          busy={{}}
          onGuide={(id) => setQueue((items) => items.filter((item) => item.id !== id))}
          onEdit={async (item) => {
            if (!onEditQueuedMessage) return;
            onEditQueuedMessage(item.content);
            setQueue((items) => items.filter((queued) => queued.id !== item.id));
          }}
          onRemove={async (id) => { setQueue((items) => items.filter((item) => item.id !== id)); }}
          onReorder={async (ids) => { setQueue(items => ids.map(id => items.find(item => item.id === id)).filter((item): item is typeof items[number] => Boolean(item))); return true; }}
        />
      )}

      {on.files && (
        <ComposerChangedFiles
          slotId="debug-files"
          changes={{
            created: ["src/components/ui/UsageMetrics.tsx"],
            modified: [
              "src/components/ui/ProgressRing.tsx",
              "src/views/Chat/ContextUsageIndicator.tsx",
              "src/views/Chat/chat-usage.css",
            ],
            deleted: ["src/views/Chat/legacy-usage.ts"],
            entries: [
              { path: "src/components/ui/UsageMetrics.tsx", status: "created", addedLines: 80, removedLines: 0 },
              { path: "src/components/ui/ProgressRing.tsx", status: "modified", addedLines: 41, removedLines: 30 },
              { path: "src/views/Chat/ContextUsageIndicator.tsx", status: "modified", addedLines: 50, removedLines: 80 },
              { path: "src/views/Chat/chat-usage.css", status: "modified", addedLines: 30, removedLines: 14 },
              { path: "src/views/Chat/legacy-usage.ts", status: "deleted", addedLines: 0, removedLines: 30 },
            ],
            addedLines: 201,
            removedLines: 154,
          }}
          onOpen={NOOP}
        />
      )}
    </ComposerNoticePreview>
  );
}
