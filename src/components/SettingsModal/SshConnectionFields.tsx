import React from "react";
import { Input, Select, Switch } from "@/components/ui";

/**
 * SSH 连接的字段集合（新建与编辑共用一份）。
 *
 * 抽出来的理由：终端左栏的「新建 SSH 连接」弹窗也要填这套字段，
 * 而设置页里的表单已经把这套字段的文案、顺序、校验习惯定好了。
 * 两处各写一份 HTML 迟早会漂移（比如一边加"跳板机"另一边没加），
 * 所以这里只保留**受控的展示层**，状态仍由调用方持有。
 */
export interface SshConnectionValues {
  host: string;
  port: string;
  username: string;
  authType: "password" | "key";
  password: string;
  keyPath: string;
  defaultRoot: string;
  strictHostKey: boolean;
}

export const EMPTY_SSH_VALUES: SshConnectionValues = {
  host: "",
  port: "22",
  username: "",
  authType: "password",
  password: "",
  keyPath: "",
  defaultRoot: "",
  strictHostKey: false,
};

export function sshValuesFromConnection(source: {
  host: string;
  port?: number | null;
  username: string;
  privateKeyPath?: string | null;
  strictHostKey?: boolean | null;
  defaultRoot?: string | null;
}): SshConnectionValues {
  return {
    host: source.host,
    port: String(source.port || 22),
    username: source.username,
    // 有私钥路径就说明这条连接是按密钥存的，否则按密码。
    authType: source.privateKeyPath ? "key" : "password",
    // 密码永远不回显：编辑时留空表示"沿用已保存的凭据"。
    password: "",
    keyPath: source.privateKeyPath || "",
    defaultRoot: source.defaultRoot || "",
    strictHostKey: source.strictHostKey === true,
  };
}

interface SshConnectionFieldsProps {
  values: SshConnectionValues;
  onChange: (patch: Partial<SshConnectionValues>) => void;
  disabled?: boolean;
}

/** 六个 `setting-row-item`：主机/用户名/认证方式/凭据/默认目录/严格 Host Key。 */
export const SshConnectionFields: React.FC<SshConnectionFieldsProps> = ({ values, onChange, disabled }) => (
  <>
    <div className="setting-row-item">
      <div className="setting-label-block">
        <span className="setting-title">主机地址 (Host)</span>
        <span className="setting-desc">IP 地址与端口号</span>
      </div>
      <div className="setting-control-block">
        <Input
          size="sm"
          value={values.host}
          disabled={disabled}
          onChange={(e) => onChange({ host: e.target.value })}
          placeholder="192.168.1.100"
          style={{ flex: 1 }}
        />
        <Input
          size="sm"
          value={values.port}
          disabled={disabled}
          onChange={(e) => onChange({ port: e.target.value })}
          placeholder="22"
          style={{ width: 62 }}
        />
      </div>
    </div>

    <div className="setting-row-item">
      <div className="setting-label-block">
        <span className="setting-title">登录用户名</span>
        <span className="setting-desc">远程机器系统账户</span>
      </div>
      <div className="setting-control-block">
        <Input
          size="sm"
          value={values.username}
          disabled={disabled}
          onChange={(e) => onChange({ username: e.target.value })}
          placeholder="root / ubuntu / android"
          style={{ width: "100%" }}
        />
      </div>
    </div>

    <div className="setting-row-item">
      <div className="setting-label-block">
        <span className="setting-title">认证方式</span>
        <span className="setting-desc">密码或私钥文件</span>
      </div>
      <div className="setting-control-block">
        <Select
          size="sm"
          value={values.authType}
          onChange={(val) => onChange({ authType: val as "password" | "key" })}
          options={[
            { label: "密码认证", value: "password" },
            { label: "私钥文件 (~/.ssh/id_rsa)", value: "key" },
          ]}
          style={{ width: "100%" }}
        />
      </div>
    </div>

    {values.authType === "password" ? (
      <div className="setting-row-item">
        <div className="setting-label-block">
          <span className="setting-title">登录密码</span>
          <span className="setting-desc">保存到系统安全凭据库</span>
        </div>
        <div className="setting-control-block">
          <Input
            size="sm"
            type="password"
            value={values.password}
            disabled={disabled}
            onChange={(e) => onChange({ password: e.target.value })}
            placeholder="••••••••"
            style={{ width: "100%" }}
          />
        </div>
      </div>
    ) : (
      <div className="setting-row-item">
        <div className="setting-label-block">
          <span className="setting-title">私钥文件路径</span>
          <span className="setting-desc">OpenSSH 格式私钥路径</span>
        </div>
        <div className="setting-control-block">
          <Input
            size="sm"
            value={values.keyPath}
            disabled={disabled}
            onChange={(e) => onChange({ keyPath: e.target.value })}
            placeholder="~/.ssh/id_rsa"
            style={{ width: "100%" }}
          />
        </div>
      </div>
    )}

    <div className="setting-row-item">
      <div className="setting-label-block">
        <span className="setting-title">默认工作目录 (Default Root)</span>
        <span className="setting-desc">远程会话初始主目录</span>
      </div>
      <div className="setting-control-block">
        <Input
          size="sm"
          value={values.defaultRoot}
          disabled={disabled}
          onChange={(e) => onChange({ defaultRoot: e.target.value })}
          placeholder="/home/android"
          style={{ width: "100%" }}
        />
      </div>
    </div>

    <div className="setting-row-item">
      <div className="setting-label-block">
        <span className="setting-title">严格 Host Key 检查</span>
        <span className="setting-desc">校对 known_hosts 避免中间人伪造</span>
      </div>
      <div className="setting-control-block" style={{ justifyContent: "flex-end" }}>
        <Switch checked={values.strictHostKey} onChange={(checked) => onChange({ strictHostKey: checked })} />
      </div>
    </div>
  </>
);
