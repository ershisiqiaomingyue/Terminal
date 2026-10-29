import React from "react";
import { Archive, RotateCcw, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/Button";
import { Modal } from "@/components/ui/Modal";
import { formatInvokeError, isTauri } from "@/platform/runtime";
import {
  archiveHistorySession,
  deleteHistorySession,
  listHistorySummaries,
  type HistorySummary,
} from "@/platform/chatApi";
import { dispatchChatNav } from "@/views/Chat/chatNavStore";
import "./archived-sessions-dialog.css";

/** 归档会话只在历史库里带着 archived 标记，正文仍然完整，恢复就是把这个标记清掉。 */
function formatArchivedAt(value: number) {
  return new Date(value).toLocaleString("zh-CN", { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" });
}

function summaryDetail(summary: HistorySummary) {
  const workspacePath = typeof summary.metadata?.workspacePath === "string" ? summary.metadata.workspacePath : "";
  const scope = summary.metadata?.scope === "workspace" && workspacePath ? workspacePath : "独立会话";
  return `${scope} · ${summary.messageCount} 条消息`;
}

export function ArchivedSessionsDialog({ onClose }: { onClose: () => void }) {
  const [summaries, setSummaries] = React.useState<HistorySummary[] | null>(null);
  const [error, setError] = React.useState("");
  const [busyId, setBusyId] = React.useState("");
  const [removing, setRemoving] = React.useState<HistorySummary | null>(null);

  const refresh = React.useCallback(async (signal?: { cancelled: boolean }) => {
    if (!isTauri()) { setSummaries([]); return; }
    try {
      const next = await listHistorySummaries(true);
      if (signal?.cancelled) return;
      setSummaries(next);
      setError("");
    } catch (reason) {
      if (signal?.cancelled) return;
      setError(formatInvokeError(reason));
      setSummaries([]);
    }
  }, []);

  React.useEffect(() => {
    const signal = { cancelled: false };
    void refresh(signal);
    return () => { signal.cancelled = true; };
  }, [refresh]);

  const restore = async (summary: HistorySummary) => {
    if (busyId) return;
    setBusyId(summary.id);
    try {
      if (isTauri()) await archiveHistorySession(summary.id, false);
      // 侧栏读的是本地快照，恢复后必须重新拉一次历史摘要，否则会话要等下次启动才回来。
      dispatchChatNav({ type: "restore-session", id: summary.id });
      setSummaries((previous) => (previous || []).filter((item) => item.id !== summary.id));
    } catch (reason) {
      setError(formatInvokeError(reason));
    } finally { setBusyId(""); }
  };

  const remove = async (summary: HistorySummary) => {
    if (busyId) return;
    setBusyId(summary.id);
    try {
      if (isTauri()) await deleteHistorySession(summary.id);
      dispatchChatNav({ type: "deleted-session", id: summary.id });
      setSummaries((previous) => (previous || []).filter((item) => item.id !== summary.id));
      setRemoving(null);
    } catch (reason) {
      setError(formatInvokeError(reason));
    } finally { setBusyId(""); }
  };

  const count = summaries?.length ?? 0;
  const description = summaries === null
    ? "正在读取归档列表…"
    : count === 0
      ? "归档只是把会话从侧栏收起来，正文和附件都还在历史库里。"
      : `共 ${count} 个归档会话 · 恢复后会回到原来的工作区`;

  return <Modal
    open
    title="归档对话"
    description={description}
    icon={<Archive size={17} />}
    busy={Boolean(busyId)}
    onClose={onClose}
    className="archived-sessions-dialog"
  >
    {error && <p className="archive-dialog-error" role="alert">{error}</p>}
    {summaries !== null && summaries.length === 0 && (
      <div className="archive-dialog-empty">
        <Archive size={22} aria-hidden="true" />
        <span>还没有归档的对话。</span>
        <small>在侧栏里对任意对话选「归档会话」，它就会出现在这里。</small>
      </div>
    )}
    {summaries !== null && summaries.length > 0 && (
      <ul className="archive-dialog-list">
        {summaries.map((summary) => <li key={summary.id} className="archive-dialog-item">
          <span className="archive-dialog-label">
            <span className="archive-dialog-title" title={summary.title}>{summary.title || "未命名对话"}</span>
            <span className="archive-dialog-meta">{formatArchivedAt(summary.updatedAt)} · {summaryDetail(summary)}</span>
          </span>
          <span className="archive-dialog-actions">
            {removing?.id === summary.id ? <>
              <Button size="xs" variant="danger" loading={busyId === summary.id} disabled={Boolean(busyId)} onClick={() => void remove(summary)}>确认删除</Button>
              <Button size="xs" variant="subtle" disabled={Boolean(busyId)} onClick={() => setRemoving(null)}>取消</Button>
            </> : <>
              <Button size="xs" variant="subtle" icon={<RotateCcw size={13} />} disabled={Boolean(busyId)} onClick={() => void restore(summary)}>恢复</Button>
              {/* 彻底删除会把历史记录一起抹掉，比归档重，所以先换成二次确认。 */}
              <Button size="xs" variant="subtle" icon={<Trash2 size={13} />} disabled={Boolean(busyId)} onClick={() => setRemoving(summary)}>删除</Button>
            </>}
          </span>
        </li>)}
      </ul>
    )}
  </Modal>;
}

export default ArchivedSessionsDialog;
