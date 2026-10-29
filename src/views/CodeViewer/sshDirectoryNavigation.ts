import type { BrowserEntry } from "./useCodeBrowser";

export type PickerDirectory = { path: string; entries: BrowserEntry[] };

export function remoteBrowserDirectory(requested: string, result: PickerDirectory): PickerDirectory {
  // Keep the browsed alias: SSH resolves links physically, but “up” should
  // return to the folder containing that link rather than its remote target.
  const path = requested.startsWith("/") && !requested.split("/").some((part) => part === "." || part === "..")
    ? requested.replace(/\/+$/, "") || "/" : result.path;
  return { path, entries: result.entries.map((entry) => ({ ...entry, path: `${path.replace(/\/$/, "")}/${entry.name}` })) };
}

export function pickerDirectory(requested: string, result: PickerDirectory): PickerDirectory {
  const directory = remoteBrowserDirectory(requested, result);
  return { ...directory, entries: directory.entries.filter((entry) => entry.isDirectory)
    .sort((a, b) => a.name.localeCompare(b.name, "zh-CN", { numeric: true })) };
}

export function pickerParent(path: string): string {
  return path.replace(/\/+$/, "").replace(/\/[^/]*$/, "") || "/";
}
