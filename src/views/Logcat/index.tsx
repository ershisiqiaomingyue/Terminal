import { readToolWindowState, useToolWindowState } from "@/platform/toolWindowState";
import { subscribeAppMonoFont } from "@/platform/font";
import { TerminalHandoff, type TerminalChunk } from "@/platform/terminalHandoff";
import { ToolBackButton } from "@/components/ui/ToolBackButton";
import React, { useState, useMemo, useRef, useEffect, useLayoutEffect } from "react";
import { createPortal } from "react-dom";
import { useConfirmAction } from "@/components/ConfirmDialog/useConfirmAction";
import { formatInvokeError, invoke, isTauri, listen, listAdbDevices, openFile, saveFile, type AdbDevice } from "@/platform/runtime";
import {
  appendLogcatLines,
  createLogcatBuffer,
  findLogcatRow,
  layoutLogcatBuffer,
  readLogcatBufferBytes,
  rebuildLogcatFilter,
  replaceLogcatLines,
  resetLogcatBuffer,
  subscribeLogcatBuffer,
  type LogcatBuffer,
} from "@/platform/logcatBuffer";
import {
  ArrowDown,
  RefreshCw,
  Play,
  Square,
  History,
  Download,
  WrapText,
  Trash2,
  Bookmark,
  Upload,
  X,
  AlertCircle,
  FileText,
} from "lucide-react";
import { Button, Select, Input } from "@/components/ui";
import { ConfirmDialog } from "@/components/ConfirmDialog/ConfirmDialog";
import { useApplicationPageAction } from "@/application/useApplicationPageAction";
import { notify, publishAiTaskActivity, notifyDesktopRequired } from "@/platform/notifications";
import "./logcat.css";

export interface LogLine {
  id: string;
  raw: string;
  time: string;
  pid: string;
  tid: string;
  level: "V" | "D" | "I" | "W" | "E" | "F";
  tag: string;
  message: string;
}

export interface HistorySession {
  id: string;
  serial: string;
  startedAt: number;
  endedAt?: number;
  lineCount: number;
  endReason?: string;
}

export interface LogcatViewProps {
  onBack?: () => void;
  active?: boolean;
}

interface LogcatWindowSnapshot {
  selectedSerial: string; captureSerial: string; running: boolean; softWrap: boolean;
  level: string; query: string; sessionId: string; bookmarks: string[];
  lines: LogLine[]; sequence: number;
}

const FILTER_FIELDS = [
  { prefix: "tag:", label: "标签名，例如 ActivityTaskManager" },
  { prefix: "message:", label: "日志正文关键词" },
  { prefix: "package:", label: "应用包名，例如 com.android.launcher" },
  { prefix: "process:", label: "进程名" },
  { prefix: "level:", label: "日志等级 V / D / I / W / E / F" },
  { prefix: "pid:", label: "进程号" },
  { prefix: "tid:", label: "线程号" },
];

let logLineSeq = 0;
function parseLogLine(raw: string): LogLine {
  logLineSeq += 1;
  const id = `l-${logLineSeq}`;
  const threadtime = raw.match(/^(\d{2}-\d{2}\s+\d{2}:\d{2}:\d{2}\.\d+)\s+(\d+)\s+(\d+)(?:\s+\d+)?\s+([VDIWEF])\s+([^:]*?):\s?(.*)$/);
  if (threadtime) {
    return {
      id,
      raw,
      time: threadtime[1],
      pid: threadtime[2],
      tid: threadtime[3],
      level: threadtime[4] as LogLine["level"],
      tag: threadtime[5].trim(),
      message: threadtime[6],
    };
  }
  const brief = raw.match(/^([VDIWEF])\/(.*?)\(\s*(\d+)\):\s?(.*)$/);
  if (brief) {
    return { id, raw, time: "", pid: brief[3], tid: "", level: brief[1] as LogLine["level"], tag: brief[2].trim(), message: brief[4] };
  }
  return { id, raw, time: "", pid: "", tid: "", level: "I", tag: "", message: raw };
}

function matchesLogQuery(line: LogLine, level: string, query: string, processNames: Record<string, string>) {
  if (level !== "all" && line.level !== level) return false;
  const text = query.trim();
  if (!text) return true;
  return text.split("|").some((group) => group.split("&").every((term) => matchesLogTerm(line, term, processNames)));
}

function matchesLogTerm(line: LogLine, rawTerm: string, processNames: Record<string, string>) {
  const input = rawTerm.trim().toLowerCase();
  if (!input) return true;
  const negated = input.startsWith("-");
  const term = (negated ? input.slice(1) : input).trim();
  if (!term) return true;
  const separator = term.indexOf(":");
  let matched = false;
  if (separator < 0) {
    matched = line.raw.toLowerCase().includes(term)
      || line.tag.toLowerCase().includes(term)
      || line.message.toLowerCase().includes(term);
  } else {
    const field = term.slice(0, separator);
    const value = term.slice(separator + 1).trim();
    const processName = processNames[line.pid] || "";
    const source = field === "tag"
      ? line.tag
      : field === "message" || field === "msg"
        ? line.message
        : field === "level"
          ? line.level
          : field === "pid"
            ? line.pid
            : field === "tid"
              ? line.tid
              : field === "package" || field === "pkg" || field === "process" || field === "proc"
                ? `${processName} ${line.raw}`
                : line.raw;
    matched = !value || source.toLowerCase().includes(value);
  }
  return negated ? !matched : matched;
}

export const LogcatView: React.FC<LogcatViewProps> = ({ onBack, active = true }) => {
  const { confirmAction, confirmationDialog } = useConfirmAction();
  const desktop = isTauri();
  const restored = useRef(readToolWindowState<LogcatWindowSnapshot>("logcat")).current;
  const startingRef = useRef(false);
  const earlyEvents = useRef<Array<{ sessionId: string; serial: string; lines: string[]; sequence: number }>>([]);
  const handoff = useRef(new TerminalHandoff(restored?.running && restored.sessionId ? { [restored.sessionId]: restored.sequence } : {}));
  const activeRef = useRef(active);
  activeRef.current = active;
  // 设备状态
  const [devices, setDevices] = useState<AdbDevice[]>(
    []
  );
  const [selectedSerial, setSelectedSerial] = useState(restored?.selectedSerial || "");
  const [isRefreshing, setIsRefreshing] = useState(false);

  // 运行采集状态
  const [running, setRunning] = useState(restored?.running || false);
  const [softWrap, setSoftWrap] = useState(restored?.softWrap ?? true);
  const [level, setLevel] = useState<string>(restored?.level || "all");
  const [query, setQuery] = useState(restored?.query || "");
  const [filterFocused, setFilterFocused] = useState(false);
  const [activeSuggestion, setActiveSuggestion] = useState(-1);
  const [errorMsg, setErrorMsg] = useState("");

  useApplicationPageAction("open.tool.logcat", (action) => {
    if (action.parameters.serial !== undefined) {
      selectedSerialRef.current = action.parameters.serial;
      setSelectedSerial(action.parameters.serial);
    }
    if (action.parameters.query !== undefined) {
      queryRef.current = action.parameters.query;
      setQuery(action.parameters.query);
      applyLiveFilter(levelRef.current, action.parameters.query);
    }
    setErrorMsg("");
    publishAiTaskActivity({ running: true, status: "running", title: "正在启动 Logcat 采集", detail: "正在应用设备和日志过滤条件。", actionId: action.actionId, progress: 10 });
    // Serialize page handoffs so repeated model calls cannot start two captures.
    actionCaptureRef.current = actionCaptureRef.current.then(() => handleToggleCapture(action.parameters.serial, action.actionId));
  });

  const bufferRef = useRef<LogcatBuffer<LogLine>>(createLogcatBuffer<LogLine>());
  const restoredBuffer = useRef(false);
  if (!restoredBuffer.current) {
    restoredBuffer.current = true;
    const lines = restored?.lines || [];
    for (const line of lines) { const match = line.id.match(/^l-(\d+)$/); if (match) logLineSeq = Math.max(logLineSeq, Number(match[1])); }
    replaceLogcatLines(bufferRef.current, lines, (line) => matchesLogQuery(line, level, query, {}), readLogcatBufferBytes());
  }
  useToolWindowState("logcat", () => {
    if (startingRef.current) throw new Error("Logcat 正在连接，请连接完成后再拖出。");
    // Live output is replayed from Rust; keep a bounded recent viewport in the snapshot.
    let bytes = 0;
    const lines: LogLine[] = [];
    for (const line of [...bufferRef.current.lines, ...pendingRawRef.current.map(parseLogLine)].reverse()) {
      bytes += JSON.stringify(line).length * 3;
      if (bytes > 2 * 1024 * 1024) break;
      lines.push(line);
    }
    return { selectedSerial, captureSerial: captureSerialRef.current, running: runningRef.current, softWrap, level, query,
      sessionId: sessionIdRef.current, bookmarks, lines: lines.reverse(), sequence: handoff.current.sequences[sessionIdRef.current] || 0 };
  });
  const [revision, setRevision] = useState(0);
  const [bufferBytes, setBufferBytes] = useState(readLogcatBufferBytes);
  const [followLatest, setFollowLatest] = useState(true);
  const [droppedLines, setDroppedLines] = useState(0);
  const [unseenLines, setUnseenLines] = useState(0);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewportHeight, setViewportHeight] = useState(480);
  const viewportRef = useRef<HTMLDivElement>(null);
  const followRef = useRef(true);
  const visibleStartRef = useRef(0);
  const anchorRef = useRef<{ id: string; delta: number } | null>(null);
  const autoScrollRef = useRef(false);
  const levelRef = useRef(level);
  const queryRef = useRef(query);
  const softWrapRef = useRef(softWrap);
  const bufferBytesRef = useRef(bufferBytes);
  const viewportWidthRef = useRef(800);
  // 换等宽字体后每个字符的宽度会变（12px 下 6.4~7.9px 不等），
  // 行高估算必须按当前字体实测，否则软换行的高度会估错、滚动到底会抖。
  const charWidthRef = useRef(7.5);
  const pendingRawRef = useRef<string[]>([]);
  const flushScheduledRef = useRef(false);
  const processNamesRef = useRef<Record<string, string>>({});
  const filterAnchorRef = useRef<HTMLDivElement>(null);
  const [filterMenuBox, setFilterMenuBox] = useState({ top: 0, left: 0, width: 320 });
  const runningRef = useRef(running);
  const captureSerialRef = useRef(restored?.captureSerial || selectedSerial);
  const actionCaptureRef = useRef(Promise.resolve());
  const stopRequestedRef = useRef(false);
  const selectedSerialRef = useRef(selectedSerial);
  levelRef.current = level;
  queryRef.current = query;
  runningRef.current = running;
  selectedSerialRef.current = selectedSerial;
  softWrapRef.current = softWrap;
  bufferBytesRef.current = bufferBytes;



  const [bookmarks, setBookmarks] = useState<string[]>(restored?.bookmarks || []);
  const sessionIdRef = useRef(restored?.sessionId || "");

  // 历史会话抽屉
  const [historyOpen, setHistoryOpen] = useState(false);
  const [historySessions, setHistorySessions] = useState<HistorySession[]>(
    []
  );
  const [selectedSessionId, setSelectedSessionId] = useState<string>("");
  const [deleteTarget, setDeleteTarget] = useState<HistorySession | null>(null);
  const [deletingHistory, setDeletingHistory] = useState(false);
  const [deleteError, setDeleteError] = useState("");
  const deleteExecuting = useRef(false);

  const estimateRowHeight = (line: LogLine, wrap: boolean, width: number) => {
    if (!wrap) return 24;
    // 可见宽度扣掉时间、pid、级别、tag 几列后的正文宽度
    const availableWidth = Math.max(200, width - 280);
    const avgCharWidth = charWidthRef.current;
    const charsPerRow = Math.max(48, Math.floor(availableWidth / avgCharWidth));
    const contentLength = line.message.length + line.tag.length + 8;
    const rows = Math.min(6, Math.max(1, Math.ceil(contentLength / charsPerRow)));
    return 4 + rows * 20;
  };

  const publishBufferScheduledRef = useRef(false);
  const publishBuffer = () => {
    // Throttle publishBuffer to avoid excessive layout calculations
    if (!activeRef.current || publishBufferScheduledRef.current) return;
    publishBufferScheduledRef.current = true;

    requestAnimationFrame(() => {
      publishBufferScheduledRef.current = false;
      if (!activeRef.current) return;
      const buffer = bufferRef.current;
      layoutLogcatBuffer(buffer, (line) => estimateRowHeight(line, softWrapRef.current, viewportWidthRef.current), `${softWrapRef.current}:${viewportWidthRef.current}:${charWidthRef.current.toFixed(2)}`);

      // Batch state updates to reduce re-renders
      setRevision((value) => value + 1);
      if (droppedLines !== buffer.dropped || unseenLines !== buffer.unseen) {
        setDroppedLines(buffer.dropped);
        setUnseenLines(buffer.unseen);
      }
    });
  };

  const matchesLine = (line: LogLine, nextLevel = levelRef.current, nextQuery = queryRef.current) =>
    matchesLogQuery(line, nextLevel, nextQuery, processNamesRef.current);

  const applyLiveFilter = (nextLevel = levelRef.current, nextQuery = queryRef.current) => {
    levelRef.current = nextLevel;
    queryRef.current = nextQuery;
    rebuildLogcatFilter(bufferRef.current, (line) => matchesLine(line, nextLevel, nextQuery));
    publishBuffer();
  };

  // 日志正文用 12px 等宽字体（logcat.css 的 .logcat-line），实测一个字符多宽。
  useLayoutEffect(() => {
    const measure = () => {
      const stack = getComputedStyle(document.documentElement).getPropertyValue("--font-mono").trim() || "monospace";
      const context = document.createElement("canvas").getContext("2d");
      if (!context) return;
      context.font = `12px ${stack}`;
      const width = context.measureText("00000000000000000000").width / 20;
      if (!Number.isFinite(width) || width <= 0) return;
      if (Math.abs(width - charWidthRef.current) < 0.01) return;
      charWidthRef.current = width;
      publishBuffer();
    };
    measure();
    void document.fonts.ready.then(measure);
    return subscribeAppMonoFont(() => {
      void document.fonts.ready.then(measure);
    });
  }, []);

  const markCapturing = () => {
    captureSerialRef.current = selectedSerialRef.current;
    if (stopRequestedRef.current || runningRef.current) return;
    runningRef.current = true;
    setRunning(true);
  };

  const queueRawLines = (rawLines: string[]) => {
    const pending = pendingRawRef.current;
    for (const raw of rawLines) pending.push(raw);
    if (pending.length > 24_000) {
      const dropped = pending.length - 24_000;
      pending.splice(0, dropped);
      bufferRef.current.dropped += dropped;
    }
    if (flushScheduledRef.current) return;
    flushScheduledRef.current = true;
    requestAnimationFrame(() => {
      flushScheduledRef.current = false;
      const batch = pendingRawRef.current.splice(0, 400).map(parseLogLine);
      if (batch.length) {
        appendLogcatLines(
          bufferRef.current,
          batch,
          (line) => matchesLine(line),
          bufferBytesRef.current,
          !followRef.current,
        );
        markCapturing();
        publishBuffer();
      }
      if (pendingRawRef.current.length) queueRawLines([]);
    });
  };

  const handleRefreshDevices = async () => {
    if (!desktop) {
      notifyDesktopRequired();
      return;
    }
    setIsRefreshing(true);
    try {
      const next = await listAdbDevices();
      setDevices(next);
      const serial = next.some((item) => item.serial === selectedSerialRef.current && item.state === "device") ? selectedSerialRef.current : next.find((item) => item.state === "device")?.serial || "";
      setSelectedSerial(serial);
      if (desktop && serial) {
        invoke<Record<string, string>>("list_adb_processes", { serial })
          .then((names) => {
            processNamesRef.current = names;
            applyLiveFilter();
          })
          .catch(() => undefined);
      }
    } catch (reason) {
      setErrorMsg(formatInvokeError(reason));
    } finally {
      setIsRefreshing(false);
    }
  };

  const loadHistory = async () => {
    if (!desktop) return;
    const history = await invoke<HistorySession[]>("list_logcat_history");
    setHistorySessions(history);
  };

  const loadHistorySession = async (id: string) => {
    setSelectedSessionId(id);
    if (!desktop) return;
    try {
      if (runningRef.current) await invoke("stop_logcat", { serial: captureSerialRef.current });
      runningRef.current = false;
      const raw = await invoke<string[]>("read_logcat_history", { request: { id, limit: 20000 } });
      followRef.current = false;
      setFollowLatest(false);
      replaceLogcatLines(bufferRef.current, raw.map(parseLogLine), (line) => matchesLine(line), bufferBytesRef.current);
      bufferRef.current.unseen = 0;
      publishBuffer();
      sessionIdRef.current = id;
      setRunning(false);
      const marks = await invoke<Array<{ line: string }>>("list_logcat_bookmarks", { sessionId: id });
      setBookmarks(marks.map((item) => item.line));
      setHistoryOpen(false);
    } catch (reason) {
      setErrorMsg(formatInvokeError(reason));
    }
  };

  const filterSuggestions = useMemo(() => {
    if (!filterFocused) return [];
    const token = query.split(/[|&]/).pop()?.trim().toLowerCase() || "";
    if (token.includes(":")) return [];
    return FILTER_FIELDS.filter((field) => field.prefix.startsWith(token));
  }, [filterFocused, query]);

  useLayoutEffect(() => {
    if (!filterFocused || filterSuggestions.length === 0) return;
    const place = () => {
      const node = filterAnchorRef.current;
      if (!node) return;
      const rect = node.getBoundingClientRect();
      setFilterMenuBox({ top: rect.bottom + 6, left: rect.left, width: Math.max(rect.width, 320) });
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [filterFocused, filterSuggestions.length, query]);

  const insertSuggestion = (value: string) => {
    const current = queryRef.current;
    const index = Math.max(current.lastIndexOf("|"), current.lastIndexOf("&"));
    const next = `${current.slice(0, index + 1)}${index >= 0 ? " " : ""}${value}`;
    queryRef.current = next;
    setQuery(next);
    setActiveSuggestion(-1);
    setFilterFocused(false);
    applyLiveFilter(levelRef.current, next);
  };

  const handleFilterKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Escape") {
      setFilterFocused(false);
      setActiveSuggestion(-1);
      return;
    }
    if (!filterSuggestions.length) return;
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActiveSuggestion((current) => {
        const next = (current + 1) % filterSuggestions.length;
        window.requestAnimationFrame(() => {
          document.getElementById(`logcat-filter-option-${next}`)?.scrollIntoView({ block: "nearest" });
        });
        return next;
      });
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActiveSuggestion((current) => {
        const next = current <= 0 ? filterSuggestions.length - 1 : current - 1;
        window.requestAnimationFrame(() => {
          document.getElementById(`logcat-filter-option-${next}`)?.scrollIntoView({ block: "nearest" });
        });
        return next;
      });
    } else if (event.key === "Enter" && activeSuggestion >= 0) {
      event.preventDefault();
      insertSuggestion(filterSuggestions[activeSuggestion].prefix);
    }
  };

  const filteredCount = useMemo(() => {
    void revision;
    return bufferRef.current.filtered.length;
  }, [revision]);

  // 添加书签
  const toggleBookmark = (line: LogLine) => {
    const key = line.raw || line.message;
    setBookmarks((prev) => (prev.includes(key) ? prev.filter((item) => item !== key) : [...prev, key]));
    if (!desktop || !sessionIdRef.current) return;
    void invoke("add_logcat_bookmark", { request: { sessionId: sessionIdRef.current, line: line.raw } }).catch((reason) => {
      setErrorMsg(formatInvokeError(reason));
    });
  };

  // 清空日志
  const handleClearLogs = () => {
    resetLogcatBuffer(bufferRef.current);
    followRef.current = true;
    setFollowLatest(true);
    publishBuffer();
  };

  const filteredLines = () => bufferRef.current.filtered.map((index) => bufferRef.current.lines[index]);

  // 导出日志
  const handleExport = async () => {
    const content = filteredLines().map((line) => line.raw).join("\n");
    if (!desktop) {
      const blob = new Blob([content], { type: "text/plain;charset=utf-8" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `logcat-${new Date().toISOString().slice(0, 19).replace(/:/g, "-")}.log`;
      a.click();
      URL.revokeObjectURL(url);
      return;
    }
    const destination = await saveFile("导出当前日志", `logcat-${Date.now()}.log`, [{ name: "Log 文件", extensions: ["log", "txt"] }]);
    if (!destination) return;
    try {
      await invoke("export_logcat_content", { request: { destination, content: `${content}\n` } });
    } catch (reason) {
      setErrorMsg(formatInvokeError(reason));
    }
  };

  const handleImport = async () => {
    if (!desktop) return;
    const path = await openFile("导入 Logcat 文本", [{ name: "Log 文件", extensions: ["log", "txt"] }]);
    if (!path) return;
    try {
      const record = await invoke<HistorySession>("import_logcat_history", { request: { path, serial: selectedSerial } });
      await loadHistory();
      await loadHistorySession(record.id);
    } catch (reason) {
      setErrorMsg(formatInvokeError(reason));
    }
  };

  const handleToggleCapture = async (serialOverride?: string, actionId?: string) => {
    let targetSerial = serialOverride || selectedSerialRef.current;
    if (!desktop) {
      notifyDesktopRequired(actionId);
      return;
    }
    if (!targetSerial && actionId) {
      try {
        const next = await listAdbDevices();
        setDevices(next);
        targetSerial = next.find((device) => device.state === "device")?.serial || "";
      } catch (reason) {
        const message = formatInvokeError(reason);
        setErrorMsg(message);
        publishAiTaskActivity({ running: false, status: "error", title: "Logcat 未启动", detail: message, actionId });
        return;
      }
    }
    if (!targetSerial) {
      const message = "请选择一个已连接设备。";
      setErrorMsg(message);
      if (actionId) {
        notify(message, "warning");
        publishAiTaskActivity({ running: false, status: "error", title: "Logcat 未启动", detail: message, actionId });
      }
      return;
    }
    selectedSerialRef.current = targetSerial;
    setSelectedSerial(targetSerial);
    try {
      if (runningRef.current && (!actionId || captureSerialRef.current === targetSerial)) {
        if (actionId) {
          publishAiTaskActivity({ running: true, status: "running", title: "Logcat 正在采集", detail: `已连接 ${targetSerial}，保留当前日志会话。`, actionId, progress: null });
          return;
        }
        stopRequestedRef.current = true;
        runningRef.current = false;
        setRunning(false);
        await invoke("stop_logcat", { serial: captureSerialRef.current });
        await loadHistory();
        if (actionId) publishAiTaskActivity({ running: false, status: "done", title: "Logcat 采集已停止", detail: "目标设备的日志会话已关闭。", actionId, progress: 100 });
        return;
      }
      stopRequestedRef.current = false;
      resetLogcatBuffer(bufferRef.current);
      followRef.current = true;
      setFollowLatest(true);
      publishBuffer();
      startingRef.current = true;
      earlyEvents.current = [];
      sessionIdRef.current = await invoke<string>("start_logcat", { serial: targetSerial });
      startingRef.current = false;
      for (const event of earlyEvents.current) if (event.sessionId === sessionIdRef.current) handoff.current.accept({ ...event, data: event.lines.join("\n") }, (data) => queueRawLines(data.split("\n")));
      earlyEvents.current = [];
      captureSerialRef.current = targetSerial;
      runningRef.current = true;
      setBookmarks([]);
      setRunning(true);
      setErrorMsg("");
      if (actionId) publishAiTaskActivity({ running: true, status: "running", title: "Logcat 正在采集", detail: `已连接 ${targetSerial}，持续接收日志。`, actionId, progress: null });
    } catch (reason) {
      const message = formatInvokeError(reason);
      startingRef.current = false;
      earlyEvents.current = [];
      runningRef.current = false;
      setRunning(false);
      setErrorMsg(message);
      if (actionId) {
        notify(message, "error");
        publishAiTaskActivity({ running: false, status: "error", title: "Logcat 启动失败", detail: message, actionId });
      }
    }
  };

  const handleConfirmDeleteHistory = async () => {
    if (!deleteTarget || deleteExecuting.current) return;
    const target = deleteTarget;
    deleteExecuting.current = true;
    setDeletingHistory(true);
    setDeleteError("");
    try {
      if (desktop) await invoke("delete_logcat_history", { request: { id: target.id } });
      setHistorySessions((prev) => prev.filter((session) => session.id !== target.id));
      if (selectedSessionId === target.id) {
        setSelectedSessionId("");
        if (desktop) { resetLogcatBuffer(bufferRef.current); publishBuffer(); }
      }
      setDeleteTarget(null);
    } catch (reason) { setDeleteError(formatInvokeError(reason)); }
    finally { deleteExecuting.current = false; setDeletingHistory(false); }
  };

  useEffect(() => {
    void handleRefreshDevices();
    void loadHistory().catch((reason) => setErrorMsg(formatInvokeError(reason)));
    if (!desktop) return;
    let stop = false;
    const unlisten: Array<() => void> = [];
    const register = (dispose: () => void) => {
      if (stop) dispose();
      else unlisten.push(dispose);
    };
    void listen<{ serial: string; sessionId: string; sequence: number; lines: string[] }>("android:logcat", (event) => {
      if (stop) return;
      if (startingRef.current) { if (earlyEvents.current.length < 32) earlyEvents.current.push(event); return; }
      if (event.sessionId !== sessionIdRef.current || !runningRef.current) return;
      markCapturing();
      handoff.current.accept({ ...event, data: event.lines.join("\n") }, (data) => queueRawLines(data.split("\n")));
    }).then(async (remove) => {
      register(remove);
      if (!restored?.running || !restored.sessionId || stop) return;
      try {
        const chunks = await invoke<TerminalChunk[] | null>("read_logcat_window_output", { sessionId: restored.sessionId, afterSequence: restored.sequence });
        if (stop) return;
        if (chunks) handoff.current.recover(restored.sessionId, chunks, (data) => queueRawLines(data.split("\n")));
        else {
          const lines = await invoke<string[]>("read_logcat_history", { request: { id: restored.sessionId, limit: 20000 } });
          if (stop) return;
          replaceLogcatLines(bufferRef.current, lines.map(parseLogLine), (line) => matchesLine(line), bufferBytesRef.current);
          runningRef.current = false; setRunning(false); publishBuffer(); void loadHistory();
        }
      } catch (error) { if (!stop) { handoff.current.recover(restored.sessionId, [], (data) => queueRawLines(data.split("\n"))); setErrorMsg(formatInvokeError(error)); } }
    });
    void listen<{ serial: string; sessionId: string; reason: string }>("android:logcat-ended", (event) => {
      if (stop || event.sessionId !== sessionIdRef.current) return;
      stopRequestedRef.current = false;
      runningRef.current = false;
      setRunning(false);
      if (event.reason && event.reason !== "已停止采集。") setErrorMsg(event.reason);
      void loadHistory();
    }).then(register);
    return () => {
      stop = true;
      unlisten.forEach((dispose) => dispose());
    };
  }, [desktop]);

  useEffect(() => subscribeLogcatBuffer((bytes) => {
    bufferBytesRef.current = bytes;
    setBufferBytes(bytes);
    rebuildLogcatFilter(bufferRef.current, (line) => matchesLine(line));
    appendLogcatLines(bufferRef.current, [], (line) => matchesLine(line), bytes, false);
    publishBuffer();
  }), []);

  useEffect(() => {
    applyLiveFilter(level, query);
  }, [level, query, softWrap]);

  useEffect(() => {
    const viewport = viewportRef.current;
    if (!active || !viewport) {
      setFilterFocused(false);
      return;
    }
    const observer = new ResizeObserver(() => {
      if (!activeRef.current || !viewport.clientWidth || !viewport.clientHeight) return;
      viewportWidthRef.current = viewport.clientWidth;
      setViewportHeight(viewport.clientHeight);
      publishBuffer();
    });
    observer.observe(viewport);
    viewportWidthRef.current = viewport.clientWidth;
    setViewportHeight(viewport.clientHeight);
    publishBuffer();
    return () => observer.disconnect();
  }, [historyOpen, active]);

  const rememberAnchor = (top: number) => {
    const buffer = bufferRef.current;
    if (!buffer.filtered.length) {
      anchorRef.current = null;
      return;
    }
    const index = findLogcatRow(buffer.offsets, top);
    const line = buffer.lines[buffer.filtered[index]];
    if (!line) return;
    anchorRef.current = { id: line.id, delta: top - (buffer.offsets[index] || 0) };
  };

  const restoreAnchor = (viewport: HTMLDivElement) => {
    const anchor = anchorRef.current;
    const buffer = bufferRef.current;
    if (!anchor) return;
    let found = -1;
    for (let index = 0; index < buffer.filtered.length; index += 1) {
      if (buffer.lines[buffer.filtered[index]]?.id === anchor.id) {
        found = index;
        break;
      }
    }
    if (found < 0) {
      viewport.scrollTop = 0;
      const first = buffer.lines[buffer.filtered[0]];
      anchorRef.current = first ? { id: first.id, delta: 0 } : null;
      return;
    }
    const nextTop = Math.max(0, (buffer.offsets[found] || 0) + anchor.delta);
    const maxTop = Math.max(0, viewport.scrollHeight - viewport.clientHeight);
    viewport.scrollTop = Math.min(nextTop, maxTop);
  };

  useLayoutEffect(() => {
    const viewport = viewportRef.current;
    if (!active || !viewport) return;
    autoScrollRef.current = true;
    if (followRef.current) {
      viewport.scrollTop = viewport.scrollHeight;
      anchorRef.current = null;
    } else {
      restoreAnchor(viewport);
    }
    visibleStartRef.current = findLogcatRow(bufferRef.current.offsets, viewport.scrollTop);
    setScrollTop(viewport.scrollTop);
    requestAnimationFrame(() => {
      autoScrollRef.current = false;
    });
  }, [revision, followLatest, active]);

  const visibleRows = useMemo(() => {
    void revision;
    const buffer = bufferRef.current;
    const offsets = buffer.offsets;
    const total = Math.max(0, offsets.length - 1);
    if (!total) return { totalHeight: 0, rows: [] as Array<{ line: LogLine; top: number; height: number; key: string }> };
    const start = findLogcatRow(offsets, Math.max(0, scrollTop - 240));
    const end = Math.min(total, findLogcatRow(offsets, scrollTop + viewportHeight + 240) + 1);
    const rows = [];
    for (let index = start; index < end; index += 1) {
      const line = buffer.lines[buffer.filtered[index]];
      if (!line) continue;
      rows.push({
        line,
        top: offsets[index],
        height: offsets[index + 1] - offsets[index],
        key: line.id,
      });
    }
    return { totalHeight: offsets[total] || 0, rows };
  }, [revision, scrollTop, viewportHeight]);

  const stickToBottom = () => {
    followRef.current = true;
    bufferRef.current.unseen = 0;
    setFollowLatest(true);
    setUnseenLines(0);
    const viewport = viewportRef.current;
    if (!viewport) return;
    autoScrollRef.current = true;
    viewport.scrollTop = viewport.scrollHeight;
    setScrollTop(viewport.scrollTop);
    requestAnimationFrame(() => {
      autoScrollRef.current = false;
    });
  };

  const handleViewportScroll = () => {
    const viewport = viewportRef.current;
    if (!activeRef.current || !viewport || autoScrollRef.current) return;
    const distance = viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight;
    const atBottom = distance <= 1;
    if (!atBottom) {
      if (followRef.current) {
        followRef.current = false;
        setFollowLatest(false);
      }
      rememberAnchor(viewport.scrollTop);
    } else if (!followRef.current) {
      followRef.current = true;
      anchorRef.current = null;
      bufferRef.current.unseen = 0;
      setFollowLatest(true);
      setUnseenLines(0);
    }
    const start = findLogcatRow(bufferRef.current.offsets, viewport.scrollTop);
    if (start !== visibleStartRef.current) {
      visibleStartRef.current = start;
      setScrollTop(viewport.scrollTop);
    }
  };

  return (
    <div className="logcat-container">
      {confirmationDialog}
      {/* 顶部操作区与面包屑导航 */}
      <header className="logcat-top-header">
        <div className="logcat-header-left">
          <ToolBackButton onBack={onBack} />
          <div className="logcat-title-wrap">
            <span className="logcat-title">Logcat 实时日志流</span>
            <span className="logcat-subtitle">· Android 系统控制台抓取诊断</span>
          </div>
        </div>

        <div className="logcat-header-right">
          <div className="logcat-status-indicator">
            <span className={`status-dot ${running ? "live" : ""}`} />
            <span>{running ? "正在采集" : "尚未开始采集"}</span>
          </div>
        </div>
      </header>

      {/* 核心操作工具栏 */}
      <div className="logcat-toolbar-section">
        <div className="logcat-toolbar-left">
          {/* 设备选择框 */}
          <div style={{ width: 230 }}>
            <Select
              size="sm"
              value={selectedSerial}
              onChange={(val) => setSelectedSerial(val)}
              options={devices.map((d) => ({
                label: d.model || d.serial,
                value: d.serial,
              }))}
            />
          </div>

          {/* 刷新设备按钮 */}
          <Button
            size="sm"
            variant="secondary"
            onClick={() => void handleRefreshDevices()}
            disabled={isRefreshing}
            title="刷新设备列表"
          >
            <RefreshCw size={13} className={isRefreshing ? "animate-spin" : ""} />
          </Button>

          {/* 日志级别选择框 */}
          <div style={{ width: 110 }}>
            <Select
              size="sm"
              value={level}
              onChange={(val) => {
                setLevel(val);
                applyLiveFilter(val, queryRef.current);
              }}
              options={[
                { label: "全部等级", value: "all" },
                { label: "Verbose", value: "V" },
                { label: "Debug", value: "D" },
                { label: "Info", value: "I" },
                { label: "Warn", value: "W" },
                { label: "Error", value: "E" },
                { label: "Fatal", value: "F" },
              ]}
            />
          </div>

          {/* 表达式过滤输入框 + 联想下拉菜单 */}
          <div className="logcat-filter-wrapper" ref={filterAnchorRef}>
            <Input
              size="sm"
              value={query}
              onChange={(e) => {
                const next = e.target.value;
                queryRef.current = next;
                setQuery(next);
                setActiveSuggestion(-1);
                setFilterFocused(true);
                applyLiveFilter(levelRef.current, next);
              }}
              onKeyDown={handleFilterKeyDown}
              placeholder="实时筛选：直接输入，或 tag: / message: / pid: ；| 或，& 且，- 排除"
              onFocus={() => setFilterFocused(true)}
              onBlur={() => setTimeout(() => setFilterFocused(false), 200)}
              clearable
              onClear={() => {
                queryRef.current = "";
                setQuery("");
                applyLiveFilter(levelRef.current, "");
              }}
            />

            {filterFocused && filterSuggestions.length > 0 && createPortal(
              <div
                className="logcat-filter-popover"
                role="listbox"
                style={{ top: filterMenuBox.top, left: filterMenuBox.left, width: filterMenuBox.width }}
              >
                {filterSuggestions.map((item, index) => (
                  <button
                    key={item.prefix}
                    id={`logcat-filter-option-${index}`}
                    type="button"
                    role="option"
                    aria-selected={index === activeSuggestion}
                    className={`filter-suggestion-btn${index === activeSuggestion ? " is-active" : ""}`}
                    onMouseEnter={() => setActiveSuggestion(index)}
                    onMouseDown={(event) => {
                      event.preventDefault();
                      insertSuggestion(item.prefix);
                    }}
                  >
                    <span className="filter-suggestion-prefix">{item.prefix}</span>
                    <span className="filter-suggestion-desc">{item.label}</span>
                  </button>
                ))}
              </div>,
              document.body
            )}
          </div>
        </div>

        {/* 工具栏右侧操作按钮组 */}
        <div className="logcat-toolbar-right">
          {/* 日志行数统计 */}
          <span className="logcat-line-stat">
            <b>{filteredCount.toLocaleString()}</b> 条日志
            {droppedLines > 0 && (
              <span title="超出屏幕缓冲后丢弃最旧的显示行，完整内容仍在会话历史中">
                {" "}· 已丢弃 {droppedLines.toLocaleString()} 条
              </span>
            )}
          </span>

          {/* 历史抽屉开关 */}
          <Button
            size="sm"
            variant="secondary"
            className={historyOpen ? "is-active" : ""}
            aria-pressed={historyOpen}
            onClick={() => setHistoryOpen(!historyOpen)}
            title="历史会话与书签"
          >
            <History size={14} />
          </Button>

          {/* 导出当前日志 */}
          <Button
            size="sm"
            variant="secondary"
            onClick={() => void handleExport()}
            disabled={!filteredCount}
            title="导出当前筛选日志为 .log 文件"
          >
            <Download size={14} />
          </Button>

          {/* 自动换行切换 */}
          <Button
            size="sm"
            variant="secondary"
            className={softWrap ? "is-active" : ""}
            aria-pressed={softWrap}
            onClick={() => setSoftWrap(!softWrap)}
            title={softWrap ? "关闭自动换行" : "开启自动换行"}
          >
            <WrapText size={14} />
          </Button>

          {/* 清空日志 */}
          <Button
            size="sm"
            variant="secondary"
            className="btn-danger-hover"
            onClick={() => confirmAction({ title: "清空当前日志", message: "确认清空当前日志视图中的缓存？未保存的日志将丢失，已保存的历史日志不受影响。", confirmLabel: "确认清空", onConfirm: handleClearLogs })}
            title="清空当前日志控制台"
          >
            <Trash2 size={14} />
          </Button>

          {/* 开始/停止采集控制按钮 */}
          <Button
            size="sm"
            variant={running ? "danger" : "success"}
            onClick={() => void handleToggleCapture()}
            style={{ gap: 6, minWidth: 96 }}
          >
            {running ? <Square size={13} fill="currentColor" /> : <Play size={13} fill="currentColor" />}
            <span>{running ? "停止采集" : "开始采集"}</span>
          </Button>
        </div>
      </div>

      {/* 错误提示横条 (若存在) */}
      {errorMsg && (
        <div className="logcat-error-bar">
          <AlertCircle size={15} />
          <span>{errorMsg}</span>
          <button
            type="button"
            onClick={() => setErrorMsg("")}
            style={{ marginLeft: "auto", background: "none", border: 0, cursor: "pointer", color: "inherit" }}
          >
            <X size={14} />
          </button>
        </div>
      )}

      {/* 主体视口：日志流控制台与历史抽屉 */}
      <div className="logcat-main-stage">
        <div className="logcat-console-pane">
        <div
          ref={viewportRef}
          className={`logcat-console-viewport ${softWrap ? "soft-wrap" : ""}`}
          role="log"
          onScroll={handleViewportScroll}
          onWheel={(event) => {
            if (event.deltaY < 0) {
              const viewport = viewportRef.current;
              if (followRef.current) {
                followRef.current = false;
                setFollowLatest(false);
              }
              if (viewport) rememberAnchor(viewport.scrollTop);
            }
          }}
        >
          {filteredCount === 0 ? (
            <div className="logcat-empty-hint">
              <FileText size={36} style={{ opacity: 0.3 }} />
              <span>{query.trim() ? "没有符合当前筛选条件的日志。" : "选择设备并点击「开始采集」后，日志将实时流式展示于此。"}</span>
            </div>
          ) : (
            <div className="logcat-virtual-list" style={{ height: visibleRows.totalHeight }}>
              {visibleRows.rows.map((row) => (
                <div
                  key={row.key}
                  className={`logcat-log-row level-${row.line.level}`}
                  style={{ top: row.top, height: row.height }}
                  onDoubleClick={() => toggleBookmark(row.line)}
                  title="双击收藏此日志"
                >
                  <span className="log-cell-time">{row.line.time}</span>
                  <span className="log-cell-pid">{row.line.pid}/{row.line.tid}</span>
                  <span className="log-level-badge">{row.line.level}</span>
                  <span className="log-cell-tag">{row.line.tag}:</span>
                  <span className="log-cell-msg">{row.line.message}</span>
                  <button
                    type="button"
                    className="log-bookmark-btn"
                    onClick={(event) => {
                      event.stopPropagation();
                      toggleBookmark(row.line);
                    }}
                    title={bookmarks.includes(row.line.raw) ? "取消书签" : "添加书签"}
                  >
                    <Bookmark
                      size={13}
                      fill={bookmarks.includes(row.line.raw) ? "currentColor" : "none"}
                    />
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>
          {!followLatest && filteredCount > 0 && (
            <div className="logcat-jump-bottom">
              <Button
                variant="secondary"
                size="sm"
                icon={<ArrowDown size={13} />}
                onClick={stickToBottom}
                title="回到最新日志并继续跟随"
              >
                置底
                {unseenLines > 0 && (
                  <span className="logcat-jump-count">{unseenLines > 999 ? "999+" : unseenLines}</span>
                )}
              </Button>
            </div>
          )}
        </div>

        {/* 历史记录侧边抽屉 */}
        {historyOpen && (
          <aside className="logcat-history-drawer">
            <div className="logcat-history-header">
              <span className="logcat-history-title">Logcat 历史会话</span>
              <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                <Button size="sm" variant="secondary" title="导入本地日志文件" onClick={() => void handleImport()}>
                  <Upload size={13} />
                </Button>
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={() => setHistoryOpen(false)}
                  title="关闭历史抽屉"
                >
                  <X size={13} />
                </Button>
              </div>
            </div>

            <div className="logcat-history-list">
              {historySessions.map((session) => {
                const isSelected = session.id === selectedSessionId;
                return (
                  <div
                    key={session.id}
                    className={`history-card-item ${isSelected ? "is-selected" : ""}`}
                    onClick={() => void loadHistorySession(session.id)}
                  >
                    <div className="history-meta-block">
                      <span className="history-time-text">
                        {new Date(session.startedAt).toLocaleTimeString()}
                      </span>
                      <span className="history-sub-text">
                        {session.serial} · {session.lineCount.toLocaleString()} 行
                      </span>
                      {session.endReason && (
                        <span className="history-reason-badge">{session.endReason}</span>
                      )}
                    </div>
                    <Button
                      size="sm"
                      variant="secondary"
                      onClick={(e) => {
                        e.stopPropagation();
                        setDeleteTarget(session);
                      }}
                      title="删除此会话记录"
                    >
                      <Trash2 size={13} style={{ color: "var(--status-danger-text)" }} />
                    </Button>
                  </div>
                );
              })}
              {historySessions.length === 0 && (
                <div style={{ padding: 16, textAlign: "center", color: "var(--text-tertiary)", fontSize: 12 }}>
                  暂无已保存的日志会话。
                </div>
              )}
            </div>

            {/* 书签快速列表 */}
            {bookmarks.length > 0 && (
              <div className="logcat-bookmarks-section">
                <span className="bookmarks-title">当前会话书签 ({bookmarks.length})</span>
                {bookmarks.slice(0, 4).map((b, idx) => (
                  <div key={idx} className="bookmark-snippet" title={b}>
                    {b}
                  </div>
                ))}
              </div>
            )}
          </aside>
        )}
      </div>

      {/* 确认删除会话弹窗 */}
      <ConfirmDialog
        open={Boolean(deleteTarget)}
        title="删除 Logcat 历史记录"
        message="将永久删除该会话、所有关联书签及保存的原始日志。此操作无法恢复，是否继续？"
        confirmLabel="确认删除"
        busy={deletingHistory}
        error={deleteError}
        onConfirm={handleConfirmDeleteHistory}
        onCancel={() => { if (!deleteExecuting.current) { setDeleteTarget(null); setDeleteError(""); } }}
      />
    </div>
  );
};

export default LogcatView;
