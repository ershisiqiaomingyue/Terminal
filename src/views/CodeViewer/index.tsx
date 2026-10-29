import { Check, Circle, LoaderCircle, AlertCircle, Download, Upload, ArrowDown, ArrowUp, Clock3, Code2, Copy, Eye, EyeOff, FolderOpen, Link2, ListTree, LockKeyhole, PanelLeft, RefreshCw, Save, Plus, X, GitBranch, GitCompareArrows } from "lucide-react";
import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { Button, Input } from "@/components/ui";
import { TaskStepStatusIcon } from "@/components/ui/TaskStepList";
import { notify } from "@/platform/notifications";
import { openDirectory, formatInvokeError, invoke } from "@/platform/runtime";
import { readToolWindowState, useToolWindowState } from "@/platform/toolWindowState";
import { useGlobalSshConnection } from "@/platform/globalSsh";
import { openGlobalSshMenu } from "@/platform/globalEnvironment";
import { isToolWindow, openToolWindowWithState } from "@/platform/toolWindows";
import { useCodeBrowser, type BrowserEntry as Entry, type BrowserSnapshot } from "./useCodeBrowser";
import { ContextMenu } from "@/components/ui/ContextMenu";
import { useGitBrowser } from "./useGitBrowser";
import { canMoveEntry, CodeFileTree } from "./CodeFileTree";
import { useTransferActions } from "./useTransferActions";
import { useUploadCompletion } from "./TransferProgress";
import { CodeBrowserControls } from "./CodeBrowserControls";
import { CodeViewerToolbar } from "./CodeViewerToolbar";
import { SshDirectoryPicker } from "./SshDirectoryPicker";
import { useNativeFileDrop } from "./useNativeFileDrop";
import type { FileTransfer } from "./fileTransfers";
import { CODE_VIEWER_OPEN_TAB_EVENT, CODE_VIEWER_FOLDER_DROP_EVENT } from "./events";
import { MarkdownFilePreview } from "./MarkdownFilePreview";
import { findFileMatches, highlightFileMatches, type FileSearchOptions } from "./fileSearch";
import { PaneResizeHandle } from "./PaneResizeHandle";
import { useFileMutations } from "./useFileMutations";
import { Pencil, Trash2 } from "lucide-react";
import { pathInMutation } from "./mutationState";
import { PROJECT_PANE } from "./paneWidth";
import { usePaneWidth } from "./usePaneWidth";
import { FileTabStrip, type FileTabItem } from "./FileTabStrip";
import { CodeHighlightEditor } from "./CodeHighlightEditor";
import { isMarkdownFile, projectFilePath, projectName as projectNameOf, tabTitles, fileName, type TabCloseAction } from "./fileTabs";
import "../ArchiveInspector/archive-inspector.css";
import "./code-viewer.css";

function folderName(path: string) { return path.split(/[\\/]/).filter(Boolean).pop() || path; }
export function CodeViewerView({ initialState, active = true }: { initialState?: BrowserSnapshot; active?: boolean }) {
  const [initial] = useState(() => initialState || readToolWindowState<BrowserSnapshot>("code-viewer"));
  const browser = useCodeBrowser(initial);
  const { target, root, tree, openFolders, activeFile, openFiles, files, draft, previewing, readOnlyFile, fileLoading, loading, dirty, recentFolders,
    setDraft, setPreviewing, setActiveFile, promoteFile, loadFolder, openFile: readFile, closeFile, closeFiles, saveFile, toggleFolder, clear } = browser;
  const { refreshFile, externalChanged, autoRefresh, setAutoRefresh, autoSave, setAutoSave, invalidatePending } = browser;
  const instanceId = useId();
  const projectId = `${instanceId}-project`;
  const outlineId = `${instanceId}-outline`;
  const [directoryMenu, setDirectoryMenu] = useState<{ entry: Entry; x: number; y: number; trigger: HTMLElement } | null>(null);
  const [showHiddenFiles, setShowHiddenFiles] = useState(() => {
    if (initial?.showHiddenFiles != null) return initial.showHiddenFiles;
    try { return localStorage.getItem("android-tool.code-viewer.show-hidden-ssh") === "true"; } catch { return false; }
  });
  const [draggedTreeEntry, setDraggedTreeEntry] = useState<Entry | null>(null);
  const closeDirectoryMenu = useCallback(() => setDirectoryMenu(null), []);
  const globalSsh = useGlobalSshConnection();
  const [remotePickerOpen, setRemotePickerOpen] = useState(false);
  const closeRemotePicker = useCallback(() => setRemotePickerOpen(false), []);
  const sshConnected = Boolean(globalSsh.connection && globalSsh.progress?.status === "connected");
  const targetConnected = target.kind === "local" || (globalSsh.connection?.id === target.connectionId && sshConnected);
  const [transferPopup, setTransferPopup] = useState<"upload" | "download" | null>(null);
  const openTransferPopup = useCallback((direction: "upload" | "download") => setTransferPopup(direction), []);
  const closeTransferPopup = useCallback(() => setTransferPopup(null), []);
  const transfers = useTransferActions(target, targetConnected, openTransferPopup);
  useEffect(() => { if (!active) closeTransferPopup(); }, [active, closeTransferPopup]);
  const gitDirectories = useMemo(() => Object.entries(tree).filter(([, entries]) => entries.some((entry) => entry.name === ".git")).map(([path]) => path), [tree]);
  const git = useGitBrowser(target, root, activeFile, active, targetConnected, autoRefresh, gitDirectories);
  const mutations = useFileMutations({ target, root, connected: targetConnected, loading, dirty,
    affectsDraft: (path) => pathInMutation(activeFile, path, target.kind === "local"),
    onComplete: async (entry, result, action) => { if (action.startsWith("create-")) await loadFolder(entry.path); else await browser.applyMutation(entry, result); await git.refresh(); } });
  const gitFile = git.fileStatus(activeFile);
  useEffect(() => {
    if (active && git.error) notify(`Git：${git.error}`, "error");
  }, [active, git.error]);
  useEffect(() => {
    if (active && root && target.kind === "ssh" && globalSsh.loaded && !targetConnected) {
      notify("原 SSH 连接已断开或切换；请通过全局连接重新连接，或打开当前服务器的目录。", "warning");
    }
  }, [active, root, target.kind, globalSsh.loaded, targetConnected]);
  useEffect(() => {
    if (target.kind === "ssh" && globalSsh.loaded && !targetConnected) invalidatePending();
  }, [target.kind, targetConnected, globalSsh.loaded, invalidatePending]);
  useEffect(() => {
    if (!active || !autoRefresh || !activeFile || !targetConnected || readOnlyFile) return;
    const check = () => { if (document.visibilityState === "visible") void refreshFile(); };
    const timer = window.setInterval(check, 5_000);
    const onVisibility = () => { if (document.visibilityState === "visible") check(); };
    document.addEventListener("visibilitychange", onVisibility);
    return () => { clearInterval(timer); document.removeEventListener("visibilitychange", onVisibility); };
  }, [active, autoRefresh, activeFile, targetConnected, readOnlyFile, refreshFile]);
  useToolWindowState("code-viewer", () => browser.snapshot, active);
  const openNewTab = (path = root) => {
    const snapshot: BrowserSnapshot = path === root ? { ...browser.snapshot, showHiddenFiles, tree: undefined, openFolders: undefined, activeFile: undefined, content: undefined, draft: undefined, fileVersion: undefined } : { target, root: path, showHiddenFiles };
    if (isToolWindow()) void openToolWindowWithState("code-viewer", "代码查看", { "code-viewer": snapshot }).catch((error) => notify(formatInvokeError(error), "error"));
    else window.dispatchEvent(new CustomEvent(CODE_VIEWER_OPEN_TAB_EVENT, { detail: snapshot }));
  };
  const selectSource = (kind: string) => {
    if (kind === target.kind) return;
    if (dirty && !window.confirm("当前文件有未保存的修改，切换来源会放弃修改，是否继续？")) return;
    if (kind === "local") { clear({ kind: "local" }); }
    else if (globalSsh.connection && sshConnected) {
      const next = { kind: "ssh" as const, connectionId: globalSsh.connection.id, name: globalSsh.connection.name };
      clear(next);
    } else { clear({ kind: "ssh", connectionId: "", name: "" }); notify("请通过右上角全局 SSH 连接服务器。", "info"); }
  };
  useEffect(() => { if (!active || !sshConnected) closeRemotePicker(); }, [active, sshConnected, closeRemotePicker]);
  useEffect(() => {
    if (initial?.root && !initial.tree) void loadFolder(initial.root, true, initial.target);
  }, []);
  const browseRemote = (path: string) => {
    if (!globalSsh.connection || !sshConnected) { openGlobalSshMenu(); return; }
    if (dirty && !window.confirm("当前文件有未保存的修改，打开其他目录会放弃修改，是否继续？")) return;
    closeRemotePicker(); void loadFolder(path, true, { kind: "ssh", connectionId: globalSsh.connection.id, name: globalSsh.connection.name });
  };
  const openRemotePicker = () => { if (!globalSsh.connection || !sshConnected) openGlobalSshMenu(); else setRemotePickerOpen(true); };
  const [historyOpen, setHistoryOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const closeSettings = useCallback(() => setSettingsOpen(false), []);
  useEffect(() => { if (!active) closeSettings(); }, [active, closeSettings]);
  const [treeQuery, setTreeQuery] = useState("");
  const [findOpen, setFindOpen] = useState(false);
  const [findQuery, setFindQuery] = useState("");
  const [findOptions, setFindOptions] = useState<FileSearchOptions>({});
  const [matchIndex, setMatchIndex] = useState(0);
  const [previewMatches, setPreviewMatches] = useState({ count: 0, truncated: false, error: "" });
  const [outlineOpen, setOutlineOpen] = useState(true);
  const [projectOpen, setProjectOpen] = useState(true);
  const bodyElement = useRef<HTMLDivElement>(null);
  const projectPane = usePaneWidth(bodyElement, Boolean(root), "android-tool.code-viewer.project-width", PROJECT_PANE);
  const contentElement = useRef<HTMLDivElement>(null);
  const editorElement = useRef<HTMLTextAreaElement | null>(null);
  // 回调必须稳定：CodeHighlightEditor 用它上报 textarea，内联箭头每次渲染都会换新引用。
  const bindEditor = useCallback((element: HTMLTextAreaElement | null) => { editorElement.current = element; }, []);
  const findInput = useRef<HTMLInputElement>(null);
  const chooseFolder = async () => {
    if (dirty && !window.confirm("当前文件有未保存的修改，打开其他目录会放弃修改，是否继续？")) return;
    try {
      const path = await openDirectory("选择代码文件夹");
      if (path) await loadFolder(path, true, { kind: "local" });
    } catch (error) { notify(`打开文件夹失败：${formatInvokeError(error)}`, "error"); }
  };
  const openFile = (entry: Entry) => {
    // 多标签：切文件不再需要「放弃未保存修改」的确认，草稿留在各自标签里。
    setFindQuery(""); setMatchIndex(0); void readFile(entry);
  };
  const selectFile = (path: string) => {
    if (path === activeFile) return;
    setFindQuery(""); setMatchIndex(0); setFindOpen(false); setActiveFile(path);
  };
  // 从悬浮层里选中的文件要挪到标签条第一个；直接点条上的标签则保持原位置。
  const selectFileFromOverflow = (path: string) => {
    setFindQuery(""); setMatchIndex(0); setFindOpen(false);
    promoteFile(path);
    setActiveFile(path);
  };
  const closeTab = (path: string) => {
    const tab = files[path];
    if (tab && tab.content !== tab.draft && !window.confirm(`「${fileName(path)}」有未保存的修改，关闭后会丢失，是否继续？`)) return;
    if (path === activeFile) { setFindQuery(""); setMatchIndex(0); setFindOpen(false); }
    closeFile(path);
  };
  // 右键菜单的批量关闭：未保存的改动只提示一次，避免连弹好几个确认框。
  const closeTabs = (paths: readonly string[], action: TabCloseAction) => {
    const dirtyTabs = paths.filter((path) => { const tab = files[path]; return Boolean(tab) && tab.content !== tab.draft; });
    if (dirtyTabs.length) {
      const names = dirtyTabs.map((path) => `「${fileName(path)}」`).join("、");
      const label = action === "all" ? "关闭全部标签" : action === "right" ? "关闭右侧标签" : "关闭其他标签";
      if (!window.confirm(`${label}：${names} 有未保存的修改，关闭后会丢失，是否继续？`)) return;
    }
    if (paths.includes(activeFile)) { setFindQuery(""); setMatchIndex(0); setFindOpen(false); }
    closeFiles(paths);
  };
  const onLocalDrop = useCallback((path: string) => {
    if (dirty && !window.confirm("当前文件有未保存的修改，打开其他目录会放弃修改，是否继续？")) return;
    void loadFolder(path, true, { kind: "local" });
  }, [dirty, loadFolder]);
  const dropDirectory = useNativeFileDrop({ active, target, connected: targetConnected, root, onLocal: onLocalDrop, onUpload: transfers.upload });
  useEffect(() => {
    if (!active) return;
    const onDrop = (event: Event) => {
      const path = (event as CustomEvent<string>).detail;
      if (typeof path === "string" && path && target.kind === "local") onLocalDrop(path);
    };
    window.addEventListener(CODE_VIEWER_FOLDER_DROP_EVENT, onDrop);
    return () => window.removeEventListener(CODE_VIEWER_FOLDER_DROP_EVENT, onDrop);
  }, [active, target.kind, onLocalDrop]);
  const onUploadComplete = useCallback((job: FileTransfer) => {
    if (target.kind !== "ssh" || job.connectionId !== target.connectionId || !job.outputPath || !targetConnected) return;
    if (tree[job.outputPath]) void loadFolder(job.outputPath);
  }, [target, targetConnected, tree, loadFolder]);
  useUploadCompletion(onUploadComplete);
  useEffect(() => { const onKeyDown = (event: KeyboardEvent) => { if (active && (event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "s" && activeFile) { event.preventDefault(); if (targetConnected && !externalChanged) void saveFile().then(() => git.refreshFile()); } }; window.addEventListener("keydown", onKeyDown); return () => window.removeEventListener("keydown", onKeyDown); }, [active, activeFile, saveFile, targetConnected, externalChanged, git.refreshFile]);
  // 自动保存：停止输入 1.2 秒后写入磁盘。冲突时由 saveFile 自己提示并放弃覆盖。
  useEffect(() => {
    if (!active || !autoSave || !dirty || !activeFile || readOnlyFile || loading || externalChanged || !targetConnected) return;
    const timer = window.setTimeout(() => { void saveFile().then(() => git.refreshFile()); }, 1_200);
    return () => window.clearTimeout(timer);
  }, [active, autoSave, dirty, activeFile, readOnlyFile, loading, externalChanged, targetConnected, draft, saveFile, git.refreshFile]);
  const activeName = useMemo(() => folderName(activeFile), [activeFile]);
  const markdownFile = isMarkdownFile(activeFile);
  // 标签条：标题只取文件名，重名时补父目录；未保存改动在名字右侧标 *。
  const tabItems = useMemo<FileTabItem[]>(() => {
    const titles = tabTitles(openFiles);
    return openFiles.map((path, index) => {
      const tab = files[path];
      return { path, title: titles[index], dirty: Boolean(tab && tab.content !== tab.draft), readOnly: Boolean(tab?.readOnlyFile) };
    });
  }, [openFiles, files]);
  const rootName = useMemo(() => projectNameOf(root), [root]);
  const currentFilePath = useMemo(() => projectFilePath(root, activeFile), [root, activeFile]);
  // 默认就是编辑态（带语法高亮）；只有 Markdown 还保留一个预览开关。
  const previewMode = previewing && markdownFile;
  const sourceMatches = useMemo(() => previewMode ? { matches: [], truncated: false, error: "" } : findFileMatches(draft, findQuery, findOptions), [draft, findQuery, previewMode, findOptions]);
  const findError = previewMode ? previewMatches.error : sourceMatches.error;
  const matchCount = previewMode ? previewMatches.count : sourceMatches.matches.length;
  const truncatedMatches = previewMode ? previewMatches.truncated : sourceMatches.truncated;
  const currentMatch = Math.min(matchIndex, Math.max(0, matchCount - 1));
  useEffect(() => {
    if (!previewMode) return;
    const root = contentElement.current?.querySelector<HTMLElement>("[data-file-search-root]");
    const result = root ? highlightFileMatches(root, findQuery, findOptions) : { matches: [], truncated: false, error: "" };
    setPreviewMatches({ count: result.matches.length, truncated: result.truncated, error: result.error });
  }, [draft, previewMode, findQuery, readOnlyFile, findOptions, git.diffOpen]);
  useEffect(() => {
    if (!findQuery || !matchCount) return;
    if (previewMode) {
      const marks = contentElement.current?.querySelectorAll<HTMLElement>("mark[data-file-match]");
      marks?.forEach((mark) => mark.classList.toggle("is-current", Number(mark.dataset.fileMatch) === currentMatch));
      contentElement.current?.querySelector(`[data-file-match="${currentMatch}"]`)?.scrollIntoView({ block: "center", inline: "nearest" });
    } else {
      const match = sourceMatches.matches[currentMatch];
      const editor = editorElement.current;
      if (!editor || !match) return;
      editor.setSelectionRange(match.start, match.end);
      const line = draft.slice(0, match.start).split("\n").length - 1;
      editor.scrollTop = Math.max(0, line * parseFloat(getComputedStyle(editor).lineHeight) - editor.clientHeight / 2);
    }
  }, [findQuery, draft, previewMode, matchCount, currentMatch, sourceMatches]);
  useEffect(() => { if (findOpen) findInput.current?.focus(); }, [findOpen]);
  useEffect(() => {
    const onFind = (event: KeyboardEvent) => {
      if (active && (event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f" && activeFile && !readOnlyFile) { event.preventDefault(); setFindOpen(true); findInput.current?.focus(); }
    };
    window.addEventListener("keydown", onFind);
    return () => window.removeEventListener("keydown", onFind);
  }, [active, activeFile, readOnlyFile]);
  const moveMatch = (direction: number) => { if (matchCount) setMatchIndex((currentMatch + direction + matchCount) % matchCount); };
  const closeFind = () => { setFindOpen(false); setFindQuery(""); setMatchIndex(0); };
  const refreshDirectory = async () => {
    // Manual refresh includes expanded, already loaded folders. Auto-refresh
    // continues to read only the open file, without scanning the tree.
    const folders = [...new Set([root, ...Object.keys(openFolders).filter((path) => openFolders[path] && tree[path])])];
    for (let index = 0; index < folders.length; index += 4) {
      await Promise.allSettled(folders.slice(index, index + 4).map((path) => loadFolder(path)));
    }
    void git.refresh();
  };
  const transferPanel = { preparing: transfers.preparing,
    onUploadFiles: () => void transfers.pickUpload(root, false), onUploadFolder: () => void transfers.pickUpload(root, true),
    uploadDisabled: !targetConnected || transfers.busy || !root };
  const copyPath = async (path: string, label: string) => {
    try { await navigator.clipboard.writeText(path); notify(`已复制${label}`, "success"); }
    catch { notify(`无法写入剪贴板，请手动复制：${path}`, "error"); }
  };
  const toggleHiddenFiles = () => setShowHiddenFiles((value) => {
    const next = !value;
    try { localStorage.setItem("android-tool.code-viewer.show-hidden-ssh", String(next)); } catch { /* The current window still keeps the preference. */ }
    return next;
  });

  return <section className="archive-container code-viewer-container">
    <CodeViewerToolbar active={active} target={target} root={root} loading={loading} connected={targetConnected}
      sshConnected={sshConnected} sshId={globalSsh.connection?.id} directoryOpen={remotePickerOpen}
      autoRefresh={autoRefresh} autoSave={autoSave} hasFile={Boolean(activeFile)} projectName={rootName} filePath={currentFilePath}
      searchOpen={findOpen} settingsOpen={settingsOpen}
      onSearch={() => findOpen ? closeFind() : setFindOpen(true)} onSettingsToggle={() => setSettingsOpen((value) => !value)} onSettingsClose={closeSettings}
      onAutoRefresh={() => setAutoRefresh((value) => !value)} onAutoSave={() => setAutoSave((value) => !value)}
      onSource={selectSource} onLocalOpen={() => void chooseFolder()} onRemoteOpen={openRemotePicker} onSshMenu={openGlobalSshMenu}
      onRefresh={refreshDirectory} onNewTab={() => openNewTab()} onHistory={() => setHistoryOpen((value) => !value)}
      downloadOpen={transferPopup === "download"} onDownloadToggle={() => setTransferPopup((current) => current === "download" ? null : "download")} onDownloadClose={closeTransferPopup}
      transferPanel={{ ...transferPanel, preparationError: transfers.error?.direction === "download" ? transfers.error.message : undefined }} />
    {!root ? <button className="archive-empty-workbench code-viewer-dropzone code-viewer-pane-card glass-card-surface" type="button" onClick={() => target.kind === "ssh" ? openRemotePicker() : void chooseFolder()}><span className="archive-empty-icon-box"><FolderOpen size={28} /></span><h3>{target.kind === "ssh" ? "浏览 SSH 目录树" : "拖入代码文件夹"}</h3><p>{target.kind === "ssh" ? "使用全局 SSH 连接，在悬浮窗中选择文件夹。" : "目录按需加载，文本文件可编辑保存。"}</p></button> : <div ref={bodyElement} className={`archive-body-split code-viewer-body glass-card-surface${projectOpen ? "" : " is-project-hidden"}`} style={{ "--project-pane-width": `${projectPane.width}px` } as React.CSSProperties}>
      <aside id={projectId} className={`archive-aside-tree code-viewer-pane-card${dropDirectory === root ? " is-upload-target" : ""}${draggedTreeEntry && canMoveEntry(draggedTreeEntry, root) ? " is-move-target" : ""}`} data-code-directory={root} aria-label="工程文件" hidden={!projectOpen}>
        <CodeBrowserControls query={treeQuery} onQuery={setTreeQuery} ssh={target.kind === "ssh"}
          uploadOpen={transferPopup === "upload"} onUploadToggle={() => setTransferPopup((current) => current === "upload" ? null : "upload")} onUploadClose={closeTransferPopup}
          transferPanel={{ ...transferPanel, preparationError: transfers.error?.direction === "upload" ? transfers.error.message : undefined }}
          onMenu={(anchor) => { closeTransferPopup(); const rect = anchor.getBoundingClientRect(); setDirectoryMenu({ entry: { name: folderName(root), path: root, isDirectory: true }, x: rect.left, y: rect.bottom, trigger: anchor }); }} />
        <div className="archive-tree-scroll-list custom-scrollbar" onContextMenu={(event) => {
          // 目录树的空白处也应当能新建：命中行内按钮时交给行自己的菜单处理。
          if (!targetConnected || loading || mutations.busy) return;
          if ((event.target as HTMLElement).closest(".code-viewer-tree-item")) return;
          event.preventDefault();
          closeTransferPopup();
          setDirectoryMenu({ entry: { name: folderName(root), path: root, isDirectory: true }, x: event.clientX, y: event.clientY, trigger: event.currentTarget });
        }}><CodeFileTree dropDirectory={dropDirectory} root={root} tree={tree} openFolders={openFolders} loadingFolders={browser.loadingFolders} activeFile={activeFile} query={treeQuery} connected={targetConnected} metadataScope={`${target.kind}:${target.kind === "ssh" ? target.connectionId : "local"}`} loadMetadata={(entry) => target.kind === "ssh"
          ? invoke("ssh_code_viewer_file_metadata", { connectionId: target.connectionId, root, path: entry.path })
          : invoke("code_viewer_file_metadata", { path: entry.path })} showHiddenFiles={target.kind !== "ssh" || showHiddenFiles} fileStatus={git.fileStatus} directoryChanged={git.directoryChanged} onOpen={openFile} onToggle={(entry) => void toggleFolder(entry)} onDirectoryMenu={(entry, event) => setDirectoryMenu({ entry, x: event.clientX, y: event.clientY, trigger: event.currentTarget })} onMove={mutations.beginMove} onDragEntryChange={setDraggedTreeEntry} />{!(tree[root] || []).some((entry) => target.kind !== "ssh" || showHiddenFiles || !entry.name.startsWith(".")) && <p className="code-viewer-tree-empty">此目录为空</p>}</div>
      </aside>
      {projectOpen && <PaneResizeHandle label="调整工程文件宽度" controls={projectId} width={projectPane.width} min={projectPane.bounds.min} max={projectPane.bounds.max} initial={PROJECT_PANE.initial} onResize={projectPane.resize} onCommit={projectPane.commit} />}
      <article className="archive-main-article code-viewer-main code-viewer-pane-card">
        <div className="archive-preview-titlebar code-viewer-tab-bar">
          <Button type="button" className="code-viewer-pane-toggle" variant="subtle" size="xs" icon={<PanelLeft size={14} strokeWidth={1.6} />} aria-label={projectOpen ? "隐藏工程文件" : "显示工程文件"} title={projectOpen ? "隐藏工程文件" : "显示工程文件"} aria-expanded={projectOpen} aria-controls={projectId} onClick={() => setProjectOpen((value) => !value)} />
          <FileTabStrip tabs={tabItems} activePath={activeFile} onSelect={selectFile} onSelectFromOverflow={selectFileFromOverflow} onClose={closeTab} onCloseMany={closeTabs} label="已打开的文件" />
          <div className="archive-preview-actions">
            {markdownFile && previewing && !readOnlyFile && <Button type="button" className={`code-viewer-pane-toggle${outlineOpen ? " is-active" : ""}`} variant="subtle" size="xs" icon={<ListTree size={14} strokeWidth={1.6} />} aria-label={outlineOpen ? "隐藏文档目录" : "显示文档目录"} title={outlineOpen ? "隐藏文档目录" : "显示文档目录"} aria-expanded={outlineOpen} aria-controls={outlineId} onClick={() => setOutlineOpen((value) => !value)} />}
            {markdownFile && !readOnlyFile && <Button type="button" className={`code-viewer-pane-toggle${previewing ? " is-active" : ""}`} variant="subtle" size="xs" icon={previewing ? <Code2 size={13} /> : <Eye size={13} />} aria-label={previewing ? "编辑 Markdown" : "预览 Markdown"} title={previewing ? "编辑 Markdown" : "预览 Markdown"} aria-pressed={previewing} onClick={() => setPreviewing(!previewing)} />}
            {/* 刷新已在第一行工具栏，这里只保留有改动时的保存按钮。 */}
            {dirty && !readOnlyFile && <Button variant="primary" size="xs" icon={<Save size={12} />} disabled={loading || externalChanged || !targetConnected} onClick={() => void saveFile().then(() => git.refreshFile())}>保存</Button>}
            {/* 状态只在有事情发生时出现：平时「已同步」不用占一个位置。 */}
            {activeFile && !readOnlyFile && !fileLoading && (externalChanged || browser.saveState === "error" || browser.saveState === "saving") && <span className={`code-file-state is-${externalChanged || browser.saveState === "error" ? "error" : "saving"}`} role="status" title={externalChanged ? "文件冲突" : undefined}>
              {externalChanged || browser.saveState === "error" ? <AlertCircle size={12} /> : <LoaderCircle className="inline-spin" size={12} />}
              {externalChanged ? "文件冲突" : browser.saveState === "error" ? "保存失败" : "保存中…"}
            </span>}
          </div>
        </div>
        {activeFile && git.repository && <div className="code-viewer-git-bar">
          <GitBranch size={13} /><span>{gitFile ? (gitFile.status === "??" ? "未跟踪" : "未提交") : "已提交"}</span>
          {gitFile?.additions != null && <em className="is-added">+{gitFile.additions}</em>}
          {gitFile?.deletions != null && <em className="is-deleted">−{gitFile.deletions}</em>}
          {git.details?.author && <span className="code-viewer-git-author" title={[git.details.author, git.details.committedAt, git.details.subject].filter(Boolean).join(" · ")}>最近提交：{git.details.author}</span>}
          {gitFile && gitFile.status !== "??" && <Button variant="subtle" size="xs" icon={<GitCompareArrows size={13} />} aria-pressed={git.diffOpen} onClick={() => git.setDiffOpen((value) => !value)}>{git.diffOpen ? "返回文件" : "改动对比"}</Button>}
          <Button variant="subtle" size="xs" icon={<RefreshCw size={13} />} aria-label="刷新 Git 状态" title="刷新 Git 状态" onClick={() => void git.refresh()} />
        </div>}
        <div className="code-viewer-file-content" ref={contentElement} aria-busy={fileLoading}>
        {/* 搜索条悬浮在正文右上角，不参与布局，不占用正文高度。 */}
        {findOpen && activeFile && !readOnlyFile && <div className="code-viewer-find" role="search" aria-label="文件内容搜索">
          <Input ref={findInput} size="sm" value={findQuery} aria-label="搜索文件内容" placeholder="搜索文件内容..." onChange={(event) => { setFindQuery(event.target.value); setMatchIndex(0); }} onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); moveMatch(event.shiftKey ? -1 : 1); } else if (event.key === "Escape") closeFind(); }} />
          {([{ key: "caseSensitive", label: "区分大小写", text: "Aa" }, { key: "wholeWord", label: "全词匹配", text: "Ab" }, { key: "regex", label: "正则表达式", text: ".*" }] as const).map((option) => <Button key={option.key} className="code-viewer-find-icon" variant="subtle" size="xs" aria-label={option.label} title={option.label} aria-pressed={Boolean(findOptions[option.key])} onClick={() => { setFindOptions((previous) => ({ ...previous, [option.key]: !previous[option.key] })); setMatchIndex(0); }}>{option.text}</Button>)}
          <span aria-live="polite">{findError || (matchCount ? `${currentMatch + 1}/${matchCount}${truncatedMatches ? "+" : ""}` : findQuery ? "无匹配" : "0/0")}</span>
          <Button className="code-viewer-find-icon" variant="subtle" size="xs" icon={<ArrowUp size={13} />} disabled={!matchCount} aria-label="上一个匹配" title="上一个匹配" onClick={() => moveMatch(-1)} />
          <Button className="code-viewer-find-icon" variant="subtle" size="xs" icon={<ArrowDown size={13} />} disabled={!matchCount} aria-label="下一个匹配" title="下一个匹配" onClick={() => moveMatch(1)} />
          <Button className="code-viewer-find-icon" variant="subtle" size="xs" icon={<X size={13} />} aria-label="关闭文件搜索" title="关闭文件搜索" onClick={closeFind} />
        </div>}
        {git.diffOpen ? <pre className="code-viewer-preview code-viewer-diff custom-scrollbar">{git.details?.diff ? git.details.diff.split("\n").map((line, index) => <span key={index} className={line.startsWith("+") ? "is-added" : line.startsWith("-") ? "is-deleted" : undefined}>{line}{"\n"}</span>) : "暂无可显示的差异"}</pre> : fileLoading ? <div className="code-viewer-file-loading" role="status" aria-live="polite"><TaskStepStatusIcon status="running" /><span>{target.kind === "ssh" ? "正在读取 SSH 文件…" : "正在读取文件…"}</span></div> :
        activeFile && !readOnlyFile ? previewMode ? <MarkdownFilePreview source={draft} showOutline={outlineOpen} outlineId={outlineId} /> :
          <CodeHighlightEditor value={draft} path={activeFile} onChange={setDraft} readOnly={loading || !targetConnected} ariaLabel={`${activeName} 编辑器`} onEditor={bindEditor} /> : <div className="archive-preview-empty code-viewer-empty-state">{readOnlyFile && <LockKeyhole size={28} />}<strong>{readOnlyFile ? "无法预览此文件" : "选择一个文件开始查看"}</strong><span>{readOnlyFile ? "请选择其他文本文件。" : "从左侧工程文件中选择文本文件。"}</span></div>}
        </div>
      </article>
    </div>}
    {remotePickerOpen && sshConnected && globalSsh.connection && <SshDirectoryPicker key={globalSsh.connection.id} connectionId={globalSsh.connection.id} server={globalSsh.connection.name}
      initialPath={target.kind === "ssh" && target.connectionId === globalSsh.connection.id && root ? root : globalSsh.connection.defaultRoot || "~"}
      onClose={closeRemotePicker} onSelect={browseRemote} />}
    {transfers.dialog}
    {mutations.dialog}
    {directoryMenu && <ContextMenu x={directoryMenu.x} y={directoryMenu.y} label="文件操作" trigger={directoryMenu.trigger} onClose={closeDirectoryMenu} items={[
      ...(directoryMenu.entry.isDirectory ? [
        { label: "新建文件", icon: <Plus size={14} />, disabled: !targetConnected || loading || mutations.busy, onSelect: () => mutations.begin(directoryMenu.entry, "create-file") },
        { label: "新建文件夹", icon: <FolderOpen size={14} />, disabled: !targetConnected || loading || mutations.busy, onSelect: () => mutations.begin(directoryMenu.entry, "create-directory") },
        { label: "在新标签中打开目录", icon: <Plus size={14} />, onSelect: () => openNewTab(directoryMenu.entry.path) },
        { label: "刷新此目录", icon: <RefreshCw size={14} />, disabled: !targetConnected, onSelect: () => { if (directoryMenu.entry.path === root) void refreshDirectory(); else { void loadFolder(directoryMenu.entry.path); void git.refresh(); } } },
      ] : [{ label: "打开文件", icon: <Eye size={14} />, disabled: !targetConnected, onSelect: () => openFile(directoryMenu.entry) }]),
      ...(target.kind === "ssh" && directoryMenu.entry.path === root ? [{ label: "显示隐藏文件", icon: showHiddenFiles ? <Eye size={14} /> : <EyeOff size={14} />, checked: showHiddenFiles, onSelect: toggleHiddenFiles }] : []),
      ...(target.kind === "ssh" ? [
        { label: "下载", icon: <Download size={14} />, disabled: !targetConnected || transfers.busy || directoryMenu.entry.kind === "symlink", onSelect: () => void transfers.download(directoryMenu.entry.path) },
        ...(directoryMenu.entry.isDirectory ? [
          { label: "上传文件到此目录", icon: <Upload size={14} />, disabled: !targetConnected || transfers.busy, onSelect: () => void transfers.pickUpload(directoryMenu.entry.path, false) },
          { label: "上传文件夹到此目录", icon: <FolderOpen size={14} />, disabled: !targetConnected || transfers.busy, onSelect: () => void transfers.pickUpload(directoryMenu.entry.path, true) },
        ] : []),
      ] : []),
      { label: "复制路径", icon: <Copy size={14} />, onSelect: () => void copyPath(directoryMenu.entry.path, "路径") },
      ...(directoryMenu.entry.path !== root ? [
        { label: "重命名", icon: <Pencil size={14} />, disabled: !targetConnected || loading || mutations.busy, onSelect: () => mutations.begin(directoryMenu.entry, "rename") },
        { label: "永久删除", icon: <Trash2 size={14} />, disabled: !targetConnected || loading || mutations.busy, onSelect: () => mutations.begin(directoryMenu.entry, "delete") },
      ] : []),
      ...(directoryMenu.entry.symlinkTarget != null ? [{ label: "复制链接目标", icon: <Link2 size={14} />, onSelect: () => void copyPath(directoryMenu.entry.symlinkTarget!, "链接目标") }] : []),
    ]} />}
    {historyOpen && <><button className="code-viewer-history-backdrop" aria-label="关闭历史记录" onClick={() => setHistoryOpen(false)} /><aside className="archive-history-overlay code-viewer-history"><div className="archive-history-head"><b>最近打开的目录</b><Button variant="subtle" size="xs" icon={<X size={13} />} aria-label="关闭" onClick={() => setHistoryOpen(false)} /></div><div className="archive-history-body custom-scrollbar">{recentFolders.length ? recentFolders.map((item) => <button className="code-viewer-history-row" type="button" key={JSON.stringify([item.target, item.path])} onClick={() => { if (dirty && !window.confirm("放弃当前文件的未保存修改？")) return; void loadFolder(item.path, true, item.target || { kind: "local" }); setHistoryOpen(false); }}><FolderOpen size={15} /><span><b>{folderName(item.path)}</b><small>{item.target?.kind === "ssh" ? "SSH · " + item.target.name + " · " : "本地 · "}{item.path}</small></span><time><Clock3 size={11} />{new Date(item.openedAt).toLocaleDateString()}</time></button>) : <div className="archive-history-empty">暂无目录记录。</div>}</div></aside></>}
  </section>;
}
