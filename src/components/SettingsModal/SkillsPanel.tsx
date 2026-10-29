import React, { useEffect, useRef, useState } from "react";
import { Download, FolderOpen, Trash2 } from "lucide-react";
import { Input, Button } from "@/components/ui";
import { chooseDirectory, listSkills, saveSkillRepositories, type SkillCatalog } from "@/platform/api";
import { formatInvokeError, isTauri } from "@/platform/runtime";
import { useSettingsNotification } from "./SettingsNotification";
import { useConfirmAction } from "@/components/ConfirmDialog/useConfirmAction";
import { SkillList } from "./SkillList";
import { SkillRemoteImport } from "./SkillRemoteImport";

export const SkillsPanel: React.FC = () => {
  const { confirmAction, confirmationDialog } = useConfirmAction();
  const notify = useSettingsNotification();
  const [skillRepoPath, setSkillRepoPath] = useState("");
  const [catalog, setCatalog] = useState<SkillCatalog | null>(null);
  const [remoteOpen, setRemoteOpen] = useState(false);
  const [localOpen, setLocalOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);

  const reload = async () => {
    if (!isTauri()) throw new Error("请在桌面应用中管理 Skills。");
    setCatalog(await listSkills());
  };
  useEffect(() => {
    void reload().catch((reason) => notify(formatInvokeError(reason), "error"));
  }, []);

  const persist = async (paths: string[]) => {
    await saveSkillRepositories(paths);
    await reload();
  };
  const addLocal = async () => {
    if (busyRef.current || !skillRepoPath.trim()) return;
    busyRef.current = true;
    setBusy(true);
    try {
      await persist([...(catalog?.repositories || []).filter((repo) => !repo.builtin).map((repo) => repo.path), skillRepoPath.trim()]);
      setSkillRepoPath("");
      setLocalOpen(false);
      notify("Skills 目录已添加。", "success");
    } catch (reason) { notify(formatInvokeError(reason), "error"); }
    finally { busyRef.current = false; setBusy(false); }
  };

  return <section className="settings-pane">
    {confirmationDialog}
    <header className="pane-header"><h2 className="pane-title">Skills 扩展</h2></header>
    <div className="skills-import-actions">
      <Button size="sm" variant="secondary" icon={<Download size={14} />} aria-expanded={remoteOpen} onClick={() => setRemoteOpen((open) => !open)}>远程下载</Button>
      <Button size="sm" variant="subtle" icon={<FolderOpen size={14} />} aria-expanded={localOpen} onClick={() => setLocalOpen((open) => !open)}>本地添加</Button>
    </div>
    <SkillRemoteImport open={remoteOpen} onChanged={reload} />
    {localOpen && <div className="skills-local-import">
      <div className="skills-import-fields">
        <Input size="sm" value={skillRepoPath} aria-label="本地技能目录" onChange={(event) => setSkillRepoPath(event.target.value)} placeholder="Skills 目录路径" />
        <Button size="sm" variant="subtle" className="skill-icon-button" title="浏览文件夹" aria-label="浏览技能目录" icon={<FolderOpen size={14} />} disabled={busy} onClick={() => void chooseDirectory("选择 Skills 目录").then((path) => path && setSkillRepoPath(path)).catch((reason) => notify(formatInvokeError(reason), "error"))} />
        <Button size="sm" variant="primary" loading={busy} disabled={!skillRepoPath.trim()} onClick={() => void addLocal()}>添加</Button>
      </div>
    </div>}
    {catalog && <SkillList skills={catalog.skills} onChanged={reload} />}
    {!!catalog?.repositories.length && <div className="skill-repositories">
      <div className="setting-group-title">技能目录</div>
      {catalog.repositories.map((repo) => <div key={repo.path} className="skill-repository">
        <div><span className="setting-title">{repo.builtin ? "内置技能库" : "外部技能目录"} · {repo.skillCount} 个技能</span><span className="skill-repository-path">{repo.path}</span></div>
        {!repo.builtin && <Button size="sm" variant="subtle" className="skill-icon-button btn-danger-hover" title="移除目录" aria-label={`移除目录 ${repo.path}`} icon={<Trash2 size={14} />} onClick={() => confirmAction({
          title: "移除 Skills 目录", message: `移除「${repo.path}」后将不再加载其技能，本地文件会保留。`, confirmLabel: "移除", onConfirm: async () => {
            await persist(catalog.repositories.filter((item) => !item.builtin && item.path !== repo.path).map((item) => item.path));
            notify("Skills 目录已移除。", "success");
          },
        })} />}
      </div>)}
    </div>}
  </section>;
};
