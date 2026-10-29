import { useCallback, useEffect, useRef, useState } from "react";
import { formatInvokeError, invoke, isTauri } from "@/platform/runtime";
import type { BrowserTarget } from "./useCodeBrowser";

export type GitFile = { path: string; status: string; additions: number | null; deletions: number | null };
type GitDirectory = { root: string; files: GitFile[] };
type GitDetails = { repoRoot: string | null; file: GitFile | null; author: string | null; committedAt: string | null; subject: string | null; diff: string };
const fileKey = (path: string, target: BrowserTarget) => target.kind === "local" ? path.replace(/\\/g, "/").toLowerCase() : path;

export function useGitBrowser(target: BrowserTarget, root: string, path: string, active: boolean, connected: boolean, autoRefresh: boolean, gitDirectories: string[]) {
  const [repositories, setRepositories] = useState<GitDirectory[]>([]);
  const [details, setDetails] = useState<GitDetails | null>(null);
  const [error, setError] = useState("");
  const [diffOpen, setDiffOpen] = useState(false);
  const generation = useRef(0);
  const pending = useRef(false);
  const fileRequest = useRef(0);
  const filePending = useRef(new Set<string>());
  const loadedScopes = useRef("");
  const currentPath = useRef(path);
  currentPath.current = path;
  const targetId = target.kind === "ssh" ? target.connectionId : "local";
  const directoriesKey = JSON.stringify(gitDirectories);
  const snapshotRepository = repositories.filter((item) => fileKey(path, target).startsWith(`${fileKey(item.root, target)}/`))
    .sort((left, right) => right.root.length - left.root.length)[0] || repositories[0] || null;
  const repository = details ? (details.repoRoot ? { root: details.repoRoot } : null) : snapshotRepository;
  useEffect(() => {
    generation.current++; pending.current = false; setRepositories([]); setDetails(null); setError(""); setDiffOpen(false);
    loadedScopes.current = "";
  }, [targetId, root, connected]);

  const loadDetails = useCallback(async (repositoryRoot: string, expectedGeneration: number) => {
    const path = currentPath.current;
    if (!path) { setDetails(null); return; }
    const request = fileRequest.current;
    const key = `${expectedGeneration}:${request}:${path}`;
    if (filePending.current.has(key)) return;
    filePending.current.add(key);
    try {
      const next = target.kind === "ssh"
        ? await invoke<GitDetails>("ssh_code_viewer_git_file", { connectionId: target.connectionId, root: repositoryRoot, path })
        : await invoke<GitDetails>("code_viewer_git_file", { path });
      if (generation.current === expectedGeneration && fileRequest.current === request && currentPath.current === path) { setDetails(next); setError(""); }
    } catch (error) { if (generation.current === expectedGeneration && fileRequest.current === request) setError(formatInvokeError(error)); }
    finally { filePending.current.delete(key); }
  }, [targetId]);

  const refresh = useCallback(async () => {
    if (!root || !active || !connected || pending.current || !isTauri()) return;
    const epoch = generation.current;
    pending.current = true;
    try {
      const scopes = [...new Set([root, ...gitDirectories])];
      const snapshots = new Map<string, GitDirectory>();
      for (const scope of scopes) {
        const existing = [...snapshots.values()].find((item) => fileKey(scope, target).startsWith(`${fileKey(item.root, target)}/`));
        if (existing && !gitDirectories.includes(scope)) continue;
        const next = target.kind === "ssh"
          ? await invoke<GitDirectory | null>("ssh_code_viewer_git_directory", { connectionId: target.connectionId, path: scope })
          : await invoke<GitDirectory | null>("code_viewer_git_directory", { path: scope });
        if (generation.current !== epoch) return;
        if (next) {
          // Retain the requested scope. Opening a small subtree must not
          // trigger a status/diff scan of the entire AOSP repository.
          const existing = snapshots.get(next.root);
          snapshots.set(next.root, existing ? { root: next.root, files: [...existing.files, ...next.files] } : next);
        }
      }
      if (generation.current !== epoch) return;
      const next = [...snapshots.values()];
      setRepositories(next); setError("");
      if (currentPath.current) await loadDetails(root, epoch);
    } catch (error) { if (generation.current === epoch) setError(formatInvokeError(error)); }
    finally { if (generation.current === epoch) pending.current = false; }
  }, [root, active, connected, targetId, loadDetails, directoriesKey]);

  useEffect(() => {
    if (!root || !active || !connected) return;
    const scopes = JSON.stringify([targetId, root, directoriesKey]);
    if (loadedScopes.current === scopes) return;
    loadedScopes.current = scopes;
    void refresh();
  }, [refresh, root, targetId, directoriesKey, active, connected]);
  useEffect(() => {
    setDetails(null); setDiffOpen(false); fileRequest.current++;
    if (!path || !root || !active || !connected || !isTauri()) return;
    const check = () => { if (document.visibilityState === "visible") void loadDetails(root, generation.current); };
    check();
    if (!autoRefresh) return;
    // Only the displayed file is polled. Tree-wide Git snapshots are loaded
    // when opening a directory or explicitly refreshing, never on a timer.
    const timer = window.setInterval(check, 5_000);
    return () => { clearInterval(timer); fileRequest.current++; };
  }, [path, root, active, connected, autoRefresh, loadDetails]);
  useEffect(() => () => { generation.current++; fileRequest.current++; }, []);
  const files = new Map(repositories.flatMap((repository) => repository.files).map((file) => [fileKey(file.path, target), file]));
  if (details && path) {
    files.delete(fileKey(path, target));
    if (details.file) files.set(fileKey(path, target), { ...details.file, path });
  }
  const fileStatus = (candidate: string) => files.get(fileKey(candidate, target));
  const directoryChanged = (path: string) => {
    const prefix = `${fileKey(path, target).replace(/\/$/, "")}/`;
    return [...files.keys()].some((file) => file.startsWith(prefix));
  };
  const refreshFile = () => active && connected && path && root ? loadDetails(root, generation.current) : Promise.resolve();
  return { repository, details, error, diffOpen, setDiffOpen, refresh, refreshFile, fileStatus, directoryChanged };
}
