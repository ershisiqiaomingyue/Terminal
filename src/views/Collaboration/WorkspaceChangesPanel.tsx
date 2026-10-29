import React, { lazy, Suspense, useEffect, useState } from "react";
import { FileCode2 } from "lucide-react";
import { Button } from "@/components/ui/Button";
import type { WorkspaceEnvironment, WorkspaceGitChange } from "@/platform/chatApi";

const ChatFilePreview = lazy(() => import("@/views/Chat/ChatFilePreview"));

type WorkspaceChangesPanelProps = {
  workspacePath?: string;
  workspaceEnvironment: WorkspaceEnvironment | null;
  changes: WorkspaceGitChange[];
  loading: boolean;
};

const statusLabel = (status: string) => ({ created: "新增", modified: "修改", deleted: "删除", renamed: "重命名" }[status] || status);
const statusMark = (status: string) => ({ created: "A", modified: "M", deleted: "D", renamed: "R" }[status] || "?");

export function WorkspaceChangesPanel({ workspacePath, workspaceEnvironment, changes, loading }: WorkspaceChangesPanelProps) {
  const [selectedPath, setSelectedPath] = useState("");
  useEffect(() => { setSelectedPath(""); }, [workspacePath]);
  useEffect(() => {
    if (selectedPath && !changes.some((change) => change.path === selectedPath)) setSelectedPath("");
  }, [changes, selectedPath]);

  if (!workspacePath) return <div className="workspace-changes-panel"><p className="workspace-changes-empty">当前对话未绑定本地工作区。</p></div>;
  if (loading) return <div className="workspace-changes-panel"><p className="workspace-changes-empty">正在读取工作区改动…</p></div>;
  if (!workspaceEnvironment?.git.available) return <div className="workspace-changes-panel"><p className="workspace-changes-empty">Git 未安装或暂时不可用。</p></div>;
  if (!workspaceEnvironment.git.repository) return <div className="workspace-changes-panel"><p className="workspace-changes-empty">当前工作区不是 Git 仓库。</p></div>;

  return <div className="workspace-changes-panel">
    {!changes.length && <p className="workspace-changes-empty">工作区没有未提交的文件改动。</p>}
    {!!changes.length && <div className="workspace-changes-list" aria-label="Git 文件改动">
      {changes.map((change) => {
        const path = change.path.replace(/\\/g, "/");
        const separator = path.lastIndexOf("/");
        const name = path.slice(separator + 1);
        const directory = separator >= 0 ? path.slice(0, separator) : "";
        const selected = selectedPath === change.path;
        return <Button key={`${change.status}-${change.path}`} className="workspace-changes-row" type="button" size="sm"
          variant={selected ? "secondary" : "subtle"} aria-pressed={selected}
          aria-label={`查看 ${change.path} 的差异（${statusLabel(change.status)}）`} title={`查看 ${change.path} 的差异`}
          icon={<FileCode2 size={14} aria-hidden="true" />} onClick={() => setSelectedPath(change.path)}>
          <span className={`workspace-change-path is-${change.status}`}>
            <span className="workspace-change-name">{name}</span>
            {directory && <small>{directory}</small>}
          </span>
          <span className="workspace-change-meta">
            <span className="workspace-change-stats"><em>+{change.addedLines}</em><i>−{change.removedLines}</i></span>
            <span className={`workspace-change-status is-${change.status}`} title={statusLabel(change.status)} aria-label={statusLabel(change.status)}>{statusMark(change.status)}</span>
          </span>
        </Button>;
      })}
    </div>}
    {!!changes.length && !selectedPath && <p className="workspace-changes-empty">选择文件查看改动。</p>}
    {selectedPath && <Suspense fallback={<p className="workspace-changes-empty">正在加载文件预览…</p>}><ChatFilePreview
      key={selectedPath}
      path={`${workspacePath.replace(/[\\/]$/, "")}/${selectedPath}`}
      workspacePath={workspacePath}
      gitPath={selectedPath}
      className="workspace-diff-view"
      onClose={() => setSelectedPath("")}
    /></Suspense>}
  </div>;
}
