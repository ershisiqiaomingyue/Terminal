import { lazy, Suspense, useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import { Spinner } from "rain-ui";
import { FilePreviewPanel } from "rain-ui";
import { FileDiffView } from "rain-ui";
import { Select } from "rain-ui";
import { getWorkspaceGitFileDiff } from "@/platform/chatApi";
import { formatInvokeError, invoke } from "@/platform/runtime";
import { ensureSshHostConnected } from "@/platform/sshHostConnection";
import { highlightFileMatches } from "@/views/CodeViewer/fileSearch";
import { escapeHtml, highlightSource, MAX_HIGHLIGHT_BYTES } from "@/views/CodeViewer/sourceHighlight";
import { parseFileDiff, type FilePreviewOrigin } from "./filePreviewDiff";
import { remotePreviewLocation, looksLocalPath, looksRemotePath, type FilePreviewTarget } from "./filePreviewTargets";
import { FileMetadataPopover } from "@/views/CodeViewer/FileMetadataPopover";
import "@/views/CodeViewer/code-viewer.css";
import "./chat-file-preview.css";

const MarkdownFilePreview = lazy(() => import("@/views/CodeViewer/MarkdownFilePreview").then((module) => ({ default: module.MarkdownFilePreview })));

/**
 * Chat file preview.
 *
 * The frame — header, source/diff switch, +N −N counts, search bar, path footer
 * and every toolbar button — is the library `FilePreviewPanel`. This file owns
 * only what is product-specific: reading bytes through Tauri, parsing the diff
 * against this turn's backup, highlighting and in-file match navigation.
 *
 * A preview reads from exactly one environment at a time (`previewTarget`):
 * the local workspace, or one SSH host of this conversation. Diff mode needs a
 * local turn backup, so it stays unavailable while reading over SSH.
 */
export default function ChatFilePreview({ path, origin, workspacePath, gitPath, className, onClose, previewTarget, previewTargets, onSelectPreviewTarget }: {
  path: string;
  origin?: FilePreviewOrigin;
  workspacePath?: string;
  gitPath?: string;
  className?: string;
  onClose: () => void;
  /** 当前从哪个环境读这个文件；缺省视为本机。 */
  previewTarget?: FilePreviewTarget | null;
  /** 可切换的环境清单；多于一项时在工具条上渲染单选下拉。 */
  previewTargets?: FilePreviewTarget[];
  onSelectPreviewTarget?: (key: string) => void;
}) {
  const panelRef = useRef<HTMLElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const [source, setSource] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [mode, setMode] = useState<"source" | "diff">(origin || workspacePath ? "diff" : "source");
  const [diff, setDiff] = useState<string | null>(null);
  const [diffError, setDiffError] = useState("");
  const [copied, setCopied] = useState(false);
  const [copyError, setCopyError] = useState("");
  const [query, setQuery] = useState("");
  const [matches, setMatches] = useState(0);
  const [matchIndex, setMatchIndex] = useState(0);
  const [markdownReady, setMarkdownReady] = useState(false);
  const [retry, setRetry] = useState(0);
  /** 环境选的是 SSH，但实际读的是本机副本（远程那份不存在或映射不出来）。 */
  const [fallbackLocal, setFallbackLocal] = useState(false);
  const searchQuery = useDeferredValue(query);
  const name = path.split(/[\\/]/).filter(Boolean).pop() || path;
  const markdown = /\.(md|markdown)$/i.test(path);
  const copyTimer = useRef<ReturnType<typeof setTimeout>>();
  const remote = previewTarget?.kind === "ssh";
  const targetKey = previewTarget?.key || "";
  // 清单里连一台 SSH 主机都没有时，远程路径在本机怎么读都读不到。
  const hasRemoteTarget = Boolean(previewTargets?.some((item) => item.kind === "ssh"));
  const location = useMemo(
    () => (previewTarget?.kind === "ssh" ? remotePreviewLocation(path, previewTarget, workspacePath) : null),
    [path, previewTarget, workspacePath],
  );
  const effectiveMode = remote ? "source" : mode;
  const text = effectiveMode === "diff" ? diff : source;
  const visibleError = effectiveMode === "diff" ? diffError : error;
  const highlighted = useMemo(() => highlightSource(source || "", path), [source, path]);
  const parsedDiff = useMemo(() => {
    if (diff === null) return null;
    const parsed = parseFileDiff(diff);
    return { ...parsed, rows: parsed.rows.map((row) => ({ ...row, html: row.kind === "meta" || diff.length > MAX_HIGHLIGHT_BYTES ? escapeHtml(row.text) : highlightSource(row.text, path) })) };
  }, [diff, path]);

  useEffect(() => {
    if (remote) { setDiff(null); setDiffError(""); return; }
    if (!origin && !workspacePath) return;
    let active = true;
    setDiff(null); setDiffError("");
    const request = workspacePath
      ? getWorkspaceGitFileDiff(workspacePath, gitPath || path)
      : invoke<string>("review_agent_file_change", { ...origin, path });
    void request.then((value) => {
      if (typeof value !== "string") throw new Error("文件差异未返回文本，请确认桌面服务已启动。");
      if (active) setDiff(value);
    }).catch((reason) => { if (active) setDiffError(formatInvokeError(reason)); });
    return () => { active = false; };
  }, [path, origin?.sessionId, origin?.turnId, workspacePath, gitPath, retry, remote]);

  useEffect(() => {
    let active = true;
    setSource(null);
    setError("");
    setMarkdownReady(false);
    const request = (async () => {
      if (previewTarget?.kind !== "ssh") {
        // 远程绝对路径交给 Windows 读只会得到「系统找不到指定的文件」。这种情况
        // 说明对话里没有任何可用的 SSH 主机，直接把原因说清楚，别让用户去猜。
        if (looksRemotePath(path) && !hasRemoteTarget) {
          throw new Error("这是远程文件，但当前对话没有可用的 SSH 服务器。请先在菜单栏连接该服务器，再从预览标题右侧选择它。");
        }
        return { text: await invoke<string>("read_code_viewer_file", { path }), via: "local" as const };
      }
      if (!location) {
        if (looksLocalPath(path)) {
          // 环境是 SSH、给的是本机路径：先按「工作区 → 远程根目录」映射去读远程那份，
          // 映射不出来（远程根是 `~` 之类）就直接读本机，别让一次环境选择把文件锁死。
          try {
            return { text: await invoke<string>("read_code_viewer_file", { path }), via: "local" as const };
          } catch {
            throw new Error("当前预览环境是 SSH，而这台机器的远程根目录不是绝对路径，读不到这个本机文件。请在上方环境里切回「本机」。");
          }
        }
        throw new Error("远程文件需要以 / 开头的绝对路径，无法定位要读取的文件。");
      }
      // 会话绑定并不等于传输通道已经建立，读文件前先确保这台主机连上。
      try {
        await ensureSshHostConnected(location.connectionId);
        return { text: await invoke<string>("read_ssh_code_viewer_file", location), via: "remote" as const };
      } catch (error) {
        if (!looksLocalPath(path)) throw error;
        // 远程那份还不存在（本地新改的文件常是这样）：退回本机副本，用户至少看得到内容。
        return { text: await invoke<string>("read_code_viewer_file", { path }), via: "local" as const };
      }
    })();
    void request.then((next) => {
      if (typeof next?.text !== "string") throw new Error("文件读取未返回文本，请确认桌面服务已启动。");
      if (!active) return;
      setSource(next.text);
      setFallbackLocal(next.via === "local" && previewTarget?.kind === "ssh");
    }).catch((reason) => { if (active) setError(formatInvokeError(reason)); });
    return () => { active = false; if (copyTimer.current) clearTimeout(copyTimer.current); };
  }, [path, retry, targetKey, location, previewTarget?.kind, hasRemoteTarget]);

  useEffect(() => {
    const root = contentRef.current?.querySelector<HTMLElement>("[data-file-search-root]");
    const result = root ? highlightFileMatches(root, searchQuery) : { matches: [] };
    setMatches(result.matches.length);
    setMatchIndex(0);
  }, [source, diff, effectiveMode, searchQuery, markdownReady, highlighted, parsedDiff]);

  useEffect(() => {
    const marks = contentRef.current?.querySelectorAll<HTMLElement>("mark[data-file-match]");
    marks?.forEach((mark) => mark.classList.toggle("is-current", Number(mark.dataset.fileMatch) === matchIndex));
    contentRef.current?.querySelector(`[data-file-match="${matchIndex}"]`)?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [matchIndex, matches, searchQuery]);

  const moveMatch = (offset: number) => { if (matches) setMatchIndex((current) => (current + offset + matches) % matches); };

  const copy = async () => {
    if (text === null) return;
    try {
      await navigator.clipboard.writeText(text);
      if (!panelRef.current?.isConnected) return;
      setCopied(true);
      setCopyError("");
      if (copyTimer.current) clearTimeout(copyTimer.current);
      copyTimer.current = setTimeout(() => setCopied(false), 1500);
    } catch (reason) {
      if (panelRef.current?.isConnected) setCopyError(`复制失败：${formatInvokeError(reason)}`);
    }
  };

  return (
    <FilePreviewPanel
      path={path}
      name={name}
      mode={effectiveMode}
      onModeChange={(next) => { setMode(next); setCopied(false); setCopyError(""); }}
      showModes={!remote && Boolean(origin || workspacePath)}
      added={parsedDiff?.added ?? 0}
      removed={parsedDiff?.removed ?? 0}
      partial={parsedDiff?.partial}
      searchable
      query={query}
      onQueryChange={setQuery}
      matchIndex={matchIndex}
      matches={matches}
      onMoveMatch={moveMatch}
      onClose={onClose}
      onCopy={() => void copy()}
      copied={copied}
      onRetry={() => setRetry((value) => value + 1)}
      loading={text === null && !visibleError}
      docked
      titleNote={fallbackLocal ? "本机副本 · 只读" : remote ? "SSH · 只读" : "只读预览"}
      className={["chat-file-preview-panel", className].filter(Boolean).join(" ")}
      headerExtra={
        <>
          {previewTarget && (previewTargets?.length || 0) > 1 && onSelectPreviewTarget && (
            <Select
              aria-label="预览环境"
              className="chat-file-preview-env"
              value={previewTarget.key}
              onChange={onSelectPreviewTarget}
              options={previewTargets!.map((item) => ({
                // 头部很窄：本机就是「本机」，远程一律写「SSH」，主机名放进下拉的说明行。
                value: item.key,
                label: item.kind === "ssh" ? "SSH" : "本机",
                description: item.root ? `${item.label} · ${item.root}` : item.label,
              }))}
            />
          )}
          <FileMetadataPopover
            cacheKey={`${targetKey}:${path}`}
            load={() => (previewTarget?.kind === "ssh" && location
              ? invoke("ssh_code_viewer_file_metadata", location)
              : invoke("code_viewer_file_metadata", { path }))}
          />
        </>
      }
      footerNote={
        <>
          {copyError && <p className="chat-file-preview-notice" role="alert">{copyError}</p>}
          {remote && <small className="chat-file-preview-notice is-quiet">{fallbackLocal ? "远程那份读不到，显示的是本机副本" : "远程只读预览"}</small>}
          {effectiveMode === "source" && source !== null && source.length > MAX_HIGHLIGHT_BYTES && !markdown && (
            <small className="chat-file-preview-notice">文件较大，已关闭语法高亮。</small>
          )}
        </>
      }
      ref={panelRef}
    >
      <div ref={contentRef} className="chat-file-preview-content">
        {visibleError ? (
          <div className="chat-file-preview-notice chat-file-preview-empty-state" role="alert"><span>{visibleError}</span></div>
        ) : text === null ? (
          <div className="chat-file-preview-notice" role="status"><Spinner size={16} className="is-inline" />{effectiveMode === "diff" ? "正在读取差异" : remote ? "正在读取远程文件" : "正在读取文件"}</div>
        ) : effectiveMode === "diff" && parsedDiff ? (
          <FileDiffView rows={parsedDiff.rows} truncated={parsedDiff.truncated}
            renderLine={(_, index) => <span dangerouslySetInnerHTML={{ __html: parsedDiff.rows[index].html }} />} />
        ) : markdown ? (
          <Suspense fallback={<div className="chat-file-preview-notice">正在加载 Markdown</div>}>
            <MarkdownFilePreview source={source!} showOutline={false} />
            <MarkdownReady onReady={() => setMarkdownReady(true)} />
          </Suspense>
        ) : (
          <pre className="chat-file-preview-source code-viewer-preview" data-file-search-root><code dangerouslySetInnerHTML={{ __html: highlighted }} /></pre>
        )}
      </div>
    </FilePreviewPanel>
  );
}

function MarkdownReady({ onReady }: { onReady: () => void }) {
  useEffect(onReady, []);
  return null;
}
