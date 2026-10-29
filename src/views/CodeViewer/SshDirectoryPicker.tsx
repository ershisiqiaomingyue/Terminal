import { useCallback, useEffect, useRef, useState } from "react";
import { ArrowUp, ChevronRight, Folder, LoaderCircle, RefreshCw, Server } from "lucide-react";
import { Button, Input } from "@/components/ui";
import { SettingsEditDialog } from "@/components/SettingsModal/SettingsEditDialog";
import { formatInvokeError, invoke } from "@/platform/runtime";
import type { BrowserEntry } from "./useCodeBrowser";
import { pickerDirectory, pickerParent, type PickerDirectory } from "./sshDirectoryNavigation";

export function SshDirectoryPicker({ connectionId, server, initialPath, onClose, onSelect }: {
  connectionId: string; server: string; initialPath: string; onClose: () => void; onSelect: (path: string) => void;
}) {
  const [path, setPath] = useState(initialPath || "~");
  const [current, setCurrent] = useState("");
  const [directories, setDirectories] = useState<BrowserEntry[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const request = useRef(0);
  const cache = useRef(new Map<string, PickerDirectory>());
  const displayed = useRef("");
  const browse = useCallback(async (next: string, refresh = false) => {
    const destination = next.trim();
    if (!destination) return;
    const epoch = ++request.current;
    setError("");
    const show = (directory: PickerDirectory) => {
      displayed.current = directory.path;
      setCurrent(directory.path); setPath(directory.path); setDirectories(directory.entries);
    };
    const saved = cache.current.get(destination);
    if (saved && !refresh) { show(saved); setBusy(false); return; }
    setBusy(true);
    try {
      const result = await invoke<PickerDirectory>("list_ssh_code_viewer_directory", { connectionId, path: destination });
      if (epoch !== request.current) return;
      const directory = pickerDirectory(destination, result);
      cache.current.set(directory.path, directory);
      if (cache.current.size > 32) cache.current.delete(cache.current.keys().next().value!);
      show(directory);
    } catch (reason) { if (epoch === request.current) { setError(formatInvokeError(reason)); if (displayed.current) setPath(displayed.current); } }
    finally { if (epoch === request.current) setBusy(false); }
  }, [connectionId]);
  useEffect(() => { cache.current.clear(); displayed.current = ""; setCurrent(""); setDirectories([]); void browse(initialPath || "~"); return () => { ++request.current; }; }, [browse, initialPath]);
  const parent = pickerParent(current);
  return <SettingsEditDialog title="选择 SSH 文件夹" onClose={onClose} className="code-ssh-directory-picker">
    <div className="code-ssh-picker-server"><Server size={14} /><span>{server} · 全局 SSH 已连接</span></div>
    <form className="code-ssh-picker-location" onSubmit={(event) => { event.preventDefault(); void browse(path); }}>
      <Button className="code-ssh-picker-icon" variant="subtle" size="sm" icon={<ArrowUp size={14} />} disabled={busy || !current || current === "/"} aria-label="上一级目录" onClick={() => void browse(parent)} />
      <Input size="sm" aria-label="SSH 目录位置" value={path} placeholder="~/project 或 /home/user/project" onChange={(event) => setPath(event.target.value)} />
      <Button type="submit" size="sm" variant="secondary" disabled={busy || !path.trim()}>前往</Button>
      <Button className="code-ssh-picker-icon" variant="subtle" size="sm" icon={<RefreshCw size={14} />} disabled={busy} aria-label="刷新候选目录" onClick={() => void browse(current || path, true)} />
    </form>
    {error && <div className="code-download-error" role="alert">{error}</div>}
    <div className="code-ssh-picker-list custom-scrollbar" aria-label="SSH 候选文件夹" aria-busy={busy}>
      {busy ? <div className="code-ssh-picker-empty"><LoaderCircle size={18} className="inline-spin" />正在加载目录…</div> : directories.length ? directories.map((entry) => <div key={entry.path} className="code-ssh-picker-folder">
        <button type="button" className="code-ssh-picker-select" aria-label={`进入文件夹 ${entry.name}`} onClick={() => void browse(entry.path)}><Folder size={16} /><span><strong>{entry.name}</strong>{entry.symlinkTarget && <small>→ {entry.symlinkTarget}</small>}</span></button>
        <Button className="code-ssh-picker-icon" size="xs" variant="subtle" icon={<ChevronRight size={15} />} aria-label={`浏览文件夹 ${entry.name}`} title="浏览子目录" onClick={() => void browse(entry.path)} />
      </div>) : !error && <div className="code-ssh-picker-empty">此目录没有子文件夹</div>}
    </div>
    <footer className="code-ssh-picker-actions"><Button variant="secondary" size="sm" onClick={onClose}>取消</Button><Button variant="primary" size="sm" disabled={!current || busy} onClick={() => onSelect(current)}>选择当前目录</Button></footer>
  </SettingsEditDialog>;
}
