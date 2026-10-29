import { useCallback, useEffect, useRef, useState } from "react";
import { formatInvokeError, invoke, isTauri } from "@/platform/runtime";
import { notify, type NotificationKind } from "@/platform/notifications";
import { remoteBrowserDirectory } from "./sshDirectoryNavigation";
import { mutatedPath } from "./mutationState";
import { insertOpenFile, isMarkdownFile, nextActiveAfterClose } from "./fileTabs";

export type BrowserTarget = { kind: "local" } | { kind: "ssh"; connectionId: string; name: string };
export type BrowserEntry = { name: string; path: string; isDirectory: boolean; size?: number | null; kind?: string; symlinkTarget?: string | null };
// 每个打开过的文件各自持有一份内容与草稿，标签之间互不影响。
export type BrowserFile = { content: string; draft: string; previewing: boolean; readOnlyFile: boolean; fileVersion: string };
export type BrowserSnapshot = {
  target: BrowserTarget;
  root: string;
  activeFile?: string;
  // 多标签：标签顺序 + 每个标签的内容。旧快照只有单文件的 content/draft 字段，
  // 下面几个 legacy 字段保留下来只为能读回旧快照。
  openFiles?: string[];
  files?: Record<string, BrowserFile>;
  tree?: Record<string, BrowserEntry[]>;
  openFolders?: Record<string, boolean>;
  content?: string;
  draft?: string;
  previewing?: boolean;
  readOnlyFile?: boolean;
  fileVersion?: string;
  autoRefresh?: boolean;
  autoSave?: boolean;
  showHiddenFiles?: boolean;
};
export type RecentFolder = { path: string; openedAt: number; target?: BrowserTarget };
const HISTORY_KEY = "android-tool.code-viewer.history";
const MAX_LOADED_FOLDERS = 256;
const emptyFile = (previewing = false): BrowserFile => ({ content: "", draft: "", previewing, readOnlyFile: false, fileVersion: "" });
export function readHistory(): RecentFolder[] {
  try {
    const value = JSON.parse(localStorage.getItem(HISTORY_KEY) || "[]");
    return Array.isArray(value) ? value.filter((item) => typeof item?.path === "string" && typeof item?.openedAt === "number"
      && (!item.target || item.target.kind === "local" || (item.target.kind === "ssh" && typeof item.target.connectionId === "string" && typeof item.target.name === "string"))).slice(0, 12) : [];
  } catch { return []; }
}
export const nonTextFile = (path: string) => /\.(?:apk|aar|jar|so|dex|class|zip|7z|rar|gz|xz|png|jpe?g|webp|gif|ico|woff2?|ttf|otf|mp3|wav|mp4|avi|pdf)$/i.test(path);

// 旧快照只有一个文件：把它摊成一个标签，保证升级后原来的打开状态还在。
function restoredFiles(initial?: BrowserSnapshot) {
  if (!initial) return {} as Record<string, BrowserFile>;
  if (initial.files) return initial.files;
  if (!initial.activeFile) return {} as Record<string, BrowserFile>;
  return { [initial.activeFile]: { content: initial.content || "", draft: initial.draft || "", previewing: initial.previewing || false, readOnlyFile: initial.readOnlyFile || false, fileVersion: initial.fileVersion || "" } };
}

export function useCodeBrowser(initial?: BrowserSnapshot) {
  const [target, setTarget] = useState<BrowserTarget>(initial?.target || { kind: "local" });
  const [root, setRoot] = useState(initial?.root || "");
  const [tree, setTree] = useState<Record<string, BrowserEntry[]>>(initial?.tree || {});
  const [openFolders, setOpenFolders] = useState<Record<string, boolean>>(initial?.openFolders || {});
  const [files, setFiles] = useState<Record<string, BrowserFile>>(() => restoredFiles(initial));
  const [openFiles, setOpenFiles] = useState<string[]>(() => initial?.openFiles || (initial?.activeFile ? [initial.activeFile] : []));
  const [activeFile, setActiveFile] = useState(initial?.activeFile || "");
  const [loadingFiles, setLoadingFiles] = useState<Record<string, boolean>>({});
  const refreshNotice = useRef("");
  const notifyRefresh = useCallback((message: string, kind: NotificationKind) => {
    // Polling every five seconds must not repeat the same error or conflict.
    if (refreshNotice.current === message) return;
    refreshNotice.current = message;
    notify(message, kind);
  }, []);
  const [saveState, setSaveState] = useState<"idle" | "saving" | "saved" | "error">("idle");
  const saving = useRef(false);
  const [pending, setPending] = useState(0);
  const [recentFolders, setRecentFolders] = useState(readHistory);
  const [externalChanged, setExternalChanged] = useState(false);
  const [autoRefresh, setAutoRefresh] = useState(initial?.autoRefresh ?? true);
  const [autoSave, setAutoSave] = useState(initial?.autoSave ?? false);
  const [loadingFolders, setLoadingFolders] = useState<Record<string, boolean>>({});
  const generation = useRef(0);
  const fileRequest = useRef(0);
  const folderAccess = useRef(new Map<string, number>());
  const openFoldersRef = useRef(openFolders);
  openFoldersRef.current = openFolders;
  const foldersPending = useRef(new Set<string>());
  const refreshPending = useRef(new Set<string>());
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);

  // 派生出的「当前标签」视图，保持与改造前一致的对外字段。
  const active = files[activeFile];
  const content = active?.content ?? "";
  const draft = active?.draft ?? "";
  const previewing = active?.previewing ?? false;
  const readOnlyFile = active?.readOnlyFile ?? false;
  const fileVersion = active?.fileVersion ?? "";
  const fileLoading = Boolean(loadingFiles[activeFile]);
  const dirty = Boolean(activeFile && content !== draft);

  // 异步回调里要读最新值，避免把整个 files 塞进依赖导致回调频繁重建。
  const filesRef = useRef(files);
  filesRef.current = files;
  const activeFileRef = useRef(activeFile);
  activeFileRef.current = activeFile;
  const openFilesRef = useRef(openFiles);
  openFilesRef.current = openFiles;
  const pendingRef = useRef(pending);
  pendingRef.current = pending;
  const dirtyRef = useRef(dirty);
  dirtyRef.current = dirty;

  const patchFile = useCallback((path: string, patch: Partial<BrowserFile>) => {
    if (!path) return;
    setFiles((previous) => ({ ...previous, [path]: { ...(previous[path] || emptyFile()), ...patch } }));
  }, []);
  const setFileLoading = useCallback((path: string, loading: boolean) => {
    if (!path) return;
    setLoadingFiles((previous) => {
      if (Boolean(previous[path]) === loading) return previous;
      const next = { ...previous };
      if (loading) next[path] = true; else delete next[path];
      return next;
    });
  }, []);

  const invalidatePending = useCallback(() => {
    generation.current++; fileRequest.current++; setSaveState("idle"); setPending(0); setLoadingFiles({}); setLoadingFolders({});
  }, []);

  const clear = useCallback((next: BrowserTarget) => {
    generation.current++; fileRequest.current++;
    folderAccess.current.clear();
    openFoldersRef.current = {};
    setTarget(next); setRoot(""); setTree({}); setOpenFolders({}); setActiveFile("");
    setFiles({}); setOpenFiles([]); setLoadingFiles({}); setPending(0);
    setExternalChanged(false);
    setLoadingFolders({}); setSaveState("idle"); refreshNotice.current = "";
  }, []);
  const remember = (path: string, folderTarget: BrowserTarget) => setRecentFolders((previous) => {
    const identity = (item: RecentFolder) => `${item.target?.kind === "ssh" ? item.target.connectionId : "local"}:${item.path}`;
    const item = { path, target: folderTarget, openedAt: Date.now() };
    const next = [item, ...previous.filter((previous) => identity(previous) !== identity(item))].slice(0, 12);
    try { localStorage.setItem(HISTORY_KEY, JSON.stringify(next)); } catch { /* Browsing still works when storage is full. */ }
    return next;
  });

  const loadFolder = useCallback(async (path: string, reset = false, nextTarget?: BrowserTarget) => {
    if (!isTauri()) { notify("代码查看器需要在桌面应用中运行。", "warning"); return; }
    const destination = nextTarget || target;
    if (reset) clear(destination);
    const epoch = generation.current;
    const key = `${epoch}:${path}`;
    if (foldersPending.current.has(key)) return;
    foldersPending.current.add(key);
    setLoadingFolders((previous) => ({ ...previous, [path]: true }));
    setPending((count) => count + 1);
    try {
      const result = destination.kind === "ssh"
        ? remoteBrowserDirectory(path, await invoke<{ path: string; entries: BrowserEntry[] }>("list_ssh_code_viewer_directory", { connectionId: destination.connectionId, path }))
        : { path, entries: await invoke<BrowserEntry[]>("list_code_viewer_directory", { path }) };
      if (!mounted.current || generation.current !== epoch) return;
      if (reset) {
        folderAccess.current.clear();
        folderAccess.current.set(result.path, Date.now());
        setRoot(result.path); setTree({ [result.path]: result.entries }); setOpenFolders({ [result.path]: true });
        remember(result.path, destination);
      } else {
        // Keep the requested alias when expanding a link: its target may live
        // outside the root, but children must remain accessible through it.
        const entries = destination.kind === "ssh" && result.path !== path
          ? result.entries.map((entry) => ({ ...entry, path: `${path.replace(/\/$/, "")}/${entry.name}` }))
          : result.entries;
        folderAccess.current.set(path, Date.now());
        setTree((previous) => {
          const next = { ...previous, [path]: entries };
          if (Object.keys(next).length <= MAX_LOADED_FOLDERS) return next;
          const evictable = Object.keys(next)
            .filter((folder) => folder !== root && folder !== path && !openFoldersRef.current[folder])
            .sort((left, right) => (folderAccess.current.get(left) || 0) - (folderAccess.current.get(right) || 0));
          while (Object.keys(next).length > MAX_LOADED_FOLDERS && evictable.length) {
            const folder = evictable.shift()!;
            delete next[folder];
            folderAccess.current.delete(folder);
          }
          return next;
        });
        setOpenFolders((previous) => ({ ...previous, [path]: true }));
      }
    } catch (error) { if (mounted.current && generation.current === epoch) notify(formatInvokeError(error), "error"); }
    finally {
      foldersPending.current.delete(key);
      if (mounted.current && generation.current === epoch) {
        setPending((count) => Math.max(0, count - 1));
        setLoadingFolders((previous) => { const next = { ...previous }; delete next[path]; return next; });
      }
    }
  }, [target, clear]);

  const getFileVersion = useCallback((path: string) => target.kind === "ssh"
    ? invoke<string>("ssh_code_viewer_file_version", { connectionId: target.connectionId, root, path })
    : invoke<string>("code_viewer_file_version", { path }), [target, root]);

  const readFile = useCallback((path: string) => target.kind === "ssh"
    ? invoke<string>("read_ssh_code_viewer_file", { connectionId: target.connectionId, root, path })
    : invoke<string>("read_code_viewer_file", { path }), [target, root]);

  // 打开文件：已经开过的标签直接切过去，不再重复读盘（否则会丢掉未保存的草稿）。
  const openFile = async (entry: BrowserEntry) => {
    setSaveState("idle"); refreshNotice.current = ""; setExternalChanged(false);
    if (filesRef.current[entry.path]) { setActiveFile(entry.path); return; }
    setActiveFile(entry.path);
    // 新标签放最左，原来的整体往右推；超过 20 个时从最右淘汰（跳过有未保存改动的）。
    setOpenFiles((previous) => {
      const { open, evicted } = insertOpenFile(previous, entry.path, (path) => {
        const file = filesRef.current[path];
        return Boolean(file) && file.content !== file.draft;
      });
      if (evicted.length) {
        setFiles((current) => {
          const next = { ...current };
          for (const path of evicted) delete next[path];
          return next;
        });
        setLoadingFiles((current) => {
          const next = { ...current };
          for (const path of evicted) delete next[path];
          return next;
        });
      }
      return open;
    });
    // 默认就是编辑态；只有 Markdown 打开时先给渲染预览，可以一键切回编辑。
    patchFile(entry.path, emptyFile(isMarkdownFile(entry.path)));
    if (nonTextFile(entry.path) || (entry.kind && entry.kind !== "file")) {
      patchFile(entry.path, { readOnlyFile: true });
      notify(entry.kind === "symlink" ? "链接目标不存在或无法访问。" : "该文件不是普通文本文件，无法预览。", "warning");
      return;
    }
    const epoch = generation.current;
    const request = ++fileRequest.current;
    setFileLoading(entry.path, true);
    setPending((count) => count + 1);
    try {
      const version = await getFileVersion(entry.path);
      const next = await readFile(entry.path);
      if (!mounted.current || generation.current !== epoch || request !== fileRequest.current) return;
      patchFile(entry.path, { fileVersion: version, content: next, draft: next });
    } catch (error) {
      if (mounted.current && generation.current === epoch && request === fileRequest.current) {
        patchFile(entry.path, { readOnlyFile: true });
        notify(formatInvokeError(error), "error");
      }
    } finally {
      if (mounted.current && generation.current === epoch) {
        setPending((count) => Math.max(0, count - 1));
        if (request === fileRequest.current) setFileLoading(entry.path, false);
      }
    }
  };

  // 关闭标签：把内容一起丢掉；关掉当前标签时按「右邻 → 左邻」的顺序换到下一个。
  const closeFile = (path: string) => {
    setOpenFiles((previous) => {
      const next = previous.filter((item) => item !== path);
      if (activeFileRef.current === path) {
        const target = nextActiveAfterClose(previous, path, activeFileRef.current);
        setActiveFile(target);
        setSaveState("idle"); setExternalChanged(false); refreshNotice.current = "";
      }
      return next;
    });
    setFiles((previous) => {
      const next = { ...previous };
      delete next[path];
      return next;
    });
    setFileLoading(path, false);
  };

  // 一次性关掉多个标签（右键菜单的「关闭右侧/其他/全部」）：
  // 当前标签也在其中时，先往右、再往左找第一个没被关掉的标签接替。
  // 顺序在 setState 之外读 ref 算好，保持 updater 是纯函数。
  const closeFiles = (paths: readonly string[]) => {
    const closing = new Set(paths);
    if (!closing.size) return;
    const previous = openFilesRef.current;
    if (closing.has(activeFileRef.current)) {
      const index = previous.indexOf(activeFileRef.current);
      const after = previous.slice(index + 1).find((item) => !closing.has(item));
      const before = previous.slice(0, index).reverse().find((item) => !closing.has(item));
      setActiveFile(after || before || "");
      setSaveState("idle"); setExternalChanged(false); refreshNotice.current = "";
    }
    setOpenFiles(previous.filter((item) => !closing.has(item)));
    setFiles((current) => {
      const next = { ...current };
      for (const path of closing) delete next[path];
      return next;
    });
    for (const path of closing) setFileLoading(path, false);
  };

  // 把已打开的标签挪到第一个（从悬浮层里选中文件时用）。
  const promoteFile = useCallback((path: string) => {
    setOpenFiles((previous) => previous.includes(path) ? [path, ...previous.filter((item) => item !== path)] : previous);
  }, []);

  const setDraft = useCallback((value: string) => { patchFile(activeFileRef.current, { draft: value }); }, [patchFile]);
  const setPreviewing = useCallback((value: boolean) => { patchFile(activeFileRef.current, { previewing: value }); }, [patchFile]);

  const refreshFile = useCallback(async (force = false) => {
    const path = activeFileRef.current;
    const tab = filesRef.current[path];
    if (!path || !tab || tab.readOnlyFile || pendingRef.current > 0) return;
    if (force && dirtyRef.current && !window.confirm("磁盘上的新内容将替换未保存的修改，是否重新加载？")) return;
    const epoch = generation.current;
    const request = fileRequest.current;
    const key = `${epoch}:${request}`;
    if (refreshPending.current.has(key)) return;
    refreshPending.current.add(key);
    if (force) setFileLoading(path, true);
    try {
      const version = await getFileVersion(path);
      if (!mounted.current || generation.current !== epoch || fileRequest.current !== request) return;
      if (!force && version === filesRef.current[path]?.fileVersion) { refreshNotice.current = ""; return; }
      if (!force && dirtyRef.current) {
        setExternalChanged(true); notifyRefresh("磁盘文件已被修改，已保留你的未保存内容。请先复制修改，再重新加载文件。", "warning"); return;
      }
      const next = await readFile(path);
      if (!mounted.current || generation.current !== epoch || fileRequest.current !== request) return;
      // A local edit may begin while the read is in flight. Never replace it.
      if (!force && dirtyRef.current) { setExternalChanged(true); notifyRefresh("文件已变化，已保留你的未保存修改。", "warning"); return; }
      patchFile(path, { content: next, draft: next, fileVersion: version });
      setExternalChanged(false); setSaveState("idle");
      refreshNotice.current = "";
      if (force) notify("已重新加载文件", "success");
    } catch (error) {
      if (mounted.current && generation.current === epoch && fileRequest.current === request) {
        if (force) notify(`重新加载文件失败：${formatInvokeError(error)}`, "error");
        else notifyRefresh(`自动刷新检查失败：${formatInvokeError(error)}`, "error");
      }
    } finally {
      refreshPending.current.delete(key);
      if (force && mounted.current && generation.current === epoch && fileRequest.current === request) setFileLoading(path, false);
    }
  }, [getFileVersion, readFile, notifyRefresh, patchFile, setFileLoading]);

  const saveFile = useCallback(async () => {
    const path = activeFileRef.current;
    const tab = filesRef.current[path];
    if (!path || !tab || tab.content === tab.draft || tab.readOnlyFile || saving.current) return;
    const epoch = generation.current;
    const request = fileRequest.current;
    // 保存过程中用户可能切到别的文件、重新载入目录，甚至关掉整个视图。
    // 那种情况下这次保存的结果不再属于当前画面，但**状态必须落地**：
    // 之前这里直接 return，saveState 永远停在 "saving"，按钮一直转圈、
    // 界面一直显示「保存中…」，用户看到的就是「保存没有成功」。
    const stale = () => !mounted.current || generation.current !== epoch || fileRequest.current !== request;
    saving.current = true; setSaveState("saving");
    setPending((count) => count + 1);
    try {
      const version = await getFileVersion(path);
      if (version !== tab.fileVersion) {
        if (!stale()) {
          setSaveState("error"); setExternalChanged(true);
          notify("磁盘文件已被修改，未覆盖保存。请先复制修改，再重新加载文件。", "warning");
        }
        return;
      }
      const savedVersion = target.kind === "ssh"
        ? await invoke<string>("write_ssh_code_viewer_file", { connectionId: target.connectionId, root, path, content: tab.draft, expectedVersion: tab.fileVersion })
        : await invoke<string>("write_code_viewer_file", { path, content: tab.draft, expectedVersion: tab.fileVersion });
      // 磁盘上确实写成功了：版本号要回填到那个标签本身，即使用户已经切走。
      if (filesRef.current[path]) patchFile(path, { fileVersion: savedVersion, content: tab.draft });
      if (!stale()) {
        setExternalChanged(false); refreshNotice.current = ""; notify("文件已保存", "success"); setSaveState("saved");
      }
    } catch (error) {
      if (!stale()) {
        setSaveState("error");
        const message = formatInvokeError(error);
        if (message.includes("未覆盖保存")) setExternalChanged(true);
        notify(message, "error");
      }
    }
    finally {
      saving.current = false;
      // pending 是纯计数器，必须无条件归还；以前只在 generation 没变时才减一，
      // 一旦保存途中发生 clear()/invalidatePending() 就会永久多 1，之后所有保存
      // 都被入口处的 pending 判断挡掉（表现同样是「保存不了」）。
      setPending((count) => Math.max(0, count - 1));
      // 兜底：任何没走到终态的路径（上面的 stale 分支）都复位，绝不停在「保存中」。
      setSaveState((state) => (state === "saving" ? "idle" : state));
    }
  }, [getFileVersion, patchFile, target, root]);

  const toggleFolder = async (entry: BrowserEntry) => {
    if (openFolders[entry.path]) {
      openFoldersRef.current = { ...openFoldersRef.current, [entry.path]: false };
      setOpenFolders((previous) => ({ ...previous, [entry.path]: false }));
    }
    else if (tree[entry.path]) {
      folderAccess.current.set(entry.path, Date.now());
      openFoldersRef.current = { ...openFoldersRef.current, [entry.path]: true };
      setOpenFolders((previous) => ({ ...previous, [entry.path]: true }));
    }
    else await loadFolder(entry.path);
  };

  const snapshot: BrowserSnapshot = { target, root, tree, openFolders, activeFile, openFiles, files, content, draft, previewing, readOnlyFile, fileVersion, autoRefresh, autoSave };
  const applyMutation = async (entry: BrowserEntry, result: { destination?: string | null }) => {
    invalidatePending();
    const remap = (path: string) => mutatedPath(path, entry.path, result.destination, target.kind === "local");
    setTree((previous) => Object.fromEntries(Object.entries(previous).flatMap(([path, entries]) => {
      const nextPath = remap(path);
      return nextPath ? [[nextPath, entries.flatMap((item) => {
        const next = remap(item.path);
        return next ? [{ ...item, path: next, name: item.path === entry.path && result.destination ? result.destination.split(/[\\/]/).pop() || item.name : item.name }] : [];
      })]] : [];
    })));
    setOpenFolders((previous) => Object.fromEntries(Object.entries(previous).flatMap(([path, open]) => remap(path) ? [[remap(path), open]] : [])));
    // 标签也要跟着改名/移动，否则点标签会去读旧路径。
    const fileMoves = openFilesRef.current.flatMap((path) => {
      const next = remap(path);
      return next && next !== path ? [[path, next] as const] : [];
    });
    if (fileMoves.length) {
      setOpenFiles((previous) => previous.map((path) => fileMoves.find(([from]) => from === path)?.[1] || path));
      setFiles((previous) => Object.fromEntries(Object.entries(previous).map(([path, file]) => [fileMoves.find(([from]) => from === path)?.[1] || path, file])));
    }
    const nextFile = remap(activeFile);
    if (nextFile !== activeFile) {
      setActiveFile(nextFile);
      if (!nextFile) { setFiles({}); setOpenFiles([]); setExternalChanged(false); }
      else {
        try { patchFile(nextFile, { fileVersion: await getFileVersion(nextFile) }); }
        catch (error) { setExternalChanged(true); notify(`名称已修改，但检查新路径失败：${formatInvokeError(error)}`, "error"); }
      }
    }
    const parent = entry.path.replace(/[\\/][^\\/]+$/, "") || root;
    const destinationParent = result.destination?.replace(/[\\/][^\\/]+$/, "") || root;
    await loadFolder(parent);
    if (destinationParent !== parent) await loadFolder(destinationParent);
  };
  return { ...snapshot, target, root, tree, openFolders, activeFile, openFiles, files, content, draft, previewing, readOnlyFile, fileLoading,
    setDraft, setPreviewing, setActiveFile, promoteFile, loading: pending > 0, dirty, recentFolders, clear, loadFolder, openFile, closeFile, closeFiles, saveFile, toggleFolder, snapshot,
    invalidatePending, refreshFile, externalChanged, autoRefresh, setAutoRefresh, autoSave, setAutoSave, loadingFolders, saveState, applyMutation };
}
