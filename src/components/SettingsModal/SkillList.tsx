import React, { useRef, useState } from "react";
import { RefreshCw, Search, Trash2 } from "lucide-react";
import { Button, Card, Input, ListItem, Switch } from "@/components/ui";
import { deleteSkill, readSkillDetails, setSkillEnabled, type SkillSummary } from "@/platform/api";
import { formatInvokeError } from "@/platform/runtime";
import { useConfirmAction } from "@/components/ConfirmDialog/useConfirmAction";
import { useSettingsNotification } from "./SettingsNotification";

export const SkillList: React.FC<{ skills: SkillSummary[]; onChanged: () => Promise<void> }> = ({ skills, onChanged }) => {
  const notify = useSettingsNotification();
  const { confirmAction, confirmationDialog } = useConfirmAction();
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const [instructions, setInstructions] = useState("");
  const [loadingDetails, setLoadingDetails] = useState(false);
  const requestSequence = useRef(0);
  const keyOf = (skill: SkillSummary) => JSON.stringify([skill.repository, skill.path]);
  const visible = skills.filter((skill) => `${skill.name} ${skill.id} ${skill.description}`.toLowerCase().includes(query.trim().toLowerCase()));

  const run = async (operation: () => Promise<void>) => {
    setBusy(true);
    try { await operation(); await onChanged(); }
    catch (error) { notify(formatInvokeError(error), "error"); }
    finally { setBusy(false); }
  };

  const openDetails = async (skill: SkillSummary) => {
    const sequence = ++requestSequence.current;
    const key = keyOf(skill);
    setInstructions("");
    if (selected === key) { setSelected(null); setLoadingDetails(false); return; }
    setSelected(key);
    setLoadingDetails(true);
    try {
      const content = await readSkillDetails(skill);
      if (sequence === requestSequence.current) setInstructions(content);
    } catch (error) {
      if (sequence === requestSequence.current) setInstructions(formatInvokeError(error));
    } finally {
      if (sequence === requestSequence.current) setLoadingDetails(false);
    }
  };

  return <Card tone="surface" className="skills-manager">
    {confirmationDialog}
    <div className="skills-toolbar">
      <span className="setting-group-title">技能 ({skills.length})</span>
      <Input size="sm" className="skills-search" prefixIcon={<Search size={14} />} value={query} aria-label="搜索技能" placeholder="搜索名称或用途" onChange={(event) => setQuery(event.target.value)} />
      <Button variant="subtle" size="sm" className="skill-icon-button" title="刷新技能" aria-label="刷新技能" disabled={busy} icon={<RefreshCw size={14} />} onClick={() => void run(async () => {})} />
    </div>
    {!visible.length && <p className="setting-desc">{skills.length ? "没有匹配的技能" : "尚未发现技能"}</p>}
    {visible.map((skill) => <ListItem className="skill-entry" key={keyOf(skill)} dense expandable showChevron={false}
      title={skill.name} meta={<span className="skill-source">{skill.builtin ? "内置" : "外部"}</span>}
      description={skill.description || "未提供用途说明"}
      footer={<span className={`skill-status ${skill.status === "ready" ? "is-ready" : ""}`}>/{skill.id} · {skill.status === "ready" ? "可调用" : skill.status === "disabled" ? "已停用" : "暂不可调用"}</span>}
      expanded={selected === keyOf(skill)} onExpandedChange={() => void openDetails(skill)}
      actions={<>
          <Switch checked={skill.enabled !== false} disabled={busy} aria-label={`启用 ${skill.name}`} onChange={(enabled) => void run(() => setSkillEnabled(skill, enabled))} />
          {!skill.builtin && <Button variant="subtle" size="sm" className="skill-icon-button btn-danger-hover" disabled={busy} title="删除技能" aria-label={`删除 ${skill.name}`} icon={<Trash2 size={14} />} onClick={() => confirmAction({
            title: "删除技能", message: `删除「${skill.name}」的 SKILL.md 后将无法调用。技能目录内的脚本和其他文件会保留，可重新导入恢复。`, confirmLabel: "删除", onConfirm: async () => {
              setBusy(true);
              try {
                await deleteSkill(skill);
                if (selected === keyOf(skill)) { ++requestSequence.current; setSelected(null); }
                await onChanged();
                notify("技能已删除。", "success");
              } finally { setBusy(false); }
            },
          })} />}
      </>}
      extra={<Card tone="subtle" className="skill-details">
        <dl><dt>调用方式</dt><dd>启用后 AI 按任务自动选择，也可输入 /{skill.id} 指定；项目自动化使用同一份配置。</dd>
          <dt>运行类型</dt><dd>{skill.runtime === "instruction" ? "指令型 · 读取说明后调用现有工具" : skill.runtime}</dd>
          <dt>状态</dt><dd>{skill.statusMessage}</dd>
          <dt>来源</dt><dd>{skill.repository}{skill.path ? ` / ${skill.path}` : ""}</dd>
          {!!skill.dependencies?.length && <><dt>依赖</dt><dd>{skill.dependencies.join("、")}</dd></>}
          {!!skill.entrypoint && <><dt>入口</dt><dd>{skill.entrypoint}</dd></>}
        </dl>
        <pre className="skill-instructions">{loadingDetails ? "正在读取技能说明..." : instructions}</pre>
      </Card>}
    />)}
  </Card>;
};
