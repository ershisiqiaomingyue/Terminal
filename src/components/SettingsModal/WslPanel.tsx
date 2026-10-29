import React, { useEffect, useRef, useState } from "react";
import { Terminal, Pencil, Trash2 } from "lucide-react";
import { Select, Input, Button } from "@/components/ui";
import { deleteWslProfile, listWslDistributions, listWslProfiles, saveWslProfile, testWslProfile, type WslDistribution, type WslProfile } from "@/platform/api";
import { formatInvokeError, isTauri } from "@/platform/runtime";
import { useSettingsNotification } from "./SettingsNotification";
import { useConfirmAction } from "@/components/ConfirmDialog/useConfirmAction";
import { WSL_PROFILES_CHANGED_EVENT } from "@/platform/globalEnvironment";
import { SettingsEditDialog } from "./SettingsEditDialog";

export const WslPanel: React.FC = () => {
  const { confirmAction, confirmationDialog } = useConfirmAction();
  const notify = useSettingsNotification();
  const [editingId, setEditingId] = useState("");
  const [wslName, setWslName] = useState("");
  const [wslDistro, setWslDistro] = useState("");
  const [wslRoot, setWslRoot] = useState("");
  const [wslShell, setWslShell] = useState("bash");
  const [savedWslList, setSavedWslList] = useState<WslProfile[]>([]);
  const [distributions, setDistributions] = useState<WslDistribution[]>([]);
  const [busy, setBusy] = useState(false);
  const formRef = useRef<HTMLDivElement>(null);
  const [formHeight, setFormHeight] = useState(0);
  const readDraft = () => ({ wslName, wslDistro, wslRoot, wslShell });
  const draftBeforeEdit = useRef<ReturnType<typeof readDraft> | null>(null);
  const closeEditor = () => {
    const draft = draftBeforeEdit.current;
    if (draft) { setWslName(draft.wslName); setWslDistro(draft.wslDistro); setWslRoot(draft.wslRoot); setWslShell(draft.wslShell); }
    draftBeforeEdit.current = null;
    setEditingId("");
  };
  const editProfile = (item: WslProfile) => {
    setFormHeight(formRef.current?.getBoundingClientRect().height || 0);
    draftBeforeEdit.current = readDraft();
    setEditingId(item.id); setWslName(item.name); setWslDistro(item.distribution);
    setWslRoot(item.defaultRoot || ""); setWslShell(item.shell);
  };

  const reload = async () => {
    if (!isTauri()) {
      notify("请在桌面应用中管理 WSL。", "error");
      return;
    }
    const [profiles, distros] = await Promise.all([listWslProfiles(), listWslDistributions().catch(() => [] as WslDistribution[])]);
    setSavedWslList(profiles);
    window.dispatchEvent(new Event(WSL_PROFILES_CHANGED_EVENT));
    setDistributions(distros);
    if (!wslDistro && distros[0]) setWslDistro(distros[0].name);
  };

  useEffect(() => {
    void reload().catch((reason) => notify(formatInvokeError(reason), "error"));
  }, []);

  const save = async () => {
    setBusy(true);
    try {
      await saveWslProfile({
        id: editingId || `wsl-${Date.now()}`,
        name: wslName.trim() || wslDistro,
        distribution: wslDistro,
        defaultRoot: wslRoot.trim() || null,
        shell: wslShell,
      });
      await reload();
      if (editingId) closeEditor();
      notify("WSL 配置已保存。", "success");
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    } finally {
      setBusy(false);
    }
  };

  const profileForm = (
      <div>
        {!editingId && <div className="setting-group-title">新建 WSL Profile</div>}
        <div className="setting-card-group">
          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">配置名称</span>
              <span className="setting-desc">界面显示的 Profile 名称</span>
            </div>
            <div className="setting-control-block">
              <Input
                size="sm"
                value={wslName}
                onChange={(e) => setWslName(e.target.value)}
                placeholder="Ubuntu-22.04 LTS"
                style={{ width: "100%" }}
              />
            </div>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">WSL 发行版 (Distribution)</span>
              <span className="setting-desc">本机检测到的 Linux 实例</span>
            </div>
            <div className="setting-control-block">
              <Select
                size="sm"
                value={wslDistro}
                onChange={(val) => setWslDistro(val as string)}
                placeholder="未检测到发行版"
                options={(wslDistro && !distributions.some((item) => item.name === wslDistro) ? [...distributions, { name: wslDistro }] : distributions).map((item) => ({
                  label: item.name,
                  value: item.name,
                }))}
                style={{ width: "100%" }}
              />
            </div>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">默认根路径 (Default Root)</span>
              <span className="setting-desc">Linux 挂载起点根目录</span>
            </div>
            <div className="setting-control-block">
              <Input
                size="sm"
                value={wslRoot}
                onChange={(e) => setWslRoot(e.target.value)}
                placeholder="/home/username"
                style={{ width: "100%" }}
              />
            </div>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">启动 Shell</span>
              <span className="setting-desc">命令执行终端程序</span>
            </div>
            <div className="setting-control-block">
              <Select
                size="sm"
                value={wslShell}
                onChange={(val) => setWslShell(val as string)}
                options={[
                  { label: "bash (/bin/bash)", value: "bash" },
                  { label: "zsh (/bin/zsh)", value: "zsh" },
                  { label: "sh (/bin/sh)", value: "sh" },
                ]}
                style={{ width: "100%" }}
              />
            </div>
          </div>

          <div className="setting-row-item" style={{ background: "var(--bg-surface)", justifyContent: "flex-end", gap: 10 }}>
            <Button size="sm" variant="secondary" disabled={busy || !editingId} onClick={() => void testWslProfile(editingId).then(() => notify("WSL 联通测试通过。", "success")).catch((reason) => notify(formatInvokeError(reason), "error"))}>测试 WSL 联通</Button>
            {editingId && <Button size="sm" variant="secondary" disabled={busy} onClick={closeEditor}>取消</Button>}
            <Button size="sm" variant="primary" disabled={busy} onClick={() => void save()}>保存配置</Button>
          </div>
        </div>
      </div>
  );
  return (
    <section className="settings-pane">
      {confirmationDialog}
      <header className="pane-header"><h2 className="pane-title">WSL 子系统</h2></header>
      <div ref={formRef}>{editingId ? <div style={{ height: formHeight }} aria-hidden="true" /> : profileForm}</div>
      {editingId && <SettingsEditDialog title="编辑 WSL 配置" busy={busy} onClose={closeEditor}>{profileForm}</SettingsEditDialog>}
      <div>
        <div className="setting-group-title">已配置的 WSL 实例 ({savedWslList.length})</div>
        {savedWslList.length > 0 && <div className="setting-card-group" style={{ padding: "10px 14px", display: "flex", flexDirection: "column", gap: 8 }}>
          {savedWslList.map((item) => (
            <div key={item.id} className="setting-item-pill">
              <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                <Terminal size={16} color="var(--accent-primary)" />
                <div>
                  <div style={{ fontSize: 13, fontWeight: 600 }}>{item.name}</div>
                  <div style={{ fontSize: 11, color: "var(--text-tertiary)", fontFamily: "var(--font-mono)" }}>
                    {item.distribution} · 路径: {item.defaultRoot || "~"} · Shell: {item.shell}
                  </div>
                </div>
              </div>
              <div style={{ marginLeft: "auto", display: "flex", alignItems: "center", gap: 6 }}>
                <Button size="xs" variant="secondary" onClick={() => void testWslProfile(item.id).then(() => notify("WSL 联通测试通过。", "success")).catch((reason) => notify(formatInvokeError(reason), "error"))}>测试</Button>
                <button type="button" className="icon-action-btn" title="编辑 WSL 配置" aria-label={`编辑 WSL ${item.name}`} disabled={busy} onClick={() => editProfile(item)}><Pencil size={12} /></button>
                <button type="button" className="icon-action-btn danger" title="删除 WSL 配置" onClick={() => confirmAction({ title: "删除 WSL 配置", message: `确认删除「${item.name}」配置？不会卸载 WSL 发行版或删除其中的文件。`, onConfirm: async () => { await deleteWslProfile(item.id); await reload(); if (editingId === item.id) closeEditor(); notify("WSL 配置已删除。", "success"); } })}>
                  <Trash2 size={12} />
                </button>
              </div>
            </div>
          ))}
        </div>}
      </div>
    </section>
  );
};
