import React, { useState } from "react";
import { Button } from "@/components/ui";
import { SettingsEditDialog } from "@/components/SettingsModal/SettingsEditDialog";
import {
  EMPTY_SSH_VALUES,
  SshConnectionFields,
  sshValuesFromConnection,
  type SshConnectionValues,
} from "@/components/SettingsModal/SshConnectionFields";
import { countActiveSshTerminals, saveSshConnection, testSshConnectionConfig, type SshConnection } from "@/platform/api";
import { useConfirmAction } from "@/components/ConfirmDialog/useConfirmAction";
import { formatInvokeError } from "@/platform/runtime";

/** 编辑现有连接时需要回填的只读字段（其余字段由 `sshValuesFromConnection` 转换）。 */
export interface EditSshTarget {
  id: string;
  name: string;
  host: string;
  port?: number | null;
  username: string;
  privateKeyPath?: string | null;
  strictHostKey?: boolean | null;
  defaultRoot?: string | null;
}

export interface NewSshDialogProps {
  /** 传了就是「编辑现有连接」，不传就是「新建」。 */
  editing?: EditSshTarget;
  /** 保存成功之后回调；新建与编辑都会调用。 */
  onCreated: (connection: SshConnection) => void;
  onClose: () => void;
  /** 保存/测试失败时的提示通道（走终端视图现有的 toast）。 */
  onError: (message: string) => void;
  onNotice: (message: string) => void;
}

const buildConfig = (values: SshConnectionValues, editing?: EditSshTarget): SshConnection => ({
  // 编辑时沿用原 id —— save_ssh_connection 是覆盖语义，换 id 会变成"另存一条"。
  id: editing?.id ?? `ssh-${Date.now()}`,
  // 名字保留用户自己的命名：编辑完不该被自动改成 `user@host` 把用户起的名字冲掉。
  name: editing?.name ?? `${values.username.trim()}@${values.host.trim()}`,
  host: values.host.trim(),
  port: Number(values.port) || 22,
  username: values.username.trim(),
  privateKeyPath: values.authType === "key" ? values.keyPath.trim() : null,
  strictHostKey: values.strictHostKey,
  defaultRoot: values.defaultRoot.trim() || null,
});

const isComplete = (values: SshConnectionValues): boolean =>
  Boolean(values.host.trim() && values.username.trim())
  && (values.authType === "key" ? Boolean(values.keyPath.trim()) : true);

/**
 * 左栏「SSH 远程服务器」的**新建连接**弹窗。
 *
 * 之前这个入口只是 "取已保存连接列表里的第一条" 打开 —— 用户没配过连接就只弹一句
 * "请先在设置中保存 SSH 连接"，等于把用户赶去设置页再回来。现在直接在这里填。
 *
 * 视觉与交互刻意和设置页里的编辑弹窗一致（同一个 `SettingsEditDialog` +
 * 同一套 `setting-row-item` 字段组件），用户在两处看到的是同一个东西。
 */
export const NewSshDialog: React.FC<NewSshDialogProps> = ({ editing, onCreated, onClose, onError, onNotice }) => {
  // 编辑：用现有连接回填字段（密码恒为空 = 沿用已保存凭据）；新建：空表。
  const [values, setValues] = useState<SshConnectionValues>(() =>
    editing ? sshValuesFromConnection(editing) : EMPTY_SSH_VALUES,
  );
  const [busy, setBusy] = useState(false);
  const isEdit = Boolean(editing);
  const { confirmAction, confirmationDialog } = useConfirmAction();

  const patch = (next: Partial<SshConnectionValues>) => setValues((prev) => ({ ...prev, ...next }));
  const complete = isComplete(values);

  // 密码认证允许留空直接保存（先存连接、连的时候再补密码）；
  // 但主机和用户名必须有，否则存下来的是一个连不上的空壳。
  const performSave = async () => {
    setBusy(true);
    try {
      const config = buildConfig(values, editing);
      const saved = await saveSshConnection(config, values.authType === "password" ? values.password || undefined : undefined);
      onNotice(isEdit ? `SSH 连接「${saved.name}」已更新。` : `SSH 连接「${saved.name}」已保存。`);
      onCreated(saved);
    } catch (reason) {
      onError(formatInvokeError(reason));
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    if (!complete) {
      onError("请填写主机地址和登录用户名。");
      return;
    }
    // v3 §21.4 S5c：编辑已连接的服务器前，列出受影响的活跃终端数。
    if (isEdit && editing) {
      try {
        const active = await countActiveSshTerminals(editing.id);
        if (active > 0) {
          confirmAction({
            title: "保存修改",
            message: `「${editing.name}」当前有 ${active} 个活跃终端会话。配置修改在下次连接时生效，已打开的终端继续使用旧配置直到重开。`,
            confirmLabel: "继续保存",
            onConfirm: () => performSave(),
          });
          return;
        }
      } catch {
        /* 终端计数失败不阻塞保存 */
      }
    }
    await performSave();
  };

  const testDraft = async () => {
    if (!complete) {
      onError("请填写主机地址和登录用户名。");
      return;
    }
    setBusy(true);
    try {
      await testSshConnectionConfig(buildConfig(values, editing), values.authType === "password" ? values.password || undefined : undefined);
      onNotice("SSH 连接测试通过。");
    } catch (reason) {
      onError(formatInvokeError(reason));
    } finally {
      setBusy(false);
    }
  };

  return (
    <SettingsEditDialog title={isEdit ? "编辑 SSH 连接" : "新建 SSH 连接"} busy={busy} onClose={onClose} className="terminal-ssh-dialog">
      <div className="setting-card-group">
        <SshConnectionFields values={values} onChange={patch} disabled={busy} />
        <div className="setting-row-item" style={{ background: "var(--bg-surface)", justifyContent: "flex-end", gap: 10 }}>
          <Button size="sm" variant="secondary" disabled={busy} onClick={() => void testDraft()}>
            测试连接
          </Button>
          <Button size="sm" variant="secondary" disabled={busy} onClick={onClose}>
            取消
          </Button>
          <Button size="sm" variant="primary" disabled={busy || !complete} onClick={() => void save()}>
            {isEdit ? "保存修改" : "保存并连接"}
          </Button>
        </div>
      </div>
      {confirmationDialog}
    </SettingsEditDialog>
  );
};
