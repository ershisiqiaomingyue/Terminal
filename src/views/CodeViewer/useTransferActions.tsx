import { useCallback, useEffect, useRef, useState } from "react";
import { Button, Input } from "@/components/ui";
import { ConfirmDialog } from "@/components/ConfirmDialog/ConfirmDialog";
import { invoke, formatInvokeError } from "@/platform/runtime";
import { recordTransfer, formatTransferBytes, useFileTransfers, type FileTransfer, type UploadConflict, type UploadPolicy } from "./fileTransfers";
import { downloadErrorMessage, downloadNeedsAuthorization, inspectAuthorizedDownload, type DownloadAccess, type DownloadInfo } from "./downloadAuthorization";
import type { BrowserTarget } from "./useCodeBrowser";

type Choice = UploadPolicy | "download" | "authorize" | null;
type Confirmation = { title: string; message: string; upload: boolean; authorize: boolean; resolve: (choice: Choice) => void };
export type TransferPreparation = { direction: "upload" | "download"; name: string };
export function useTransferActions(target: BrowserTarget, connected: boolean, onOpen: (direction: "upload" | "download") => void) {
  const [confirmation, setConfirmation] = useState<Confirmation | null>(null);
  const [busy, setBusy] = useState(false);
  const [preparing, setPreparing] = useState<TransferPreparation | null>(null);
  const [error, setError] = useState<{ direction: "upload" | "download"; message: string } | null>(null);
  const pending = useRef<Confirmation | null>(null);
  const checking = useRef(false);
  const mounted = useRef(true);
  const sudoPassword = useRef("");
  const ownDownloads = useRef(new Map<string, { path: string; connectionId: string; elevated: boolean }>());
  const jobs = useFileTransfers();
  const targetKey = target.kind === "ssh" ? `ssh:${target.connectionId}` : target.kind;
  const latestTarget = useRef({ key: targetKey, connected });
  latestTarget.current = { key: targetKey, connected };
  useEffect(() => {
    pending.current?.resolve(null); pending.current = null;
    sudoPassword.current = ""; setConfirmation(null);
  }, [targetKey, connected]);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; pending.current?.resolve(null); pending.current = null; sudoPassword.current = ""; }; }, []);
  const ask = (title: string, message: string, upload: boolean, authorize = false) => new Promise<Choice>((resolve) => {
    if (!mounted.current) { resolve(null); return; }
    sudoPassword.current = "";
    const value = { title, message, upload, authorize, resolve }; pending.current = value; setConfirmation(value);
  });
  const choose = (choice: Choice) => { if (choice !== "authorize") sudoPassword.current = ""; pending.current?.resolve(choice); pending.current = null; setConfirmation(null); };
  const download = useCallback(async (path: string, permissionRequired = false) => {
    if (target.kind !== "ssh" || !connected || checking.current) return;
    checking.current = true; setBusy(true); setError(null); setPreparing({ direction: "download", name: path.split("/").filter(Boolean).pop() || path }); onOpen("download");
    let access: DownloadAccess | undefined;
    const isCurrent = () => mounted.current && latestTarget.current.connected
      && latestTarget.current.key === `ssh:${target.connectionId}`;
    try {
      const prepared = await inspectAuthorizedDownload(
        (downloadAccess) => invoke<DownloadInfo>("inspect_code_download", { connectionId: target.connectionId, path, downloadAccess }),
        async () => {
          if (!isCurrent()) return null;
          if (await ask("申请远程读取权限", `当前 SSH 账号无权读取“${path}”。是否使用 sudo 重新检查并下载？优先使用 SSH 已保存的账号密码，仅授权本次读取和打包。若 sudo 使用不同密码，可在下方填写；不修改源文件权限。`, false, true) !== "authorize") return null;
          if (!isCurrent()) { sudoPassword.current = ""; return null; }
          const password = sudoPassword.current; sudoPassword.current = "";
          return { elevated: true, sudoPassword: password || undefined };
        }, permissionRequired,
      );
      if (!prepared) return;
      const { info } = prepared; access = prepared.access;
      if (!isCurrent()) return;
      const confirmedLarge = info.requiresConfirmation;
      if (confirmedLarge && await ask("确认下载大文件", `“${info.name}”约 ${formatTransferBytes(info.bytes)}，超过 1 GB。是否继续压缩并下载？`, false) !== "download") return;
      if (!isCurrent()) return;
      const job = await invoke<FileTransfer>("start_code_download", { connectionId: target.connectionId, path, confirmedLarge, downloadAccess: access });
      ownDownloads.current.set(job.id, { path, connectionId: target.connectionId, elevated: access.elevated });
      recordTransfer(job); if (mounted.current) onOpen("download");
    } catch (reason) { if (isCurrent()) setError({ direction: "download", message: downloadErrorMessage(formatInvokeError(reason)) }); }
    finally { sudoPassword.current = ""; if (access) access.sudoPassword = undefined; checking.current = false; if (mounted.current) { setBusy(false); setPreparing(null); } }
  }, [target, connected, onOpen]);
  useEffect(() => {
    if (busy || checking.current || target.kind !== "ssh" || !connected) return;
    for (const job of jobs) {
      const source = ownDownloads.current.get(job.id);
      if (!source || job.status === "running") continue;
      ownDownloads.current.delete(job.id);
      // A file can be stat-able during inspection but unreadable when tar opens
      // it. Offer the same authorization after that background failure, once.
      if (job.status === "failed" && !source.elevated && source.connectionId === target.connectionId
          && downloadNeedsAuthorization(job.error)) {
        void download(source.path, true); break;
      }
    }
  }, [jobs, busy, target, connected, download]);
  const upload = useCallback(async (paths: string[], destination: string) => {
    if (target.kind !== "ssh" || !connected || checking.current || !paths.length) return;
    checking.current = true; setBusy(true); setError(null); setPreparing({ direction: "upload", name: paths.length === 1 ? paths[0].split(/[\\/]/).pop() || paths[0] : `${paths.length} 个文件或目录` }); onOpen("upload");
    try {
      const conflicts = await invoke<UploadConflict[]>("inspect_code_upload", { connectionId: target.connectionId, paths, destination });
      if (!mounted.current) return;
      let policy: UploadPolicy = "reject";
      if (conflicts.length) {
        const choice = await ask("远程存在同名文件", `${conflicts.slice(0, 8).map((item) => item.name).join("、")}${conflicts.length > 8 ? ` 等 ${conflicts.length} 项` : ""} 已存在。覆盖会替换远程同名文件或目录；保留两份会自动添加 .1、.2。`, true);
        if (!choice || choice === "download" || choice === "authorize") return;
        policy = choice;
      }
      const job = await invoke<FileTransfer>("start_code_upload", { connectionId: target.connectionId, paths, destination, policy, conflicts });
      recordTransfer(job); if (mounted.current) onOpen("upload");
    } catch (reason) { if (mounted.current) setError({ direction: "upload", message: formatInvokeError(reason) }); }
    finally { checking.current = false; if (mounted.current) { setBusy(false); setPreparing(null); } }
  }, [target, connected, onOpen]);
  const pickUpload = async (destination: string, directory: boolean) => {
    try {
      const { open } = await import("@tauri-apps/plugin-dialog");
      const paths = await open({ title: directory ? "选择上传文件夹" : "选择上传文件", directory, multiple: true });
      if (paths && mounted.current) await upload(typeof paths === "string" ? [paths] : paths, destination);
    } catch (reason) { if (mounted.current) { setError({ direction: "upload", message: formatInvokeError(reason) }); onOpen("upload"); } }
  };
  return { download, upload, pickUpload, busy, preparing, error, dialog: confirmation && <ConfirmDialog open className="code-transfer-confirm" title={confirmation.title} message={confirmation.message}
    variant="primary" confirmLabel={confirmation.authorize ? "授权本次下载" : confirmation.upload ? "保留两份上传" : "继续下载"} cancelLabel="取消"
    onConfirm={() => choose(confirmation.authorize ? "authorize" : confirmation.upload ? "rename" : "download")} onCancel={() => choose(null)}
    additionalAction={confirmation.upload ? <Button size="sm" variant="danger" onClick={() => choose("overwrite")}>覆盖远程同名文件</Button> : undefined} >
    {confirmation.authorize && <Input key="download-sudo-password" type="password" aria-label="不同的 sudo 密码（可选）" placeholder="留空使用 SSH 已保存密码或免密 sudo" autoComplete="off" onChange={(event) => { sudoPassword.current = event.target.value; }} />}
    </ConfirmDialog> };
}
