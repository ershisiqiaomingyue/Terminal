import { ToolBackButton } from "@/components/ui/ToolBackButton";
import { useToolWindowValue, useToolDetachGuard } from "@/platform/toolWindowState";
import { SegmentedControl } from "@/components/ui/SegmentedControl";
import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  PackageOpen,
  GitCompareArrows,
  FolderOutput,
  PencilLine,
  Code2,
  History,
  Trash2,
  Copy,
  Search,
  Folder,
  FolderOpen,
  File,
  RotateCcw,
  Save,
  Upload,
  RefreshCw,
  HardDrive,
  Cpu,
  Layers,
  FileCode,
  ShieldCheck,
  Check,
  LoaderCircle,
  X,
} from "lucide-react";
import { Button, Input, Badge, PropertyGrid, type PropertyItem } from "@/components/ui";
import { useConfirmAction } from "@/components/ConfirmDialog/useConfirmAction";
import { formatInvokeError, invoke, isTauri, openDirectory, openFile, saveFile, waitForTask } from "@/platform/runtime";
import { useApplicationPageAction } from "@/application/useApplicationPageAction";
import { androidPackageInputHint, classifyAndroidPackageInput } from "@/application/androidPackageInput";
import { notify, publishAiTaskActivity, notifyDesktopRequired } from "@/platform/notifications";
import "./archive-inspector.css";

// ─── Tauri API 辅助 ───────────────────────────────────────────
const isTauriEnv = (): boolean =>
  typeof window !== "undefined" &&
  ("__TAURI_INTERNALS__" in window || "__TAURI__" in window);

async function safeOpenDialog(options: Record<string, unknown>): Promise<string | string[] | null> {
  try {
    const plugin: any = await new Function('return import("@tauri-apps/plugin-dialog")')();
    return await plugin.open(options);
  } catch {
    return null;
  }
}

// ─── 类型定义 ───────────────────────────────────────────────
export interface ArchiveEntry {
  path: string;
  compressedSize: number;
  uncompressedSize: number;
  isDirectory: boolean;
}

export interface ArchiveInspection {
  path: string;
  fileName: string;
  format: string;
  size: number;
  sha256: string;
  entries: ArchiveEntry[];
  entryCount: number;
  compressedSize: number;
  uncompressedSize: number;
}

interface ArchiveHistoryRecord {
  id: string;
  archivePath: string;
  fileName: string;
  sha256: string;
  openedAt: number;
  updatedAt: number;
  workspacePath?: string | null;
  apktoolVersion?: string | null;
  manifestChanged: boolean;
  outputPath?: string | null;
  verification?: string | null;
  installedSerial?: string | null;
  installResult?: string | null;
}

interface ArchiveEntryPreview {
  path: string;
  size: number;
  kind: string;
  content: string;
  truncated: boolean;
}

interface ArchiveSearchMatch {
  entryPath: string;
  line?: number | null;
  column?: number | null;
  preview: string;
  kind: string;
}

interface ArchiveSearchResult {
  matches: ArchiveSearchMatch[];
  scannedEntries: number;
  skippedEntries: number;
  truncated: boolean;
}

interface ArchivePreviewTarget {
  entryPath: string;
  line?: number | null;
}

export interface TreeNode {
  name: string;
  path: string;
  isDirectory: boolean;
  entry?: ArchiveEntry;
  children: TreeNode[];
}

export interface VisibleTreeNode {
  node: TreeNode;
  depth: number;
}

interface JadxWorkspace {
  workspacePath: string;
  version: string;
  files: string[];
}

interface ArchiveInspectorViewProps {
  onBack?: () => void;
}

const JADX_FILE_PAGE_SIZE = 400;

function formatArchiveHistoryTime(timestamp: number) {
  return new Intl.DateTimeFormat("zh-CN", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  }).format(new Date(timestamp));
}

const XML_TOKEN_PATTERN = /(<!--[\s\S]*?-->|<\/?[\w:.-]+|[\w:.-]+(?=\s*=)|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*')/g;
const SOURCE_TOKEN_PATTERN = /(\/\/.*$|\/\*.*?\*\/|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|\b(?:abstract|annotation|boolean|break|case|catch|class|const|continue|data|default|do|else|enum|extends|final|for|fun|if|implements|import|in|instanceof|interface|internal|new|null|object|open|override|package|private|protected|public|return|sealed|static|string|super|switch|this|throw|throws|try|val|var|void|when|while)\b|\b\d+(?:\.\d+)?\b)/g;

function archivePreviewLanguage(path: string, kind: string): "xml" | "source" | "plain" | "binary" {
  if (kind === "binary") return "binary";
  const lower = path.toLowerCase();
  if (lower.endsWith(".xml") || lower.endsWith(".html") || lower.endsWith(".htm")) return "xml";
  if (kind === "java" || /\.(java|kt|kts|smali|js|ts|c|cc|cpp|h|hpp|rs|gradle)$/.test(lower)) return "source";
  return "plain";
}

function syntaxTokenClass(token: string, language: ReturnType<typeof archivePreviewLanguage>): string {
  if (token.startsWith("//") || token.startsWith("/*") || token.startsWith("<!--")) return "comment";
  if (token.startsWith("\"") || token.startsWith("'")) return "string";
  if (/^\d/.test(token)) return "number";
  if (language === "xml") return token.startsWith("<") ? "tag" : "attribute";
  return "keyword";
}

function renderCodeFragment(text: string, query: string): React.ReactNode[] {
  const normalizedQuery = query.trim();
  if (!normalizedQuery) return [text];
  const folded = text.toLocaleLowerCase("en-US");
  const needle = normalizedQuery.toLocaleLowerCase("en-US");
  const fragments: React.ReactNode[] = [];
  let offset = 0;
  let match = folded.indexOf(needle, offset);
  while (match >= 0) {
    if (match > offset) fragments.push(text.slice(offset, match));
    fragments.push(<mark className="archive-code-search-hit" key={`${match}-${needle}`}>{text.slice(match, match + normalizedQuery.length)}</mark>);
    offset = match + normalizedQuery.length;
    match = folded.indexOf(needle, offset);
  }
  if (offset < text.length) fragments.push(text.slice(offset));
  return fragments.length ? fragments : [text];
}

function renderCodeLine(line: string, language: ReturnType<typeof archivePreviewLanguage>, query: string): React.ReactNode[] {
  if (language === "plain" || language === "binary") return renderCodeFragment(line, query);
  const pattern = language === "xml" ? XML_TOKEN_PATTERN : SOURCE_TOKEN_PATTERN;
  pattern.lastIndex = 0;
  const tokens: React.ReactNode[] = [];
  let offset = 0;
  let match: RegExpExecArray | null;
  while ((match = pattern.exec(line)) !== null) {
    if (match.index > offset) tokens.push(...renderCodeFragment(line.slice(offset, match.index), query));
    const token = match[0];
    tokens.push(
      <span className={`archive-code-token ${syntaxTokenClass(token, language)}`} key={`${match.index}-${token}`}>
        {renderCodeFragment(token, query)}
      </span>
    );
    offset = match.index + token.length;
  }
  if (offset < line.length) tokens.push(...renderCodeFragment(line.slice(offset), query));
  return tokens.length ? tokens : [line];
}

interface ArchiveCodeViewerProps {
  content: string;
  path: string;
  kind: string;
  query: string;
  targetLine?: number | null;
}

const ArchiveCodeViewer = React.forwardRef<HTMLPreElement, ArchiveCodeViewerProps>(
  ({ content, path, kind, query, targetLine }, ref) => {
    const language = archivePreviewLanguage(path, kind);
    const lines = useMemo(() => content.split(/\r?\n/), [content]);
    return (
      <pre ref={ref} className={`archive-preview-code custom-scrollbar language-${language}`}>
        <code>
          {lines.map((line, index) => {
            const lineNumber = index + 1;
            return (
              <span
                className={`archive-code-line${targetLine === lineNumber ? " is-search-target" : ""}`}
                data-line={lineNumber}
                key={`${lineNumber}-${line}`}
              >
                <span className="archive-code-line-number" aria-hidden="true">{lineNumber}</span>
                <span className="archive-code-line-content">{renderCodeLine(line, language, query)}</span>
              </span>
            );
          })}
        </code>
      </pre>
    );
  }
);

ArchiveCodeViewer.displayName = "ArchiveCodeViewer";

export const ArchiveInspectorView: React.FC<ArchiveInspectorViewProps> = ({ onBack }) => {
  const { confirmAction, confirmationDialog } = useConfirmAction();
  const desktop = isTauri();
  const [primary, setPrimary] = useToolWindowValue<ArchiveInspection | null>("archive", "primary", null);
  const [isLoadingArchive, setIsLoadingArchive] = useState(false);
  const [tab, setTab] = useToolWindowValue<"overview" | "preview" | "jadx">("archive", "tab", "overview");
  const [treeQuery, setTreeQuery] = useToolWindowValue("archive", "treeQuery", "");
  const [selectedPath, setSelectedPath] = useToolWindowValue("archive", "selectedPath", "AndroidManifest.xml");
  useToolDetachGuard("archive", isLoadingArchive);
  const [expandedPaths, setExpandedPaths] = useState<Set<string>>(new Set(["res", "res/layout", "META-INF", "lib", "lib/arm64-v8a"]));
  const [historyOpen, setHistoryOpen] = useState(false);
  const [archiveHistory, setArchiveHistory] = useState<ArchiveHistoryRecord[]>([]);
  const [historyLoading, setHistoryLoading] = useState(false);

  // Manifest 编辑态
  const [manifestDraft, setManifestDraft] = useToolWindowValue("archive", "manifestDraft", "");
  const [originalManifest, setOriginalManifest] = useToolWindowValue("archive", "originalManifest", "");
  const [manifestEditing, setManifestEditing] = useToolWindowValue("archive", "manifestEditing", false);
  const [previewText, setPreviewText] = useToolWindowValue("archive", "previewText", "");
  const [previewKind, setPreviewKind] = useToolWindowValue("archive", "previewKind", "text");
  const [previewTarget, setPreviewTarget] = useToolWindowValue<ArchivePreviewTarget | null>("archive", "previewTarget", null);
  const [previewSearchQuery, setPreviewSearchQuery] = useToolWindowValue("archive", "previewSearchQuery", "");
  const [jadxText, setJadxText] = useToolWindowValue("archive", "jadxText", "");
  const [jadxWorkspacePath, setJadxWorkspacePath] = useToolWindowValue("archive", "jadxWorkspacePath", "");
  const [jadxVersion, setJadxVersion] = useToolWindowValue("archive", "jadxVersion", "");
  const [jadxFiles, setJadxFiles] = useToolWindowValue<string[]>("archive", "jadxFiles", []);
  const [selectedJadxFile, setSelectedJadxFile] = useToolWindowValue("archive", "selectedJadxFile", "");
  const [jadxLoadingFile, setJadxLoadingFile] = useState("");
  const [jadxFileQuery, setJadxFileQuery] = useToolWindowValue("archive", "jadxFileQuery", "");
  const [jadxFileDisplayLimit, setJadxFileDisplayLimit] = useState(JADX_FILE_PAGE_SIZE);
  const [editSessionId, setEditSessionId] = useToolWindowValue("archive", "editSessionId", "");
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const [searchResult, setSearchResult] = useState<ArchiveSearchResult | null>(null);
  const [searching, setSearching] = useState(false);
  const [searchError, setSearchError] = useState("");
  const [activeSearchIndex, setActiveSearchIndex] = useState(0);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const previewCodeRef = useRef<HTMLPreElement>(null);
  const searchRequestRef = useRef(0);
  const lastShiftPressedRef = useRef(0);

  const showToast = (msg: string) => {
    notify(msg, "info");
  };

  const refreshArchiveHistory = useCallback(async () => {
    if (!desktop) {
      notifyDesktopRequired();
      return;
    }
    setHistoryLoading(true);
    try {
      const records = await invoke<ArchiveHistoryRecord[]>("list_archive_history");
      setArchiveHistory(records);
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    } finally {
      setHistoryLoading(false);
    }
  }, [desktop]);

  const copyHash = () => {
    if (primary) {
      navigator.clipboard?.writeText(primary.sha256);
      showToast("已复制 SHA-256 校验和到剪贴板");
    }
  };

  const formatBytes = (bytes: number) => {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
  };

  // 生成文件树
  const tree = useMemo<TreeNode[]>(() => {
    const root: TreeNode = { name: "", path: "", isDirectory: true, children: [] };
    const directories = new Map<string, TreeNode>([["", root]]);

    for (const entry of primary?.entries || []) {
      const parts = entry.path.replace(/\\/g, "/").split("/").filter(Boolean);
      let parent = root;
      let currentPath = "";

      parts.forEach((part, index) => {
        currentPath = currentPath ? `${currentPath}/${part}` : part;
        const isLeaf = index === parts.length - 1;
        let node = parent.children.find((item) => item.name === part);

        if (!node) {
          node = {
            name: part,
            path: currentPath,
            isDirectory: !isLeaf || entry.isDirectory,
            children: [],
          };
          parent.children.push(node);
        }
        if (isLeaf) {
          node.isDirectory = entry.isDirectory;
          node.entry = entry;
        }
        if (node.isDirectory) {
          directories.set(currentPath, node);
        }
        parent = node;
      });
    }

    const sortNodes = (nodes: TreeNode[]) => {
      nodes.sort(
        (a, b) =>
          Number(b.isDirectory) - Number(a.isDirectory) ||
          a.name.localeCompare(b.name)
      );
      nodes.forEach((n) => sortNodes(n.children));
    };

    sortNodes(root.children);
    return root.children;
  }, [primary]);

  // 过滤树节点
  const visibleTreeNodes = useMemo<VisibleTreeNode[]>(() => {
    const query = treeQuery.trim().toLowerCase();
    const includesMatch = (node: TreeNode): boolean =>
      !query ||
      node.name.toLowerCase().includes(query) ||
      node.children.some(includesMatch);

    const result: VisibleTreeNode[] = [];
    const visit = (nodes: TreeNode[], depth: number) => {
      nodes.forEach((node) => {
        if (!includesMatch(node)) return;
        result.push({ node, depth });
        if (node.isDirectory && (query || expandedPaths.has(node.path))) {
          visit(node.children, depth + 1);
        }
      });
    };

    visit(tree, 0);
    return result;
  }, [tree, treeQuery, expandedPaths]);

  const toggleTreeNode = (node: TreeNode) => {
    if (!node.isDirectory) {
      setSelectedPath(node.path);
      setTab("preview");
      setManifestEditing(false);
      setPreviewTarget(null);
      setPreviewSearchQuery("");
      return;
    }
    const next = new Set(expandedPaths);
    if (next.has(node.path)) {
      next.delete(node.path);
    } else {
      next.add(node.path);
    }
    setExpandedPaths(next);
    setSelectedPath(node.path);
    setManifestEditing(false);
    setPreviewTarget(null);
    setPreviewSearchQuery("");
  };

  const inspectArchive = useCallback(async (path: string) => {
    const taskId = await invoke<string>("start_android_archive_inspection", { path });
    const task = await waitForTask(taskId);
    if (task.status !== "done" || !task.artifactPath) {
      throw new Error(task.error || task.message || "压缩包检查未完成");
    }
    return invoke<ArchiveInspection>("read_android_archive_inspection", { artifactPath: task.artifactPath });
  }, []);

  const loadArchive = useCallback(async (path: string, actionId?: string) => {
    const input = classifyAndroidPackageInput(path);
    if (!input) {
      const message = androidPackageInputHint(path);
      notify(message, "warning");
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "文件类型不支持", detail: message, actionId });
      return;
    }
    if (!desktop) {
      notifyDesktopRequired(actionId);
      return;
    }

    const fileName = path.split(/[\\/]/).filter(Boolean).pop() || path;
    setIsLoadingArchive(true);
    notify(androidPackageInputHint(path), "info");
    if (actionId) publishAiTaskActivity({ running: true, status: "running", title: "正在解析 Android 文件", detail: `正在读取 ${fileName}。`, actionId, progress: 15 });
    try {
      const inspection = await inspectArchive(path);
      setPrimary(inspection);
      setSelectedPath(inspection.entries.find((entry) => entry.path === "AndroidManifest.xml")?.path || inspection.entries.find((entry) => !entry.isDirectory)?.path || "");
      setTab("overview");
      setManifestEditing(false);
      setEditSessionId("");
      setJadxWorkspacePath("");
      setJadxVersion("");
      setJadxFiles([]);
      setSelectedJadxFile("");
      setJadxText("");
      setJadxFileQuery("");
      setJadxFileDisplayLimit(JADX_FILE_PAGE_SIZE);
      setSearchOpen(false);
      setSearchQuery("");
      setSearchResult(null);
      setPreviewSearchQuery("");
      setPreviewTarget(null);
      const message = `已解析 ${inspection.fileName}，${inspection.entryCount ? `${inspection.entryCount} 个条目` : inspection.format}`;
      notify(message, "success");
      void refreshArchiveHistory();
      if (actionId) publishAiTaskActivity({ running: false, status: "done", title: "Android 文件解析完成", detail: message, actionId, progress: 100 });
    } catch (reason) {
      const message = formatInvokeError(reason);
      notify(message, "error");
      if (actionId) publishAiTaskActivity({ running: false, status: "error", title: "Android 文件解析失败", detail: message, actionId });
    } finally {
      setIsLoadingArchive(false);
    }
  }, [desktop, inspectArchive, refreshArchiveHistory]);

  const handleDeleteArchiveHistory = (record: ArchiveHistoryRecord) => {
    confirmAction({ title: "删除解析历史", message: `确认删除「${record.fileName}」的解析历史？不会删除原始文件或解析目录。`, onConfirm: async () => {
      await invoke("delete_archive_history", { request: { id: record.id, deleteWorkspace: false } });
      await refreshArchiveHistory();
      notify(`已从历史记录移除 ${record.fileName}。`, "success");
    } });
  };

  const handlePickFile = async () => {
    try {
      // Native extension filters cannot express `library.aar1` or `app.apk (2)`.
      // Display every file, then validate it with the shared package-name rule.
      const selected = await openFile("打开 Android 包或原生库");
      if (!selected) return;
      await loadArchive(selected);
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  useApplicationPageAction("open.tool.archive", (action) => {
    const path = action.parameters.path;
    if (!path) {
      const message = "缺少 APK 或 JAR 的本地路径；请在包查看器中选择文件。";
      notify(message, "warning");
      publishAiTaskActivity({ running: false, status: "error", title: "包解析等待输入", detail: message, actionId: action.actionId });
      return;
    }
    void loadArchive(path, action.actionId);
  });

  const handleCompare = async () => {
    if (!desktop || !primary || isNativeLibrary) {
      showToast("请选择对比的 APK/JAR 文件");
      return;
    }
    const right = await openFile("选择对比 Android 包");
    if (!right) return;
    const rightInput = classifyAndroidPackageInput(right);
    if (!rightInput || rightInput.kind === "native-library") {
      notify("包对比仅支持 APK、AAR、JAR、AAB、APKS 或 ZIP 文件。", "warning");
      return;
    }
    try {
      const taskId = await invoke<string>("start_android_archive_compare", { request: { leftPath: primary.path, rightPath: right } });
      const task = await waitForTask(taskId);
      if (task.status !== "done" || !task.artifactPath) throw new Error(task.error || "对比未完成");
      const result = await invoke<{ added: string[]; removed: string[]; changed: string[]; unchangedCount: number }>("read_android_archive_compare_result", { artifactPath: task.artifactPath });
      showToast(`对比完成：新增 ${result.added.length}，删除 ${result.removed.length}，变化 ${result.changed.length}，未变 ${result.unchangedCount}`);
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  const handleExtract = async () => {
    if (!desktop || !primary) {
      showToast("请在桌面端先打开包文件");
      return;
    }
    const destination = await openDirectory("选择解包目录");
    if (!destination) return;
    try {
      const taskId = await invoke<string>("start_android_archive_extract", { request: { archivePath: primary.path, destination } });
      const task = await waitForTask(taskId);
      if (task.status !== "done") throw new Error(task.error || task.message || "解包未完成");
      showToast(`已解包到 ${destination}`);
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  const handleDecode = async () => {
    if (!desktop || !primary) {
      showToast("请在桌面端先打开 APK");
      return;
    }
    try {
      showToast("正在使用 Apktool 解码 APK...");
      const taskId = await invoke<string>("start_apk_edit_session", { request: { archivePath: primary.path } });
      const task = await waitForTask(taskId);
      if (task.status !== "done" || !task.artifactPath) throw new Error(task.error || "APK 解码未完成");
      const sessionId = task.artifactPath.split(/[\\/]/).filter(Boolean).pop() || "";
      const session = await invoke<{ id: string; manifest: string }>("resume_apk_edit_session", { request: { id: sessionId } });
      setEditSessionId(session.id);
      setOriginalManifest(session.manifest);
      setManifestDraft(session.manifest);
      setSelectedPath("AndroidManifest.xml");
      setTab("preview");
      setManifestEditing(true);
      showToast("APK 已解码，可以编辑清单。");
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  const handleBuild = async () => {
    if (!desktop || !editSessionId) {
      showToast("请先解码 APK");
      return;
    }
    const destination = await saveFile("保存重建 APK", "rebuilt.apk", [{ name: "APK", extensions: ["apk"] }]);
    if (!destination) return;
    try {
      const taskId = await invoke<string>("start_signed_apk_build", { request: { sessionId: editSessionId, manifest: manifestDraft, destination } });
      const task = await waitForTask(taskId);
      if (task.status !== "done") throw new Error(task.error || "重建未完成");
      setOriginalManifest(manifestDraft);
      setManifestEditing(false);
      showToast(`已重建并签名：${destination}`);
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  const handleJadx = async () => {
    if (!desktop || !primary) {
      showToast("请在桌面端先打开 APK 或 JAR");
      return;
    }
    try {
      const taskId = await invoke<string>("start_jadx_decompile", { request: { archivePath: primary.path } });
      const task = await waitForTask(taskId);
      if (task.status !== "done" || !task.artifactPath) throw new Error(task.error || "JADX 反编译未完成");
      const workspace = await invoke<JadxWorkspace>("inspect_jadx_workspace", { workspacePath: task.artifactPath });
      setJadxWorkspacePath(workspace.workspacePath);
      setJadxVersion(workspace.version || "未知版本");
      setJadxFiles(workspace.files);
      setJadxFileQuery("");
      setJadxFileDisplayLimit(JADX_FILE_PAGE_SIZE);
      const first = workspace.files.find((file) => /\.(java|kt)$/i.test(file)) || workspace.files[0];
      if (!first) {
        setJadxText("反编译完成，但没有可读源码文件。");
        setSelectedJadxFile("");
      } else {
        const source = await invoke<string>("read_jadx_source", { request: { workspacePath: task.artifactPath, relativePath: first } });
        setSelectedJadxFile(first);
        setJadxText(source);
      }
      setTab("jadx");
      showToast("JADX 反编译完成");
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  const handleJadxFileSelect = async (relativePath: string) => {
    setSelectedJadxFile(relativePath);
    if (!desktop) {
      notifyDesktopRequired();
      return;
    }
    if (!jadxWorkspacePath) {
      notify("JADX 工作区尚未准备完成。", "warning");
      return;
    }
    setJadxLoadingFile(relativePath);
    try {
      const source = await invoke<string>("read_jadx_source", {
        request: { workspacePath: jadxWorkspacePath, relativePath },
      });
      setJadxText(source);
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    } finally {
      setJadxLoadingFile("");
    }
  };

  useEffect(() => {
    if (!desktop || !primary || !selectedPath || selectedPath.endsWith("/")) return;
    let cancelled = false;
    void invoke<ArchiveEntryPreview>("preview_android_archive_entry", { request: { archivePath: primary.path, entryPath: selectedPath } })
      .then((preview) => {
        if (!cancelled) {
          setPreviewText(preview.content);
          setPreviewKind(preview.kind);
        }
      })
      .catch((reason) => {
        if (!cancelled) {
          setPreviewText(formatInvokeError(reason));
          setPreviewKind("text");
        }
      });
    return () => {
      cancelled = true;
    };
  }, [desktop, primary, selectedPath]);

  const selectedEntry = primary?.entries.find(
    (e) => e.path.replace(/\\/g, "/").replace(/\/$/, "") === selectedPath.replace(/\/$/, "")
  );
  const isNativeLibrary = primary?.format.startsWith("ELF SO") ?? false;
  const isApkArchive = primary?.format.startsWith("APK") ?? false;
  const previewContent = previewText || "选择条目后显示预览。";
  const effectivePreviewKind = previewKind;
  const searchMatches = searchResult?.matches || [];
  const filteredJadxFiles = useMemo(() => {
    const query = jadxFileQuery.trim().toLocaleLowerCase("en-US");
    if (!query) return jadxFiles;
    return jadxFiles.filter((file) => file.toLocaleLowerCase("en-US").includes(query));
  }, [jadxFileQuery, jadxFiles]);
  const visibleJadxFiles = filteredJadxFiles.slice(0, jadxFileDisplayLimit);

  const openArchiveSearch = useCallback(() => {
    if (!primary) {
      notify("请先打开 APK、AAR、JAR 或其他 Android 包。", "info");
      return;
    }
    if (isNativeLibrary) {
      notify("原生 SO 库暂不提供包内条目搜索。", "info");
      return;
    }
    setSearchOpen(true);
    setSearchQuery("");
    setSearchResult(null);
    setSearchError("");
    setActiveSearchIndex(0);
  }, [isNativeLibrary, primary]);

  const openSearchMatch = useCallback((match: ArchiveSearchMatch) => {
    const parts = match.entryPath.replace(/\\/g, "/").split("/").filter(Boolean);
    setExpandedPaths((current) => {
      const next = new Set(current);
      for (let index = 1; index < parts.length; index += 1) {
        next.add(parts.slice(0, index).join("/"));
      }
      return next;
    });
    setSelectedPath(match.entryPath);
    setTab("preview");
    setManifestEditing(false);
    setPreviewTarget({ entryPath: match.entryPath, line: match.line });
    setPreviewSearchQuery(searchQuery.trim());
    setSearchOpen(false);
  }, [searchQuery]);

  useEffect(() => {
    if (!searchOpen) return;
    const timer = window.setTimeout(() => searchInputRef.current?.focus(), 0);
    return () => window.clearTimeout(timer);
  }, [searchOpen]);

  useEffect(() => {
    if (!searchOpen || !primary || isNativeLibrary) return;
    const query = searchQuery.trim();
    if (!query) {
      setSearchResult(null);
      setSearchError("");
      setSearching(false);
      return;
    }
    const requestId = ++searchRequestRef.current;
    const timer = window.setTimeout(() => {
      setSearching(true);
      setSearchError("");
      const search = invoke<ArchiveSearchResult>("search_android_archive_entries", { request: { archivePath: primary.path, query } });
      void search
        .then((result) => {
          if (requestId !== searchRequestRef.current) return;
          const known = new Set(result.matches.map((match) => `${match.entryPath}|${match.kind}|${match.line || ""}`));
          const localPathMatches = primary.entries
            .filter((entry) => !entry.isDirectory && entry.path.toLocaleLowerCase("en-US").includes(query.toLocaleLowerCase("en-US")))
            .map<ArchiveSearchMatch>((entry) => ({ entryPath: entry.path, preview: entry.path, kind: "文件路径" }))
            .filter((match) => !known.has(`${match.entryPath}|${match.kind}|`))
            .slice(0, Math.max(0, 120 - result.matches.length));
          setSearchResult({ ...result, matches: [...localPathMatches, ...result.matches] });
          setActiveSearchIndex(0);
        })
        .catch((reason) => {
          if (requestId !== searchRequestRef.current) return;
          setSearchResult(null);
          setSearchError(formatInvokeError(reason));
        })
        .finally(() => {
          if (requestId === searchRequestRef.current) setSearching(false);
        });
    }, 280);
    return () => window.clearTimeout(timer);
  }, [desktop, isNativeLibrary, primary, searchOpen, searchQuery]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.shiftKey && event.key.toLowerCase() === "f") {
        event.preventDefault();
        openArchiveSearch();
        return;
      }
      if (event.key === "Shift" && !event.repeat) {
        const now = Date.now();
        if (now - lastShiftPressedRef.current <= 360) {
          openArchiveSearch();
          lastShiftPressedRef.current = 0;
        } else {
          lastShiftPressedRef.current = now;
        }
      }
      if (event.key === "Escape" && searchOpen) {
        event.preventDefault();
        setSearchOpen(false);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [openArchiveSearch, searchOpen]);

  useEffect(() => {
    if (tab !== "preview" || !previewTarget?.line || previewTarget.entryPath !== selectedPath) return;
    const frame = window.requestAnimationFrame(() => {
      previewCodeRef.current
        ?.querySelector<HTMLElement>(`[data-line="${previewTarget.line}"]`)
        ?.scrollIntoView({ block: "center" });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [previewContent, previewTarget, selectedPath, tab]);

  // 12项包属性遥测数据 (与设备总览同款 PropertyGrid)
  const archiveProperties: PropertyItem[] = useMemo(() => {
    if (!primary) return [];
    return [
      {
        key: "name",
        name: "包文件名",
        label: "包文件名",
        icon: <PackageOpen size={14} />,
        value: primary.fileName,
      },
      {
        key: "format",
        name: "归档格式",
        label: "归档格式",
        icon: <Layers size={14} />,
        value: isNativeLibrary ? primary.format : `${primary.format} (Android Archive)`,
      },
      {
        key: "size",
        name: "压缩后体积",
        label: "压缩后体积",
        icon: <HardDrive size={14} />,
        value: formatBytes(primary.compressedSize),
      },
      {
        key: "uncompressed",
        name: "解压后总体积",
        label: "解压后总体积",
        icon: <HardDrive size={14} />,
        value: formatBytes(primary.uncompressedSize),
      },
      {
        key: "entries",
        name: "总条目数量",
        label: "总条目数量",
        icon: <FileCode size={14} />,
        value: `${primary.entryCount} 个文件 / 目录`,
      },
      {
        key: "dex",
        name: "DEX 代码段",
        label: "DEX 代码段",
        icon: <Cpu size={14} />,
        value: isNativeLibrary ? "不适用（ELF 原生库）" : primary.entries.filter((entry) => /(^|\/)classes\d*\.dex$/i.test(entry.path)).map((entry) => entry.path).join(" · ") || "未发现 DEX",
      },
      {
        key: "manifest",
        name: "清单声明状态",
        label: "清单声明状态",
        icon: <Check size={14} />,
        value: isNativeLibrary ? "不适用（ELF 原生库）" : "未检测",
        valueType: isNativeLibrary ? "default" : "unknown",
      },
      {
        key: "signing",
        name: "V1 / V2 签名方案",
        label: "V1 / V2 签名方案",
        icon: <ShieldCheck size={14} />,
        value: isNativeLibrary ? "不适用（ELF 原生库）" : "未验签",
        valueType: isNativeLibrary ? "default" : "unknown",
      },
    ];
  }, [primary, isNativeLibrary]);

  return (
    <div className={`archive-container${isLoadingArchive ? " is-importing" : ""}`}>
      {confirmationDialog}
      {/* 1. 顶部 Header (与 ADB 设备管理 100% 对齐) */}
      <header className="archive-top-header">
        <div className="archive-header-left">
          <ToolBackButton onBack={onBack} />
          <div className="archive-title-wrap">
            <h1 className="archive-title">Android 包与原生库分析</h1>
            <span className="archive-subtitle">
              深度解析 Android 安装包结构、SHA-256 完整性校验、清单重构与反编译
            </span>
          </div>
        </div>

        <div className="archive-header-right">
          <Button
            variant="primary"
            size="sm"
            icon={<PackageOpen size={13} />}
            onClick={handlePickFile}
          >
            打开 Android 包
          </Button>
          {!isNativeLibrary && (
            <Button
              variant="subtle"
              size="sm"
              icon={<Search size={15} />}
              title="搜索包内文件与内容 (Ctrl+Shift+F，双击 Shift)"
              aria-label="搜索包内文件与内容"
              onClick={openArchiveSearch}
            />
          )}
          {!isNativeLibrary && (
          <Button
            variant="secondary"
            size="sm"
            icon={<GitCompareArrows size={13} />}
            onClick={() => void handleCompare()}
          >
            对比包文件
          </Button>
          )}
          <Button
            variant="secondary"
            size="sm"
            icon={<History size={13} />}
            onClick={() => {
              const nextOpen = !historyOpen;
              setHistoryOpen(nextOpen);
              if (nextOpen) void refreshArchiveHistory();
            }}
          >
            历史记录
          </Button>
        </div>
      </header>

      {/* 2. 顶层副工具条：快速状态与操作动作 */}
      {primary && (
        <div className="archive-actions-bar">
          <div className="archive-bar-info">
            <span>当前包：</span>
            <strong>{primary.fileName}</strong>
            <small>· {formatBytes(primary.size)} · {primary.entryCount} 个条目</small>
          </div>

          <div className="archive-bar-buttons">
            {!isNativeLibrary && <>
            <Button
              variant="secondary"
              size="sm"
              icon={<FolderOutput size={13} />}
              onClick={() => void handleExtract()}
            >
              解包到目录
            </Button>
            <Button
              variant="primary"
              size="sm"
              icon={<Code2 size={13} />}
              onClick={() => void handleJadx()}
            >
              JADX 源码
            </Button>
            </>}
            {isNativeLibrary && <span className="archive-readonly-status">ELF 原生库仅支持只读分析</span>}
          </div>
        </div>
      )}

      {/* 3. 主体分栏工作区 */}
      <div className="archive-body-split">
        {/* 左侧：包结构导航树 */}
        <aside className="archive-aside-tree">
          <div className="archive-aside-header">
            <span>包文件结构</span>
            <span className="adb-dev-badge-count">{primary?.entries.length || 0}</span>
          </div>

          {!isNativeLibrary && <div className="archive-aside-search">
            <Input
              size="sm"
              placeholder="过滤文件或目录..."
              value={treeQuery}
              onChange={(e) => setTreeQuery(e.target.value)}
              prefixIcon={<Search size={13} />}
            />
          </div>}

          <div className="archive-tree-scroll-list custom-scrollbar">
            {isNativeLibrary && <div className="archive-tree-empty">ELF 原生库没有 ZIP 条目结构。</div>}
            {!isNativeLibrary && visibleTreeNodes.map((item) => {
              const isSelected = selectedPath === item.node.path;
              const isFolder = item.node.isDirectory;
              const isOpen = isFolder && expandedPaths.has(item.node.path);

              return (
                <button
                  key={item.node.path}
                  type="button"
                  className={`archive-tree-item ${isSelected ? "selected" : ""}`}
                  style={{ paddingLeft: `${10 + item.depth * 14}px` }}
                  onClick={() => toggleTreeNode(item.node)}
                >
                  {isFolder ? (
                    isOpen ? <FolderOpen size={14} color="var(--warning-text)" /> : <Folder size={14} color="var(--warning-text)" />
                  ) : (
                    <File size={13} color="var(--control-text)" />
                  )}
                  <span className="tree-name">{item.node.name}</span>
                  {!isFolder && item.node.entry && (
                    <span className="tree-size">{formatBytes(item.node.entry.uncompressedSize)}</span>
                  )}
                </button>
              );
            })}
          </div>
        </aside>

        {/* 右侧：主详情与工作台 */}
        <article className={`archive-main-article${primary ? "" : " is-empty"}`}>
          {primary ? (
            <>
              {/* 卡片头部 */}
          <div className="archive-card-header">
                <div className="archive-card-title-col">
                  <h2>{primary.fileName}</h2>
                  <p className="archive-card-meta">
                    <span>{primary.format} 格式</span>
                    <span>·</span>
                    <span>{formatBytes(primary.size)}</span>
                    <span>·</span>
                    <span style={{ fontFamily: "var(--font-mono)", fontSize: 11 }}>
                      SHA-256: {primary.sha256.slice(0, 16)}...
                    </span>
                  </p>
                </div>
                <div className="archive-card-actions">
                  <Button variant="secondary" size="sm" icon={<Copy size={13} />} onClick={copyHash}>
                    复制 SHA-256
                  </Button>
                </div>
              </div>

              {/* 四大 Tab 导航胶囊 (与 ADB 设备管理 100% 对齐) */}
              {!isNativeLibrary && <div className="archive-tabs-container">
                <SegmentedControl value={tab} onChange={setTab} label="包分析视图" className="archive-seg-group" options={[
                  { value: "overview", label: "包概览与校验" }, { value: "preview", label: "文件内容预览" },
                  { value: "jadx", label: "JADX 源码逆向" },
                ]} />
              </div>}

              {/* TAB 1: 包概览与遥测属性 (单张干净卡片，无外框线) */}
              {tab === "overview" && (
                <div className="archive-tab-pane archive-telemetry-panel custom-scrollbar">
                  <PropertyGrid items={archiveProperties} columns={2} />

                  <div style={{ marginTop: 8 }}>
                    <div style={{ fontSize: 12, fontWeight: 600, color: "var(--text-secondary)", marginBottom: 8 }}>
                      完整 SHA-256 校验和哈希签名
                    </div>
                    <code className="archive-hash-box">
                      {primary.sha256}
                    </code>
                  </div>
                </div>
              )}

              {/* TAB 2: 文件内容预览 */}
              {tab === "preview" && (
                <div className="archive-tab-pane archive-preview-viewport">
                  <div className="archive-preview-titlebar">
                    <div>
                      <span>预览条目：</span>
                      <strong>{selectedPath || "未选择文件"}</strong>
                      {selectedPath === "AndroidManifest.xml" && manifestEditing && (
                        <span className="archive-preview-edit-state">
                          <Badge variant="warning">{manifestDraft !== originalManifest ? "已编辑" : "未修改"}</Badge>
                        </span>
                      )}
                    </div>
                    <div className="archive-preview-actions">
                      {selectedPath === "AndroidManifest.xml" && !manifestEditing && isApkArchive && (
                        <Button
                          variant="secondary"
                          size="xs"
                          icon={<PencilLine size={12} />}
                          onClick={() => void handleDecode()}
                        >
                          编辑并重编译
                        </Button>
                      )}
                      {selectedPath === "AndroidManifest.xml" && manifestEditing && (
                        <>
                          <Button
                            variant="secondary"
                            size="xs"
                            icon={<RotateCcw size={12} />}
                            disabled={manifestDraft === originalManifest}
                            onClick={() => {
                              setManifestDraft(originalManifest);
                              showToast("已还原为原始清单");
                            }}
                          >
                            还原
                          </Button>
                          <Button
                            variant="primary"
                            size="xs"
                            icon={<Save size={12} />}
                            onClick={() => void handleBuild()}
                          >
                            重编译 APK
                          </Button>
                        </>
                      )}
                      {!manifestEditing && selectedEntry && (
                        <span className="archive-preview-size">
                          压缩: {formatBytes(selectedEntry.compressedSize)} · 解压: {formatBytes(selectedEntry.uncompressedSize)}
                        </span>
                      )}
                    </div>
                  </div>
                  {manifestEditing ? (
                    <textarea
                      className="archive-editor-textarea custom-scrollbar"
                      value={manifestDraft}
                      onChange={(e) => setManifestDraft(e.target.value)}
                      spellCheck={false}
                      aria-label="AndroidManifest.xml 编辑器"
                    />
                  ) : (
                    <ArchiveCodeViewer
                      ref={previewCodeRef}
                      content={previewContent}
                      path={selectedPath}
                      kind={effectivePreviewKind}
                      query={previewSearchQuery}
                      targetLine={previewTarget?.entryPath === selectedPath ? previewTarget.line : null}
                    />
                  )}
                </div>
              )}

              {/* TAB 3: JADX 源码逆向 */}
              {tab === "jadx" && (
                <div className="archive-tab-pane archive-jadx-split">
                  <div className="archive-jadx-file-list custom-scrollbar">
                    <div className="archive-jadx-file-list-head">
                      <span>源码文件</span>
                      <Badge variant="neutral">{jadxFiles.length}</Badge>
                    </div>
                    {jadxFiles.length ? <>
                      <Input
                        size="sm"
                        placeholder="过滤源码文件..."
                        value={jadxFileQuery}
                        onChange={(event) => {
                          setJadxFileQuery(event.target.value);
                          setJadxFileDisplayLimit(JADX_FILE_PAGE_SIZE);
                        }}
                        prefixIcon={<Search size={12} />}
                      />
                      <div className="archive-jadx-file-results">
                      {visibleJadxFiles.map((file) => (
                      <button
                        type="button"
                        key={file}
                        className={`archive-jadx-file-item${selectedJadxFile === file ? " selected" : ""}`}
                        title={file}
                        onClick={() => void handleJadxFileSelect(file)}
                      >
                        <File size={13} />
                        <span>{file}</span>
                      </button>
                      ))}
                      {filteredJadxFiles.length > visibleJadxFiles.length && (
                        <Button
                          variant="subtle"
                          size="xs"
                          className="archive-jadx-load-more"
                          onClick={() => setJadxFileDisplayLimit((limit) => limit + JADX_FILE_PAGE_SIZE)}
                        >
                          显示更多（{visibleJadxFiles.length} / {filteredJadxFiles.length}）
                        </Button>
                      )}
                      {!visibleJadxFiles.length && <div className="archive-jadx-empty">没有匹配的源码文件。</div>}
                      </div>
                    </> : (
                      <div className="archive-jadx-empty">尚未开始反编译。点击上方“JADX 源码”生成完整源码索引。</div>
                    )}
                  </div>
                  <div className="archive-preview-viewport">
                    <div className="archive-preview-titlebar">
                      <strong>{selectedJadxFile || "JADX 源码预览"}</strong>
                      <span className="archive-preview-size">{jadxLoadingFile ? "正在读取源码..." : jadxVersion && jadxVersion !== "已完成" ? `JADX ${jadxVersion}` : "JADX"}</span>
                    </div>
                    <ArchiveCodeViewer
                      content={jadxText || "请选择源码文件。"}
                      path={selectedJadxFile || "source.java"}
                      kind="java"
                      query=""
                    />
                  </div>
                </div>
              )}
            </>
          ) : (
            <div className="archive-empty-workbench" onClick={() => { if (!isLoadingArchive) void handlePickFile(); }}>
              <div className="archive-empty-icon-box">
                {isLoadingArchive ? <LoaderCircle size={28} className="archive-import-spinner" /> : <Upload size={28} />}
              </div>
              <h3>{isLoadingArchive ? "正在解析 Android 文件" : "拖入 Android 包或原生 SO 库"}</h3>
              <p>{isLoadingArchive ? "正在读取文件结构和完整性信息。" : "支持 APK、AAR、JAR、AAB、APKS、ZIP、SO 及下载重名文件，例如 sdk.aar1、app.apk (1)。"}</p>
            </div>
          )}
        </article>
      </div>

      {/* 4. 侧边历史记录抽屉 (带防穿透实体背板与遮罩层) */}
      {historyOpen && (
        <>
          <div
            className="archive-history-backdrop"
            onClick={() => setHistoryOpen(false)}
          />
          <aside className="archive-history-overlay">
            <div className="archive-history-head">
              <h3>包历史记录</h3>
              <Button variant="subtle" size="xs" onClick={() => setHistoryOpen(false)}>
                ✕
              </Button>
            </div>
            <div className="archive-history-body custom-scrollbar">
              {historyLoading && <div className="archive-history-empty">正在读取解析历史...</div>}
              {!historyLoading && !archiveHistory.length && (
                <div className="archive-history-empty">暂无解析记录。成功打开 APK、AAR、JAR、AAB、APKS、ZIP 或 SO 后会自动保存在这里。</div>
              )}
              {!historyLoading && archiveHistory.map((record) => (
                <div className="archive-history-row" key={record.id}>
                  <strong title={record.archivePath}>{record.fileName}</strong>
                  <small>{formatArchiveHistoryTime(record.updatedAt)} · {record.sha256.slice(0, 12)}</small>
                  {record.workspacePath && <span className="archive-history-workspace">存在可恢复的 APK 编辑工作区</span>}
                  {record.outputPath && <span className="archive-history-output">已生成重编译 APK</span>}
                  <div className="archive-history-actions">
                    <Button
                      variant="secondary"
                      size="xs"
                      onClick={() => {
                        setHistoryOpen(false);
                        void loadArchive(record.archivePath);
                      }}
                    >
                      重新打开
                    </Button>
                    <Button
                      variant="danger"
                      size="xs"
                      icon={<Trash2 size={12} />}
                      title="仅删除历史记录，不删除本地文件"
                      onClick={() => void handleDeleteArchiveHistory(record)}
                    >
                      删除记录
                    </Button>
                  </div>
                </div>
              ))}
            </div>
          </aside>
        </>
      )}

      {searchOpen && (
        <div className="archive-content-search-overlay" onMouseDown={() => setSearchOpen(false)}>
          <section
            className="archive-content-search-dialog"
            role="dialog"
            aria-modal="true"
            aria-label="搜索包内内容"
            onMouseDown={(event) => event.stopPropagation()}
          >
            <header className="archive-content-search-header">
              <Input
                ref={searchInputRef}
                size="lg"
                prefixIcon={<Search size={18} />}
                value={searchQuery}
                placeholder="搜索类、字段、资源或文件名..."
                wrapperClassName="archive-content-search-input"
                onChange={(event) => setSearchQuery(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "ArrowDown") {
                    event.preventDefault();
                    setActiveSearchIndex((index) => Math.min(index + 1, Math.max(0, searchMatches.length - 1)));
                  }
                  if (event.key === "ArrowUp") {
                    event.preventDefault();
                    setActiveSearchIndex((index) => Math.max(index - 1, 0));
                  }
                  if (event.key === "Enter" && searchMatches[activeSearchIndex]) {
                    event.preventDefault();
                    openSearchMatch(searchMatches[activeSearchIndex]);
                  }
                }}
              />
              <Button
                variant="subtle"
                size="sm"
                icon={<X size={16} />}
                title="关闭搜索"
                aria-label="关闭搜索"
                onClick={() => setSearchOpen(false)}
              />
            </header>
            <div className="archive-content-search-status" aria-live="polite">
              {searching && <span><LoaderCircle size={14} className="archive-import-spinner" />正在搜索包内内容...</span>}
              {!searching && !searchQuery.trim() && <span>输入关键词后搜索文件路径、文本、DEX 与 Class 字符串。</span>}
              {!searching && searchResult && <span>找到 {searchMatches.length} 个结果，已扫描 {searchResult.scannedEntries} 个条目。</span>}
              {searchResult?.truncated && <span>结果已受容量限制截断。</span>}
            </div>
            <div className="archive-content-search-results custom-scrollbar" role="listbox" aria-label="包内搜索结果">
              {searchError && <div className="archive-content-search-message error">{searchError}</div>}
              {!searching && !searchError && searchQuery.trim() && searchResult && !searchMatches.length && (
                <div className="archive-content-search-message">没有匹配的文件或内容。</div>
              )}
              {searchMatches.map((match, index) => (
                <button
                  type="button"
                  className={`archive-content-search-result${activeSearchIndex === index ? " active" : ""}`}
                  role="option"
                  aria-selected={activeSearchIndex === index}
                  key={`${match.entryPath}-${match.kind}-${match.line || "path"}-${index}`}
                  onMouseEnter={() => setActiveSearchIndex(index)}
                  onFocus={() => setActiveSearchIndex(index)}
                  onClick={() => openSearchMatch(match)}
                >
                  <span className="archive-content-search-result-icon"><FileCode size={15} /></span>
                  <span className="archive-content-search-result-copy">
                    <b>{match.entryPath}</b>
                    <small>{match.preview}</small>
                  </span>
                  <span className="archive-content-search-result-meta">
                    <Badge variant="neutral">{match.kind}</Badge>
                    {match.line && <em>第 {match.line} 行{match.column ? `:${match.column}` : ""}</em>}
                  </span>
                </button>
              ))}
            </div>
            <footer className="archive-content-search-footer">
              <span><kbd>↑</kbd><kbd>↓</kbd> 选择 <kbd>Enter</kbd> 定位 <kbd>Esc</kbd> 关闭</span>
              <Button variant="subtle" size="sm" onClick={() => setSearchOpen(false)}>关闭</Button>
            </footer>
          </section>
        </div>
      )}
    </div>
  );
};

export default ArchiveInspectorView;
