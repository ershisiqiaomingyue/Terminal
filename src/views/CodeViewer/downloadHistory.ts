import type { FileTransfer } from "./fileTransfers";

export type DownloadRecord = { id: string; name: string; server: string; path: string; finishedAt: number; bytes: number;
  availability?: "present" | "missing" | "unavailable"; checkedAt?: number };
export type DownloadFileStatus = { path: string; status: "present" | "missing" | "unavailable"; isDirectory: boolean; isSymlink: boolean; error?: string | null };
export const DOWNLOAD_HISTORY_LIMIT = 100;

/** Downloads are extracted into outputPath; the archive preserves the source basename. */
export function completedDownload(job: FileTransfer): DownloadRecord | null {
  if (job.direction !== "download" || job.status !== "completed" || !job.outputPath) return null;
  const name = job.name;
  if (!name || name === "." || name === ".." || /[\\/\0]/.test(name)) return null;
  const path = job.filePath || `${job.outputPath.replace(/[\\/]+$/, "")}/${name}`;
  return { id: job.id, name, server: job.server, path, finishedAt: job.finishedAt || job.startedAt, bytes: job.bytesTotal };
}

export function mergeDownloadHistory(...groups: readonly DownloadRecord[][]): DownloadRecord[] {
  const records = new Map<string, DownloadRecord>();
  for (const group of groups) for (const record of group) {
    const previous = records.get(record.id);
    records.set(record.id, previous && previous.path === record.path && (previous.checkedAt || 0) > (record.checkedAt || 0) ? { ...record, availability: previous.availability, checkedAt: previous.checkedAt } : record);
  }
  return [...records.values()].sort((a, b) => b.finishedAt - a.finishedAt).slice(0, DOWNLOAD_HISTORY_LIMIT);
}

export function parseDownloadHistory(value: string | null): DownloadRecord[] {
  try {
    const stored = JSON.parse(value || "null");
    if (stored?.version !== 1 || !Array.isArray(stored.records)) return [];
    return mergeDownloadHistory(stored.records.filter((item: unknown): item is DownloadRecord => {
      if (!item || typeof item !== "object") return false;
      const record = item as DownloadRecord;
      return [record.id, record.name, record.server, record.path].every((field) => typeof field === "string")
        && !!record.id && !!record.name && /^(?:[a-z]:[\\/]|\/|\\\\)/i.test(record.path) && !record.path.includes("\0")
        && (record.availability == null || ["present", "missing", "unavailable"].includes(record.availability))
        && (record.checkedAt == null || Number.isFinite(record.checkedAt))
        && Number.isFinite(record.finishedAt) && Number.isFinite(record.bytes) && record.bytes >= 0;
    }));
  } catch { return []; }
}
