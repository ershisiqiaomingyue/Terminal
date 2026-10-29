import { useSyncExternalStore } from "react";
import { invoke, isTauri } from "@/platform/runtime";
import { listen } from "@tauri-apps/api/event";
import { rememberDownload } from "./downloadHistoryStore";

export type FileTransfer = {
  id: string; connectionId: string; server: string; name: string;
  direction: "upload" | "download"; status: "running" | "completed" | "failed" | "cancelled";
  phase: "checking" | "compressing" | "transferring" | "extracting" | "completed";
  bytesDone: number; bytesTotal: number; startedAt: number; finishedAt?: number;
  outputPath?: string; filePath?: string; error?: string;
};
export type UploadConflict = { name: string; version: string };
export type TransferSettings = { downloadDirectory?: string; defaultDirectory: string };
export type UploadPolicy = "reject" | "overwrite" | "rename";

let snapshot: FileTransfer[] = [];
const subscribers = new Set<() => void>();
let generation = 0;
function emit() { subscribers.forEach((subscriber) => subscriber()); }
export function recordTransfer(job: FileTransfer) {
  const previous = snapshot.find((item) => item.id === job.id);
  // A start response can arrive after a progress/completion event.
  if (previous && (previous.finishedAt || (previous.phase !== "checking" && job.phase === "checking"))) return;
  snapshot = [job, ...snapshot.filter((item) => item.id !== job.id)].sort((a, b) => b.startedAt - a.startedAt).slice(0, 40);
  rememberDownload(job);
  emit();
}
function subscribe(callback: () => void) {
  subscribers.add(callback);
  if (subscribers.size === 1 && isTauri()) {
    const epoch = ++generation;
    let stop: (() => void) | undefined;
    let timer: ReturnType<typeof setInterval> | undefined;
    const sync = async () => {
      const before = snapshot;
      try {
        const jobs = await invoke<FileTransfer[]>("list_code_transfers");
        if (epoch !== generation) return;
        // Keep events received while the snapshot request was in flight.
        const newer = new Set(snapshot.filter((job) => before.find((old) => old.id === job.id) !== job).map((job) => job.id));
        for (const job of jobs) if (!newer.has(job.id)) recordTransfer(job);
      } catch { /* The next reconciliation recovers missed events. */ }
    };
    void listen<FileTransfer>("code-transfer-progress", (event) => { if (epoch === generation) recordTransfer(event.payload); })
      .then((unlisten) => { if (epoch !== generation) unlisten(); else stop = unlisten; })
      .finally(() => { if (epoch === generation) void sync(); }).catch(() => undefined);
    timer = setInterval(() => { if (document.visibilityState === "visible" && snapshot.some((job) => job.status === "running")) void sync(); }, 2000);
    cleanup = () => { ++generation; stop?.(); clearInterval(timer); };
  }
  return () => { subscribers.delete(callback); if (!subscribers.size) cleanup(); };
}
let cleanup = () => {};
export function useFileTransfers() { return useSyncExternalStore(subscribe, () => snapshot); }
export function formatTransferBytes(bytes: number) {
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(2)} GB`;
  if (bytes >= 1024 ** 2) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${bytes} B`;
}
