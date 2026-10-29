import { useCallback, useEffect, useRef, useState } from "react";
import { formatInvokeError, invoke } from "@/platform/runtime";
import type { DownloadFileStatus, DownloadRecord } from "./downloadHistory";
import { updateDownloadAvailability } from "./downloadHistoryStore";
import { useFileMutations } from "./useFileMutations";

export function useDownloadFiles(history: DownloadRecord[], enabled: boolean) {
  const [files, setFiles] = useState<Record<string, DownloadFileStatus>>({});
  const [error, setError] = useState("");
  const mounted = useRef(true);
  // StrictMode 会先 mount→unmount→mount，cleanup 里置 false 之后必须在这里再置回 true，
  // 否则探测结果永远不会写回 state，面板就只能在点击时才发现文件已丢失。
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const pathsKey = JSON.stringify(history.map((record) => record.path));
  const refresh = useCallback(async () => {
    if (!enabled) return;
    const checkedAt = Date.now();
    const checks = await invoke<DownloadFileStatus[]>("inspect_code_download_files", { paths: JSON.parse(pathsKey) });
    if (!mounted.current) return checks;
    updateDownloadAvailability(checks, checkedAt);
    setFiles(Object.fromEntries(checks.map((check) => [check.path, check])));
    return checks;
  }, [enabled, pathsKey]);
  const mutations = useFileMutations({ target: { kind: "local" }, root: "download-history", connected: true, loading: false,
    dirty: false, affectsDraft: () => false,
    onComplete: async (entry) => {
      const missing: DownloadFileStatus = { path: entry.path, status: "missing", isDirectory: false, isSymlink: false };
      updateDownloadAvailability([missing]);
      setFiles((current) => ({ ...current, [entry.path]: missing }));
    },
  });
  useEffect(() => {
    if (!enabled || mutations.busy) return;
    let disposed = false, pending = false;
    const check = async () => {
      if (pending || disposed || document.visibilityState === "hidden") return;
      pending = true;
      try { await refresh(); if (!disposed) setError(""); }
      catch (reason) { if (!disposed) setError(`检查下载文件失败：${formatInvokeError(reason)}`); }
      finally { pending = false; }
    };
    void check();
    const timer = window.setInterval(() => void check(), 5000);
    window.addEventListener("focus", check); document.addEventListener("visibilitychange", check);
    return () => { disposed = true; clearInterval(timer); window.removeEventListener("focus", check); document.removeEventListener("visibilitychange", check); };
  }, [enabled, refresh, mutations.busy]);
  const remove = (record: DownloadRecord) => {
    const info = files[record.path];
    if (!info || info.status !== "present" || record.availability === "missing") return;
    const separator = Math.max(record.path.lastIndexOf("/"), record.path.lastIndexOf("\\"));
    mutations.begin({ name: record.name, path: record.path, isDirectory: info.isDirectory,
      symlinkTarget: info.isSymlink ? "（链接）" : undefined }, "delete", record.path.slice(0, separator + 1));
  };
  return { files, error, remove, mutations, refresh };
}
