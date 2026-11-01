import { useEffect, useState } from "react";
import { FolderOpen, RotateCcw } from "lucide-react";
import { Button, Input } from "rain-ui";
import { invoke, openDirectory, formatInvokeError, isTauri } from "@/platform/runtime";
import { usePortal } from "rain-ui";
import { SettingRow } from "./settingsUI";
import type { TransferSettings } from "@/views/CodeViewer/fileTransfers";

export function FileTransferSettings() {
  const [settings, setSettings] = useState<TransferSettings | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const portal = usePortal();
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
      portal(directory ? "已设置 SSH 文件下载位置" : "已恢复系统下载目录", "success");
    } catch (reason) { setError(formatInvokeError(reason)); }
    finally { setBusy(false); }
  };
  return (
    <SettingRow
      title="SSH 文件下载位置"
      desc="按“服务器 / 时间戳”保存到系统下载目录。"
      controlClassName="flex-wrap justify-end gap-1.5"
    >
      <Input
        readOnly
        size="sm"
        variant="surface"
        aria-label="SSH 文件下载位置"
        value={settings?.downloadDirectory || settings?.defaultDirectory || ""}
        placeholder="系统下载目录"
        title={settings?.downloadDirectory || settings?.defaultDirectory}
        className="w-[220px]"
      />
      <div className="flex items-center gap-1.5">
        <Button
          size="icon-sm"
          variant="outline"
          aria-label="选择目录"
          title="选择目录"
          icon={<FolderOpen size={15} />}
          disabled={!settings || busy}
          onClick={() => { void openDirectory("选择 SSH 下载位置").then((directory) => { if (directory) return save(directory); }).catch((reason) => setError(formatInvokeError(reason))); }}
        />
        <Button
          size="icon-sm"
          variant="outline"
          aria-label="恢复默认"
          title="恢复默认"
          icon={<RotateCcw size={15} />}
          disabled={!settings?.downloadDirectory || busy}
          onClick={() => void save(null)}
        />
      </div>
      {error && <span role="alert" className="w-full text-right text-xs text-destructive">{error}</span>}
    </SettingRow>
  );
}


