import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { Button } from "rain-ui";
import { createPortal } from "react-dom";
import { Boxes, ChevronDown, FolderOpen, Paperclip, Plus, Settings2 } from "lucide-react";
import { Checkbox } from "rain-ui";
import { notify } from "@/platform/notifications";
import { readAutomationProjects, selectedAutomationProjectIds } from "./projectSelection";
import "./composer-project-menu.css";

type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onFile: () => void;
  onDirectory: () => void;
  projectIds?: string[];
  onProjectIdsChange: (ids: string[]) => void;
};

export function ComposerAddMenu({ open, onOpenChange, onFile, onDirectory, projectIds, onProjectIdsChange }: Props) {
  const root = useRef<HTMLDivElement>(null);
  const projectTrigger = useRef<HTMLButtonElement>(null);
  const projectPanel = useRef<HTMLDivElement>(null);
  const [projectsOpen, setProjectsOpen] = useState(false);
  const [projects, setProjects] = useState(readAutomationProjects);
  const [position, setPosition] = useState({ left: 12, bottom: 12, width: 300, maxHeight: 420 });
  const selected = selectedAutomationProjectIds(projects, projectIds);
  const choices = projects.filter((project) => project.id !== "general");
  const allSelected = choices.length > 0 && selected.length === choices.length;

  useEffect(() => {
    if (!open) { setProjectsOpen(false); return; }
    setProjects(readAutomationProjects());
    const outside = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node) && !projectPanel.current?.contains(event.target as Node)) onOpenChange(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      if (projectsOpen) { setProjectsOpen(false); projectTrigger.current?.focus(); }
      else { onOpenChange(false); root.current?.querySelector<HTMLButtonElement>(".composer-icon")?.focus(); }
    };
    document.addEventListener("pointerdown", outside);
    document.addEventListener("keydown", escape);
    return () => { document.removeEventListener("pointerdown", outside); document.removeEventListener("keydown", escape); };
  }, [open, projectsOpen, onOpenChange]);

  useLayoutEffect(() => {
    if (!open || !projectsOpen) return;
    const place = () => {
      const anchor = projectTrigger.current?.getBoundingClientRect();
      if (!anchor) return;
      const width = Math.min(320, window.innerWidth - 24);
      setPosition({ left: Math.max(12, Math.min(anchor.left, window.innerWidth - width - 12)),
        bottom: window.innerHeight - anchor.top + 8, width, maxHeight: Math.min(420, Math.max(100, anchor.top - 20)) });
    };
    place();
    projectPanel.current?.querySelector<HTMLInputElement>("input:not(:disabled)")?.focus({ preventScroll: true });
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => { window.removeEventListener("resize", place); window.removeEventListener("scroll", place, true); };
  }, [open, projectsOpen]);

  const choose = (id: string, name: string, checked: boolean) => {
    onProjectIdsChange(checked ? [...selected, id] : selected.filter((value) => value !== id));
    notify(checked ? `已选择项目“${name}”，下一条消息将带入配置。` : `已取消选择项目“${name}”。`, "success");
  };
  const openSettings = () => {
    onOpenChange(false);
    window.dispatchEvent(new CustomEvent("android-tool-open-settings", { detail: { section: "project-automation" } }));
  };

  return <div ref={root} className="composer-pop">
    <Button type="button" variant="bare" className="composer-icon" title="添加文件、工作目录或项目配置" aria-label="添加文件、工作目录或项目配置" aria-expanded={open} onClick={() => onOpenChange(!open)}><Plus size={18} /></Button>
    {open && <div className="composer-menu">
      <div className="composer-menu-heading">添加</div>
      <Button type="button" variant="bare" className="composer-menu-row" onClick={() => { onOpenChange(false); onFile(); }}><Paperclip size={15} /><span>文件和文件夹</span><small>本地</small></Button>
      <Button type="button" variant="bare" className="composer-menu-row" onClick={() => { onOpenChange(false); onDirectory(); }}><FolderOpen size={15} /><span>工作目录</span><small>本地</small></Button>
      <button ref={projectTrigger} type="button" className="composer-menu-row" aria-label="选择项目" aria-expanded={projectsOpen} aria-controls="composer-project-options" onClick={() => setProjectsOpen((value) => !value)}><Boxes size={15} /><span>选择项目</span><small>{selected.length ? `${selected.length} 个` : "通用"}</small><ChevronDown size={13} /></button>
      <div className="composer-menu-divider" />
      <div className="composer-menu-heading secondary">接入范围</div>
      <div className="composer-menu-muted">所选文件、目录作为会话来源；所选项目配置与通用配置一起提供给 AI。</div>
    </div>}
    {open && projectsOpen && createPortal(<div ref={projectPanel} id="composer-project-options" className="composer-project-popover" role="dialog" aria-label="选择项目配置" style={position}>
      <div className="composer-project-select-all"><Checkbox label="全选" aria-label="全选项目" disabled={!choices.length} checked={allSelected} indeterminate={selected.length > 0 && !allSelected} onChange={(checked) => {
        onProjectIdsChange(checked ? choices.map((project) => project.id) : []);
        notify(checked ? `已选择全部 ${choices.length} 个项目，下一条消息将带入配置。` : "已取消全部项目选择，保留通用配置。", "success");
      }} /></div>
      <div className="composer-project-list" role="group" aria-label="项目配置列表">
        <Checkbox checked disabled label={<span className="composer-project-label"><b>通用</b><small>默认加入每轮对话</small></span>} aria-label="通用项目（默认启用）" />
        {choices.map((project) => <Checkbox key={project.id} checked={selected.includes(project.id)} aria-label={`选择项目 ${project.name}`} onChange={(checked) => choose(project.id, project.name, checked)} label={<span className="composer-project-label"><b title={project.name}>{project.name}</b>{typeof project.description === "string" && project.description.trim() && <small title={project.description}>{project.description}</small>}</span>} />)}
        {!choices.length && <p className="composer-project-empty">暂无项目，可在下方添加。</p>}
      </div>
      <div className="composer-project-divider" role="separator" />
      <Button type="button" variant="bare" className="composer-project-settings" onClick={openSettings}><Settings2 size={14} /><span>添加项目与自动化配置</span><Plus size={13} /></Button>
    </div>, document.body)}
  </div>;
}
