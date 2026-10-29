import { useSyncExternalStore } from "react";
import type { FileTransfer } from "./fileTransfers";
import { completedDownload, mergeDownloadHistory, parseDownloadHistory, type DownloadRecord, type DownloadFileStatus } from "./downloadHistory";

const key = "android-tool.code-viewer.download-history.v1";
const listeners = new Set<() => void>();
let snapshot: DownloadRecord[] | undefined;
function read() { try { return parseDownloadHistory(localStorage.getItem(key)); } catch { return []; } }
function getSnapshot() { return snapshot ??= read(); }
function emit() { listeners.forEach((listener) => listener()); }
function onStorage(event: StorageEvent) {
  if (event.storageArea !== localStorage || (event.key !== key && event.key !== null)) return;
  snapshot = mergeDownloadHistory(getSnapshot(), read()); emit();
}
function subscribe(callback: () => void) {
  if (!listeners.size) window.addEventListener("storage", onStorage);
  listeners.add(callback);
  return () => { listeners.delete(callback); if (!listeners.size) window.removeEventListener("storage", onStorage); };
}
/** 删除一条历史记录（不动本地文件）。v3 §22 十六轮：历史条目右键菜单。 */
export function removeDownloadRecord(id: string) {
  const next = getSnapshot().filter((record) => record.id !== id);
  snapshot = next;
  try { localStorage.setItem(key, JSON.stringify({ version: 1, records: next })); } catch { /* 本会话内仍生效 */ }
  emit();
}

/** 清空全部历史记录（不动本地文件）。 */
export function clearDownloadHistory() {
  snapshot = [];
  try { localStorage.setItem(key, JSON.stringify({ version: 1, records: [] })); } catch { /* 本会话内仍生效 */ }
  emit();
}

export function rememberDownload(job: FileTransfer) {
  const record = completedDownload(job);
  if (!record) return;
  const next = mergeDownloadHistory(read(), getSnapshot(), [record]);
  if (JSON.stringify(next) === JSON.stringify(getSnapshot())) return;
  snapshot = next;
  try { localStorage.setItem(key, JSON.stringify({ version: 1, records: snapshot })); } catch { /* Keep this session's history when storage is unavailable. */ }
  emit();
}
export function useDownloadHistory() { return useSyncExternalStore(subscribe, getSnapshot); }

export function updateDownloadAvailability(checks: DownloadFileStatus[], checkedAt = Date.now()) {
  const byPath = new Map(checks.map((check) => [check.path, check.status]));
  const previous = mergeDownloadHistory(getSnapshot(), read());
  const next = previous.map((record) => {
    const availability = byPath.get(record.path);
    return availability && checkedAt >= (record.checkedAt || 0) ? { ...record, availability, checkedAt } : record;
  });
  if (JSON.stringify(next) === JSON.stringify(getSnapshot())) return;
  snapshot = next;
  try { localStorage.setItem(key, JSON.stringify({ version: 1, records: snapshot })); } catch { /* Retain in-memory status. */ }
  emit();
}
