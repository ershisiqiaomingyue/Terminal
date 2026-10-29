import React, { useEffect, useState } from "react";
import { Button, Input, Switch } from "@/components/ui";
import { getEnvironmentNetworkSettings, saveEnvironmentNetworkSettings } from "@/platform/api";
import { formatInvokeError, isTauri } from "@/platform/runtime";
import { useSettingsNotification } from "./SettingsNotification";

export const NetworkPanel: React.FC = () => {
  const notify = useSettingsNotification();
  const [proxyUrl, setProxyUrl] = useState("");
  const [proxyEnabled, setProxyEnabled] = useState(false);
  const [proxyChildProcesses, setProxyChildProcesses] = useState(false);
  const [ready, setReady] = useState(false);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let active = true;
    if (!isTauri()) {
      notify("请在桌面应用中配置网络代理。", "error");
      return;
    }
    void getEnvironmentNetworkSettings()
      .then((settings) => {
        if (!active) return;
        setProxyUrl(settings.proxyUrl || "");
        setProxyEnabled(settings.proxyEnabled);
        setProxyChildProcesses(settings.proxyChildProcesses);
        setReady(true);
      })
      .catch((reason) => {
        if (active) notify(formatInvokeError(reason), "error");
      });
    return () => { active = false; };
  }, [notify]);

  const saveProxy = async () => {
    if (!ready || saving) return;
    setSaving(true);
    try {
      const settings = await saveEnvironmentNetworkSettings({
        proxyUrl: proxyUrl.trim() || null,
        proxyEnabled,
        proxyChildProcesses,
      });
      setProxyUrl(settings.proxyUrl || "");
      setProxyEnabled(settings.proxyEnabled);
      setProxyChildProcesses(settings.proxyChildProcesses);
      notify(settings.proxyEnabled ? "代理配置已保存并启用。" : "代理已关闭，配置已保存。", "success");
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="settings-pane">
      <header className="pane-header">
        <h2 className="pane-title">网络</h2>
      </header>
      <div>
        <div className="setting-group-title">网络访问</div>
        <div className="setting-card-group">
          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">启用代理</span>
              <span className="setting-desc">工具下载、GitHub 版本查询、主题、Skills、AI 和 MCP HTTP 请求使用此代理。</span>
            </div>
            <div className="setting-control-block" style={{ justifyContent: "flex-end" }}>
              <Switch aria-label="启用代理" checked={proxyEnabled} disabled={!ready || saving} onChange={setProxyEnabled} />
            </div>
          </div>
          <div className="setting-row-item settings-network-proxy-row">
            <div className="setting-label-block">
              <span className="setting-title">HTTP / HTTPS 代理</span>
              <span className="setting-desc">关闭代理会保留地址，重新启用时无需填写。</span>
            </div>
            <div className="setting-control-block">
              <Input
                size="sm"
                value={proxyUrl}
                disabled={!ready || saving}
                onChange={(event) => setProxyUrl(event.target.value)}
                placeholder="http://127.0.0.1:7890"
                aria-label="HTTP 或 HTTPS 代理地址"
                style={{ width: "100%" }}
              />
            </div>
          </div>
          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">本地命令使用代理</span>
              <span className="setting-desc">默认关闭。开启后，应用新启动的本地终端及 Git、npm 等命令也使用此代理。</span>
            </div>
            <div className="setting-control-block" style={{ justifyContent: "flex-end" }}>
              <Switch aria-label="本地命令使用代理" checked={proxyChildProcesses} disabled={!ready || saving || !proxyEnabled} onChange={setProxyChildProcesses} />
            </div>
          </div>
          <div className="setting-row-item settings-network-proxy-actions">
            <Button size="sm" variant="secondary" disabled={!ready || saving} onClick={() => { setProxyUrl(""); setProxyEnabled(false); }}>清空</Button>
            <Button size="sm" variant="primary" disabled={!ready} loading={saving} onClick={() => void saveProxy()}>保存</Button>
          </div>
        </div>
        <p className="settings-network-footnote">保存后生效；已有终端需重新打开。SSH / WSL 需单独配置，未开启本地命令代理时保留其原有网络设置。</p>
      </div>
    </section>
  );
};
