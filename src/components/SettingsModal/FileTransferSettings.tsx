import { useEffect, useState } from "react";
import { FolderOpen, RotateCcw } from "lucide-react";
import { Button, Input } from "@/components/ui";
import { invoke, openDirectory, formatInvokeError, isTauri } from "@/platform/runtime";
import { useSettingsNotification } from "./SettingsNotification";
import type { TransferSettings } from "@/views/CodeViewer/fileTransfers";
import "@/views/CodeViewer/file-transfers.css";

export function FileTransferSettings() {
  const [settings, setSettings] = useState<TransferSettings | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const notify = useSettingsNotification();
  useEffect(() => {
    let disposed = false;
    if (isTauri()) void invoke<TransferSettings>("get_code_transfer_settings").then((value) => { if (!disposed) setSettings(value); }).catch((reason) => { if (!disposed) setError(formatInvokeError(reason)); });
    return () => { disposed = true; };
  }, []);
  const save = async (directory: string | null) => {
    setBusy(true); setError("");
    try {
      await invoke("save_code_transfer_settings", { downloadDirectory: directory });
      setSettings(await invoke<TransferSettings>("get_code_transfer_settings"));
      notify(directory ? "已设置 SSH 文件下载位置" : "已恢复系统下载目录", "success");
    } catch (reason) { setError(formatInvokeError(reason)); }
    finally { setBusy(false); }
  };
  return <div className="setting-row-item">
    <div className="setting-label-block"><span className="setting-title">SSH 文件下载位置</span><span className="setting-desc">默认按“服务器 / 时间戳”保存到系统下载目录。自定义目录直接保存文件，同名时保留两份。上传和下载均先压缩再传输、解压。</span></div>
    <div className="setting-control-block theme-setting-control">
      <Input size="sm" readOnly aria-label="SSH 文件下载位置" value={settings?.downloadDirectory || settings?.defaultDirectory || ""} placeholder="系统下载目录" title={settings?.downloadDirectory || settings?.defaultDirectory} />
      <div className="theme-source-actions"><Button variant="secondary" size="xs" icon={<FolderOpen size={13} />} disabled={!settings || busy} onClick={() => { void openDirectory("选择 SSH 下载位置").then((directory) => { if (directory) return save(directory); }).catch((reason) => setError(formatInvokeError(reason))); }}>选择目录</Button><Button variant="subtle" size="xs" icon={<RotateCcw size={13} />} disabled={!settings?.downloadDirectory || busy} onClick={() => void save(null)}>恢复默认</Button></div>
      {error && <span role="alert" className="code-transfer-error">{error}</span>}
    </div>
  </div>;
}
