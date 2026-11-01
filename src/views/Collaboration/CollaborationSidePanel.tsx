import React, { useCallback, useEffect, useMemo, useState } from "react";
import { Button } from "rain-ui";
import { CheckCircle2, CircleAlert, Clock3, ListRestart, LoaderCircle, Play, RefreshCw, Square } from "lucide-react";
import { cancelAgentRequest, listCollaborationJobs, recoverCollaborationAgents, runCollaborationAgent, type CollaborationAgentSummary, type CollaborationJobView, type WorkspaceEnvironment, type WorkspaceGitChange } from "@/platform/chatApi";
import { formatInvokeError, isTauri } from "@/platform/runtime";
import { WorkspaceChangesPanel } from "./WorkspaceChangesPanel";
import "./collaboration.css";

type CollaborationSidePanelProps = {
  active?: boolean;
  view: "agents" | "changes";
  width: number;
  threadId: string;
  workspacePath?: string;
  workspaceEnvironment: WorkspaceEnvironment | null;
  workspaceGitChanges: WorkspaceGitChange[];
  workspaceLoading: boolean;
};

const statusText = (status: string) => ({ queued: "等待调度", running: "执行中", interrupted: "等待恢复", completed: "已完成", completed_with_errors: "已完成，有错误", failed: "失败", cancelled: "已停止" }[status] || status);

function AgentStatusIcon({ status }: { status: string }) {
  if (status === "running") return <LoaderCircle size={14} className="collaboration-spin" />;
  if (status === "completed") return <CheckCircle2 size={14} />;
  if (status === "failed") return <CircleAlert size={14} />;
  return <Clock3 size={14} />;
}

export const CollaborationSidePanel: React.FC<CollaborationSidePanelProps> = ({ active = true, view, width, threadId, workspacePath, workspaceEnvironment, workspaceGitChanges, workspaceLoading }) => {
  const [jobs, setJobs] = useState<CollaborationJobView[]>([]);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const refresh = useCallback(async () => {
    if (!threadId || !isTauri()) return [];
    const next = await listCollaborationJobs(threadId);
    setJobs(next);
    return next;
  }, [threadId]);

  useEffect(() => { if (active && view === "agents") void refresh().catch((error) => setMessage(formatInvokeError(error))); }, [active, view, refresh]);
  const activeAgents = useMemo(() => jobs.flatMap((job) => job.agents).filter((agent) => ["queued", "running", "interrupted"].includes(agent.status)), [jobs]);
  const completedAgents = useMemo(() => jobs.flatMap((job) => job.agents).filter((agent) => ["completed", "failed", "cancelled"].includes(agent.status)), [jobs]);
  useEffect(() => {
    if (!active || view !== "agents" || !activeAgents.length || !isTauri()) return;
    const timer = window.setInterval(() => { if (document.visibilityState === "visible") void refresh().catch(() => undefined); }, 900);
    return () => window.clearInterval(timer);
  }, [active, view, activeAgents.length, refresh]);

  const dispatch = useCallback(async (job: CollaborationJobView) => {
    const runnable = job.agents.filter((agent) => agent.status === "queued" || agent.status === "interrupted");
    if (!runnable.length) return;
    setBusy(true); setMessage(`正在启动 ${runnable.length} 个子智能体`);
    try {
      const results = await Promise.allSettled(runnable.map((agent) => runCollaborationAgent(job.job.id, agent.id)));
      const failed = results.find((result): result is PromiseRejectedResult => result.status === "rejected");
      await refresh(); setMessage(failed ? formatInvokeError(failed.reason) : "");
    } catch (error) { setMessage(formatInvokeError(error)); } finally { setBusy(false); }
  }, [refresh]);

  const recover = async () => {
    setBusy(true);
    try { const recovered = await recoverCollaborationAgents(); await refresh(); setMessage(recovered.length ? `已恢复 ${recovered.length} 个中断子智能体。` : "没有需要恢复的子智能体。"); }
    catch (error) { setMessage(formatInvokeError(error)); } finally { setBusy(false); }
  };
  const stopAgent = async (agent: CollaborationAgentSummary) => { try { await cancelAgentRequest(agent.sessionId, agent.turnId, "collaboration_stop"); await refresh(); } catch (error) { setMessage(formatInvokeError(error)); } };
  const displayed = (statuses: string[]) => jobs.flatMap((job) => job.agents.filter((agent) => statuses.includes(agent.status)).map((agent) => ({ job, agent })));

  return <aside id="chat-workspace-side-panel" className="collaboration-side-panel" style={{ width, flexBasis: width }} aria-label={view === "agents" ? "子智能体" : "文件改动"}>
    {view === "agents" ? <>
    <div className="collaboration-side-scroll">
      <section className="collaboration-side-section"><header><span>已开启 · {activeAgents.length}</span><Button type="button" variant="bare" title="刷新子智能体" aria-label="刷新子智能体" onClick={() => { void refresh(); }}><RefreshCw size={14} /></Button></header>{!activeAgents.length && <p className="collaboration-side-empty">没有已开启的子智能体</p>}{displayed(["queued", "running", "interrupted"]).map(({ job, agent }) => <div className="collaboration-side-agent" key={agent.id}><span className={`collaboration-side-agent-icon is-${agent.status}`}><AgentStatusIcon status={agent.status} /></span><div><b>{agent.title}</b><small>{statusText(agent.status)} · {job.job.goal}</small></div>{agent.status === "running" ? <Button type="button" variant="bare" title="停止子智能体" aria-label="停止子智能体" onClick={() => { void stopAgent(agent); }}><Square size={13} /></Button> : <Button type="button" variant="bare" title="调度子智能体" aria-label="调度子智能体" disabled={busy} onClick={() => { void dispatch(job); }}><Play size={13} /></Button>}</div>)}</section>
      <section className="collaboration-side-section"><header><span>完成 · {completedAgents.length}</span><Button type="button" variant="bare" title="检查中断任务" aria-label="检查中断任务" disabled={busy} onClick={() => { void recover(); }}><ListRestart size={14} /></Button></header>{!completedAgents.length && <p className="collaboration-side-empty">还没有已完成的子智能体</p>}{displayed(["completed", "failed", "cancelled"]).map(({ job, agent }) => <details className="collaboration-side-agent completed" key={agent.id}><summary><span className={`collaboration-side-agent-icon is-${agent.status}`}><AgentStatusIcon status={agent.status} /></span><div><b>{agent.title}</b><small>{statusText(agent.status)} · {job.job.goal}</small></div></summary>{(agent.summary || agent.error) && <p>{agent.summary || agent.error}</p>}</details>)}</section>
      {message && <p className="collaboration-side-message" role="status">{message}</p>}
    </div>
    </> : <WorkspaceChangesPanel workspacePath={workspacePath} workspaceEnvironment={workspaceEnvironment} changes={workspaceGitChanges} loading={workspaceLoading} />}
  </aside>;
};
