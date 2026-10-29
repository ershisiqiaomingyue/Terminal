import React, { useState } from "react";
import { useConfirmAction } from "@/components/ConfirmDialog/useConfirmAction";
import { RotateCcw, Save } from "lucide-react";
import { Button, Select } from "@/components/ui";
import { DEFAULT_THINKING_COPY, THINKING_COPY_LABELS, THINKING_COPY_PHASES, THINKING_COPY_PRESET_OPTIONS, readThinkingCopy, setThinkingCopy, thinkingCopyForPreset, thinkingCopyFromDraft, thinkingCopyPresetFor, type ThinkingCopy, type ThinkingCopyPhase, type ThinkingCopyPreset } from "@/platform/thinkingCopy";
import { useSettingsNotification } from "./SettingsNotification";
import { formatInvokeError } from "@/platform/runtime";

const draftFor = (copy: ThinkingCopy) => Object.fromEntries(THINKING_COPY_PHASES.map((phase) => [phase, copy[phase].join("\n")])) as Record<ThinkingCopyPhase, string>;

export function AgentThinkingSettings() {
  const { confirmAction, confirmationDialog } = useConfirmAction();
  const notify = useSettingsNotification();
  const [draft, setDraft] = useState(() => draftFor(readThinkingCopy()));
  const [preset, setPreset] = useState<ThinkingCopyPreset>(() => thinkingCopyPresetFor(readThinkingCopy()));
  const save = (reset = false) => {
    try {
      const copy = reset ? DEFAULT_THINKING_COPY : thinkingCopyFromDraft(draft);
      setThinkingCopy(copy);
      setDraft(draftFor(copy));
      setPreset(thinkingCopyPresetFor(copy));
      notify(reset ? "已恢复默认文案。" : "思考文案已保存。", "success");
    } catch (error) { notify(formatInvokeError(error), "error"); }
  };
  return <div>
    <div className="setting-group-title">AI 状态文案</div>
    <div className="setting-row-item thinking-copy-style">
      <div className="setting-label-block">
        <label className="setting-title" id="thinking-copy-preset-label">文案风格</label>
        <span className="setting-desc">这里显示的是状态提示，不是模型的内部思维过程。</span>
      </div>
      <div className="setting-control-block">
        <Select
          size="sm"
          aria-label="AI 状态文案风格"
          value={preset}
          options={[
            ...THINKING_COPY_PRESET_OPTIONS,
            ...(preset === "custom" ? [{ value: "custom", label: "自定义（当前）", disabled: true }] : []),
          ]}
          onChange={(value) => {
            if (value === "custom") return;
            const nextPreset = value as Exclude<ThinkingCopyPreset, "custom">;
            const nextCopy = thinkingCopyForPreset(nextPreset);
            setPreset(nextPreset);
            setDraft(draftFor(nextCopy));
          }}
          style={{ maxWidth: 320 }}
        />
      </div>
    </div>
    <div className="setting-card-group">
      {THINKING_COPY_PHASES.map((phase) => <div className="setting-row-item thinking-copy-row" key={phase}>
        <div className="setting-label-block"><label className="setting-title" htmlFor={`thinking-${phase}`}>{THINKING_COPY_LABELS[phase]}</label></div>
        <div className="setting-control-block"><textarea id={`thinking-${phase}`} className="desk-input thinking-copy-input" rows={4} value={draft[phase]} placeholder={DEFAULT_THINKING_COPY[phase].join("\n")} onChange={(event) => { setPreset("custom"); setDraft((previous) => ({ ...previous, [phase]: event.target.value })); }} /></div>
      </div>)}
    </div>
    <div className="settings-preference-actions"><Button size="sm" icon={<RotateCcw size={14} />} onClick={() => confirmAction({ title: "恢复默认文案", message: "确认用默认文案覆盖当前自定义文案和未保存的修改？", confirmLabel: "确认恢复", onConfirm: () => save(true) })}>恢复默认文案</Button><Button size="sm" variant="primary" icon={<Save size={14} />} onClick={() => save()}>保存文案</Button></div>
    {confirmationDialog}
  </div>;
}
