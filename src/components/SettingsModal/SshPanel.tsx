import React, { useEffect, useRef, useState } from "react";
import { Server, Pencil, Trash2 } from "lucide-react";
import { Button } from "@/components/ui";
import {
  countActiveSshTerminals,
  deleteSshConnection,
  listSshConnections,
  saveSshConnection,
  testSshConnection,
  testSshConnectionConfig,
  type SshConnection,
} from "@/platform/api";
import { formatInvokeError, isTauri } from "@/platform/runtime";
import { useSettingsNotification } from "./SettingsNotification";
import { useConfirmAction } from "@/components/ConfirmDialog/useConfirmAction";
import { SettingsEditDialog } from "./SettingsEditDialog";
import {
  EMPTY_SSH_VALUES,
  SshConnectionFields,
  sshValuesFromConnection,
  type SshConnectionValues,
} from "./SshConnectionFields";

export const SshPanel: React.FC = () => {
  const { confirmAction, confirmationDialog } = useConfirmAction();
  const notify = useSettingsNotification();
  const [sshEditingId, setSshEditingId] = useState("");
  // 八个字段收成一个对象：字段的 JSX 已经搬进 SshConnectionFields，
  // 与终端左栏的「新建 SSH 连接」弹窗共用同一份展示层。
  const [values, setValues] = useState<SshConnectionValues>(EMPTY_SSH_VALUES);
  const [savedSshList, setSavedSshList] = useState<SshConnection[]>([]);
  const [busy, setBusy] = useState(false);
  const formRef = useRef<HTMLDivElement>(null);
  const [formHeight, setFormHeight] = useState(0);
  const patchValues = (patch: Partial<SshConnectionValues>) =>
    setValues((prev) => ({ ...prev, ...patch }));
  // 进入编辑前先快照表单，取消编辑时原样回填，别把用户没保存的草稿冲掉。
  const draftBeforeEdit = useRef<SshConnectionValues | null>(null);
  const closeEditor = () => {
    const draft = draftBeforeEdit.current;
    if (draft) setValues(draft);
    draftBeforeEdit.current = null;
    setSshEditingId("");
  };
  const editConnection = (item: SshConnection) => {
    setFormHeight(formRef.current?.getBoundingClientRect().height || 0);
    draftBeforeEdit.current = values;
    setSshEditingId(item.id);
    setValues(sshValuesFromConnection(item));
  };

  const reload = async () => {
    if (!isTauri()) {
      notify("请在桌面应用中管理 SSH 连接。", "error");
      return;
    }
    setSavedSshList(await listSshConnections());
  };

  useEffect(() => {
    void reload().catch((reason) => notify(formatInvokeError(reason), "error"));
  }, []);

  const currentConfig = (): SshConnection => ({
    id: sshEditingId || `ssh-${Date.now()}`,
    name: `${values.username.trim()}@${values.host.trim()}`,
    host: values.host.trim(),
    port: Number(values.port) || 22,
    username: values.username.trim(),
    privateKeyPath: values.authType === "key" ? values.keyPath.trim() : null,
    strictHostKey: values.strictHostKey,
    defaultRoot: values.defaultRoot.trim() || null,
  });

  const save = async () => {
    setBusy(true);
    try {
      await saveSshConnection(currentConfig(), values.authType === "password" ? values.password : undefined);
      await reload();
      if (sshEditingId) closeEditor();
      else setValues((prev) => ({ ...prev, password: "", strictHostKey: false }));
      notify("SSH 连接已保存。", "success");
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    } finally {
      setBusy(false);
    }
  };

  const testDraft = async () => {
    setBusy(true);
    try {
      await testSshConnectionConfig(currentConfig(), values.authType === "password" ? values.password : undefined);
      notify("SSH 连接测试通过。", "success");
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    } finally {
      setBusy(false);
    }
  };

  const connectionForm = (
      <div>
        {!sshEditingId && <div className="setting-group-title">新建 SSH 连接</div>}
        <div className="setting-card-group">
          <SshConnectionFields values={values} onChange={patchValues} disabled={busy} />

          <div className="setting-row-item" style={{ background: "var(--bg-surface)", justifyContent: "flex-end", gap: 10 }}>
            <Button size="sm" variant="secondary" disabled={busy} onClick={() => void testDraft()}>测试连接并浏览目录</Button>
            {sshEditingId && <Button size="sm" variant="secondary" disabled={busy} onClick={closeEditor}>取消</Button>}
            <Button size="sm" variant="primary" disabled={busy} onClick={() => void save()}>保存 SSH 连接</Button>
          </div>
        </div>
      </div>
  );
  return (
    <section className="settings-pane">
      {confirmationDialog}
      <header className="pane-header"><h2 className="pane-title">SSH 远程</h2></header>
      <div ref={formRef}>{sshEditingId ? <div style={{ height: formHeight }} aria-hidden="true" /> : connectionForm}</div>
      {sshEditingId && <SettingsEditDialog title="编辑 SSH 连接" busy={busy} onClose={closeEditor}>{connectionForm}</SettingsEditDialog>}
      <div>
        <div className="setting-group-title">已配置的 SSH 连接 ({savedSshList.length})</div>
        {savedSshList.length > 0 && <div className="setting-card-group" style={{ padding: "10px 14px", display: "flex", flexDirection: "column", gap: 8 }}>
          {savedSshList.map((item) => (
            <div key={item.id} className="setting-item-pill">
              <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                <Server size={16} color="var(--accent-primary)" />
                <div>
                  <div style={{ fontSize: 13, fontWeight: 600 }}>{item.name}</div>
                  <div style={{ fontSize: 11, color: "var(--text-tertiary)", fontFamily: "var(--font-mono)" }}>
                    {item.username}@{item.host}:{item.port} · {item.defaultRoot || "~"}
                  </div>
                </div>
              </div>
              <div style={{ marginLeft: "auto", display: "flex", alignItems: "center", gap: 6 }}>
                <Button size="xs" variant="secondary" onClick={() => void testSshConnection(item.id).then(() => notify("SSH 连接测试通过。", "success")).catch((reason) => notify(formatInvokeError(reason), "error"))}>测试</Button>
                <button type="button" className="icon-action-btn" title="编辑连接" aria-label={`编辑 SSH ${item.name}`} disabled={busy} onClick={() => editConnection(item)}>
                  <Pencil size={12} />
                </button>
                <button type="button" className="icon-action-btn danger" title="删除连接" onClick={() => void countActiveSshTerminals(item.id).then((active) => active || 0).catch(() => 0).then((active) => confirmAction({ title: "删除 SSH 连接", message: `确认删除「${item.name}」及其保存的凭据？不会删除远程文件。${active > 0 ? `该服务器有 ${active} 个活跃终端会话，删除后将断开。` : ""}`, onConfirm: async () => { await deleteSshConnection(item.id); await reload(); if (sshEditingId === item.id) closeEditor(); notify("SSH 连接已删除。", "success"); } }))}>
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
