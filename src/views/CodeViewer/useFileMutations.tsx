import { useEffect, useRef, useState } from "react";
import { Checkbox } from "rain-ui";
import { Input } from "rain-ui";
import { ConfirmDialog } from "rain-ui";
import { notify } from "@/platform/notifications";
import { formatInvokeError, invoke } from "@/platform/runtime";
import type { BrowserEntry, BrowserTarget } from "./useCodeBrowser";
import { actionNeedsName, buildMutationRequest } from "./mutationRequest";
import "./file-mutations.css";

export type MutationResult = { command: string; version: string; destination?: string | null; isDirectory: boolean; isSymlink: boolean; conflict: boolean };
export type MutationAction = "rename" | "delete" | "move" | "create-file" | "create-directory";
type Action = MutationAction;
type Selection = { entry: BrowserEntry; action: Action; target: BrowserTarget; root: string; destinationDirectory?: string };
export function useFileMutations({ target, root, connected, loading, dirty, affectsDraft, onComplete }: {
  target: BrowserTarget; root: string; connected: boolean; loading: boolean; dirty: boolean;
  affectsDraft: (path: string) => boolean;
  onComplete: (entry: BrowserEntry, result: MutationResult, action: Action) => Promise<void>;
}) {
  const [selection, setSelection] = useState<Selection | null>(null);
  const [name, setName] = useState("");
  const [recursive, setRecursive] = useState(false);
  const [overwrite, setOverwrite] = useState(false);
  const [preview, setPreview] = useState<MutationResult | null>(null);
  const [preparing, setPreparing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const executing = useRef(false);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const identity = target.kind === "ssh" ? `ssh:${target.connectionId}:${root}` : `local:${root}`;
  const identityRef = useRef(identity); identityRef.current = identity;
  useEffect(() => { if (!executing.current) { setSelection(null); } }, [identity]);
  // 重命名与新建都要把输入框里的名字送到后端；早先只给 rename 传 newName，
  // 新建时后端拿到 null 就直接报「请输入新名称」。
  const nameRequired = Boolean(selection && actionNeedsName(selection.action));
  const requestInput = selection ? { root: selection.root, path: selection.entry.path, action: selection.action, name, recursive, overwrite, expectedVersion: preview?.version || null, destinationDirectory: selection.destinationDirectory || null } : null;
  const request = requestInput ? buildMutationRequest(requestInput) : null;
  useEffect(() => {
    setPreview(null); setError("");
    if (!selection || !connected) { setPreparing(false); return; }
    // An empty naming field is an unfinished form, not a filesystem error.
    if (selection.action === "move" && !selection.destinationDirectory) { setPreparing(false); return; }
    if (nameRequired && !name.trim()) { setPreparing(false); return; }
    let cancelled = false;
    setPreparing(true);
    // Debounce naming input; only the explicit confirmation performs mutations.
    const timer = window.setTimeout(() => {
      const command = selection.target.kind === "ssh" ? "preview_ssh_code_viewer_mutation" : "preview_code_viewer_mutation";
      // 预览请求不能带 expectedVersion（此时还没有指纹），其余字段与执行请求完全同源。
      const previewRequest = buildMutationRequest({ root: selection.root, path: selection.entry.path, action: selection.action, name, recursive, overwrite, expectedVersion: null, destinationDirectory: selection.destinationDirectory || null });
      void invoke<MutationResult>(command, { request: previewRequest,
        ...(selection.target.kind === "ssh" ? { connectionId: selection.target.connectionId } : {}) })
        .then((value) => { if (!cancelled) setPreview(value); })
        .catch((reason) => { if (!cancelled) setError(formatInvokeError(reason)); })
        .finally(() => { if (!cancelled) setPreparing(false); });
    }, selection.action === "rename" || selection.action.startsWith("create-") ? 180 : 0);
    return () => { cancelled = true; clearTimeout(timer); };
  }, [selection, name, recursive, overwrite, connected]);
  const begin = (entry: BrowserEntry, action: Action, operationRoot = root) => {
    if (!connected || loading || executing.current) return;
    setName(action === "rename" ? entry.name : ""); setRecursive(false); setOverwrite(false); setPreview(null); setError("");
    setSelection({ entry, action, target, root: operationRoot });
  };
  const beginMove = (entry: BrowserEntry, destinationDirectory: string) => {
    if (!connected || loading || executing.current) return;
    setName(""); setRecursive(false); setOverwrite(false); setPreview(null); setError("");
    setSelection({ entry, action: "move", target, root, destinationDirectory });
  };
  const execute = async () => {
    if (!selection || !request || !preview || !connected || loading || preparing || executing.current) return;
    executing.current = true; setBusy(true); setError("");
    const currentIdentity = identity;
    const message = selection.action === "move" ? `移动 ${selection.entry.name}` : `${selection.action === "delete" ? "删除" : selection.action === "rename" ? "重命名" : selection.action === "create-file" ? "新建文件" : "新建文件夹"} ${creating ? name : selection.entry.name}`;
    try {
      const command = selection.target.kind === "ssh" ? "execute_ssh_code_viewer_mutation" : "execute_code_viewer_mutation";
      const result = await invoke<MutationResult>(command, { request: { ...request, acknowledged: true }, ...(selection.target.kind === "ssh" ? { connectionId: selection.target.connectionId } : {}) });
      if (!mounted.current || identityRef.current !== currentIdentity) return;
      setSelection(null);
      await onComplete(selection.entry, result, selection.action);
      const text = `${message}${selection.action === "rename" ? ` → ${name}` : selection.action === "move" && result.destination ? ` → ${result.destination}` : ""}：已完成`;
      notify(text, "success");
    } catch (reason) {
      if (mounted.current && identityRef.current === currentIdentity) {
        const text = formatInvokeError(reason); setError(text); notify(`${message}：失败 · ${text}`, "error");
      }
    } finally { executing.current = false; if (mounted.current) setBusy(false); }
  };
  const deleting = selection?.action === "delete";
  const moving = selection?.action === "move";
  const creating = selection?.action === "create-file" || selection?.action === "create-directory";
  const naming = selection?.action === "rename" || creating;
  const createLabel = selection?.action === "create-file" ? "新建文件" : "新建文件夹";
  const directory = preview ? preview.isDirectory : selection?.entry.isDirectory && selection.entry.symlinkTarget == null;
  const unsaved = Boolean(selection && !creating && dirty && affectsDraft(selection.entry.path));
  const conflict = Boolean(moving && preview?.conflict);
  const overwriteReady = !conflict || overwrite;
  const destinationName = selection?.destinationDirectory?.split(/[\\/]/).filter(Boolean).pop() || "目标文件夹";
  // Present the operation, not transport guards or Rust implementation details.
  const operation = selection?.target.kind !== "ssh" ? "" : deleting
    ? directory ? recursive ? "rm -r" : "rmdir" : "rm"
    : selection.action === "rename" ? "mv -nT" : moving ? overwrite ? "rm -rf && mv -nT" : "mv -nT" : selection.action === "create-directory" ? "mkdir" : "创建文件";
  return {
    begin, beginMove, busy,
    dialog: selection && <ConfirmDialog open className="code-mutation-confirm" key={`${selection.action}:${selection.entry.path}`} busy={busy}
      confirmDisabled={preparing || !connected || loading || !preview || !overwriteReady} acknowledgementKey={`${recursive}:${name}:${overwrite}`}
      title={deleting ? "危险操作：永久删除" : moving ? `确认移动${selection.entry.isDirectory ? "文件夹" : "文件"}` : creating ? createLabel : `确认重命名${selection.entry.isDirectory ? "文件夹" : "文件"}`}
      message={deleting ? `永久删除“${selection.entry.name}”？不进入回收站，不创建任何备份，无法恢复，后果自负。` : moving ? `将“${selection.entry.name}”移动到“${destinationName}”。目标位置有同名项时，只有在下方明确勾选覆盖后才会继续。` : creating ? "输入名称，同名项不会被覆盖。" : `为“${selection.entry.name}”输入新名称，同名项不会被覆盖。`}
      variant={deleting || overwrite ? "danger" : "primary"} confirmLabel={deleting ? "永久删除" : moving ? overwrite ? "覆盖并移动" : "确认移动" : creating ? "确认新建" : "确认重命名"} error={deleting || !naming ? error : undefined}
      acknowledgeLabel={deleting ? "我理解无备份、不可恢复，并自行承担后果" : undefined}
      onConfirm={() => { if (preview) void execute(); }} onCancel={() => { if (!busy) setSelection(null); }}>
      {unsaved && <p className="code-mutation-warning">此项包含当前编辑文件。{deleting ? "未保存的编辑也会被丢弃。" : "未保存的编辑会保留在新路径下。"}</p>}
      {conflict && <div className="code-mutation-conflict"><p>目标文件夹中已有同名项。普通确认已禁用；覆盖会永久替换目标，不会创建备份。</p><Checkbox label="我确认覆盖同名项（永久替换目标）" checked={overwrite} disabled={busy} onChange={setOverwrite} /></div>}
      {naming && <div className="code-mutation-name-field">
        <Input value={name} disabled={busy} aria-label="新名称" placeholder={creating ? selection.action === "create-file" ? "文件名称" : "文件夹名称" : "新名称"} aria-invalid={Boolean(error)} aria-describedby="code-mutation-name-error" onChange={(event) => setName(event.target.value)} />
        <p id="code-mutation-name-error" className="code-mutation-name-error" role="status" aria-live="polite">{error || "\u00a0"}</p>
      </div>}
      {deleting && directory && <Checkbox label="同时删除全部子目录和文件" checked={recursive} disabled={busy} onChange={setRecursive} />}
      {deleting && directory && <small>{recursive ? "将永久删除此目录内的全部内容，包括所有层级的子目录。" : "未勾选时仅删除空目录；非空目录会拒绝删除。"}</small>}
      {(!creating && (preview?.isSymlink || selection.entry.symlinkTarget != null)) && <p className="code-mutation-warning">这是软链接。操作仅作用于链接本身，不{deleting ? "删除" : moving ? "移动链接指向的目标" : "重命名"}链接指向的目标。</p>}
      {(deleting || moving) && operation && <small className="code-mutation-operation">操作：<code>{operation}</code></small>}
    </ConfirmDialog>,
  };
}
