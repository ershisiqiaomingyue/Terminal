import React, { useEffect, useState } from "react";
import { useConfirmAction } from "@/components/ConfirmDialog/useConfirmAction";
import { RotateCcw, Save } from "lucide-react";
import { Button, Input, Select } from "@/components/ui";
import { LOGCAT_BUFFER_OPTIONS, readLogcatBufferBytes, setLogcatBufferBytes, subscribeLogcatBuffer } from "@/platform/logcatBuffer";
import { DEFAULT_TOOL_PREFERENCES, TOOL_PARAMETERS, TOOL_PARAMETER_RULES, readToolPreferences, setToolPreferences, toolPreferencesFromDraft, type ToolPreferences } from "@/platform/toolPreferences";
import { useSettingsNotification } from "./SettingsNotification";
import { formatInvokeError } from "@/platform/runtime";

const draftFor = (values: ToolPreferences) => ({ simpleperfDuration: String(values.simpleperfDuration), methodDuration: String(values.methodDuration), methodInterval: String(values.methodInterval) });

export function ToolsPanel() {
  const { confirmAction, confirmationDialog } = useConfirmAction();
  const notify = useSettingsNotification();
  const [buffer, setBuffer] = useState(readLogcatBufferBytes);
  const [draft, setDraft] = useState(() => draftFor(readToolPreferences()));
  useEffect(() => subscribeLogcatBuffer(setBuffer), []);

  const save = (reset = false) => {
    try {
      const next = reset ? DEFAULT_TOOL_PREFERENCES : toolPreferencesFromDraft(draft);
      setToolPreferences(next);
      setDraft(draftFor(next));
      notify(reset ? "采样默认值已恢复。" : "采样默认值已保存。", "success");
    } catch (error) { notify(formatInvokeError(error), "error"); }
  };

  return <section className="settings-pane">
    <header className="pane-header"><h2 className="pane-title">工具</h2></header>
    <div>
      <div className="setting-group-title">Logcat</div>
      <div className="setting-card-group">
        <div className="setting-row-item">
          <div className="setting-label-block"><span className="setting-title">Logcat 屏幕缓冲</span><span className="setting-desc">默认 2 MB；完整采集仍保留在会话历史中。</span></div>
          <div className="setting-control-block"><Select size="sm" aria-label="Logcat 屏幕缓冲" value={String(buffer)} onChange={(value) => setLogcatBufferBytes(Number(value))} options={LOGCAT_BUFFER_OPTIONS.map((option) => ({ label: option.label, value: String(option.bytes) }))} style={{ width: "100%" }} /></div>
        </div>
      </div>
    </div>
    <div>
      <div className="setting-group-title">采样默认值</div>
      <div className="setting-card-group">
        {TOOL_PARAMETERS.map((key) => {
          const rule = TOOL_PARAMETER_RULES[key];
          return <div className="setting-row-item" key={key}>
            <div className="setting-label-block"><label className="setting-title" htmlFor={`tool-${key}`}>{rule.label}</label><span className="setting-desc">{rule.min}–{rule.max} {rule.unit}</span></div>
            <div className="setting-control-block"><Input id={`tool-${key}`} type="number" size="sm" min={rule.min} max={rule.max} step={1} value={draft[key]} onChange={(event) => setDraft((previous) => ({ ...previous, [key]: event.target.value }))} /></div>
          </div>;
        })}
      </div>
      <div className="settings-preference-actions"><Button size="sm" icon={<RotateCcw size={14} />} onClick={() => confirmAction({ title: "恢复采样默认值", message: "确认用默认采样参数覆盖当前配置和未保存的修改？", confirmLabel: "确认恢复", onConfirm: () => save(true) })}>恢复默认</Button><Button size="sm" variant="primary" icon={<Save size={14} />} onClick={() => save()}>保存默认值</Button></div>
    </div>
    {confirmationDialog}
  </section>;
}
