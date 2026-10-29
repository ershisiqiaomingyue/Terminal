import React, { useRef, useState } from "react";
import { Download } from "lucide-react";
import { Button, Checkbox, Input } from "@/components/ui";
import { importSkillRepository, previewSkillRepository, type SkillRemotePreview } from "@/platform/api";
import { formatInvokeError } from "@/platform/runtime";
import { useSettingsNotification } from "./SettingsNotification";
import { useConfirmAction } from "@/components/ConfirmDialog/useConfirmAction";

export function SkillRemoteImport({ open, onChanged }: { open: boolean; onChanged: () => Promise<void> }) {
  const notify = useSettingsNotification();
  const { confirmAction, confirmationDialog } = useConfirmAction();
  const [url, setUrl] = useState("");
  const [branch, setBranch] = useState("");
  const [preview, setPreview] = useState<SkillRemotePreview | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);

  const fetchPreview = async () => {
    if (busyRef.current || !url.trim()) return;
    busyRef.current = true; setBusy(true); setPreview(null);
    try {
      const result = await previewSkillRepository(url.trim(), branch.trim(), "");
      setPreview(result);
      setSelected(result.skills.filter((skill) => !skill.conflict).map((skill) => skill.sourcePath));
    } catch (error) { notify(formatInvokeError(error), "error"); }
    finally { busyRef.current = false; setBusy(false); }
  };
  const download = async () => {
    if (busyRef.current || !preview || !selected.length) return;
    busyRef.current = true; setBusy(true);
    try {
      const result = await importSkillRepository(preview.url, preview.branch, preview.destination,
        selected.map((sourcePath) => ({ sourcePath, resolution: preview.skills.find((skill) => skill.sourcePath === sourcePath)?.conflict ? "overwrite" : "keep" })));
      await onChanged(); setPreview(null);
      notify(`已下载 ${result.importedIds.length} 个技能。`, "success");
    } catch (error) { notify(formatInvokeError(error), "error"); throw error; }
    finally { busyRef.current = false; setBusy(false); }
  };
  const downloadSelected = () => {
    const conflicts = preview?.skills.filter((skill) => skill.conflict && selected.includes(skill.sourcePath)) || [];
    if (conflicts.length) {
      confirmAction({ title: "覆盖已有技能", message: `将替换已有技能：${conflicts.map((skill) => skill.name).join("、")}。`, confirmLabel: "覆盖并下载", onConfirm: download });
    } else { void download().catch(() => {}); }
  };
  return <>
    {confirmationDialog}
    {open && <div className="skills-remote-import">
      <div className="skills-import-fields">
        <Input size="sm" value={url} aria-label="Skills GitHub 地址" placeholder="https://github.com/owner/repository" disabled={busy} onChange={(event) => { setUrl(event.target.value); setPreview(null); }} />
        <Input size="sm" className="skill-branch-input" value={branch} aria-label="Skills 分支" placeholder="默认分支" disabled={busy} onChange={(event) => { setBranch(event.target.value); setPreview(null); }} />
        <Button size="sm" variant="subtle" icon={<Download size={14} />} loading={busy} disabled={!url.trim()} onClick={() => void fetchPreview()}>拉取</Button>
      </div>
      {preview && <>
        {preview.skills.map((skill) => <label key={skill.sourcePath} className="skill-remote-candidate">
          <Checkbox aria-label={`下载 ${skill.name}`} disabled={busy} checked={selected.includes(skill.sourcePath)} onChange={(checked) => setSelected((paths) => checked ? [...paths, skill.sourcePath] : paths.filter((path) => path !== skill.sourcePath))} />
          <span><span className="setting-title">{skill.name}</span><span className="setting-desc">{skill.description || skill.id}</span>{skill.conflict && <span className="skill-conflict">已存在，下载将覆盖</span>}</span>
        </label>)}
        <Button size="sm" variant="primary" loading={busy} disabled={!selected.length} icon={<Download size={14} />} onClick={downloadSelected}>下载所选 ({selected.length})</Button>
      </>}
    </div>}
  </>;
}
