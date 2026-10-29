import { ComposerAttachmentCard } from "./ComposerAttachmentCard";
import React, {
  lazy,
  Suspense,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { setDiagnosticContext } from "@/platform/frontendHealth";
import { readComponentDebugTargets, subscribeComponentDebugTargets } from "@/platform/componentDebugPreferences";
import { ConversationRail } from "./ConversationRail";
import { ComposerChangedFiles } from "./ComposerChangedFiles";
import { ComposerNoticeDock } from "./ComposerNoticeDock";
import { BackgroundTasks } from "./BackgroundTasks";
import { ContextUsageIndicator } from "./ContextUsageIndicator";
import { AccessModeMenu } from "@/components/ui/AccessModeMenu";
import { formatTokenCount } from "./tokenCount";
import { latestContextUsageDetails, resolveContextUsageScan } from "./contextUsageDetails";
import {
  AppWindow,
  ArrowDown,
  ArrowUp,
  Bot,
  Bug,
  Check,
  FileCode2,
  FilePenLine,
  FolderOpen,
  Info,
  PanelLeft,
  Pencil,
  Search,
  Send,
  ShieldCheck,
  Sparkles,
  Square,
  SquareTerminal,
  Wrench,
  X,
} from "lucide-react";
import { chooseDirectory, listAgentProviders, listSkills, SKILLS_CHANGED_EVENT, type AgentProvider } from "@/platform/api";
import { environmentTargetForSsh } from "@/platform/executionEnvironment";
import { AGENT_PROVIDERS_CHANGED_EVENT } from "@/platform/agentProviderEvents";
import { effectiveReasoningEffort } from "@/platform/modelCapabilities";
import { useGlobalEnvironment } from "@/platform/globalEnvironment";
import {
  archiveHistorySession,
  cancelAgentRequest,
  deleteHistorySession,
  enqueueAgentMessage,
  guideQueuedAgentMessage,
  exportHistorySessions,
  forkConversationThread,
  getProviderQuota,
  getHistorySession,
  readLocalAttachmentForCopy,
  removeQueuedAgentMessage,
  reorderQueuedAgentMessages,
  updateQueuedAgentMessage,
  resolveConversationPermission,
  resumeConversationTurn,
  rewindConversationBeforeMessage,
  startConversationTurn,
  recoverCollaborationAgents,
  steerAgentRequest,
  listHistorySummaries,
  searchHistorySessions,
  saveHistorySession,
  storeBrowserAttachment,
  prepareChatAttachment,
  type AgentEventPayload,
  type AgentImageAttachment,
  type AgentQuota,
  type HistoryRecord,
  indexLocalLogs,
  searchLocalLogs,
  readLocalLogRange,
  rebuildLocalLogTimeline,
  getWorkspaceGitChanges,
  getWorkspaceEnvironment,
  type WorkspaceEnvironment,
  type WorkspaceGitChange,
} from "@/platform/chatApi";
import { APP_ACTION_CATALOG } from "@/application/pageActions";
import { BorderGlow } from "@/components/BorderGlow/BorderGlow";
import { ConfirmDialog } from "@/components/ConfirmDialog/ConfirmDialog";
import { AgentModelPicker } from "@/components/AgentModelPicker/AgentModelPicker";
import { notify } from "@/platform/notifications";
import { Button } from "@/components/ui/Button";
import { Tooltip } from "@/components/ui/Tooltip";
import { Spinner } from "@/components/ui/Spinner";
import { ChatTerminalPanel } from "./ChatTerminalPanel";
import { ChatImagePreview } from "./ChatImagePreview";
import { ComposerInput } from "./ComposerInput";
import { ComposerAddMenu } from "./ComposerAddMenu";
import { ChatPermissionBar } from "./ChatPermissionBar";
import { GoalBadge, activeGoalFromSnapshot } from "./GoalBadge";
import { ChatQueuePanel } from "./ChatQueuePanel";
import { submittedTaskIds } from "./activityState";
import { activeComposerTaskPlan } from "./taskPlanSnapshot";
import {
  ComposerTaskList,
  composerFileChanges,
} from "./ChatTaskPanels";
import { abandonedDraftIds, conversationHasHistory, findEmptyConversation } from "./newConversation";
import { ChatFilePreviewBoundary } from "./ChatFilePreviewBoundary";
import "./chat-debug-inline.css";
import { ChatDebugFileChanges, ChatDebugNotices, ChatDebugPicker } from "./ChatDebugNotices";
import type { FilePreviewOrigin } from "./filePreviewDiff";
import { useTurnQuotaChange } from "./useTurnQuotaChange";
import { quotaConsumptionKey, useQuotaConsumptionRefresh } from "./useQuotaConsumptionRefresh";
import { mergeHistorySummary, sanitizeAssistantDisplayText, terminalForMessage } from "./historyRecovery";
import { useConversationScroll } from "./useConversationScroll";
import { useConversationVirtualizer } from "./useConversationVirtualizer";
import { useAgentThreadSnapshot } from "./useAgentThreadSnapshot";
import { conversationExecutionTarget, conversationEnvironmentError } from "./sessionScopedState";
import { BOUND_ENVIRONMENTS_EVENT, executionTargetsPayload, sessionBoundEnvironments, updateSessionBoundEnvironments, type BindEnvironmentDetail } from "./conversationEnvironments";
import { filePreviewTargets, resolvePreviewTarget } from "./filePreviewTargets";
import {
  readChatUsage,
  sessionUsageWithLive,
  visibleTurnUsage,
  type TurnPricing,
} from "./chatUsage";
import { ChatUsageSummary } from "./ChatUsageSummary";
import { hasQuotaAmounts, quotaCacheKey, readQuotaCache, supportsAutomaticQuotaRefresh } from "@/platform/providerQuota";
import { contextUsageForDisplay, ModelRequestGuard, reconcileModelSelection } from "./modelSelectionState";
import { activeThreadTurnId, projectLiveThreadMessages } from "./threadSnapshotMessages";
import { messageFileChanges, pendingFileChanges } from "./fileChangePresentation";
import {
  snapshotExecutionTarget,
  snapshotIsSending,
  conversationIsSending,
  snapshotPermission,
  snapshotQueuedMessages,
} from "./threadSnapshotRuntime";
import { readThinkingCopy, subscribeThinkingCopy } from "@/platform/thinkingCopy";
import { collapseSettledTimelines, collapseTimelines, retainLiveTimelines, timelineStateKey, updateTimelineExpanded } from "./timelineState";
import { resumableInterruptedTurnId } from "./interruptedRunPresentation";
import { uniqueChatSessions, workspaceKey } from "./chatIdentity";
import { localArtifactPathKey, markLocalArtifactSources } from "./localArtifactPaths";
import { toolEnvironmentLabel } from "./toolIdentity";
import { updateSessionModelSelection, updateSessionSources } from "./sessionScopedState";
import { pruneSessionDraft, readSessionDraft, writeSessionDraft } from "./sessionDraftStore";
import type {
  ChatMessage,
  ChatSession,
  ChatSource,
  ChatSourceKind,
  ComposerTaskStep,
  ExecutionTarget,
  InferenceLevel,
  LivePermissionRequest,
  SandboxMode,
  TaskPlanSettings,
} from "./types";
import type { AgentTurnStartReceipt } from "@/platform/chatApi";
import {
  CHAT_NAV_EVENT,
  CHAT_SESSIONS_STORAGE_KEY,
  CHAT_WORKSPACES_STORAGE_KEY,
  acknowledgeChatNavDetail,
  readChatNavSnapshot,
  readChatSessions,
  readDeletedChatSessionIds,
  markChatSessionDeleted,
  publishChatSessions,
  readWorkspaces,
  takePendingChatNavDetails,
  writeChatNavSnapshot,
  type ChatNavDetail,
  type WorkspaceEntry,
} from "./chatNavStore";
import { CollaborationSidePanel } from "@/views/Collaboration/CollaborationSidePanel";
import { PaneResizeHandle } from "@/components/ResizablePane/PaneResizeHandle";
import { usePaneWidth } from "@/components/ResizablePane/usePaneWidth";
import { WORKSPACE_PANE } from "@/components/ResizablePane/paneWidth";
import {
  cancelAndroidTask,
  formatInvokeError,
  invoke,
  isTauri,
  listAndroidTasks,
  listen,
  openFile,
  saveFile,
  type AndroidTask,
} from "@/platform/runtime";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import "@/components/BorderGlow/border-glow.css";
import "./conversation.css";
import "./chatAppearance.css";
import "./styles/conversation-surfaces.css";
import "./chat-notices.css";
import "./styles/conversation-notice-dock.css";
import {
  loadSessions,
  upsertHistorySession,
  historyRecordToSession,
  sessionToHistoryInput,
} from "./sessionHistory";
import { assistantResultText, formatHistoryTime, toolStepTitle } from "./chatMessagePresentation";
import {
  classifyDropped,
  readBrowserFile,
  richTextToMarkdown,
} from "./chatSources";
import { projectAutomationContext } from "./projectAutomationContext";
import { takeLegacyProjectRecovery, writeProjectPreference } from "@/platform/projectPreferences";
import { cacheBrowserValue } from "@/platform/browserStorage";
import { recoverWorkspaceEntries } from "./chatIdentity";
import { ChatMessageRow } from "./ChatMessageRow";
import { conversationHistoryReadAhead } from "./historyReadAhead";
import { consumeComposerText, messageAttachmentsToSources, remainingComposerSources, restoreComposerSources } from "./composerSubmission";
import { createAttachmentPreparation, preparedSource } from "./attachmentPreparation";
import { serializeChatCache } from "./chatCacheSerialization";

const ChatFilePreview = lazy(() => import("./ChatFilePreview"));

const STORAGE_KEY = CHAT_SESSIONS_STORAGE_KEY;

const WORKSPACE_KEY = CHAT_WORKSPACES_STORAGE_KEY;

const CHAT_SELECTION_KEY = "android-tool.chat.selection.v1";

const INFERENCE_LEVEL_KEY = "android-tool.chat.inference-level.v1";

const TASK_PLAN_SETTINGS_KEY = "android-tool.chat.task-plan-settings.v1";

const CHAT_CACHE_SAVE_DEBOUNCE_MS = 500;

const DEFAULT_TASK_PLAN_SETTINGS: TaskPlanSettings = { mode: "auto", minSteps: 3 };

function readTaskPlanSettings(): TaskPlanSettings {
  try {
    const value = JSON.parse(window.localStorage.getItem(TASK_PLAN_SETTINGS_KEY) || "{}") as Partial<TaskPlanSettings>;
    return {
      mode: value.mode === "always" || value.mode === "off" ? value.mode : "auto",
      minSteps: Math.max(2, Math.min(5, Number(value.minSteps) || DEFAULT_TASK_PLAN_SETTINGS.minSteps)),
    };
  } catch {
    return DEFAULT_TASK_PLAN_SETTINGS;
  }
}

// TODO(Jira): register the existing native Jira commands and an explicit,
// credential-aware issue context workflow when Jira is brought back into scope.

function readChatSelection(): { providerId: string; modelId: string; sandboxMode: SandboxMode } | null {
  try {
    const parsed = JSON.parse(window.localStorage.getItem(CHAT_SELECTION_KEY) || "");
    if (!parsed || typeof parsed.providerId !== "string") return null;
    const sandboxMode = parsed.sandboxMode === "risk" || parsed.sandboxMode === "full" ? parsed.sandboxMode : "ask";
    return { providerId: parsed.providerId, modelId: typeof parsed.modelId === "string" ? parsed.modelId : "", sandboxMode };
  } catch {
    return null;
  }
}

function estimateTokens(text: string) {
  return Math.ceil(text.length / 2);
}

function formatContextWindow(value?: number | null) {
  if (!value || value <= 0) return "";
  if (value >= 1_000_000) {
    const mega = value / 1_000_000;
    return `${Number.isInteger(mega) ? mega : mega.toFixed(1)}M`;
  }
  if (value >= 1_000) {
    const kilo = value / 1000;
    return `${Number.isInteger(kilo) ? kilo : kilo.toFixed(1)}K`;
  }
  return String(value);
}

interface ChatViewProps {
  active?: boolean;
  navigationExpanded?: boolean;
  onToggleNavigation?: () => void;
}

type PendingRemoval = { kind: "session"; id: string; title: string } | { kind: "workspace"; path: string; title: string };

export const ChatView: React.FC<ChatViewProps> = React.memo(({ active = true, navigationExpanded = true, onToggleNavigation }: ChatViewProps) => {
  const [thinkingCopy, setThinkingCopy] = useState(readThinkingCopy);
  useEffect(() => subscribeThinkingCopy(setThinkingCopy), []);
  const [initialDeletedSessionIds] = useState(readDeletedChatSessionIds);
  const deletedSessionIds = useRef(initialDeletedSessionIds);
  const deletingSessionIds = useRef(new Set<string>());
  const pendingHistorySaves = useRef(new Map<string, Set<Promise<void>>>());
  const rewindingSessionIds = useRef(new Set<string>());
  const saveSessionHistory = useCallback((input: Parameters<typeof saveHistorySession>[0]) => {
    conversationHistoryReadAhead.invalidate(input.id);
    if (deletedSessionIds.current.has(input.id) || deletingSessionIds.current.has(input.id) || rewindingSessionIds.current.has(input.id)) {
      return Promise.reject(new Error("该对话正在删除或已删除"));
    }
    const pending = pendingHistorySaves.current.get(input.id) || new Set<Promise<void>>();
    const request = saveHistorySession(input);
    pending.add(request);
    pendingHistorySaves.current.set(input.id, pending);
    const settled = () => {
      pending.delete(request);
      if (!pending.size) pendingHistorySaves.current.delete(input.id);
    };
    void request.then(settled, settled);
    return request;
  }, []);
  const [sessions, setSessionsState] = useState<ChatSession[]>(() => loadSessions());
  const setSessions = useCallback<React.Dispatch<React.SetStateAction<ChatSession[]>>>((update) => {
    setSessionsState((previous) => {
      const next = typeof update === "function" ? update(previous) : update;
      return deletedSessionIds.current.size ? next.filter((item) => !deletedSessionIds.current.has(item.id)) : next;
    });
  }, []);
  const [sessionId, setSessionId] = useState<string>(() => {
    const saved = readChatNavSnapshot().sessionId;
    const localSessions = sessions;
    if (saved && !deletedSessionIds.current.has(saved) && (!localSessions.length || localSessions.some((item) => item.id === saved && !item.archived))) return saved;
    return [...localSessions].filter((item) => !item.archived).sort((left, right) => right.updatedAt - left.updatedAt)[0]?.id || "";
  });
  const [title, setTitle] = useState("新的对话");
  const [draft, setDraft] = useState("");
  /**
   * True while a session switch is restoring that session's stored draft, so
   * the draft-persistence effect below does not immediately write the
   * restored value back (and cannot race the switch itself).
   */
  const restoringDraftRef = useRef(false);
  const composerEditorRef = useRef<HTMLDivElement | null>(null);
  const captureComposerEditor = useCallback((element: HTMLDivElement | null) => { composerEditorRef.current = element; }, []);
  const [editingMessageId, setEditingMessageId] = useState("");
  const editingSourcesBeforeRef = useRef<ChatSource[] | null>(null);
  const [historyOpen, setHistoryOpen] = useState(false);
  const [historyQuery, setHistoryQuery] = useState("");
  const [historySearchResults, setHistorySearchResults] = useState<ChatSession[] | null>(null);
  const [debouncedHistoryQuery, setDebouncedHistoryQuery] = useState("");
  const [terminalOpen, setTerminalOpen] = useState(false);
  const [collaborationOpen, setCollaborationOpen] = useState(false);
  const [collaborationSideView, setCollaborationSideView] = useState<"agents" | "changes">("agents");
  /* 组件调试：勾选后立即把对应组件渲染到它在对话页的真实位置，
     不需要先发一条消息。 */
  const [componentsDebugOpen, setComponentsDebugOpen] = useState(false);
  const [componentsDebug, setComponentsDebug] = useState<Record<string, boolean>>({});
  const debugAnchorRef = useRef<HTMLDivElement>(null);
  const debugMenuRef = useRef<HTMLDivElement>(null);
  const [debugPickerPosition, setDebugPickerPosition] = useState({ top: 92, right: 16 });
  const [debugTargets, setDebugTargets] = useState(readComponentDebugTargets);
  useEffect(() => subscribeComponentDebugTargets(setDebugTargets), []);
  useEffect(() => {
    if (!debugTargets.chat) setComponentsDebugOpen(false);
  }, [debugTargets.chat]);
  useEffect(() => {
    if (!componentsDebugOpen) return;
    const outside = (event: PointerEvent) => {
      const target = event.target as Node;
      if (!debugAnchorRef.current?.contains(target) && !debugMenuRef.current?.contains(target)) setComponentsDebugOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      event.preventDefault();
      setComponentsDebugOpen(false);
      debugAnchorRef.current?.querySelector<HTMLButtonElement>("button")?.focus({ preventScroll: true });
    };
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("pointerdown", outside, true);
      document.removeEventListener("keydown", escape);
    };
  }, [componentsDebugOpen]);
  useEffect(() => {
    if (!componentsDebugOpen) return;
    const update = () => {
      const rect = debugAnchorRef.current?.getBoundingClientRect();
      const footer = workspaceRef.current?.querySelector<HTMLElement>(".chat-footer");
      const footerRect = footer?.getBoundingClientRect();
      if (!rect || !footerRect) return;
      setDebugPickerPosition({
        top: rect.bottom - footerRect.top + 14,
        right: footerRect.right - rect.right,
      });
    };
    update();
    window.addEventListener("resize", update);
    window.addEventListener("scroll", update, true);
    return () => {
      window.removeEventListener("resize", update);
      window.removeEventListener("scroll", update, true);
    };
  }, [componentsDebugOpen]);
  const toggleComponentDebug = (key: string) => setComponentsDebug((prev) => ({ ...prev, [key]: !prev[key] }));
  const sidePanelLayoutRef = useRef<HTMLDivElement>(null);
  const [environmentOpen, setEnvironmentOpen] = useState(false);
  const environmentAnchorRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!environmentOpen) return;
    const outside = (event: PointerEvent) => {
      if (!environmentAnchorRef.current?.contains(event.target as Node)) setEnvironmentOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      event.preventDefault();
      setEnvironmentOpen(false);
      environmentAnchorRef.current?.querySelector<HTMLButtonElement>("button")?.focus({ preventScroll: true });
    };
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("pointerdown", outside, true);
      document.removeEventListener("keydown", escape);
    };
  }, [environmentOpen]);
  const [taskPlanOpen, setTaskPlanOpen] = useState(false);
  const [taskPlanSettings, setTaskPlanSettings] = useState<TaskPlanSettings>(() => readTaskPlanSettings());
  const [sandboxOpen, setSandboxOpen] = useState(false);
  const [composerMenuOpen, setComposerMenuOpen] = useState(false);
  const [quotaPhase, setQuotaPhase] = useState<"idle" | "checking" | "done" | "error">("idle");
  const [quota, setQuota] = useState<AgentQuota | null>(null);
  const [quotaKey, setQuotaKey] = useState("");
  const [quotaCheckedAt, setQuotaCheckedAt] = useState<number | null>(null);
  const [quotaError, setQuotaError] = useState("");
  const [quotaAutoRefreshBlockedKey, setQuotaAutoRefreshBlockedKey] = useState("");
  const [contextChecking, setContextChecking] = useState(false);
  const [standaloneSources, setStandaloneSources] = useState<ChatSource[]>([]);
  const [standaloneProjectIds, setStandaloneProjectIds] = useState<string[] | undefined>();
  const [dropActive, setDropActive] = useState(false);
  const [dropNotice, setDropNotice] = useState("");
  const [fullAccessOpen, setFullAccessOpen] = useState(false);
  const [pendingRemoval, setPendingRemoval] = useState<PendingRemoval | null>(null);
  const [removalBusy, setRemovalBusy] = useState(false);
  const [removalError, setRemovalError] = useState("");
  const [removeWorkspaceSessions, setRemoveWorkspaceSessions] = useState(false);
  const removalExecuting = useRef(false);
  // 归档是不可见的「移出列表」操作，误点后会话会从侧栏消失，必须二次确认。
  const [pendingArchive, setPendingArchive] = useState<{ id: string; title: string } | null>(null);
  const [archiveBusy, setArchiveBusy] = useState(false);
  const [savedSelection] = useState(readChatSelection);
  const [sandboxMode, setSandboxMode] = useState<SandboxMode>(savedSelection?.sandboxMode || "ask");
  const [providers, setProviders] = useState<AgentProvider[]>([]);
  const providersRef = useRef(providers);
  providersRef.current = providers;
  const [providerRevision, setProviderRevision] = useState(0);
  const [providerId, setProviderId] = useState(savedSelection?.providerId || "");
  const [modelId, setModelId] = useState(savedSelection?.modelId || "");
  const modelSelectionRef = useRef({ providerId, modelId });
  modelSelectionRef.current = { providerId, modelId };
  const quotaRequests = useRef(new ModelRequestGuard());
  const quotaReading = useRef<{ key: string; quota: AgentQuota | null; checkedAt: number | null }>({ key: "", quota: null, checkedAt: null });
  const turnPricing = useRef(new Map<string, TurnPricing>());
  const contextRequests = useRef(new ModelRequestGuard());
  const [inferenceLevel, setInferenceLevel] = useState<InferenceLevel>(() => {
    const value = window.localStorage.getItem(INFERENCE_LEVEL_KEY);
    return value === "low" || value === "high" || value === "xhigh" || value === "max" ? value : "medium";
  });
  const [knownWindow, setKnownWindow] = useState<{ key: string; value: number } | null>(null);
  const globalEnvironment = useGlobalEnvironment();
  const globalSsh = globalEnvironment.ssh;
  const sshConnections = useMemo(() => globalSsh.connection && globalSsh.progress?.status === "connected" ? [globalSsh.connection] : [], [globalSsh.connection, globalSsh.progress?.status]);
  const wslProfiles = globalEnvironment.profiles;
  const [workspaceEnvironment, setWorkspaceEnvironment] = useState<WorkspaceEnvironment | null>(null);
  const [workspaceEnvironmentLoading, setWorkspaceEnvironmentLoading] = useState(false);
  const [workspaceGitChanges, setWorkspaceGitChanges] = useState<WorkspaceGitChange[]>([]);
  const [androidTasks, setAndroidTasks] = useState<AndroidTask[]>([]);
  const [copiedId, setCopiedId] = useState("");
  const [attachmentPreview, setAttachmentPreview] = useState<{ name: string; dataUrl: string } | null>(null);
  const [activity, setActivity] = useState("");
  const [composerActionPending, setComposerActionPending] = useState<"queue" | "steer" | "edit" | "">("");
  const [queueMutationState, setQueueMutationState] = useState<Record<string, "guide" | "edit" | "remove" | "reorder">>({});
  const [permissionInstruction, setPermissionInstruction] = useState("");
  const [resolvingPermissionKey, setResolvingPermissionKey] = useState<string | null>(null);
  const [logPanelOpen, setLogPanelOpen] = useState(false);
  const [logSearch, setLogSearch] = useState("");
  const [logStatus, setLogStatus] = useState("");
  const [logHits, setLogHits] = useState<Array<Record<string, unknown>>>([]);
  const [logTimeline, setLogTimeline] = useState<Array<Record<string, unknown>>>([]);
  const [logExcerpt, setLogExcerpt] = useState("");
  const [historyHydrationEpoch, setHistoryHydrationEpoch] = useState(0);
  const [skillHint, setSkillHint] = useState("");
  const [skills, setSkills] = useState<Array<{ id: string; name: string; description: string }>>([]);
  const [scope, setScope] = useState<"workspace" | "standalone">("standalone");
  const [workspaces, setWorkspaces] = useState<WorkspaceEntry[]>(readWorkspaces);
  const [workspacePath, setWorkspacePath] = useState("");
  const workspaceRef = useRef<HTMLElement>(null);
  const pendingForcedBottomSessionRef = useRef("");
  const terminalSnapshotHydrationKeys = useRef(new Set<string>());
  // 已经因为「本轮结束」自动收起过的折叠区 key，保证自动收起只发生一次。
  const settledTimelinesRef = useRef(new Set<string>());
  // 因为流式过程自动展开过的 key：只有这些才在落定时被收起。
  const liveTimelinesRef = useRef(new Set<string>());
  const nativeHistoryLoaded = useRef(false);
  const nativeHistorySummariesLoaded = useRef(false);
  const sessionsRef = useRef(sessions);
  sessionsRef.current = sessions;
  const hydratedHistoryIds = useRef(new Set<string>());
  const [historyReadState, setHistoryReadState] = useState<{ id: string; status: "loading" | "ready" | "error" }>({ id: "", status: "ready" });
  const recoveryHistoryRefreshIds = useRef(new Set<string>());
  const historyHydrationAttempts = useRef(new Map<string, number>());
  const knownHistorySessionIds = useRef(new Set<string>());
  const historySaveTimer = useRef<number | null>(null);
  const chatCacheSaveTimer = useRef<number | null>(null);
  const historyTimer = useRef<number | null>(null);
  const historyAnchorRef = useRef<HTMLDivElement>(null);
  const sandboxTriggerRef = useRef<HTMLButtonElement>(null);
  const resolvingPermissionRequest = useRef<string | null>(null);
  const composerActionRef = useRef(new Set<string>());
  const queueMutationRef = useRef(new Set<string>());
  const initialSubmissionLockedUntilRef = useRef(new Map<string, number>());
  // N4-B: mirror of the 1s submission lock above, made reactive so the send
  // button can show a visible "submitting" state instead of silently dropping
  // clicks. Timers clear each scope once its lock expires.
  const [submissionLocks, setSubmissionLocks] = useState<Record<string, number>>({});
  const submissionLockTimersRef = useRef(new Map<string, ReturnType<typeof setTimeout>>());
  const markInitialSubmissionLocked = useCallback((scope: string, until: number) => {
    setSubmissionLocks((current) => ({ ...current, [scope]: until }));
    const previousTimer = submissionLockTimersRef.current.get(scope);
    if (previousTimer) clearTimeout(previousTimer);
    const timer = setTimeout(() => {
      submissionLockTimersRef.current.delete(scope);
      setSubmissionLocks((current) => {
        if (!(scope in current)) return current;
        const next = { ...current };
        delete next[scope];
        return next;
      });
    }, Math.max(0, until - Date.now() + 20));
    submissionLockTimersRef.current.set(scope, timer);
  }, []);
  useEffect(() => () => {
    for (const timer of submissionLockTimersRef.current.values()) clearTimeout(timer);
    submissionLockTimersRef.current.clear();
  }, []);
  const preparingSubmissionsRef = useRef(new Map<string, { turnId: string; cancelled: boolean; dispatched: boolean }>());
  const [attachmentPreparation] = useState(() => createAttachmentPreparation({ local: prepareChatAttachment, browser: storeBrowserAttachment }));
  const activeSessionIdRef = useRef(sessionId);
  const navigationRevision = useRef(0);

  const session = sessions.find((item) => item.id === sessionId && !item.archived) || null;
  const visibleSessionId = session?.id || "";
  const sources = session ? session.sources || [] : standaloneSources;
  const automationProjectIds = session ? session.automationProjectIds : standaloneProjectIds;
  const setAutomationProjectIds = useCallback((ids: string[]) => {
    if (!visibleSessionId) setStandaloneProjectIds(ids);
    else setSessions((previous) => previous.map((item) => item.id === visibleSessionId ? { ...item, automationProjectIds: ids } : item));
  }, [visibleSessionId]);
  const setComposerMenu = useCallback((open: boolean) => {
    setComposerMenuOpen(open);
    if (open) setSandboxOpen(false);
  }, []);
  const setSources = useCallback<React.Dispatch<React.SetStateAction<ChatSource[]>>>((update) => {
    if (!visibleSessionId) {
      setStandaloneSources(update);
      return;
    }
    setSessions((previous) => {
      const owner = previous.find((item) => item.id === visibleSessionId);
      if (!owner) return previous;
      const current = owner.sources || [];
      const next = typeof update === "function" ? update(current) : update;
      return updateSessionSources(previous, visibleSessionId, next);
    });
  }, [visibleSessionId]);
  const threadSnapshot = useAgentThreadSnapshot(visibleSessionId);
  useEffect(() => {
    if (!isTauri()) return;
    let active = true;
    for (const source of sources) {
      if (source.prepared || source.preparationError) continue;
      void attachmentPreparation.warm(source).then((prepared) => {
        if (active) setSources((current) => current.map((item) => item.path === source.path && item.dataUrl === source.dataUrl
          ? preparedSource(item, prepared) : item));
      }, (error) => {
        if (active) setSources((current) => current.map((item) => item.path === source.path && item.dataUrl === source.dataUrl
          ? { ...item, preparing: false, preparationError: formatInvokeError(error) } : item));
      });
    }
    return () => { active = false; };
  }, [sources, setSources, attachmentPreparation]);
  const renderedActivity = snapshotIsSending(threadSnapshot) && threadSnapshot?.runtime ? threadSnapshot.runtime.activity : activity;
  const snapshotUsage = threadSnapshot?.runtime?.usage;
  const sessionMessages = session?.messages;
  const messages = useMemo(() => sessionMessages ? projectLiveThreadMessages(sessionMessages, threadSnapshot) : [], [sessionMessages, threadSnapshot]);
  const editingMessage = editingMessageId ? messages.find((message) => message.id === editingMessageId && message.role === "user") : undefined;
  const turnActivityByMessage = useMemo(() => {
    const turns = new Map<string, { ownerId: string; pending: boolean; startedAt?: number; finishedAt?: number; events: AgentEventPayload[] }>();
    for (const message of messages) {
      if (message.role !== "assistant") continue;
      const key = message.turnId || message.id;
      const current = turns.get(key);
      if (!current) {
        turns.set(key, {
          ownerId: message.id,
          pending: Boolean(message.pending),
          startedAt: message.startedAt,
          finishedAt: message.finishedAt,
          events: [...(message.events || [])],
        });
        continue;
      }
      current.pending ||= Boolean(message.pending);
      current.events.push(...(message.events || []));
      if (message.startedAt && (!current.startedAt || message.startedAt < current.startedAt)) current.startedAt = message.startedAt;
      if (message.finishedAt && (!current.finishedAt || message.finishedAt > current.finishedAt)) current.finishedAt = message.finishedAt;
    }
    return new Map(messages.map((message) => {
      const activity = message.role === "assistant" ? turns.get(message.turnId || message.id) : undefined;
      return [message.id, activity] as const;
    }));
  }, [messages]);
  const renderedQueuedMessages = useMemo(() => snapshotQueuedMessages(threadSnapshot), [threadSnapshot?.queue]);
  useEffect(() => {
    if (!sessionId) return;
    const pending = new Set<string>();
    for (const message of messages) {
      if (message.role === "assistant" && turnActivityByMessage.get(message.id)?.pending) pending.add(timelineStateKey(message));
    }
    if (pending.size) {
      // 流式过程中过程区是自动展开的；记下这些 key，落定时才知道该收谁。
      for (const key of pending) liveTimelinesRef.current.add(key);
      setSessions((previous) => previous.map((candidate) => {
        if (candidate.id !== sessionId) return candidate;
        const next = retainLiveTimelines(candidate.timelineExpanded, pending);
        return next === candidate.timelineExpanded ? candidate : { ...candidate, timelineExpanded: next };
      }));
    }
    // AI 回复落定后只留正文：思考、指令、工具输出这些过程条目自动折回
    // 「已处理 X 秒」那一行，等价于用户点了一次折叠头。只收本轮自动展开过的轮次——
    // 历史消息里用户自己点开的展开态不动；每个 key 只决定一次，
    // 所以事后再点开也不会被按回去。
    const settled = collapseSettledTimelines(
      sessionsRef.current.find((item) => item.id === sessionId)?.timelineExpanded,
      [...liveTimelinesRef.current],
      pending,
      settledTimelinesRef.current,
    );
    if (!settled.length) return;
    for (const key of settled) {
      settledTimelinesRef.current.add(key);
      liveTimelinesRef.current.delete(key);
    }
    setSessions((previous) => previous.map((candidate) => candidate.id === sessionId
      ? { ...candidate, timelineExpanded: collapseTimelines(candidate.timelineExpanded, settled) } : candidate));
  }, [sessionId, messages, turnActivityByMessage]);
  const snapshotSending = snapshotIsSending(threadSnapshot);
  const sending = conversationIsSending(threadSnapshot, session?.messages || []);
  // N3-A/N3-C: the agent can be stopped while it is sending or while the
  // current session still has an in-flight submission preparation.
  const stopAvailable = sending || (sessionId ? preparingSubmissionsRef.current.has(sessionId) : false);
  // N4-B: the 1s initial-submission lock for the composer's current scope,
  // surfaced so the send button can visibly disable instead of ignoring clicks.
  const submissionLocked = (submissionLocks[sessionId || "new-session"] || 0) > Date.now();
  useEffect(() => {
    setDiagnosticContext({ threadId: visibleSessionId, turnId: threadSnapshot?.activeTurn?.id || "", running: sending });
  }, [visibleSessionId, threadSnapshot?.activeTurn?.id, sending]);
  const snapshotPermissionValue: LivePermissionRequest | null = snapshotPermission(threadSnapshot, sessionId);
  const snapshotPermissionKey = snapshotPermissionValue && `${snapshotPermissionValue.sessionId}:${snapshotPermissionValue.requestId}`;
  useEffect(() => {
    const turn = threadSnapshot?.activeTurn;
    if (!sessionId || !turn || turn.status === "running" || !isTauri()) return;
    const key = `${sessionId}:${turn.id}`;
    if (terminalSnapshotHydrationKeys.current.has(key)) return;
    terminalSnapshotHydrationKeys.current.add(key);
    // Keep the settled reply locally before the runtime advances to a queued
    // turn or history hydration finishes. Its file card belongs to this turn.
    setSessions((previous) => previous.map((item) => item.id === sessionId
      ? { ...item, messages: projectLiveThreadMessages(item.messages, threadSnapshot) } : item));
    void getHistorySession(sessionId).then((record) => {
      if (recoveryHistoryRefreshIds.current.has(sessionId)) {
        terminalSnapshotHydrationKeys.current.delete(key);
        return;
      }
      if (!record) {
        terminalSnapshotHydrationKeys.current.delete(key);
        return;
      }
      knownHistorySessionIds.current.add(sessionId);
      hydratedHistoryIds.current.add(sessionId);
      setSessions((previous) => upsertHistorySession(previous, historyRecordToSession(record)));
    }).catch(() => { terminalSnapshotHydrationKeys.current.delete(key); });
  }, [sessionId, threadSnapshot]);
  useEffect(() => {
    if (resolvingPermissionRequest.current && resolvingPermissionRequest.current !== snapshotPermissionKey) {
      resolvingPermissionRequest.current = null;
    }
    setPermissionInstruction("");
    setResolvingPermissionKey(null);
  }, [snapshotPermissionKey]);
  const activePermission = snapshotPermissionValue;
  const activeTarget = useMemo(() => snapshotExecutionTarget(threadSnapshot, snapshotSending, sshConnections, wslProfiles), [snapshotSending, sshConnections, threadSnapshot?.runtime?.executionTarget, wslProfiles]);
  const projectWorkspacePath = session?.workspacePath || workspacePath;
  const projectTarget = session?.executionTarget;
  const projectBoundEnvironments = session?.boundEnvironments;
  const target = useMemo(() => activeTarget || conversationExecutionTarget(projectWorkspacePath, { executionTarget: projectTarget, boundEnvironments: projectBoundEnvironments }, globalEnvironment.target),
    [activeTarget, projectWorkspacePath, projectTarget, projectBoundEnvironments, globalEnvironment.target]);
  const targetError = activeTarget ? "" : conversationEnvironmentError(target, globalEnvironment.target, globalEnvironment.error, projectBoundEnvironments || []);
  const boundEnvironments = useMemo(() => sessionBoundEnvironments({ boundEnvironments: projectBoundEnvironments }),
    [projectBoundEnvironments]);
  const timelineExpanded = session?.timelineExpanded || {};
  const lastMessage = messages.at(-1);
  const backgroundTurnId = lastMessage?.turnId || threadSnapshot?.activeTurn?.id;
  const backgroundTurnMessage = backgroundTurnId ? messages.find(message => message.turnId === backgroundTurnId) : undefined;
  const backgroundTurn = backgroundTurnId ? {
    id: backgroundTurnId,
    startedAt: threadSnapshot?.activeTurn?.id === backgroundTurnId ? threadSnapshot.activeTurn.startedAtMs
      : backgroundTurnMessage?.startedAt || backgroundTurnMessage?.createdAt,
  } : undefined;
  const messageLayoutVersion = useMemo(() => [
    messages.length,
    lastMessage?.id || "",
    lastMessage?.content.length || 0,
    (lastMessage?.events || []).length,
    lastMessage?.events?.at(-1)?.sequence ?? "",
    lastMessage?.events?.at(-1)?.text?.length ?? "",
    Object.entries(timelineExpanded).filter(([, expanded]) => expanded).map(([id]) => id).join(","),
  ].join(":"), [lastMessage, messages.length, timelineExpanded]);
  const {
    activeUserMessageId,
    markUserScrollIntent,
    onScroll,
    onScrollKeyDown,
    onScrollTouchMove,
    onScrollTouchStart,
    onScrollWheel,
    preserveTimelinePosition,
    scheduleFollowBottom,
    scrollRef,
    scrollToLatest,
    scrollToMessage: scrollToMountedMessage,
    showScrollToBottom,
    suspendAutomaticFollow,
  } = useConversationScroll({ sessionId, messages, layoutVersion: messageLayoutVersion, terminalOpen });
  const messageIndexById = useMemo(() => new Map(messages.map((message, index) => [message.id, index])), [messages]);
  const messageVirtualizer = useConversationVirtualizer({
    messages,
    layoutVersion: messageLayoutVersion,
    scrollRef,
    followBottom: scheduleFollowBottom,
  });
  const virtualMessages = messageVirtualizer.getVirtualItems();
  const scrollToMessage = useCallback((messageId: string) => {
    const index = messageIndexById.get(messageId);
    if (index === undefined) return;
    suspendAutomaticFollow();
    messageVirtualizer.scrollToIndex(index, { align: "center" });
    window.requestAnimationFrame(() => scrollToMountedMessage(messageId));
  }, [messageIndexById, messageVirtualizer, scrollToMountedMessage, suspendAutomaticFollow]);
  const markerMessages = session?.messages || messages;
  const userMessageMarkers = useMemo(() => {
    const userMessages = markerMessages.filter((message) => message.role === "user");
    return userMessages.map((message, index) => {
      const preview = message.content.trim().replace(/\s+/g, " ");
      return {
        id: message.id,
        preview: preview.length > 72 ? `${preview.slice(0, 72)}…` : preview || `第 ${index + 1} 条输入`,
      };
    });
  }, [markerMessages]);
  const logPaths = sources.filter((item) => item.kind === "text" || item.kind === "archive").map((item) => item.path).filter((path) => !path.startsWith("browser:"));
  const provider = providers.find((item) => item.id === providerId) || providers[0];
  const modelChoices = provider?.models.length
    ? provider.models
    : provider?.model
      ? [{ id: provider.model, name: provider.model, contextWindow: provider.contextWindow }]
      : [];
  const model = modelChoices.find((item) => item.id === modelId) || modelChoices[0];
  const modelConfigKey = JSON.stringify([providerRevision, provider?.id, provider?.kind, provider?.brand,
    provider?.baseUrl, provider?.inputPricePerMillion, provider?.outputPricePerMillion, provider?.currency, provider?.contextWindow,
    provider?.quotaUrls?.balanceUrl, provider?.quotaUrls?.spendingUrl,
    model?.id, model?.protocol, model?.protocols, model?.contextWindow]);
  quotaRequests.current.select(modelConfigKey);
  contextRequests.current.select(modelConfigKey);
  const selectedKnownWindow = knownWindow?.key === modelConfigKey ? knownWindow.value : null;
  const selectedContextWindow = selectedKnownWindow
    ?? model?.contextWindow
    ?? provider?.contextWindow;
  const activeEstimateTurnId = threadSnapshot?.activeTurn?.id || "";
  const durableTokenEstimate = useMemo(() => {
    let input = 0;
    let output = 0;
    for (const message of session?.messages || []) {
      if (activeEstimateTurnId && message.turnId === activeEstimateTurnId) continue;
      if (message.role === "user") input += estimateTokens(message.content);
      else output += estimateTokens(message.content);
    }
    return { input, output };
  }, [activeEstimateTurnId, session?.messages]);
  const liveTurnTokenEstimate = useMemo(() => {
    if (!activeEstimateTurnId) return { input: 0, output: 0 };
    let input = 0;
    let output = 0;
    let found = false;
    for (let index = messages.length - 1; index >= 0; index -= 1) {
      const message = messages[index];
      if (message.turnId !== activeEstimateTurnId) {
        if (found) break;
        continue;
      }
      found = true;
      if (message.role === "user") input += estimateTokens(message.content);
      else output += estimateTokens(assistantResultText(message));
    }
    return { input, output };
  }, [activeEstimateTurnId, messages]);
  const inputTokens = durableTokenEstimate.input + liveTurnTokenEstimate.input + estimateTokens(draft);
  const outputTokens = durableTokenEstimate.output + liveTurnTokenEstimate.output;
  const contextDisplay = contextUsageForDisplay(sending, readChatUsage(snapshotUsage), selectedContextWindow, inputTokens + outputTokens);
  const contextScan = useMemo(() => {
    return resolveContextUsageScan(threadSnapshot, session?.messages || [], model?.id || "", provider?.id || "", session?.usageTurnId, session?.contextUsageEvents);
  }, [threadSnapshot, session?.messages, session?.usageTurnId, session?.contextUsageEvents, model?.id, provider?.id]);
  const contextDetails = contextScan.reading;
  // A new turn clears the live items, and a restored session starts without one.
  // Until the current turn reports, keep the newest measured breakdown of this
  // session instead of blanking the three rows or jumping to a character estimate.
  const lastMeasuredContext = useMemo(() => (contextDetails ? null
    : latestContextUsageDetails(session?.messages || [], model?.id || "", provider?.id || "", session?.contextUsageEvents)),
    [contextDetails, session?.messages, session?.contextUsageEvents, model?.id, provider?.id]);
  const measuredContext = contextDetails ?? lastMeasuredContext;
  // P8-D：当前轮与历史回退都没有有效读数、而最新一条读数自相矛盾
  // （system+tools > total）时，把矛盾读数交给用量环显示「数据异常」，
  // 不再静默丢弃成空白；只要有任何有效读数就不展示异常。
  const contextAnomaly = measuredContext ? null : contextScan.anomaly;
  // A reading is only meaningful against the window it was measured with, so a
  // measured breakdown keeps its own window instead of being divided by a
  // window the user has since switched to.
  const contextWindow = (measuredContext?.window ?? contextDisplay.window) ?? null;
  // A measured breakdown is the last real reading; the draft has not been sent
  // yet, so adding a character estimate would inflate the reported number.
  const usedTokens = measuredContext ? measuredContext.total : contextDisplay.tokens;
  const visibleUsage = visibleTurnUsage(session?.usage, session?.usageTurnId, threadSnapshot);
  const displayedQuota = quotaKey === modelConfigKey ? quota : null;
  const displayedQuotaPhase = quotaKey === modelConfigKey ? quotaPhase : "idle";
  const visibleTurnId = threadSnapshot?.activeTurn?.id || session?.usageTurnId;
  const sessionTokens = sessionUsageWithLive(session?.usageTotals, visibleUsage, threadSnapshot?.activeTurn);
  const visiblePricing = visibleTurnId ? turnPricing.current.get(JSON.stringify([sessionId, visibleTurnId])) : undefined;
  const turnQuota = useTurnQuotaChange({ sessionId, turnId: visibleTurnId, providerKey: provider ? quotaCacheKey(provider) : "",
    quota: displayedQuota, checkedAt: quotaKey === modelConfigKey ? quotaCheckedAt : null,
    finishedAt: threadSnapshot?.activeTurn?.status !== "running" ? threadSnapshot?.activeTurn?.finishedAtMs : undefined });
  const executionRoot = (candidate: ExecutionTarget) => {
    if (candidate.kind === "local") return candidate.root?.trim() || session?.workspacePath || workspacePath;
    if (candidate.root?.trim()) return candidate.root.trim();
    if (candidate.kind === "ssh") return sshConnections.find((item) => item.id === candidate.id)?.defaultRoot?.trim() || "";
    return wslProfiles.find((item) => item.id === candidate.id)?.defaultRoot?.trim() || "";
  };

  const quotaTerminalTurn = threadSnapshot?.activeTurn && threadSnapshot.activeTurn.status !== "running"
    ? JSON.stringify([sessionId, threadSnapshot.activeTurn.id, threadSnapshot.activeTurn.finishedAtMs]) : "";

  useLayoutEffect(() => {
    activeSessionIdRef.current = sessionId;
    navigationRevision.current++;
    // These values describe the visible session only. Clear them before the
    // next session paints so a background turn cannot leave stale UI behind.
    setActivity("");
    setPermissionInstruction("");
    setComposerActionPending("");
    setQueueMutationState({});
    setEditingMessageId("");
    // Drafts and model selection are per-session: restore the target
    // session's stored values (falling back to the global defaults for
    // sessions that never picked a model or typed anything) so switching
    // conversations never leaks one conversation's pending input or model
    // into another.
    const nextSession = sessionId ? sessionsRef.current.find((item) => item.id === sessionId) : null;
    const restoredSelection = nextSession?.modelSelection;
    if (restoredSelection) {
      const reconciled = providersRef.current.length
        ? reconcileModelSelection(providersRef.current, restoredSelection)
        : restoredSelection;
      if (reconciled.providerId !== modelSelectionRef.current.providerId
        || reconciled.modelId !== modelSelectionRef.current.modelId) {
        modelSelectionRef.current = reconciled;
        setProviderId(reconciled.providerId);
        setModelId(reconciled.modelId);
      }
    }
    restoringDraftRef.current = true;
    setDraft(readSessionDraft(sessionId));
  }, [sessionId]);

  // Keep the composer draft on the conversation that owns it. Drafts live in
  // their own store (see sessionDraftStore) so typing never mutates session
  // state and never triggers the chat-cache / native-history writers.
  useEffect(() => {
    // A switch just restored this session's draft. Skip exactly one run: the
    // update that switched the session can still carry the previous
    // conversation's text here, and writing that would be the leak this
    // guard exists to prevent. The restored value is already in the store.
    if (restoringDraftRef.current) {
      restoringDraftRef.current = false;
      return;
    }
    const currentId = activeSessionIdRef.current;
    if (!currentId || currentId !== sessionId) return;
    writeSessionDraft(currentId, draft);
  }, [draft, sessionId]);

  const closeComposerMenus = () => {
    setComposerMenuOpen(false);
    setTaskPlanOpen(false);
  };

  useEffect(() => {
    cacheBrowserValue(TASK_PLAN_SETTINGS_KEY, JSON.stringify(taskPlanSettings));
  }, [taskPlanSettings]);

  const refreshUsage = useCallback(async (force = false) => {
    const guard = quotaRequests.current;
    const revision = guard.begin();
    const cacheKey = provider ? quotaCacheKey(provider) : "";
    let cachedQuota: ReturnType<typeof readQuotaCache> = null;
    try {
      cachedQuota = cacheKey ? readQuotaCache(window.localStorage, cacheKey)
        || readQuotaCache(window.localStorage, `android-tool.chat.quota.v2.${modelConfigKey}`) : null;
      if (cachedQuota) window.localStorage.setItem(cacheKey, JSON.stringify(cachedQuota));
    } catch { /* Refresh remains usable without browser storage. */ }
    setQuotaKey(modelConfigKey);
    const previous = quotaReading.current.key === modelConfigKey ? quotaReading.current : null;
    setQuota(previous?.quota || cachedQuota?.quota || null);
    setQuotaCheckedAt(previous?.checkedAt || cachedQuota?.checkedAt || null);
    setQuotaError("");
    setQuotaPhase("checking");
    try {
      if (isTauri()) {
        if (provider) {
          if (!force && cachedQuota && Date.now() - cachedQuota.checkedAt >= 0 && Date.now() - cachedQuota.checkedAt < 30_000) {
            setQuota(cachedQuota.quota);
            setQuotaCheckedAt(cachedQuota.checkedAt);
            quotaReading.current = { key: modelConfigKey, quota: cachedQuota.quota, checkedAt: cachedQuota.checkedAt };
            setQuotaPhase("done");
            return;
          }
          const snapshot = await getProviderQuota({
            id: provider.id,
            name: provider.name,
            model: model?.id || provider.model,
            kind: provider.kind,
            brand: provider.brand || null,
            baseUrl: provider.baseUrl,
            currency: provider.currency || null,
            inputPricePerMillion: provider.inputPricePerMillion ?? null,
            outputPricePerMillion: provider.outputPricePerMillion ?? null,
          });
          if (!guard.isCurrent(revision)) return;
          // Keep a cached numeric reading while remembering that this account
          // explicitly cannot be queried automatically anymore.
          setQuotaAutoRefreshBlockedKey(supportsAutomaticQuotaRefresh(snapshot) ? "" : modelConfigKey);
          if (!hasQuotaAmounts(snapshot) && (previous?.quota || cachedQuota)) {
            setQuotaError(snapshot.message || "供应商未返回额度数据");
            setQuotaPhase("error");
            return;
          }
          setQuota(snapshot);
          quotaReading.current = { key: modelConfigKey, quota: snapshot, checkedAt: Date.now() };
          if (hasQuotaAmounts(snapshot)) try {
            window.localStorage.setItem(cacheKey, JSON.stringify({ checkedAt: Date.now(), quota: snapshot }));
          } catch { /* Quota is still usable when browser storage is full. */ }
        }
      }
      setQuotaCheckedAt(Date.now());
      setQuotaPhase("done");
    } catch (error) {
      if (guard.isCurrent(revision)) {
        setQuotaError(formatInvokeError(error));
        setQuotaPhase("error");
      }
    }
  }, [modelConfigKey]);

  useEffect(() => {
    if (!isTauri() || !provider) {
      setQuota(null);
      setQuotaCheckedAt(null);
      setQuotaPhase("idle");
      return;
    }
    void refreshUsage();
  }, [refreshUsage]);

  const automaticQuotaRefresh = quotaAutoRefreshBlockedKey !== modelConfigKey && supportsAutomaticQuotaRefresh(displayedQuota);
  useQuotaConsumptionRefresh({ enabled: isTauri() && Boolean(provider) && automaticQuotaRefresh, accountKey: modelConfigKey,
    turnKey: JSON.stringify([sessionId, visibleTurnId]), consumptionKey: quotaConsumptionKey(visibleUsage),
    terminalKey: quotaTerminalTurn, refresh: refreshUsage });

  useEffect(() => {
    if (!active || !isTauri() || !provider || !model || !messages.length || sending || !automaticQuotaRefresh) return;
    const timer = window.setInterval(() => { if (document.visibilityState === "visible") void refreshUsage(); }, 60_000);
    return () => window.clearInterval(timer);
  }, [active, refreshUsage, messages.length, sending, automaticQuotaRefresh]);

  const refreshContext = useCallback(async () => {
    const guard = contextRequests.current;
    const revision = guard.begin();
    setContextChecking(true);
    try {
      if (!isTauri() || !provider || !model?.id) return;
      const value = await invoke<number | null>("get_agent_model_context_window", {
        modelId: model.id,
        kind: provider.kind,
        brand: provider.brand || null,
      });
      if (!guard.isCurrent(revision)) return;
      if (value) setKnownWindow({ key: modelConfigKey, value });
    } catch (error) {
      if (guard.isCurrent(revision)) setActivity(`无法刷新模型上下文窗口：${formatInvokeError(error)}`);
    } finally {
      if (guard.isCurrent(revision)) setContextChecking(false);
    }
  }, [modelConfigKey]);

  const refreshLogs = async () => {
    if (!logPaths.length) {
      setLogStatus("请先添加日志或压缩包来源。");
      return;
    }
    setLogStatus("正在索引日志…");
    try {
      const [indexed, timeline] = await Promise.all([indexLocalLogs(logPaths), rebuildLocalLogTimeline(logPaths)]);
      setLogTimeline(timeline);
      setLogStatus(`已索引 ${indexed.length} 个来源，发现 ${timeline.length} 个时间线事件。`);
    } catch (error) { setLogStatus(formatInvokeError(error)); }
  };

  const searchLogs = async () => {
    if (!logSearch.trim()) return;
    setLogStatus("正在搜索日志…");
    try {
      const hits = await searchLocalLogs(logPaths, logSearch.trim());
      setLogHits(hits);
      setLogStatus(`找到 ${hits.length} 条结果。`);
    } catch (error) { setLogStatus(formatInvokeError(error)); }
  };

  const openLogHit = async (hit: Record<string, unknown>) => {
    try {
      const source = String(hit.source || hit.path || "");
      const member = typeof hit.member === "string" ? hit.member : null;
      const startLine = Number(hit.line || hit.lineNumber || 1);
      const result = await readLocalLogRange({ paths: logPaths, source, member, startLine, maxLines: 80, maxBytes: 48 * 1024 });
      const lines = Array.isArray(result.lines) ? result.lines.join("\n") : "";
      setLogExcerpt(result.text || lines || "没有可显示的日志内容。");
    } catch (error) { setLogStatus(formatInvokeError(error)); }
  };

  const latestTurnMessages = useMemo(() => {
    let latest: ChatMessage | undefined;
    let latestIndex = -1;
    for (let index = messages.length - 1; index >= 0; index -= 1) {
      if (messages[index].role === "assistant") {
        latest = messages[index];
        latestIndex = index;
        break;
      }
    }
    if (!latest) return [];
    if (!latest.turnId) return [latest];
    const turnMessages: ChatMessage[] = [];
    for (let index = latestIndex; index >= 0; index -= 1) {
      const message = messages[index];
      if (message.turnId === latest.turnId) {
        if (message.role === "assistant") turnMessages.unshift(message);
        continue;
      }
      if (turnMessages.length && message.turnId) break;
    }
    return turnMessages;
  }, [messages]);
  const snapshotFileChanges = useMemo(() => threadSnapshot?.items.find((item) => item.kind === "file_changes"), [threadSnapshot?.items]);
  // G1/G2：goal 徽章——从 goal 工具调用的事件流恢复最新跨轮目标状态。
  const activeGoalBadge = useMemo(() => activeGoalFromSnapshot(threadSnapshot), [threadSnapshot]);
  const composerFileChange = useMemo(() => composerFileChanges(pendingFileChanges(threadSnapshot, messages)), [threadSnapshot, messages]);
  const taskPlan = useMemo(() => {
    const latest = latestTurnMessages.at(-1);
    const parsed = activeComposerTaskPlan(threadSnapshot);
    if (!parsed) return null;
    const active = snapshotSending || latestTurnMessages.some((message) => message.pending);
    const terminal = latest ? terminalForMessage(latest) : null;
    const steps = Array.isArray(parsed.steps) ? parsed.steps.flatMap((item, stepIndex) => {
      if (!item || typeof item !== "object") return [];
      const step = item as Record<string, unknown>;
      const title = String(step.title || "").trim();
      if (!title) return [];
      let status: ComposerTaskStep["status"] = step.status === "done" || step.status === "completed" ? "done" : step.status === "error" ? "error" : step.status === "running" || step.status === "in_progress" ? "running" : "pending";
      if (!active && (status === "running" || status === "pending")) {
        status = terminal?.status === "cancelled" ? "cancelled" : terminal?.status === "failed" ? "error" : "pending";
      }
      return [{ id: typeof step.id === "string" && step.id ? step.id : `${latest?.id || sessionId}-${stepIndex}`, title, status }];
    }) : [];
    if (!String(parsed.goal || "").trim() && !steps.length) return null;
    return {
      goal: String(parsed.goal || "").trim(),
      steps,
      startedAt: latestTurnMessages[0]?.startedAt || latestTurnMessages[0]?.createdAt,
      finishedAt: active ? undefined : latest?.finishedAt || terminal?.timestampMs,
      active,
    };
  }, [latestTurnMessages, sessionId, snapshotSending, threadSnapshot?.items]);
  // P5-B/P5-E：停止只作用于「当前活跃 turn」提交的后台任务。归属判定优先用
  // 后端下发的 task.turnId（create_task 在受理点盖章，权威）——旧任务缺失时
  // 回退本地事件流推断（managedTaskIds）。双源取交集以覆盖两类任务。
  const activeTurnIdForTasks = useMemo(() => activeThreadTurnId(threadSnapshot, messages), [threadSnapshot, messages]);
  const backendOwnedTaskIds = useMemo(() => new Set(androidTasks
    .filter((task) => task.turnId && (!activeTurnIdForTasks || task.turnId === activeTurnIdForTasks))
    .map((task) => task.id)), [androidTasks, activeTurnIdForTasks]);
  const managedTaskIds = useMemo(() => submittedTaskIds(latestTurnMessages
    .filter((message) => !activeTurnIdForTasks || message.turnId === activeTurnIdForTasks)
    .flatMap((message) => message.events || [])), [latestTurnMessages, activeTurnIdForTasks]);
  const managedTasks = androidTasks.filter((task) => task.kind !== "agent" && task.status === "running"
    && (managedTaskIds.has(task.id) || backendOwnedTaskIds.has(task.id)));
  const taskSteps: ComposerTaskStep[] = taskPlan?.steps.length ? taskPlan.steps : managedTasks.map((task) => ({
    id: task.id,
    title: task.message || task.kind,
    status: task.status === "running" ? "running" : task.status === "error" ? "error" : "pending",
    cancelId: task.id,
  }));
  const taskGoal = taskPlan?.goal || managedTasks.find((task) => task.status === "running")?.message || managedTasks[0]?.message || "";
  const taskStartedAt = taskPlan?.startedAt || managedTasks.find((task) => task.status === "running")?.startedAt;
  const taskActive = Boolean(taskPlan?.active) || managedTasks.length > 0;
  const taskPlanVisible = Boolean(taskPlan) || managedTasks.length > 0;
  const taskPlanKey = `${taskGoal}:${taskStartedAt || 0}`;
  const showTaskPlan = taskPlanVisible && session?.dismissedTaskPlanKey !== taskPlanKey;
  // 任务编排卡片不再提供设置面板：运行中由输入栏的暂停按钮停止，
  // 暂停后卡片右侧才出现删除按钮。
  const dismissTaskPlan = useCallback(() => {
    if (!visibleSessionId) return;
    setSessions((previous) => previous.map((item) => item.id === visibleSessionId
      ? { ...item, dismissedTaskPlanKey: taskPlanKey } : item));
  }, [visibleSessionId, taskPlanKey]);

  const refreshWorkspaceEnvironment = async () => {
    if (!workspacePath) {
      setWorkspaceEnvironment(null);
      setWorkspaceGitChanges([]);
      return;
    }
    setWorkspaceEnvironmentLoading(true);
    try {
      const environment = await getWorkspaceEnvironment(workspacePath);
      setWorkspaceEnvironment(environment);
      setWorkspaceGitChanges(environment.git.repository ? await getWorkspaceGitChanges(workspacePath) : []);
    } catch (error) {
      setWorkspaceEnvironment(null);
      setWorkspaceGitChanges([]);
      setActivity(formatInvokeError(error));
    } finally {
      setWorkspaceEnvironmentLoading(false);
    }
  };

  useEffect(() => {
    if (!snapshotFileChanges || !workspacePath) return;
    void refreshWorkspaceEnvironment();
  }, [snapshotFileChanges, workspacePath]);

  useEffect(() => {
    if (!isTauri()) return;
    let active = true;
    let unlisten: () => void = () => {};
    void listen<Record<string, unknown>>("collaboration:job-created", (payload) => {
      const eventThreadId = String(payload.threadId || payload.thread_id || "");
      const jobId = String(payload.jobId || payload.job_id || "");
      if (!eventThreadId || !jobId || eventThreadId !== sessionId) return;
      // The native coordinator owns dispatch/recovery. The panel observes the
      // durable job and keeps manual retry controls for interrupted workers.
      setCollaborationSideView("agents");
      setCollaborationOpen(true);
    }).then((stop) => {
      if (active) unlisten = stop;
      else stop();
    }).catch(() => undefined);
    return () => {
      active = false;
      unlisten();
    };
  }, [sessionId]);

  useEffect(() => {
    if (!active || !isTauri()) return;
    let stopped = false;
    let refreshing = false;
    let previousTasks = "[]";
    const tick = () => {
      if (stopped || refreshing || document.visibilityState === "hidden") return;
      refreshing = true;
      void listAndroidTasks().then((tasks) => {
        if (stopped) return;
        const activeTasks = tasks.filter((task) => task.status === "running" || task.status === "queued");
        const signature = JSON.stringify(activeTasks);
        if (signature === previousTasks) return;
        previousTasks = signature;
        setAndroidTasks(activeTasks);
      }).catch(() => undefined).finally(() => { refreshing = false; });
    };
    tick();
    const timer = window.setInterval(tick, 2000);
    return () => { stopped = true; window.clearInterval(timer); };
  }, [active]);

  const openProviderSettings = () => {
    closeComposerMenus();
    window.dispatchEvent(new CustomEvent("android-tool-open-settings", { detail: { section: "agent" } }));
  };

  const slashQuery = draft.match(/(?:^|\s)\/([^/\s]*)$/)?.[1];
  const builtinTools = [
    { id: "read_local_file", label: "读取文件", description: "读取一个本地文件" },
    { id: "list_local_files", label: "读取目录", description: "列出目录中的文件" },
    { id: "run_local_shell", label: "运行命令", description: "在当前执行目标运行命令" },
    { id: "search_local_logs", label: "搜索日志", description: "在已选日志里搜索" },
  ];
  const applicationTools = APP_ACTION_CATALOG
    .filter((action) => action.target.kind === "tool")
    .map((action) => ({
      kind: "app" as const,
      id: action.id.replace("open.tool.", ""),
      label: action.label,
      description: `${action.destination} · ${action.description}`,
    }));
  const slashItems = slashQuery === undefined ? [] : [
    ...skills.filter((item) => !slashQuery || item.id.toLowerCase().includes(slashQuery.toLowerCase()) || item.name.toLowerCase().includes(slashQuery.toLowerCase())).slice(0, 8).map((item) => ({ kind: "skill" as const, id: item.id, label: item.name, description: item.description || "" })),
    ...builtinTools.filter((item) => !slashQuery || item.id.includes(slashQuery.toLowerCase()) || item.label.includes(slashQuery)).map((item) => ({ kind: "tool" as const, id: item.id, label: item.label, description: item.description })),
    ...applicationTools.filter((item) => !slashQuery || item.id.includes(slashQuery.toLowerCase()) || item.label.includes(slashQuery) || item.description.includes(slashQuery)),
  ];
  const applySlashItem = (item: typeof slashItems[number]) => {
    const token = item.kind === "app"
      ? `请使用应用工具：${item.label}。`
      : item.kind === "tool"
        ? `/tool:${item.id} `
        : `/${item.id} `;
    setDraft((value) => value.replace(/(?:^|\s)\/[^/\s]*$/, (match) => match.startsWith(" ") ? ` ${token}` : token));
  };
  const [slashIndex, setSlashIndex] = useState(0);
  const [slashMenuDismissed, setSlashMenuDismissed] = useState(false);
  const visibleSlashItems = slashMenuDismissed ? [] : slashItems;
  const [pathMenu, setPathMenu] = useState<{ x: number; y: number; path: string } | null>(null);
  const pathMenuRef = useRef<HTMLDivElement>(null);
  const slashMenuRef = useRef<HTMLDivElement>(null);
  const slashOpen = slashQuery !== undefined && !slashMenuDismissed;
  useEffect(() => {
    if (!pathMenu && !slashOpen) return;
    const outside = (event: PointerEvent) => {
      const target = event.target as Node;
      if (pathMenu && !pathMenuRef.current?.contains(target)) setPathMenu(null);
      if (slashOpen && !slashMenuRef.current?.contains(target)) setSlashMenuDismissed(true);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      event.preventDefault();
      if (pathMenu) setPathMenu(null);
      if (slashOpen) setSlashMenuDismissed(true);
    };
    document.addEventListener("pointerdown", outside, true);
    document.addEventListener("keydown", escape);
    return () => {
      document.removeEventListener("pointerdown", outside, true);
      document.removeEventListener("keydown", escape);
    };
  }, [pathMenu, slashOpen]);
  const [filePreviewPath, setFilePreviewPath] = useState<string | null>(null);
  const [filePreviewOrigin, setFilePreviewOrigin] = useState<FilePreviewOrigin>();
  // 空串表示「还没手动选过」，由路径形态与对话默认环境自动决定。
  const [previewEnvironmentKey, setPreviewEnvironmentKey] = useState<string>("");
  const sidePanelWidth = usePaneWidth(sidePanelLayoutRef, collaborationOpen || Boolean(filePreviewPath), "android-tool.chat.workspace-panel-width", WORKSPACE_PANE);
  const closeFilePreview = useCallback(() => setFilePreviewPath(null), []);
  useEffect(() => { setFilePreviewPath(null); setPreviewEnvironmentKey(""); }, [sessionId]);
  // 预览是单选：本机工作区 + 该对话绑定的 SSH 主机，默认跟随对话的默认执行环境。
  // 兜底清单补两类主机：会话发送时保存的执行环境（工作区是盘符时会被判成本机而丢掉），
  // 以及已保存的 SSH 服务器。少了它们，本机对话里由 `ssh__*` 改出的远程文件
  // 没有任何主机可归属，点开只能用本机命令去读 `/data/...` 并报「找不到文件」。
  //
  // 全局 SSH 快照每 2 秒轮询发布一次新数组。若清单跟着数组引用重建，预览面板的
  // `location` 会跟着换新对象，读文件 effect 每 2 秒重跑一次，所以按字段签名缓存
  // 复用同一个数组。
  const previewFallbackCache = useRef<{ signature: string; targets: ExecutionTarget[] }>({ signature: "", targets: [] });
  const previewFallbackSignature = useMemo(() => [
    projectTarget ? `${projectTarget.kind}:${projectTarget.id || ""}:${projectTarget.root || ""}:${projectTarget.label}` : "",
    globalSsh.connection?.id || "",
    ...globalSsh.connections.map((item) => `${item.id}|${item.name}|${item.defaultRoot || ""}`),
  ].join("\n"), [projectTarget, globalSsh.connection, globalSsh.connections]);
  const previewFallbackTargets = useMemo<ExecutionTarget[]>(() => {
    if (previewFallbackCache.current.signature === previewFallbackSignature) return previewFallbackCache.current.targets;
    const result: ExecutionTarget[] = [];
    if (projectTarget) result.push(projectTarget);
    if (globalSsh.connection) result.push(environmentTargetForSsh(globalSsh.connection));
    for (const connection of globalSsh.connections) result.push(environmentTargetForSsh(connection));
    previewFallbackCache.current = { signature: previewFallbackSignature, targets: result };
    return result;
  }, [previewFallbackSignature, projectTarget, globalSsh.connection, globalSsh.connections]);
  const previewTargets = useMemo(
    () => filePreviewTargets(target, boundEnvironments, projectWorkspacePath, previewFallbackTargets),
    [target, boundEnvironments, projectWorkspacePath, previewFallbackTargets],
  );
  const previewTarget = useMemo(
    () => resolvePreviewTarget(previewTargets, filePreviewPath || "", target, previewEnvironmentKey),
    [previewTargets, filePreviewPath, target, previewEnvironmentKey],
  );

  const addSource = (path: string, kind: ChatSourceKind, name = path.split(/[\\/]/).filter(Boolean).pop() || path, dataUrl?: string, size?: number, bindWorkspace = false) => {
    const source: ChatSource = { path, name, kind, dataUrl, size, preparing: isTauri() };
    if (isTauri()) void attachmentPreparation.warm(source).catch(() => undefined);
    setSources((prev) => prev.some((item) => item.path === path) ? prev : [...prev, source]);
    if (kind === "directory" && bindWorkspace) {
      setWorkspaces((prev) => prev.some((item) => workspaceKey(item.path) === workspaceKey(path)) ? prev : [{ path, name }, ...prev]);
      setWorkspacePath(path);
      setScope("workspace");
    }
  };

  const acceptDropped = (incoming: ChatSource[], rejected: string[]) => {
    incoming.forEach((item) => addSource(item.path, item.kind, item.name, item.dataUrl, item.size));
    if (!rejected.length) {
      setDropNotice("");
      return;
    }
    setDropNotice(`已跳过不支持的文件：${rejected.slice(0, 3).join("、")}`);
    window.setTimeout(() => setDropNotice(""), 4200);
  };

  const onChatDragOver = (event: React.DragEvent) => {
    if (![...event.dataTransfer.types].includes("Files")) return;
    event.preventDefault();
    event.dataTransfer.dropEffect = "copy";
    setDropActive(true);
  };

  const onChatDragLeave = (event: React.DragEvent) => {
    if (event.currentTarget.contains(event.relatedTarget as Node | null)) return;
    setDropActive(false);
  };

  const addDroppedPaths = async (paths: string[]) => {
    const unique = [...new Set(paths.map((path) => path.trim()).filter(Boolean))];
    if (!unique.length) return;
    acceptDropped(unique.map((path) => {
      const name = path.split(/[\\/]/).filter(Boolean).pop() || path;
      const directory = /[\\/]$/.test(path);
      return { path, name, kind: classifyDropped(name, directory) || (directory ? "directory" : "document") };
    }), []);
  };
  const addDroppedPathsRef = useRef(addDroppedPaths);
  addDroppedPathsRef.current = addDroppedPaths;

  useEffect(() => {
    if (!isTauri()) return;
    let stop = () => {};
    let alive = true;
    void getCurrentWebview().onDragDropEvent((event) => {
      const payload = event.payload;
      const bounds = workspaceRef.current?.getBoundingClientRect();
      const visible = Boolean(bounds && bounds.width > 100 && bounds.height > 100);
      if (!visible) return;
      const scale = Math.max(window.devicePixelRatio || 1, 1);
      const point = "position" in payload ? payload.position : undefined;
      const inside = Boolean(bounds && point && point.x / scale >= bounds.left && point.x / scale <= bounds.right && point.y / scale >= bounds.top && point.y / scale <= bounds.bottom);
      if (payload.type === "enter" || payload.type === "over") {
        setDropActive(inside);
        return;
      }
      if (payload.type === "leave") {
        setDropActive(false);
        return;
      }
      setDropActive(false);
      if (payload.type === "drop" && inside) void addDroppedPathsRef.current(payload.paths);
    }).then((unlisten) => {
      if (!alive) unlisten();
      else stop = unlisten;
    }).catch(() => undefined);
    return () => {
      alive = false;
      stop();
    };
  }, []);

  const onChatDrop = async (event: React.DragEvent) => {
    event.preventDefault();
    setDropActive(false);
    const accepted: ChatSource[] = [];
    const rejected: string[] = [];
    const seen = new Set<string>();
    const take = (name: string, path: string, isDirectory: boolean, dataUrl?: string) => {
      const kind = classifyDropped(name, isDirectory);
      const key = path || name;
      if (!kind) {
        if (name) rejected.push(name);
        return;
      }
      if (seen.has(key)) return;
      seen.add(key);
      accepted.push({ path: key, name, kind, dataUrl });
    };
    const items = [...event.dataTransfer.items];
    if (items.length) {
      for (const item of items) {
        const entry = item.webkitGetAsEntry?.();
        const file = item.getAsFile() as (File & { path?: string }) | null;
        if (entry?.isDirectory) take(entry.name, file?.path || entry.fullPath || entry.name, true);
        else if (file) {
          const nativePath = file.path && file.path !== file.name ? file.path : "";
          take(file.name, nativePath || `browser:${crypto.randomUUID()}:${file.name}`, false, nativePath ? undefined : await readBrowserFile(file));
        }
      }
    } else {
      for (const file of [...event.dataTransfer.files]) {
        const withPath = file as File & { path?: string };
        const nativePath = withPath.path && withPath.path !== file.name ? withPath.path : "";
        take(file.name, nativePath || `browser:${crypto.randomUUID()}:${file.name}`, false, nativePath ? undefined : await readBrowserFile(file));
      }
    }
    acceptDropped(accepted, rejected);
  };

  const onComposerPaste = async (event: React.ClipboardEvent<HTMLDivElement>) => {
    const clipboard = event.clipboardData;
    const image = [...clipboard.items].find((item) => item.type.startsWith("image/"));
    if (image) {
      const file = image.getAsFile();
      if (file) {
        event.preventDefault();
        const dataUrl = await readBrowserFile(file);
        addSource(`browser:${crypto.randomUUID()}:${file.name || "pasted-image.png"}`, "image", file.name || "pasted-image.png", dataUrl);
        return;
      }
    }
    const html = clipboard.getData("text/html");
    if (!html || !/<[a-z][\s\S]*>/i.test(html)) return;
    event.preventDefault();
    const text = richTextToMarkdown(html).replace(/\r\n/g, "\n").trim();
    if (text) {
      // Escape clipboard text and use the editor's BR-based line structure.
      // Native insertion respects the selection and retains undo history.
      const content = document.createElement("div");
      content.textContent = text;
      document.execCommand("insertHTML", false, content.innerHTML.replace(/\n/g, "<br>"));
    }
  };

  useEffect(() => {
    writeChatNavSnapshot({ sessionId, scope, workspacePath });
    window.dispatchEvent(new Event(CHAT_NAV_EVENT));
  }, [sessionId, scope, workspacePath]);

  useLayoutEffect(() => { publishChatSessions(sessions); }, [sessions]);

  // Drafts are local reservations until a message is sent. Leaving one should
  // remove its navigation/cache entry without touching any native transcript.
  useEffect(() => {
    if (isTauri() && !nativeHistorySummariesLoaded.current) return;
    const abandoned = abandonedDraftIds(sessionsRef.current, sessionId, knownHistorySessionIds.current)
      .filter(id => !preparingSubmissionsRef.current.has(id) && !pendingHistorySaves.current.has(id));
    if (!abandoned.length) return;
    for (const id of abandoned) {
      deletedSessionIds.current.add(id);
      markChatSessionDeleted(id);
      conversationHistoryReadAhead.invalidate(id);
      // The id can never be selected again (deleted ids are persisted and
      // filtered out of every listing), so its draft would be unreachable.
      pruneSessionDraft(id);
    }
    sessionsRef.current = sessionsRef.current.filter(item => !deletedSessionIds.current.has(item.id));
    setSessions(sessionsRef.current);
    try { window.localStorage.setItem(STORAGE_KEY, JSON.stringify(sessionsRef.current)); } catch { /* Optional cache. */ }
  }, [sessionId, historyHydrationEpoch]);

  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    const selectionRevision = navigationRevision.current;
    const recovery = recoverCollaborationAgents().catch(() => []);
    const applySummaries = (summaries: Awaited<ReturnType<typeof listHistorySummaries>>) => {
      if (cancelled) return;
      nativeHistorySummariesLoaded.current = true;
      const conversationSummaries = summaries.filter((summary) => summary.metadata?.scope !== "collaboration_agent" && !deletedSessionIds.current.has(summary.id));
      if (takeLegacyProjectRecovery()) {
        setWorkspaces((previous) => recoverWorkspaceEntries(previous, conversationSummaries));
      }
      const collaborationSessionIds = new Set(summaries
        .filter((summary) => summary.metadata?.scope === "collaboration_agent")
        .map((summary) => summary.id));
      for (const summary of conversationSummaries) knownHistorySessionIds.current.add(summary.id);
      setHistoryHydrationEpoch((epoch) => epoch + 1);
      setSessions((previous) => {
        const previousById = new Map(previous.map((session) => [session.id, session]));
        const summariesAsSessions: ChatSession[] = conversationSummaries.map((item) => {
          const existing = previousById.get(item.id);
          return mergeHistorySummary(item, existing);
        });
        const summaryIds = new Set(summariesAsSessions.map((item) => item.id));
        const localOnly = previous.filter((session) => !summaryIds.has(session.id) && !collaborationSessionIds.has(session.id));
        return uniqueChatSessions([...summariesAsSessions, ...localOnly]).sort((a, b) => b.updatedAt - a.updatedAt);
      });
      // A hot reload or a cleared local cache can leave the view without a
      // selected id even though the native history is available. Select the
      // most recent standalone session so its full record is hydrated below.
      if (!activeSessionIdRef.current && navigationRevision.current === selectionRevision) {
        const available = conversationSummaries.filter((item) => !item.archived && !deletedSessionIds.current.has(item.id));
        const preferred = available.find((item) => item.metadata?.scope !== "workspace") || available[0];
        if (preferred) {
          setSessionId(preferred.id);
          setScope(preferred.metadata?.scope === "workspace" ? "workspace" : "standalone");
          setWorkspacePath(typeof preferred.metadata?.workspacePath === "string" ? preferred.metadata.workspacePath : "");
        }
      }
    };
    // Startup has committed interrupted run/message state before mounting this view.
    const initial = listHistorySummaries().then(applySummaries).catch(() => {
      if (!cancelled && takeLegacyProjectRecovery()) setWorkspaces((previous) => [...previous]);
    });
    void Promise.all([initial, recovery]).then(async ([, agents]) => {
      if (cancelled) return;
      nativeHistoryLoaded.current = true;
      setHistoryHydrationEpoch(epoch => epoch + 1);
      if (!agents.length) return;
      for (const run of agents) {
        conversationHistoryReadAhead.invalidate(run.sessionId);
        hydratedHistoryIds.current.delete(run.sessionId);
        historyHydrationAttempts.current.delete(run.sessionId);
        recoveryHistoryRefreshIds.current.add(run.sessionId);
      }
      setHistoryHydrationEpoch((epoch) => epoch + 1);
      applySummaries(await listHistorySummaries());
    }).catch(() => undefined);
    return () => { cancelled = true; };
  }, []);

  useEffect(() => {
    if (!isTauri() || !sessionId || (hydratedHistoryIds.current.has(sessionId) && !recoveryHistoryRefreshIds.current.has(sessionId))) return;
    const afterRecovery = recoveryHistoryRefreshIds.current.has(sessionId);
    const hasHistory = conversationHasHistory(sessionsRef.current.find((item) => item.id === sessionId), knownHistorySessionIds.current.has(sessionId));
    if (!afterRecovery && !hasHistory) {
      setHistoryReadState({ id: sessionId, status: "ready" });
      return;
    }
    let cancelled = false;
    setHistoryReadState({ id: sessionId, status: "loading" });
    const scheduleRetry = () => {
      if (cancelled) return;
      setHistoryReadState({ id: sessionId, status: "error" });
      if (!hasHistory && !afterRecovery) return;
      const attempts = historyHydrationAttempts.current.get(sessionId) || 0;
      if (attempts >= 4) return;
      historyHydrationAttempts.current.set(sessionId, attempts + 1);
      window.setTimeout(() => {
        if (!cancelled) setHistoryHydrationEpoch((epoch) => epoch + 1);
      }, 180 * (attempts + 1));
    };
    const read = afterRecovery
      ? invoke<HistoryRecord | null>("get_history_session", { id: sessionId })
      : conversationHistoryReadAhead.take(sessionId) || getHistorySession(sessionId);
    void read.then(async (record) => {
      if (cancelled || deletedSessionIds.current.has(sessionId)) return;
      if (!afterRecovery && recoveryHistoryRefreshIds.current.has(sessionId)) return;
      if (!record) {
        // A successful null read is different from an IPC/storage error.
        // Confirm against native navigation before dropping a stale cache.
        const summaries = await listHistorySummaries();
        if (cancelled) return;
        const current = sessionsRef.current.find(item => item.id === sessionId);
        const running = current?.messages.some(message => message.pending)
          || current?.agentRuns?.some(run => ["running", "preparing", "waiting", "waiting_for_permission"].includes(run.status))
          || preparingSubmissionsRef.current.has(sessionId) || pendingHistorySaves.current.has(sessionId);
        if (!summaries.some(summary => summary.id === sessionId) && !running) {
          deletedSessionIds.current.add(sessionId);
          markChatSessionDeleted(sessionId);
          conversationHistoryReadAhead.invalidate(sessionId);
          knownHistorySessionIds.current.delete(sessionId);
          sessionsRef.current = sessionsRef.current.filter(item => item.id !== sessionId);
          setSessions(sessionsRef.current);
          try { window.localStorage.setItem(STORAGE_KEY, JSON.stringify(sessionsRef.current)); } catch { /* Optional cache. */ }
          if (activeSessionIdRef.current === sessionId) {
            setSessionId("");
            setTitle("新的对话");
            setDraft("");
            setHistoryReadState({ id: "", status: "ready" });
          }
          return;
        }
        scheduleRetry();
        return;
      }
      hydratedHistoryIds.current.add(sessionId);
      recoveryHistoryRefreshIds.current.delete(sessionId);
      historyHydrationAttempts.current.delete(sessionId);
      knownHistorySessionIds.current.add(sessionId);
      setHistoryReadState({ id: sessionId, status: "ready" });
      const restored = historyRecordToSession(record);
      setSessions((previous) => upsertHistorySession(previous, restored));
      setTitle((current) => current === "新的对话" ? restored.title : current);
      setScope(restored.scope || "standalone");
      setWorkspacePath(restored.workspacePath || "");
      pendingForcedBottomSessionRef.current = sessionId;
    }).catch(scheduleRetry);
    return () => { cancelled = true; };
  }, [historyHydrationEpoch, sessionId]);

  useLayoutEffect(() => {
    if (!sessionId || pendingForcedBottomSessionRef.current !== sessionId) return;
    pendingForcedBottomSessionRef.current = "";
    // The optimistic pair has committed at this point. Settling now uses the
    // new virtual rows instead of the pre-send scroll height.
    scrollToLatest(true);
  }, [messageLayoutVersion, scrollToLatest, sessionId]);

  useEffect(() => {
    if (sending) return;
    let cancelled = false;
    if (chatCacheSaveTimer.current != null) window.clearTimeout(chatCacheSaveTimer.current);
    chatCacheSaveTimer.current = window.setTimeout(() => {
      chatCacheSaveTimer.current = null;
      void serializeChatCache(sessions, () => cancelled).then(serialized => {
        if (cancelled || serialized === null) return;
        window.localStorage.setItem(STORAGE_KEY, serialized);
        window.dispatchEvent(new Event(CHAT_NAV_EVENT));
      }).catch(() => {
        // Native history is authoritative. A full local cache must never make
        // the conversation renderer fail when storage is unavailable/full.
      });
    }, CHAT_CACHE_SAVE_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      if (chatCacheSaveTimer.current != null) window.clearTimeout(chatCacheSaveTimer.current);
    };
  }, [sending, sessions]);

  useEffect(() => {
    const flushChatCache = () => {
      if (chatCacheSaveTimer.current != null) window.clearTimeout(chatCacheSaveTimer.current);
      chatCacheSaveTimer.current = null;
      try {
        window.localStorage.setItem(STORAGE_KEY, JSON.stringify(sessionsRef.current));
      } catch {
        // Native history remains available after restart.
      }
    };
    window.addEventListener("pagehide", flushChatCache);
    return () => window.removeEventListener("pagehide", flushChatCache);
  }, []);

  useEffect(() => {
    // The local cache may contain a recovery placeholder until the complete
    // native record, including its terminal event, has been hydrated.
    if (!isTauri()
      || !nativeHistoryLoaded.current
      || !session
      || !session.messages.length
      || !hydratedHistoryIds.current.has(session.id)
      || recoveryHistoryRefreshIds.current.has(session.id)
      || sending) return;
    if (historySaveTimer.current) window.clearTimeout(historySaveTimer.current);
    historySaveTimer.current = window.setTimeout(() => {
      void saveSessionHistory(sessionToHistoryInput(session)).catch(() => undefined);
    }, 120);
    return () => {
      if (historySaveTimer.current) window.clearTimeout(historySaveTimer.current);
    };
  }, [session, sending, saveSessionHistory]);

  useEffect(() => {
    writeProjectPreference(WORKSPACE_KEY, JSON.stringify(workspaces));
    window.dispatchEvent(new Event(CHAT_NAV_EVENT));
  }, [workspaces]);

  /** 侧栏菜单动的是会话元数据（标题、置顶）：改完立刻把这条记录回写历史库。 */
  const patchSession = useCallback((id: string, patch: Partial<ChatSession>) => {
    const existing = sessionsRef.current.find((item) => item.id === id);
    if (!existing) return;
    const updated = { ...existing, ...patch };
    const next = sessionsRef.current.map((item) => (item.id === id ? updated : item));
    sessionsRef.current = next;
    setSessions(next);
    void saveSessionHistory(sessionToHistoryInput(updated)).catch(() => undefined);
  }, [saveSessionHistory]);

  useEffect(() => {
    const onNavDetail = (detail: ChatNavDetail | undefined) => {
      if (!detail) return;
      if (detail.type === "new-standalone") createConversation("standalone", "", detail.draft || "");
      if (detail.type === "new-workspace-session" && detail.path) createConversation("workspace", detail.path);
      if (detail.type === "open-session" && detail.id) {
        const picked = sessionsRef.current.find((item) => item.id === detail.id && !item.archived);
        if (!picked || deletedSessionIds.current.has(picked.id)) return;
        const prepared = conversationHistoryReadAhead.peek(picked.id);
        if (prepared) setSessions(previous => upsertHistorySession(previous, historyRecordToSession(prepared)));
        setScope(picked?.scope === "workspace" ? "workspace" : "standalone");
        setWorkspacePath(picked?.workspacePath || "");
        setSessionId(detail.id);
      }
      if (detail.type === "workspace" && detail.path) {
        setScope("workspace");
        setWorkspacePath(detail.path);
        const match = readChatSessions().find((item) => !item.archived && item.scope === "workspace" && workspaceKey(item.workspacePath || "") === workspaceKey(detail.path!));
        if (match) {
          setSessionId(match.id);
          setTitle(match.title);
        } else {
          setSessionId("");
          setStandaloneProjectIds(undefined);
          setTitle(detail.name || "新的对话");
          setDraft("");
        }
      }
      if (detail.type === "add-workspace" && detail.path && detail.name) {
        const path = detail.path;
        const name = detail.name;
        setWorkspaces((prev) => prev.some((item) => workspaceKey(item.path) === workspaceKey(path)) ? prev : [{ path, name }, ...prev]);
      }
      if (detail.type === "remove-workspace" && detail.path) {
        if (removalExecuting.current) return;
        setRemovalError("");
        setRemoveWorkspaceSessions(true);
        setPendingRemoval({ kind: "workspace", path: detail.path, title: readWorkspaces().find((item) => item.path === detail.path)?.name || detail.path });
      }
      if (detail.type === "remove-session" && detail.id) {
        if (removalExecuting.current) return;
        setRemovalError("");
        setPendingRemoval({ kind: "session", id: detail.id, title: sessions.find((item) => item.id === detail.id)?.title || "此对话" });
      }
      if (detail.type === "pin-session" && detail.id) patchSession(detail.id, { pinned: detail.next === true });
      if (detail.type === "rename-session" && detail.id && typeof detail.title === "string") {
        const title = detail.title.trim();
        if (title) patchSession(detail.id, { title });
      }
      if (detail.type === "archive-session" && detail.id) {
        const item = sessionsRef.current.find((candidate) => candidate.id === detail.id);
        if (item) setPendingArchive({ id: item.id, title: item.title || "此对话" });
      }
      if (detail.type === "restore-session" && detail.id) void restoreArchivedSession(detail.id);
      if (detail.type === "deleted-session" && detail.id) forgetArchivedSession(detail.id);
      if (detail.type === "fork-session" && detail.id) {
        const source = sessionsRef.current.find((candidate) => candidate.id === detail.id);
        if (!source || !isTauri()) return;
        void forkConversationThread(source.id).then((record) => {
          const forked = historyRecordToSession(record);
          knownHistorySessionIds.current.add(forked.id);
          sessionsRef.current = upsertHistorySession(sessionsRef.current, forked);
          setSessions(sessionsRef.current);
          setScope(forked.scope === "workspace" ? "workspace" : "standalone");
          setWorkspacePath(forked.workspacePath || "");
          setSessionId(forked.id);
        }).catch((error) => setActivity(`分叉对话失败：${formatInvokeError(error)}`));
      }
    };
    const onNav = (event: Event) => {
      const detail = (event as CustomEvent<ChatNavDetail>).detail;
      if (detail) acknowledgeChatNavDetail(detail);
      onNavDetail(detail);
    };
    window.addEventListener(CHAT_NAV_EVENT, onNav);
    for (const detail of takePendingChatNavDetails()) onNavDetail(detail);
    return () => window.removeEventListener(CHAT_NAV_EVENT, onNav);
  }, [workspacePath, sessionId, sessions]);

  // 菜单栏（全局执行环境）勾选把 SSH / WSL 绑定到某个对话；这里统一写入会话并持久化。
  useEffect(() => {
    const onBound = (event: Event) => {
      const detail = (event as CustomEvent<BindEnvironmentDetail>).detail;
      if (!detail?.sessionId || !detail.target) return;
      const current = sessionsRef.current;
      const next = updateSessionBoundEnvironments(current, detail.sessionId, detail.target, detail.bound);
      if (next === current) return;
      sessionsRef.current = next;
      setSessions(next);
      const updated = next.find((item) => item.id === detail.sessionId);
      if (updated) void saveSessionHistory(sessionToHistoryInput(updated)).catch(() => undefined);
    };
    window.addEventListener(BOUND_ENVIRONMENTS_EVENT, onBound);
    return () => window.removeEventListener(BOUND_ENVIRONMENTS_EVENT, onBound);
  }, [saveSessionHistory]);

  useEffect(() => {
    if (!session) return;
    setTitle(session.title);
  }, [sessionId]);

  useEffect(() => {
    if (!providerId && !modelId) return;
    cacheBrowserValue(CHAT_SELECTION_KEY, JSON.stringify({ providerId, modelId, sandboxMode }));
  }, [providerId, modelId, sandboxMode]);

  useEffect(() => { cacheBrowserValue(INFERENCE_LEVEL_KEY, inferenceLevel); }, [inferenceLevel]);

  useEffect(() => {
    setKnownWindow((current) => current?.key === modelConfigKey ? current : null);
    setContextChecking(false);
  }, [modelConfigKey]);

  useEffect(() => {
    if (!isTauri() || !provider || !model?.id || model.contextWindow || provider.contextWindow) return;
    void refreshContext();
  }, [refreshContext]);


  useEffect(() => {
    if (!isTauri()) return;
    let active = true;
    let revision = 0;
    const reloadProviders = async () => {
      const currentRevision = ++revision;
      try {
        const nextProviders = await listAgentProviders();
        if (!active || currentRevision !== revision) return;
        const nextSelection = reconcileModelSelection(nextProviders, modelSelectionRef.current);
        setProviders(nextProviders);
        setProviderRevision((current) => current + 1);
        modelSelectionRef.current = nextSelection;
        setProviderId(nextSelection.providerId);
        setModelId(nextSelection.modelId);
      } catch (reason) {
        if (active && currentRevision === revision) setSkillHint(formatInvokeError(reason));
      }
    };
    const onProvidersChanged = () => {
      quotaRequests.current.invalidate();
      quotaReading.current = { key: "", quota: null, checkedAt: null };
      contextRequests.current.invalidate();
      setQuota(null);
      setQuotaCheckedAt(null);
      setQuotaPhase("checking");
      setQuotaError("");
      // Credentials are intentionally absent from the persistent cache key.
      // Invalidate on configuration edits, including token-only changes.
      try {
        for (const configured of providersRef.current) window.localStorage.removeItem(quotaCacheKey(configured));
      } catch { /* Storage may be unavailable. */ }
      setKnownWindow(null);
      setContextChecking(false);
      void reloadProviders();
    };
    window.addEventListener(AGENT_PROVIDERS_CHANGED_EVENT, onProvidersChanged);
    void reloadProviders();
    const reloadSkills = () => void listSkills()
      .then((catalog) => {
        if (!active) return;
        const ready = catalog.skills.filter((skill) => skill.runtime === "instruction" && skill.status === "ready");
        setSkillHint(`${ready.length} 个 Skill 可用，AI 按需选择，也可输入 / 指定`);
        setSkills(ready);
      })
      .catch((reason) => { if (active) setSkillHint(formatInvokeError(reason)); });
    reloadSkills();
    window.addEventListener(SKILLS_CHANGED_EVENT, reloadSkills);
    return () => {
      active = false;
      quotaRequests.current.invalidate();
      contextRequests.current.invalidate();
      window.removeEventListener(AGENT_PROVIDERS_CHANGED_EVENT, onProvidersChanged);
      window.removeEventListener(SKILLS_CHANGED_EVENT, reloadSkills);
    };
  }, []);

  // Debounce 搜索查询以优化性能
  useEffect(() => {
    const timer = setTimeout(() => setDebouncedHistoryQuery(historyQuery), 300);
    return () => clearTimeout(timer);
  }, [historyQuery]);

  useEffect(() => {
    const query = debouncedHistoryQuery.trim();
    if (!isTauri() || !query) {
      setHistorySearchResults(null);
      return;
    }
    let cancelled = false;
    void searchHistorySessions(query).then((records) => {
      if (!cancelled) setHistorySearchResults(records.filter(record => !deletedSessionIds.current.has(record.id)).map(historyRecordToSession));
    }).catch((error) => { if (!cancelled) setActivity(formatInvokeError(error)); });
    return () => { cancelled = true; };
  }, [debouncedHistoryQuery]);

  useEffect(() => {
    if (!historyOpen) return;
    const closeOnOutsidePointerDown = (event: PointerEvent) => {
      const anchor = historyAnchorRef.current;
      if (anchor && event.target instanceof Node && anchor.contains(event.target)) return;
      setHistoryOpen(false);
    };
    document.addEventListener("pointerdown", closeOnOutsidePointerDown, true);
    return () => document.removeEventListener("pointerdown", closeOnOutsidePointerDown, true);
  }, [historyOpen]);

  const filteredHistory = useMemo(() => {
    const query = debouncedHistoryQuery.trim().toLowerCase();
    if (!query) return sessions;
    if (historySearchResults) return historySearchResults.filter(item => !deletedSessionIds.current.has(item.id));
    return sessions.filter((item) => {
      if (item.title.toLowerCase().includes(query)) return true;
      // 只搜索最近 20 条消息以优化性能
      return item.messages.slice(-20).some((message) => message.content.toLowerCase().includes(query));
    });
  }, [sessions, debouncedHistoryQuery, historySearchResults]);
  const activeHistory = filteredHistory.filter((item) => !item.archived);
  const archivedHistory = filteredHistory.filter((item) => item.archived);

  const ensureSession = (nextTitle: string, nextMessages: ChatMessage[], sessionKey = sessionId || `chat-${Date.now()}`, activate = true) => {
    const id = sessionKey;
    if (activate) { activeSessionIdRef.current = id; setSessionId(id); }
    setSessions((prev) => {
      const existing = prev.find((item) => item.id === id);
      const next = {
        ...existing,
        id,
        title: nextTitle,
        updatedAt: Date.now(),
        createdAt: existing?.createdAt ?? Date.now(),
        archived: existing?.archived ?? false,
        scope: existing?.scope || (workspacePath ? "workspace" : "standalone"),
        workspacePath: existing?.workspacePath || workspacePath || undefined,
        messages: nextMessages,
        sources: existing?.sources || sources,
        automationProjectIds: existing?.automationProjectIds ?? automationProjectIds,
        executionTarget: existing?.executionTarget || target,
        modelSelection: existing?.modelSelection ?? { providerId: modelSelectionRef.current.providerId, modelId: modelSelectionRef.current.modelId },
      };
      return existing ? prev.map((item) => (item.id === id ? next : item)) : [next, ...prev];
    });
  };

  const patchMessage = (sessionKey: string, messageId: string, update: (message: ChatMessage) => ChatMessage) => {
    setSessions((prev) => prev.map((item) => item.id !== sessionKey ? item : {
      ...item,
      updatedAt: Date.now(),
      messages: item.messages.map((message) => message.id === messageId ? update(message) : message),
    }));
  };

  const sendMessage = async (override?: string, steer = false, resumeTurnId?: string): Promise<boolean> => {
    const submittedDraft = draft;
    const submissionNavigation = navigationRevision.current;
    const submittedSources = sources.map((source) => ({ ...source }));
    const submittedProjectContext = projectAutomationContext(automationProjectIds);
    const clearSubmittedComposer = () => {
      if (activeSessionIdRef.current === sessionId && navigationRevision.current === submissionNavigation && override === undefined) {
        setDraft((current) => consumeComposerText(current, submittedDraft));
      }
      setSources((current) => remainingComposerSources(current, submittedSources));
    };
    const rawContent = (override ?? draft).trim();
    if (activePermission && rawContent && !resumeTurnId && !editingMessageId) {
      if (submittedSources.length) {
        notify("请先用文字回答当前询问；附件可以在回答后发送。", "info");
        return false;
      }
      const accepted = await resolvePermission("other", rawContent);
      if (accepted) clearSubmittedComposer();
      return accepted;
    }
    const selectedSkillIds: string[] = [];
    let requestedTool: string | null = null;
    const content = rawContent.replace(/(?:^|\s)\/(tool:)?([a-zA-Z0-9_-]+)\/?(?=\s|$)/g, (full, toolPrefix: string | undefined, id: string) => {
      if (toolPrefix) {
        requestedTool = id;
        return full.startsWith(" ") ? " " : "";
      }
      if (!skills.some((skill) => skill.id === id)) return full;
      selectedSkillIds.push(id);
      return full.startsWith(" ") ? " " : "";
    }).replace(/[ \t]{2,}/g, " ").trim();
    if (!content) return false;
    const requestSessionId = sessionId;
    if (rewindingSessionIds.current.has(requestSessionId)) return false;
    const requestIsSending = Boolean(requestSessionId && requestSessionId === sessionId && sending);
    if (requestIsSending) {
      const operation: "queue" | "steer" = steer ? "steer" : "queue";
      const operationKey = requestSessionId;
      if (composerActionRef.current.has(operationKey)) return false;
      composerActionRef.current.add(operationKey);
      setComposerActionPending(operation);
      let composerCleared = false;
      const acknowledgeOperation = () => {
        clearSubmittedComposer();
        composerCleared = true;
      };
      const updateOperationActivity = (message: string) => {
        if (activeSessionIdRef.current === requestSessionId) setActivity(message);
      };
      const reportOperationFailure = (error: unknown) => {
        if (composerCleared) {
          setSessions((previous) => previous.map((item) => item.id === requestSessionId ? {
            ...item, sources: restoreComposerSources(item.sources || [], submittedSources),
          } : item));
          if (activeSessionIdRef.current === requestSessionId && navigationRevision.current === submissionNavigation && override === undefined) {
            setDraft((current) => current || submittedDraft);
          }
        }
        const message = formatInvokeError(error);
        updateOperationActivity(message);
        notify(message, "error");
      };
      try {
      if (steer && requestSessionId) {
        try {
          const steerSources = submittedSources;
          // The active turn already owns its workspace context. A steer only
          // contributes sources explicitly attached to this follow-up.
          acknowledgeOperation();
          const { localArtifacts: artifacts, browserArtifacts, imageAttachments: attachments } = await attachmentPreparation.submission(steerSources);
          const activeAssistant = [...messages].reverse().find((message) => message.role === "assistant" && message.pending);
          const activeTurnId = activeThreadTurnId(threadSnapshot, messages) || activeAssistant?.turnId || null;
          await steerAgentRequest({ sessionId: requestSessionId, message: content, attachments, artifacts: [...markLocalArtifactSources(artifacts, undefined, steerSources), ...browserArtifacts], turnId: activeTurnId });
          pendingForcedBottomSessionRef.current = requestSessionId;
          updateOperationActivity("已发送给当前 Agent，正在接收追加要求。");
          scrollToLatest();
          return true;
        } catch (error) {
          reportOperationFailure(error);
          return false;
        }
      } else {
        if (!requestSessionId || !isTauri()) {
          reportOperationFailure("当前会话不可排队，请在桌面应用中重试。");
          return false;
        }
        if (!provider || !(model?.id || provider.model)) {
          reportOperationFailure("请先在设置里保存供应商，并在输入栏选择模型。");
          return false;
        }
        if (targetError) {
          reportOperationFailure(targetError);
          return false;
        }
        const queueTarget = target;
        const queueSources = submittedSources;
        const queueRoot = executionRoot(queueTarget);
        const queuedTurnId = `queued-turn-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
        // Native bind_workspace_artifact already binds workspacePath. Only
        // explicit attachments need frontend metadata; do not stat the root twice.
        acknowledgeOperation();
        const { localArtifacts, browserArtifacts, imageAttachments } = await attachmentPreparation.submission(queueSources);
        try {
          await enqueueAgentMessage({
            modelId: `${provider.id}:${model?.id || provider.model}`,
            message: content,
            artifacts: [...markLocalArtifactSources(localArtifacts, projectWorkspacePath, queueSources), ...browserArtifacts.filter(Boolean)],
            attachments: imageAttachments.filter((item): item is NonNullable<typeof item> => item !== null),
            workspacePath: projectWorkspacePath || queueRoot || null,
            sessionId: requestSessionId,
            turnId: queuedTurnId,
            permissionMode: sandboxMode,
            toolPolicy: "auto",
            skillIds: [...new Set(selectedSkillIds)],
            requestedTool,
            reasoningEffort: effectiveReasoningEffort(model, inferenceLevel),
            taskPlanMode: taskPlanSettings.mode,
            taskPlanMinSteps: taskPlanSettings.minSteps,
            context: submittedProjectContext,
            executionTarget: {
              kind: queueTarget.kind,
              connectionId: queueTarget.kind === "ssh" ? queueTarget.id : null,
              profileId: queueTarget.kind === "wsl" ? queueTarget.id : null,
              root: queueRoot || null,
            },
            executionTargets: executionTargetsPayload(queueTarget, queueRoot, boundEnvironments),
          });
          updateOperationActivity("已加入当前会话队列。");
        } catch (error) {
          reportOperationFailure(error);
          return false;
        }
      }
      return true;
      } catch (error) {
        reportOperationFailure(error);
        return false;
      } finally {
        composerActionRef.current.delete(operationKey);
        if (activeSessionIdRef.current === requestSessionId) setComposerActionPending("");
      }
    }
    const submissionNow = Date.now();
    const submissionScope = requestSessionId || "new-session";
    if (submissionNow < (initialSubmissionLockedUntilRef.current.get(submissionScope) || 0)) return false;
    initialSubmissionLockedUntilRef.current.set(submissionScope, submissionNow + 1_000);
    markInitialSubmissionLocked(submissionScope, submissionNow + 1_000);
    const currentTarget = target;
    const now = Date.now();
    const editingMessage = editingMessageId ? messages.find((message) => message.id === editingMessageId && message.role === "user") : undefined;
    const editingIndex = editingMessage ? messages.indexOf(editingMessage) : -1;
    let conversationMessages = messages;
    let historySession = session;
    let rewindPromise: Promise<HistoryRecord> | null = null;
    let editedMessageForRetry: ChatMessage | undefined;
    const id = sessionId || `chat-${crypto.randomUUID()}`;
    conversationHistoryReadAhead.invalidate(id);
    const resumedAssistant = resumeTurnId
      ? messages.find((message) => message.role === "assistant" && message.turnId === resumeTurnId)
      : undefined;
    if (resumeTurnId && (!sessionId || !resumedAssistant || resumableInterruptedTurnId(messages, session?.agentRuns) !== resumeTurnId)) {
      setActivity("未找到可继续的任务轮次。");
      return false;
    }
    const isActiveSession = () => activeSessionIdRef.current === id;
    const restoreUnsentComposer = () => {
      if (resumeTurnId) return;
      setSessions((previous) => previous.map((item) => item.id === id ? {
        ...item, sources: restoreComposerSources(item.sources || [], submittedSources),
      } : item));
      if (isActiveSession() && override === undefined) setDraft((current) => current || submittedDraft);
    };
    const turnId = `turn-${now}-${crypto.randomUUID()}`;
    const requestSources = resumeTurnId ? [] : submittedSources;
    const requestWorkspacePath = projectWorkspacePath;
    const requestTarget = currentTarget;
    const requestExecutionRoot = executionRoot(requestTarget);
    const userMessage: ChatMessage = {
      id: `m-${now}`,
      role: "user",
      content,
      createdAt: now,
      turnId,
      attachments: requestSources.map((item) => {
        const artifactPath = item.prepared?.artifact.path || item.path;
        const imagePath = item.kind === "image"
          ? item.prepared?.image?.path || item.imagePath || artifactPath
          : undefined;
        return {
          name: item.name,
          kind: item.kind,
          path: artifactPath,
          ...(imagePath ? { imagePath } : {}),
          ...(item.prepared?.image?.mediaType ? { mediaType: item.prepared.image.mediaType } : {}),
          origin: "input" as const,
        };
      }),
    };
    const assistantId = `m-${now + 1}`;
    const activeAssistantId = () => assistantId;
    const stopIfRemoving = () => {
      if (!deletedSessionIds.current.has(id) && !deletingSessionIds.current.has(id)) return false;
      patchMessage(id, assistantId, (message) => ({ ...message, pending: false, finishedAt: Date.now() }));
      return true;
    };
    const baseTitle = session?.title && session.title !== "新的对话" ? session.title : content.slice(0, 24);
    const nextTitle = baseTitle;
    const refuse = (text: string) => {
      setActivity(text);
      notify(text, "error");
    };
    if (!isTauri()) {
      refuse("浏览器预览不能调用模型。请在桌面应用里发送。");
      return false;
    }
    if (!resumeTurnId && (!provider || !(model?.id || provider.model))) {
      refuse("请先在设置里保存供应商，并在输入栏选择模型。");
      return false;
    }
    if (!resumeTurnId && targetError) {
      refuse(targetError);
      return false;
    }
    if (editingMessage && editingIndex >= 0) {
      const messageToRewind = editingMessage;
      editedMessageForRetry = messageToRewind;
      rewindingSessionIds.current.add(id);
      if (historySaveTimer.current) window.clearTimeout(historySaveTimer.current);
      // Start the native rewind immediately, but do not hold back the
      // optimistic user/assistant pair below. The old path awaited this
      // operation before rendering anything, so a large event history made
      // the edit button look stuck for several seconds.
      rewindPromise = (async () => {
        await Promise.allSettled([...(pendingHistorySaves.current.get(id) || [])]);
        if (stopIfRemoving()) throw new Error("对话已被删除");
        const record = await rewindConversationBeforeMessage(id, messageToRewind.id, messageToRewind.turnId);
        if (stopIfRemoving()) throw new Error("对话已被删除");
        return record;
      })();
      // The visible transcript should already be the prefix that survives the
      // edit. The durable rewind is awaited below before the model request.
      conversationMessages = messages.slice(0, editingIndex);
    }
    const quotaBeforeTurn = !resumeTurnId && provider && automaticQuotaRefresh
      ? (displayedQuota?.usedAmount != null || displayedQuota?.remainingAmount != null) && quotaCheckedAt && now - quotaCheckedAt >= 0 && now - quotaCheckedAt < 30_000
        ? Promise.resolve({ quota: displayedQuota, checkedAt: quotaCheckedAt })
        : Promise.resolve(null)
      : Promise.resolve(null);
    const assistantMessage: ChatMessage = { id: assistantId, role: "assistant", content: "", createdAt: now + 1, startedAt: now, pending: true, turnId };
    const turnMessages = [...conversationMessages, userMessage, assistantMessage];
    const activateSubmission = activeSessionIdRef.current === requestSessionId && navigationRevision.current === submissionNavigation;
    ensureSession(nextTitle, turnMessages, id, activateSubmission);
    if (editingMessage && activateSubmission) {
      setEditingMessageId("");
      editingSourcesBeforeRef.current = null;
    }
    if (!resumedAssistant) {
      setSessions((previous) => previous.map((item) => item.id === id ? {
        ...item,
        sources: remainingComposerSources(item.sources || requestSources, requestSources),
        artifacts: item.artifacts || [],
      } : item));
    }
    // Acknowledge the click before any disk/IPC await, including edit rewind.
    pendingForcedBottomSessionRef.current = id;
    if (activateSubmission) {
      setTitle(nextTitle);
      if (!resumedAssistant && override === undefined) setDraft((current) => consumeComposerText(current, submittedDraft));
      setActivity("正在准备请求");
    }
    if (!resumedAssistant && !requestSessionId) setStandaloneSources((current) => remainingComposerSources(current, requestSources));
    const preparingSubmission = { turnId, cancelled: false, dispatched: false };
    preparingSubmissionsRef.current.set(id, preparingSubmission);
    const attachmentsReady = attachmentPreparation.submission(requestSources);
    // History may fail first; keep a rejected image/storage task observed until
    // its result is consumed, so it cannot become an unhandled rejection.
    void attachmentsReady.catch(() => undefined);
    if (rewindPromise) {
      try {
        const record = await rewindPromise;
        historySession = historyRecordToSession(record);
        knownHistorySessionIds.current.add(id);
        hydratedHistoryIds.current.add(id);
      } catch (error) {
        const notice = `无法编辑此消息：${formatInvokeError(error)}`;
        rewindingSessionIds.current.delete(id);
        preparingSubmissionsRef.current.delete(id);
        setSessions((previous) => previous.map((item) => item.id === id ? { ...item, messages } : item));
        restoreUnsentComposer();
        if (isActiveSession()) {
          setComposerActionPending("");
          setActivity(notice);
          if (editedMessageForRetry) setEditingMessageId(editedMessageForRetry.id);
        }
        patchMessage(id, assistantId, (message) => ({
          ...message,
          pending: false,
          finishedAt: Date.now(),
          error: true,
          content: notice,
        }));
        notify(notice, "error");
        return false;
      }
      rewindingSessionIds.current.delete(id);
      if (isActiveSession()) setComposerActionPending("");
    }
    // Create the native record before the turn starts so its final event can
    // be appended durably even when a short request finishes before debounce.
    const initialHistorySession: ChatSession = {
      id,
      title: nextTitle,
      updatedAt: now,
      archived: session?.archived ?? false,
      scope: session?.scope || (requestWorkspacePath ? "workspace" : "standalone"),
      workspacePath: session?.workspacePath || requestWorkspacePath || undefined,
      dshStorageId: historySession?.dshStorageId,
      messages: turnMessages,
      contextCheckpoint: historySession?.contextCheckpoint,
      sources: resumedAssistant ? session?.sources : remainingComposerSources(session?.sources || requestSources, requestSources),
      automationProjectIds,
      artifacts: historySession?.artifacts || [],
      usage: historySession?.usage,
      executionTarget: requestTarget,
      timelineExpanded: session?.timelineExpanded,
      dismissedTaskPlanKey: session?.dismissedTaskPlanKey,
      modelSelection: session?.modelSelection,
    };
    // A turn must have a native record before it starts. The backend writes
    // its terminal event directly to that record if the WebView closes first.
    let nativeHistoryReady = hydratedHistoryIds.current.has(id);
    let historySetupFailed = false;
    if (!hydratedHistoryIds.current.has(id)) {
      let existingHistory: HistoryRecord | null | undefined;
      try {
        existingHistory = await getHistorySession(id);
      } catch {
        existingHistory = undefined;
        historySetupFailed = true;
      }
      if (stopIfRemoving()) { preparingSubmissionsRef.current.delete(id); return false; }
      if (existingHistory) {
        knownHistorySessionIds.current.add(id);
        hydratedHistoryIds.current.add(id);
        nativeHistoryReady = true;
        const restored = historyRecordToSession(existingHistory);
        // Native history cannot contain the not-yet-dispatched message.
        const restoredWithTurn = {
          ...restored,
          messages: [...restored.messages.filter((message) => message.turnId !== turnId),
            ...turnMessages.filter((message) => message.turnId === turnId)],
        };
        setSessions((previous) => upsertHistorySession(previous, restoredWithTurn));
      } else if (existingHistory === null) {
        const seeded = await saveSessionHistory(sessionToHistoryInput(initialHistorySession)).then(() => true).catch(() => false);
        if (seeded) {
          knownHistorySessionIds.current.add(id);
          hydratedHistoryIds.current.add(id);
          nativeHistoryReady = true;
        } else {
          historySetupFailed = true;
        }
      }
    }
    if (stopIfRemoving()) { preparingSubmissionsRef.current.delete(id); return false; }
    if (!nativeHistoryReady) {
      preparingSubmissionsRef.current.delete(id);
      restoreUnsentComposer();
      const notice = historySetupFailed
        ? "历史会话无法初始化，未开始本次任务。请稍后重试。"
        : "历史会话尚未就绪，未开始本次任务。请稍后重试。";
      patchMessage(id, activeAssistantId(), (message) => ({
        ...message,
        pending: false,
        finishedAt: Date.now(),
        error: true,
        content: notice,
      }));
      if (isActiveSession()) setActivity(notice);
      notify(notice, "error");
      return false;
    }
    let accepted = true;
    let requestError = "";
    let requestAccepted = false;
    try {
      if (preparingSubmission.cancelled) throw new Error("请求已取消，尚未发送给模型。");
      const { localArtifacts, browserArtifacts, imageAttachments, prepared } = await attachmentsReady;
      if (prepared.length) {
        patchMessage(id, userMessage.id, (message) => ({
          ...message,
          attachments: requestSources.map((source, index) => ({ name: source.name, kind: source.kind,
            path: prepared[index].artifact.path, imagePath: prepared[index].image?.path || undefined,
            mediaType: prepared[index].image?.mediaType, origin: "input" })),
        }));
      }
      const artifacts = [...markLocalArtifactSources(localArtifacts, requestWorkspacePath, requestSources), ...browserArtifacts.filter(Boolean)];
      // A slow billing endpoint must never hold the send button. Use a fresh
      // cached reading for this turn; the normal background refresh will
      // update the next one.
      const beforeTurn = await quotaBeforeTurn;
      if (beforeTurn) turnQuota.begin(id, turnId, Date.now(), beforeTurn.quota, beforeTurn.checkedAt);
      if (!resumeTurnId && provider) {
        turnPricing.current.set(JSON.stringify([id, turnId]), {
          inputPricePerMillion: provider.inputPricePerMillion,
          outputPricePerMillion: provider.outputPricePerMillion,
          currency: provider.currency,
        });
        if (turnPricing.current.size > 200) turnPricing.current.delete(turnPricing.current.keys().next().value!);
      }
      setSessions((previous) => previous.map((item) => item.id === id ? { ...item, artifacts: artifacts as Array<Record<string, unknown>> } : item));
      if (stopIfRemoving()) return false;
      if (preparingSubmission.cancelled) throw new Error("请求已取消，尚未发送给模型。");
      preparingSubmission.dispatched = true;
      // N10：受理回执只带 turn_id/status（数据本体走事件流）；旧实现对
      // text/usage/contextCheckpoint 的读取恒为空，已删除。N8：后端撞上并发
      // 时会自动降级入队并回 status="queued"，这里按成功处理，队列经事件流展示。
      const response = (resumeTurnId
        ? await resumeConversationTurn<AgentTurnStartReceipt>(id, resumeTurnId, turnId, sandboxMode)
        : await startConversationTurn<AgentTurnStartReceipt>(id, turnId, {
            modelId: `${provider!.id}:${model?.id || provider!.model}`,
            message: content,
            artifacts,
            attachments: imageAttachments.filter((item): item is NonNullable<typeof item> => item !== null),
            workspacePath: requestWorkspacePath || requestExecutionRoot || null,
            sessionId: id,
            turnId,
            permissionMode: sandboxMode,
            toolPolicy: "auto",
            skillIds: [...new Set(selectedSkillIds)],
            requestedTool,
            reasoningEffort: effectiveReasoningEffort(model, inferenceLevel),
            taskPlanMode: taskPlanSettings.mode,
            taskPlanMinSteps: taskPlanSettings.minSteps,
            context: submittedProjectContext,
            executionTarget: {
              kind: requestTarget.kind,
              connectionId: requestTarget.kind === "ssh" ? requestTarget.id : null,
              profileId: requestTarget.kind === "wsl" ? requestTarget.id : null,
              root: requestExecutionRoot || null,
            },
            executionTargets: executionTargetsPayload(requestTarget, requestExecutionRoot, boundEnvironments),
          })) || {};
      requestAccepted = true;
      if (preparingSubmission.cancelled) {
        await cancelAgentRequest(id, turnId, "cancel_submission").catch((error) => notify(formatInvokeError(error), "error"));
      }
      if (response.status === "queued" && isActiveSession()) {
        setActivity("当前会话仍在处理上一条消息，本条已加入队列。");
      }
      if (isActiveSession()) scrollToLatest();
    } catch (error) {
      const detail = formatInvokeError(error);
      accepted = false;
      requestError = detail;
      if (!requestAccepted) {
        restoreUnsentComposer();
        if (!preparingSubmission.cancelled) notify(detail, "error");
      }
    } finally {
      if (preparingSubmissionsRef.current.get(id) === preparingSubmission) preparingSubmissionsRef.current.delete(id);
      const persistedAt = Date.now();
      setSessions((previous) => previous.map((item) => item.id === id ? {
        ...item,
        updatedAt: Math.max(item.updatedAt, persistedAt),
      } : item));
      // Start/resume acknowledges dispatch while the native task keeps running.
      // Its history may still describe the previous turn here. Hydrate only
      // when the terminal snapshot arrives, keeping the optimistic pair intact.
      if (requestError) {
        setSessions((previous) => previous.map((item) => item.id !== id ? item : {
          ...item,
          messages: item.messages.map((message) => message.role !== "assistant" || message.turnId !== turnId || !message.pending ? message : {
            ...message, pending: false, finishedAt: Date.now(), error: !requestError.includes("请求已取消"),
            content: message.content || requestError,
            events: [...(message.events || []), { type: "status", status: requestError.includes("请求已取消") ? "cancelled" : "failed", message: requestError, terminalDiagnostic: true }],
          }),
        }));
      }
      if (isActiveSession()) setActivity(requestError || "正在等待模型响应");
      if (isActiveSession()) {
        pendingForcedBottomSessionRef.current = id;
        scrollToLatest(true);
      }
    }
    return accepted;
  };

  const resolvePermission = async (decision: "once" | "session" | "deny" | "other", text = permissionInstruction): Promise<boolean> => {
    if (!activePermission) return false;
    const current = activePermission;
    const key = `${current.sessionId}:${current.requestId}`;
    if (resolvingPermissionRequest.current === key) return false;
    resolvingPermissionRequest.current = key;
    setResolvingPermissionKey(key);
    try {
      await resolveConversationPermission(
        current.sessionId,
        current.requestId,
        decision,
        decision === "other" ? text.trim() || null : null,
      );
      if (activeSessionIdRef.current === current.sessionId && resolvingPermissionRequest.current === key) setPermissionInstruction("");
      return true;
    } catch (error) {
      if (resolvingPermissionRequest.current === key) resolvingPermissionRequest.current = null;
      setResolvingPermissionKey((value) => value === key ? null : value);
      if (activeSessionIdRef.current === current.sessionId) setActivity(formatInvokeError(error));
      notify(formatInvokeError(error), "error");
      return false;
    }
  };

  const stopAgent = async () => {
    if (!sessionId) return;
    const stoppingSessionId = sessionId;
    const preparingSubmission = preparingSubmissionsRef.current.get(stoppingSessionId);
    if (preparingSubmission) {
      preparingSubmission.cancelled = true;
      setActivity(preparingSubmission.dispatched ? "正在停止任务…" : "已取消发送，正在保留草稿。");
      if (!preparingSubmission.dispatched) {
        // P1-D: the submission may dispatch right after this check. Fire a
        // best-effort backend interrupt for its turn so that just-dispatched
        // turn cannot keep running; errors are ignored (no turn started yet).
        void cancelAgentRequest(stoppingSessionId, preparingSubmission.turnId, "stop_button").catch(() => undefined);
        return;
      }
    }
    const activeTurnId = preparingSubmission?.turnId || activeThreadTurnId(threadSnapshot, messages);
    setActivity("正在停止任务…");
    // N11-A: settle each cancellation independently so one uncancellable
    // background task (device install phase) can never mask the agent's own
    // stop result; task failures are reported separately below.
    const [agentResult, ...taskResults] = await Promise.allSettled([
      cancelAgentRequest(stoppingSessionId, activeTurnId, "stop_button"),
      ...managedTasks.map((task) => cancelAndroidTask(task.id)),
    ]);
    const taskFailures = taskResults
      .filter((result): result is PromiseRejectedResult => result.status === "rejected")
      .map((result) => formatInvokeError(result.reason));
    if (taskFailures.length) {
      notify(`部分后台任务无法取消：${taskFailures.join("；")}`, "error");
    }
    if (agentResult.status === "rejected") {
      const detail = formatInvokeError(agentResult.reason);
      if (activeSessionIdRef.current === stoppingSessionId) setActivity(detail);
      notify(detail, "error");
      return;
    }
    const cancellation = agentResult.value;
    if (cancellation.completed) {
      setSessions((previous) => previous.map((session) => session.id !== stoppingSessionId ? session : { ...session,
        messages: session.messages.map((message) => message.role === "assistant" && message.pending && (!activeTurnId || message.turnId === activeTurnId)
          ? { ...message, pending: false, finishedAt: message.finishedAt || Date.now() } : message) }));
      if (activeSessionIdRef.current === stoppingSessionId) {
        setActivity("已暂停");
      }
    } else if (activeSessionIdRef.current === stoppingSessionId) {
      setActivity(cancellation.requested ? "已请求停止，正在等待运行中的指令退出…" : "任务状态已切换，本次停止未确认，请重试。");
    }
  };

  const guideQueuedMessage = async (queueId: string) => {
    if (!sessionId) return;
    const queueSessionId = sessionId;
    const mutationKey = `${queueSessionId}:${queueId}`;
    if (queueMutationRef.current.has(mutationKey)) return;
    queueMutationRef.current.add(mutationKey);
    setQueueMutationState((current) => ({ ...current, [queueId]: "guide" }));
    try {
      await guideQueuedAgentMessage(queueSessionId, queueId);
      if (activeSessionIdRef.current === queueSessionId) setActivity("已追加给当前 Agent。");
    } catch (error) {
      if (activeSessionIdRef.current === queueSessionId) setActivity(formatInvokeError(error));
      notify(formatInvokeError(error), "error");
    } finally {
      queueMutationRef.current.delete(mutationKey);
      if (activeSessionIdRef.current === queueSessionId) setQueueMutationState((current) => {
        const next = { ...current };
        delete next[queueId];
        return next;
      });
    }
  };

  const removeQueuedMessage = async (queueId: string) => {
    if (!sessionId) return;
    const queueSessionId = sessionId;
    const mutationKey = `${queueSessionId}:${queueId}`;
    if (queueMutationRef.current.has(mutationKey)) return;
    queueMutationRef.current.add(mutationKey);
    setQueueMutationState((current) => ({ ...current, [queueId]: "remove" }));
    try {
      await removeQueuedAgentMessage(queueSessionId, queueId);
      if (activeSessionIdRef.current === queueSessionId) setActivity("已删除排队消息。");
    } catch (error) {
      if (activeSessionIdRef.current === queueSessionId) setActivity(formatInvokeError(error));
      notify(formatInvokeError(error), "error");
      throw error;
    } finally {
      queueMutationRef.current.delete(mutationKey);
      if (activeSessionIdRef.current === queueSessionId) setQueueMutationState((current) => {
        const next = { ...current };
        delete next[queueId];
        return next;
      });
    }
  };

  const restoreQueuedMessageToComposer = useCallback((item: { content: string; images?: AgentImageAttachment[]; artifacts?: Array<{ path: string; name: string; kind: string; size?: number | null }> }) => {
    setDraft((current) => current.trim() ? `${current.replace(/\s+$/, "")}\n${item.content}` : item.content);
    const matchedImageArtifacts = new Set<number>();
    const restoredImages: ChatSource[] = (item.images || []).map((image) => ({
      path: (() => {
        const artifactIndex = item.artifacts?.findIndex((artifact, index) => !matchedImageArtifacts.has(index)
          && artifact.name.toLowerCase() === image.name.toLowerCase() && artifact.kind !== "directory") ?? -1;
        if (artifactIndex >= 0) {
          matchedImageArtifacts.add(artifactIndex);
          return item.artifacts![artifactIndex].path;
        }
        return image.path || `browser:${image.name}`;
      })(),
      name: image.name,
      kind: "image",
      ...(image.path ? { imagePath: image.path } : {}),
      ...(image.dataUrl ? { dataUrl: image.dataUrl } : {}),
    }));
    const imagePaths = new Set(restoredImages.map((source) => localArtifactPathKey(source.path)));
    const imageNames = new Set(restoredImages.map((source) => source.name.toLowerCase()));
    const restoredArtifacts: ChatSource[] = (item.artifacts || []).flatMap((artifact) => {
      if (!artifact.path || imagePaths.has(localArtifactPathKey(artifact.path))
        || (artifact.kind !== "directory" && imageNames.has(artifact.name.toLowerCase()))) return [];
      const directory = artifact.kind === "directory";
      return [{ path: artifact.path, name: artifact.name, kind: classifyDropped(artifact.name, directory) || (directory ? "directory" : "document"),
        ...(artifact.size == null ? {} : { size: artifact.size }) }];
    });
    const restoredSources = [...restoredImages, ...restoredArtifacts];
    if (restoredSources.length) setSources((current) => [
      ...current,
      ...restoredSources.filter((source) => !current.some((existing) => localArtifactPathKey(existing.path) === localArtifactPathKey(source.path)
        || (source.dataUrl && existing.dataUrl === source.dataUrl))),
    ]);
    window.requestAnimationFrame(() => composerEditorRef.current?.focus());
  }, [setSources]);

  const editQueuedMessage = async (item: typeof renderedQueuedMessages[number]) => {
    if (!sessionId) return;
    const queueSessionId = sessionId;
    const mutationKey = `${queueSessionId}:${item.id}`;
    if (queueMutationRef.current.has(mutationKey)) return;
    queueMutationRef.current.add(mutationKey);
    setQueueMutationState((current) => ({ ...current, [item.id]: "edit" }));
    try {
      await removeQueuedAgentMessage(queueSessionId, item.id);
      if (activeSessionIdRef.current !== queueSessionId) return;
      restoreQueuedMessageToComposer(item);
      setActivity("已将排队消息移回输入框。");
    } catch (error) {
      if (activeSessionIdRef.current === queueSessionId) setActivity(formatInvokeError(error));
      notify(formatInvokeError(error), "error");
    } finally {
      queueMutationRef.current.delete(mutationKey);
      if (activeSessionIdRef.current === queueSessionId) setQueueMutationState((current) => {
        const next = { ...current };
        delete next[item.id];
        return next;
      });
    }
  };

  const updateQueuedMessage = async (queueId: string, content: string) => {
    if (!sessionId || !content.trim()) return false;
    const queueSessionId = sessionId;
    const mutationKey = `${queueSessionId}:${queueId}`;
    if (queueMutationRef.current.has(mutationKey)) return false;
    queueMutationRef.current.add(mutationKey);
    setQueueMutationState((current) => ({ ...current, [queueId]: "edit" }));
    try {
      await updateQueuedAgentMessage(queueSessionId, queueId, content.trim());
      if (activeSessionIdRef.current === queueSessionId) setActivity("已更新排队消息。");
      return true;
    } catch (error) {
      if (activeSessionIdRef.current === queueSessionId) setActivity(formatInvokeError(error));
      notify(formatInvokeError(error), "error");
      return false;
    } finally {
      queueMutationRef.current.delete(mutationKey);
      if (activeSessionIdRef.current === queueSessionId) setQueueMutationState((current) => {
        const next = { ...current };
        delete next[queueId];
        return next;
      });
    }
  };

  const reorderQueuedMessages = async (queueIds: string[]) => {
    if (!sessionId || queueIds.length < 2) return true;
    const queueSessionId = sessionId;
    const mutationKey = `${queueSessionId}:reorder`;
    if (queueMutationRef.current.has(mutationKey) || queueIds.some(id => queueMutationRef.current.has(`${queueSessionId}:${id}`))) return false;
    queueMutationRef.current.add(mutationKey);
    setQueueMutationState(current => ({ ...current, ...Object.fromEntries(queueIds.map(id => [id, "reorder" as const])) }));
    try {
      await reorderQueuedAgentMessages(queueSessionId, queueIds);
      return true;
    } catch (error) {
      if (activeSessionIdRef.current === queueSessionId) setActivity(formatInvokeError(error));
      notify(formatInvokeError(error), "error");
      return false;
    } finally {
      queueMutationRef.current.delete(mutationKey);
      if (activeSessionIdRef.current === queueSessionId) setQueueMutationState(current => {
        const next = { ...current };
        for (const id of queueIds) if (next[id] === "reorder") delete next[id];
        return next;
      });
    }
  };

  const copyAttachmentText = async (value: string, successMessage: string) => {
    try {
      await navigator.clipboard.writeText(value);
      setActivity(successMessage);
    } catch (error) {
      setActivity(`复制失败：${formatInvokeError(error)}`);
    }
  };

  const clearActiveConversation = () => {
    activeSessionIdRef.current = "";
    setSessionId("");
    setTitle("新的对话");
    setDraft("");
    setStandaloneSources([]);
    setStandaloneProjectIds(undefined);
    setWorkspacePath("");
    setScope("standalone");
    setFilePreviewPath(null);
    setFilePreviewOrigin(undefined);
    setCollaborationOpen(false);
    setTerminalOpen(false);
    setEnvironmentOpen(false);
    setTaskPlanOpen(false);
    setWorkspaceEnvironment(null);
    setWorkspaceGitChanges([]);
    setActivity("");
  };

  const removeChatSession = async (id: string) => {
    conversationHistoryReadAhead.invalidate(id);
    deletingSessionIds.current.add(id);
    const deleting = sessionsRef.current.find((item) => item.id === id);
    const activeTurnId = [...(deleting?.messages || [])].reverse().find((message) => message.role === "assistant" && message.pending)?.turnId || null;
    try {
      if (isTauri()) {
        const cancelled = await cancelAgentRequest(id, activeTurnId, "delete_session");
        if (!cancelled.completed) {
          throw new Error("Agent 仍在停止中，停止完成后才能删除该会话。");
        }
        // Drain writes already sent to native storage before deleting its record.
        await Promise.allSettled([...(pendingHistorySaves.current.get(id) || [])]);
        await deleteHistorySession(id);
      }
      deletedSessionIds.current.add(id);
      markChatSessionDeleted(id);
      knownHistorySessionIds.current.delete(id);
      hydratedHistoryIds.current.delete(id);
      if (historySaveTimer.current != null) window.clearTimeout(historySaveTimer.current);
      if (chatCacheSaveTimer.current != null) window.clearTimeout(chatCacheSaveTimer.current);
      sessionsRef.current = sessionsRef.current.filter((item) => item.id !== id);
      setSessions((previous) => previous.filter((item) => item.id !== id));
      pruneSessionDraft(id);
      if (activeSessionIdRef.current === id) clearActiveConversation();
      try { window.localStorage.setItem(STORAGE_KEY, JSON.stringify(sessionsRef.current)); } catch { /* Native deletion is already committed. */ }
    } catch (error) {
      setActivity(`删除会话失败：${formatInvokeError(error)}`);
      throw error;
    } finally {
      deletingSessionIds.current.delete(id);
    }
  };

  const openAttachment = useCallback((path: string, origin?: FilePreviewOrigin) => {
    const absolute = /^(?:[A-Za-z]:[\\/]|[\\/])/.test(path);
    // 本机与 WSL 的相对路径按工作区拼绝对路径；SSH 对话保持原样，交给预览按
    // 远程根目录解析——拼上 Windows 工作区会让远端文件被误判成本机文件。
    const joinLocal = !absolute && Boolean(workspacePath) && target.kind !== "ssh";
    setFilePreviewPath(joinLocal ? `${workspacePath.replace(/[\\/]+$/, "")}/${path}` : path);
    setFilePreviewOrigin(origin?.sessionId && origin.turnId ? origin : undefined);
    // 每次打开新文件都让路径形态重新决定环境，免得上一次的手动选择把本机文件送去远程读。
    setPreviewEnvironmentKey("");
    setCollaborationOpen(false);
  }, [workspacePath, target.kind]);

  const copyAttachmentContent = async (path: string) => {
    try {
      const content = await readLocalAttachmentForCopy(path);
      await copyAttachmentText(content, "已复制文件内容");
    } catch (error) {
      setActivity(`复制文件内容失败：${formatInvokeError(error)}`);
    }
  };

  const createConversation = (nextScope: "workspace" | "standalone", path = "", initialDraft = "") => {
    const existing = findEmptyConversation(sessionsRef.current, nextScope, path, activeSessionIdRef.current, knownHistorySessionIds.current);
    if (existing) {
      if (existing.id !== activeSessionIdRef.current) {
        // Seed the reused conversation's draft before switching: the sessionId
        // layout effect restores the target conversation's draft from the
        // store, so the seed has to be written there first.
        writeSessionDraft(existing.id, initialDraft);
      } else if (initialDraft) {
        setDraft(initialDraft);
      }
      setSessionId(existing.id);
      setTitle(existing.title);
      setScope(nextScope);
      setWorkspacePath(existing.workspacePath || "");
      activeSessionIdRef.current = existing.id;
      return;
    }
    const id = `chat-${crypto.randomUUID()}`;
    const created: ChatSession = {
      id,
      title: "新的对话",
      updatedAt: Date.now(),
      createdAt: Date.now(),
      archived: false,
      scope: nextScope,
      workspacePath: path || undefined,
      messages: [],
    };
    writeSessionDraft(id, initialDraft);
    // Reserve immediately so queued clicks before React commits reuse this id.
    sessionsRef.current = [created, ...sessionsRef.current];
    activeSessionIdRef.current = id;
    setSessions((previous) => [created, ...previous]);
    setSessionId(id);
    setTitle("新的对话");
    setDraft(initialDraft);
    setScope(nextScope);
    setWorkspacePath(path);
  };

  const addWorkspace = async () => {
    const path = await chooseDirectory("选择工作区目录");
    if (!path) return;
    const name = path.split(/[\\/]/).filter(Boolean).pop() || path;
    setWorkspaces((prev) => prev.some((item) => workspaceKey(item.path) === workspaceKey(path)) ? prev : [{ path, name }, ...prev]);
    setWorkspacePath(path);
    setScope("workspace");
  };

  const toggleTerminal = () => {
    setTerminalOpen((value) => !value);
  };

  const toggleComponentsDebug = () => {
    setComponentsDebugOpen((value) => !value);
    setFilePreviewPath(null);
  };

  const toggleWorkspaceSidePanel = (view: "agents" | "changes") => {
    setFilePreviewPath(null);
    if (collaborationOpen && collaborationSideView === view) {
      setCollaborationOpen(false);
      return;
    }
    setCollaborationSideView(view);
    setCollaborationOpen(true);
    if (view === "changes") void refreshWorkspaceEnvironment();
  };

  const showWorkspaceSidePanel = (view: "agents" | "changes") => {
    setFilePreviewPath(null);
    setCollaborationSideView(view);
    setCollaborationOpen(true);
    if (view === "changes") void refreshWorkspaceEnvironment();
  };


  const openHistorySession = async (id: string) => {
    if (deletedSessionIds.current.has(id)) return;
    try {
      if (isTauri()) {
        const record = await getHistorySession(id);
        if (deletedSessionIds.current.has(id)) return;
        if (record) {
          const restored = historyRecordToSession(record);
          hydratedHistoryIds.current.add(id);
          knownHistorySessionIds.current.add(id);
          setSessions((previous) => upsertHistorySession(previous, restored));
          setTitle(restored.title);
          setScope(restored.scope || "standalone");
          setWorkspacePath(restored.workspacePath || "");
        }
      }
      setSessionId(id);
      setHistoryOpen(false);
      scrollToLatest(true);
    } catch (error) { setActivity(formatInvokeError(error)); }
  };

  const setHistoryArchived = async (item: ChatSession, archived: boolean) => {
    conversationHistoryReadAhead.invalidate(item.id);
    try {
      if (isTauri()) await archiveHistorySession(item.id, archived);
      setSessions((previous) => previous.map((session) => session.id === item.id ? { ...session, archived } : session));
      if (archived && activeSessionIdRef.current === item.id) clearActiveConversation();
    } catch (error) { setActivity(formatInvokeError(error)); }
  };

  /** 归档悬浮窗恢复会话：悬浮窗自己已经调过 archiveHistorySession(false)，这里只把
      记录重新读回侧栏快照——不重复发一次写请求。 */
  const restoreArchivedSession = async (id: string) => {
    conversationHistoryReadAhead.invalidate(id);
    try {
      if (!isTauri()) {
        setSessions((previous) => previous.map((session) => session.id === id ? { ...session, archived: false } : session));
        return;
      }
      const record = await getHistorySession(id);
      if (!record) return;
      const restored = historyRecordToSession(record);
      hydratedHistoryIds.current.add(id);
      knownHistorySessionIds.current.add(id);
      sessionsRef.current = upsertHistorySession(sessionsRef.current, restored);
      setSessions(sessionsRef.current);
    } catch (error) { setActivity(`恢复会话失败：${formatInvokeError(error)}`); }
  };

  /** 归档悬浮窗彻底删除：悬浮窗已删除历史库记录，这里清掉本地快照与读缓存，
      避免侧栏在下次刷新前还留着这条会话。 */
  const forgetArchivedSession = (id: string) => {
    conversationHistoryReadAhead.invalidate(id);
    deletedSessionIds.current.add(id);
    markChatSessionDeleted(id);
    knownHistorySessionIds.current.delete(id);
    hydratedHistoryIds.current.delete(id);
    sessionsRef.current = sessionsRef.current.filter((item) => item.id !== id);
    setSessions((previous) => previous.filter((item) => item.id !== id));
    pruneSessionDraft(id);
    if (activeSessionIdRef.current === id) clearActiveConversation();
  };

  const exportHistory = async () => {
    try {
      const content = isTauri() ? await exportHistorySessions() : JSON.stringify(sessions, null, 2);
      const path = await saveFile("导出聊天历史", "android-tool-history.json", [{ name: "JSON", extensions: ["json"] }]);
      if (!path) return;
      await invoke("write_history_export", { path, content });
    } catch (error) { setActivity(formatInvokeError(error)); }
  };

  const interruptedRunsByTurnId = useMemo(() => new Map((session?.agentRuns || [])
    .filter((run) => run.status === "interrupted")
    .map((run) => [run.turnId, run])), [session?.agentRuns]);
  const turnFileChangesByMessage = useMemo(() => messageFileChanges(messages, sessionId), [messages, sessionId]);
  const awaitingHistory = !messages.length && Boolean(sessionId)
    && conversationHasHistory(session, knownHistorySessionIds.current.has(sessionId)) && (
    (historyReadState.id === sessionId && historyReadState.status !== "ready")
    || (!hydratedHistoryIds.current.has(sessionId) && (!session || Boolean(session.historyMessageCount)))
  );
  const handleTimelineExpandedChange = useCallback((messageId: string, expanded: boolean) => {
    const activeId = activeSessionIdRef.current;
    if (!activeId) return;
    setSessions((previous) => previous.map((candidate) => candidate.id === activeId
      ? { ...candidate, timelineExpanded: updateTimelineExpanded(candidate.timelineExpanded, messageId, expanded) }
      : candidate));
  }, []);
  const handlePreviewAttachment = useCallback((attachment: { name: string; dataUrl: string }) => {
    setAttachmentPreview(attachment);
  }, []);
  const handlePathMenu = useCallback((path: string, x: number, y: number) => {
    setPathMenu({ path, x, y });
  }, []);
  const handleCopyMessage = useCallback((messageId: string, content: string) => {
    void navigator.clipboard.writeText(content);
    setCopiedId(messageId);
  }, []);
  const handleEditMessage = useCallback((message: ChatMessage) => {
    if (sending || message.role !== "user" || (!message.content.trim() && !message.attachments?.length)) return;
    editingSourcesBeforeRef.current = sources.map((source) => ({ ...source }));
    setEditingMessageId(message.id);
    setDraft(message.content);
    setSources(messageAttachmentsToSources(message.attachments));
    window.requestAnimationFrame(() => composerEditorRef.current?.focus());
  }, [sending, setSources, sources]);
  // 消息行上的 fork 按钮：和侧栏菜单的「分叉会话」是同一条通路（复制历史再切过去），
  // 差别只是入口更近——想从这条回复岔开时不用先回侧栏找当前对话。
  const handleForkMessage = useCallback(() => {
    const sourceId = activeSessionIdRef.current;
    const source = sourceId ? sessionsRef.current.find((candidate) => candidate.id === sourceId) : undefined;
    if (!source || !isTauri()) return;
    void forkConversationThread(source.id).then((record) => {
      const forked = historyRecordToSession(record);
      knownHistorySessionIds.current.add(forked.id);
      sessionsRef.current = upsertHistorySession(sessionsRef.current, forked);
      setSessions(sessionsRef.current);
      setScope(forked.scope === "workspace" ? "workspace" : "standalone");
      setWorkspacePath(forked.workspacePath || "");
      setSessionId(forked.id);
    }).catch((error) => setActivity(`分叉对话失败：${formatInvokeError(error)}`));
  }, []);

  return (
    <>
    <section className="conversation-page">
      <div className="conversation-content-area">
        <div className="conversation-toolbar">
          {onToggleNavigation && <Button type="button" variant="surface" size="sm" className="chat-icon-button conversation-navigation-toggle" title={navigationExpanded ? "收起导航" : "展开导航"} aria-label={navigationExpanded ? "收起导航" : "展开导航"} aria-expanded={navigationExpanded} aria-controls="app-context-navigation" icon={<PanelLeft size={14} strokeWidth={1.7} />} onClick={onToggleNavigation} />}
          <div className="session-header-actions">
              <Button type="button" variant="surface" size="sm" className={`chat-icon-button${collaborationOpen && collaborationSideView === "agents" ? " active" : ""}`} title={collaborationOpen && collaborationSideView === "agents" ? "隐藏子智能体" : "打开子智能体"} aria-label={collaborationOpen && collaborationSideView === "agents" ? "隐藏子智能体" : "打开子智能体"} aria-pressed={collaborationOpen && collaborationSideView === "agents"} icon={<Bot size={14} />} onClick={() => toggleWorkspaceSidePanel("agents")} />
              <Button type="button" variant="surface" size="sm" className={`chat-icon-button${collaborationOpen && collaborationSideView === "changes" ? " active" : ""}`} title={collaborationOpen && collaborationSideView === "changes" ? "隐藏文件改动" : "打开文件改动"} aria-label={collaborationOpen && collaborationSideView === "changes" ? "隐藏文件改动" : "打开文件改动"} aria-pressed={collaborationOpen && collaborationSideView === "changes"} icon={<FilePenLine size={14} />} onClick={() => toggleWorkspaceSidePanel("changes")} />
              <Button type="button" variant="surface" size="sm" className={`chat-icon-button${terminalOpen ? " active" : ""}`} title={terminalOpen ? "隐藏终端" : "打开终端"} aria-label={terminalOpen ? "隐藏终端" : "打开终端"} aria-pressed={terminalOpen} icon={<SquareTerminal size={14} />} onClick={toggleTerminal} />
            {debugTargets.chat && <div ref={debugAnchorRef} className="chat-debug-anchor">
            <Button
              type="button"
              variant="surface"
              size="sm"
              className={`chat-icon-button${componentsDebugOpen ? " active" : ""}`}
              title={componentsDebugOpen ? "隐藏组件调试" : "选择要显示的调试组件"}
              aria-label={componentsDebugOpen ? "隐藏组件调试" : "选择要显示的调试组件"}
              aria-pressed={componentsDebugOpen}
              icon={<Bug size={14} />}
              onClick={toggleComponentsDebug}
            />
            {componentsDebugOpen && <ChatDebugPicker
              on={componentsDebug}
              onToggle={toggleComponentDebug}
              position={debugPickerPosition}
              portalRoot={workspaceRef.current?.querySelector<HTMLElement>(".chat-footer") || document.body}
              menuRef={debugMenuRef}
            />}
            </div>}
            <div ref={environmentAnchorRef} className="environment-anchor">
              <Button type="button" variant="surface" size="sm" className={`chat-icon-button${environmentOpen ? " active" : ""}`} title="查看当前会话环境" aria-label="查看当前会话环境" aria-expanded={environmentOpen} icon={<Info size={14} />} onClick={() => { const opening = !environmentOpen; setEnvironmentOpen(opening); if (opening) void refreshWorkspaceEnvironment(); }} />
              {environmentOpen && (
                <aside className="environment-popover">
                  <div className="popover-heading"><span>工作区环境</span><span className="popover-readonly">本地只读</span></div>
                  <div className="popover-section environment-workspace-card">
                    <FolderOpen size={15} />
                    <div><b>{workspaceEnvironment?.workspaceName || (workspacePath ? "工作区" : "独立对话")}</b><small>{workspacePath || "未绑定工作区目录"}</small></div>
                  </div>
                  {boundEnvironments.length > 0 && <div className="popover-section environment-bound-card">
                    <b>已绑定环境</b>
                    <ul className="environment-bound-list">
                      {boundEnvironments.map((item, index) => <li key={`${item.kind}:${item.id}`}>
                        <span>{item.label}</span>
                        <small>{index === 0 ? "默认执行环境" : "供 target 调用"}</small>
                      </li>)}
                    </ul>
                  </div>}
                  {workspacePath ? <div className="popover-muted">{workspaceEnvironmentLoading ? "正在读取 Git 状态…" : !workspaceEnvironment?.git.available ? "未安装 Git" : !workspaceEnvironment.git.repository ? "当前目录不是 Git 仓库" : `${workspaceEnvironment.git.branch || "未检测到分支"} · ${workspaceEnvironment.git.dirty ? `${workspaceEnvironment.git.changedFiles} 个文件有变更` : "工作区干净"} · +${workspaceEnvironment.git.addedLines} / -${workspaceEnvironment.git.removedLines} 行${workspaceEnvironment.git.remote ? ` · ${workspaceEnvironment.git.remote}` : ""}`}</div> : <div className="popover-muted">独立对话不会绑定项目目录，也不会自动读取 Git 状态。</div>}
                  {workspaceEnvironment?.git.repository && workspaceGitChanges.length > 0 && <button type="button" className="workspace-git-change-link" onClick={() => { setEnvironmentOpen(false); showWorkspaceSidePanel("changes"); }}><FileCode2 size={13} />在右侧查看 {workspaceGitChanges.length} 个文件改动</button>}
                  <div className="environment-actions">
                    <Button type="button" variant="secondary" size="xs" disabled title="Git 提交将在后续版本接入" icon={<Check size={13} />}>提交</Button>
                    <Button type="button" variant="secondary" size="xs" disabled title="Git 推送将在后续版本接入" icon={<Send size={13} />}>推送</Button>
                  </div>
                </aside>
              )}
            </div>
          </div>
        </div>

        <div className="chat-main-layout" ref={sidePanelLayoutRef} data-split={filePreviewPath || collaborationOpen ? "on" : "off"} style={{ "--chat-side-panel-width": `${sidePanelWidth.width}px` } as React.CSSProperties}>
          <section ref={workspaceRef} className={`chat-workspace${dropActive ? " drop-target-active" : ""}${!messages.length && !awaitingHistory && !terminalOpen ? " is-empty" : ""}`} onDragEnter={onChatDragOver} onDragOver={onChatDragOver} onDragLeave={onChatDragLeave} onDrop={(event) => { void onChatDrop(event); }}>
            {dropActive && <div className="chat-drop-overlay"><FolderOpen size={20} /><span>松开以添加文件或文件夹</span></div>}
            <div className="chat-conversation-viewport">
            <div
              ref={scrollRef}
              className="chat-scroll"
              tabIndex={0}
              onWheel={onScrollWheel}
              onTouchStart={onScrollTouchStart}
              onTouchMove={onScrollTouchMove}
              onKeyDown={onScrollKeyDown}
              onPointerDown={(event) => {
                const bounds = event.currentTarget.getBoundingClientRect();
                if (event.clientX >= bounds.right - 18) markUserScrollIntent();
              }}
              onScroll={onScroll}
            >
              {awaitingHistory ? <div className="chat-empty" role="status">
                {historyReadState.id === sessionId && historyReadState.status === "error" ? <>
                  <p>对话暂未读取成功。</p>
                  <Button variant="secondary" size="sm" onClick={() => { historyHydrationAttempts.current.delete(sessionId); setHistoryHydrationEpoch(epoch => epoch + 1); }}>重新加载</Button>
                </> : <><Spinner size={18} /><p>正在加载对话…</p></>}
              </div> : !messages.length ? (
                <div className="chat-empty">
                  <div className="empty-mark"><Bot size={27} /></div>
                  <h1>新的对话</h1>
                  <p>{skillHint || "描述问题，或输入 / 调用 Skill 与工具。"}</p>
                </div>
              ) : (
                <div className="message-list is-virtual" style={{ height: `${messageVirtualizer.getTotalSize()}px` }}>
                  {virtualMessages.map((virtualRow) => {
                    const index = virtualRow.index;
                    const item = messages[index];
                    if (!item) return null;
                    const showPersistentMessageActions = index < messages.length - 1;
                    const turnActivity = turnActivityByMessage.get(item.id);
                    const ownsTurnActivity = item.role === "assistant" && turnActivity?.ownerId === item.id;
                    const interruptedRun = ownsTurnActivity && item.turnId ? interruptedRunsByTurnId.get(item.turnId) : undefined;
                    return <div
                      key={virtualRow.key}
                      ref={messageVirtualizer.measureElement}
                      data-index={index}
                      className="message-virtual-row"
                      style={{ transform: `translateY(${virtualRow.start}px)` }}
                    >
                      <ChatMessageRow
                        thinkingCopy={thinkingCopy}
                        sessionId={sessionId}
                        item={item}
                        index={index}
                        showPersistentMessageActions={showPersistentMessageActions}
                        interruptedRun={interruptedRun}
                        renderedActivity={turnActivity?.pending ? renderedActivity : ""}
                        timelineExpanded={timelineExpanded[timelineStateKey(item)] ?? timelineExpanded[item.id]}
                        fileChanges={turnFileChangesByMessage.get(item.id)}
                        workspacePath={workspacePath}
                        copied={copiedId === item.id}
                        ownsTurnActivity={ownsTurnActivity}
                        turnPending={Boolean(turnActivity?.pending)}
                        turnStartedAt={turnActivity?.startedAt}
                        turnFinishedAt={turnActivity?.finishedAt}
                        turnEvents={turnActivity?.events}
                        onTimelineExpandedChange={handleTimelineExpandedChange}
                        onLayoutChange={preserveTimelinePosition}
                        onPreviewAttachment={handlePreviewAttachment}
                        onPathMenu={handlePathMenu}
                        onOpenAttachment={openAttachment}
                        onCopy={handleCopyMessage}
                        onEdit={handleEditMessage}
                        onFork={handleForkMessage}
                      />
                    </div>;
                  })}
                </div>
              )}
              {debugTargets.chat && componentsDebug.replyFiles && <ChatDebugFileChanges workspacePath={workspacePath} />}
            </div>
            {userMessageMarkers.length > 0 && <ConversationRail key={`rail:${sessionId}`} markers={userMessageMarkers} activeId={activeUserMessageId} onNavigate={scrollToMessage} />}
            {showScrollToBottom && <div className="chat-floating-controls">
              {showScrollToBottom && <button type="button" className="scroll-to-bottom" title="回到底部" aria-label="回到底部" onClick={() => scrollToLatest(true)}><ArrowDown size={16} /></button>}
            </div>}
            </div>

            <div className="chat-footer">
              <ComposerNoticeDock key={`notice:${sessionId}`}>
                <ChatDebugNotices on={debugTargets.chat ? componentsDebug : {}} onEditQueuedMessage={(content) => restoreQueuedMessageToComposer({ content })} />
              {activePermission && (
                <ChatPermissionBar requestKey={snapshotPermissionKey || ""} toolLabel={toolStepTitle(activePermission.toolName)} input={activePermission.input} environmentLabel={toolEnvironmentLabel(activePermission.toolName)} question={activePermission.question} busy={resolvingPermissionKey === snapshotPermissionKey} instruction={permissionInstruction} expiresAtMs={activePermission.expiresAtMs} onInstructionChange={setPermissionInstruction} onResolve={(decision) => { void resolvePermission(decision); }} />
              )}
              <ChatQueuePanel items={renderedQueuedMessages} busy={queueMutationState} onPreview={handlePreviewAttachment} onGuide={(queueId) => { void guideQueuedMessage(queueId); }} onEdit={(item) => { void editQueuedMessage(item); }} onRemove={removeQueuedMessage} onReorder={reorderQueuedMessages} />
              <GoalBadge goal={activeGoalBadge} />
              {composerFileChange && <ComposerChangedFiles changes={composerFileChange} workspacePath={workspacePath} onOpen={(path) => { void openAttachment(path, { sessionId: composerFileChange.sessionId || sessionId || "", turnId: composerFileChange.turnId || "" }); }} />}
              <BackgroundTasks sessionId={sessionId} turn={backgroundTurn} />
              {showTaskPlan && <ComposerTaskList steps={taskSteps} goal={taskGoal} startedAt={taskStartedAt} finishedAt={taskPlan?.finishedAt} active={taskActive} onOpenPlanner={setTaskPlanOpen} onCollapse={() => setTaskPlanOpen(false)} onDelete={dismissTaskPlan}>
              </ComposerTaskList>}
              </ComposerNoticeDock>
              <div className="composer-stack">
              <div className="composer-row">
                <BorderGlow
                  className="composer-shell composer-card-surface"
                  animated={sending && active}
                  followPointer
                  borderRadius={24}
                  borderWidth={2}
                  backgroundColor="var(--bg-canvas)"
                  colors={["#c084fc", "#f472b6", "#38bdf8"]}
                  >
                  <div className="composer-input-surface">
                    {editingMessage && <div className="composer-editing-banner" role="status">
                      <Pencil size={13} />
                      <span>正在编辑此消息，发送后将在当前对话中重新回复</span>
                      <button type="button" title="取消编辑" aria-label="取消编辑" onClick={() => {
                        setEditingMessageId("");
                        setDraft("");
                        if (editingSourcesBeforeRef.current) setSources(editingSourcesBeforeRef.current);
                        editingSourcesBeforeRef.current = null;
                      }}><X size={13} /></button>
                    </div>}
                    {sources.length > 0 && (
                      <div className="composer-attachment-strip">
                        {sources.map((item) => <ComposerAttachmentCard key={item.path} source={item}
                          onRemove={() => setSources((prev) => prev.filter((row) => row.path !== item.path))}
                          onRetry={() => setSources((prev) => prev.map((source) => source.path === item.path
                            ? { ...source, preparationError: undefined, preparing: true } : source))}
                          onPreview={setAttachmentPreview} />)}
                      </div>
                    )}
                    {dropNotice && <div className="drop-notice">{dropNotice}</div>}
                    {logPanelOpen && <section className="chat-log-panel">
                      <div className="chat-log-toolbar"><b>日志证据</b><span>{logStatus}</span><button type="button" title="关闭日志面板" onClick={() => setLogPanelOpen(false)}><X size={13} /></button></div>
                      <div className="chat-log-search"><input value={logSearch} placeholder="搜索错误码、组件或关键词" onChange={(event) => setLogSearch(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") void searchLogs(); }} /><button type="button" onClick={() => void searchLogs()} title="搜索日志"><Search size={13} /></button></div>
                      {logTimeline.length > 0 && <div className="chat-log-timeline">{logTimeline.slice(0, 6).map((event, index) => <span key={`${String(event.timestamp || event.time || "")}-${index}`}>{String(event.timestamp || event.time || "")} {String(event.message || event.summary || event.text || "")}</span>)}</div>}
                      {logHits.length > 0 && <div className="chat-log-hits">{logHits.slice(0, 6).map((hit, index) => <button key={`${String(hit.source || "")}-${index}`} type="button" onClick={() => void openLogHit(hit)}><b>{String(hit.source || hit.path || "日志")}</b><span>{String(hit.excerpt || hit.text || "")}</span></button>)}</div>}
                      {logExcerpt && <pre className="chat-log-excerpt">{logExcerpt}</pre>}
                    </section>}
                    {/* Remounted per session: a contentEditable keeps its DOM text — and
                        an in-flight IME composition — across a session switch, which would
                        let this conversation's unsent text land in the next one's draft. */}
                    <ComposerInput key={`composer:${sessionId}`} onEditorReady={captureComposerEditor} value={draft} skillIds={skills.map((item) => item.id)} toolIds={builtinTools.map((item) => item.id)} onChange={(value) => { setDraft(value); setSlashIndex(0); setSlashMenuDismissed(false); }} onPaste={(event) => { void onComposerPaste(event); }} onKeyDown={(event) => {
                      if (visibleSlashItems.length) {
                        if (event.key === "ArrowDown") { event.preventDefault(); setSlashIndex((value) => (value + 1) % visibleSlashItems.length); return; }
                        if (event.key === "ArrowUp") { event.preventDefault(); setSlashIndex((value) => (value - 1 + visibleSlashItems.length) % visibleSlashItems.length); return; }
                        if (event.key === "Escape") { event.preventDefault(); setDraft((value) => value.replace(/(?:^|\s)\/[^/\s]*$/, "")); return; }
                        if (event.key === "Enter" && !event.shiftKey) {
                          event.preventDefault();
                          const picked = visibleSlashItems[Math.min(slashIndex, visibleSlashItems.length - 1)];
                          applySlashItem(picked);
                          return;
                        }
                      }
                      if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
                        event.preventDefault();
                        void sendMessage(undefined, true);
                      } else if (event.key === "Enter" && !event.shiftKey) {
                        event.preventDefault();
                        void sendMessage();
                      } else if (event.key === "Escape"
                        && !event.nativeEvent.isComposing
                        && !slashOpen && !composerMenuOpen && !sandboxOpen
                        && stopAvailable) {
                        // N3-C: Esc stops the running agent. Composer-scoped on
                        // purpose so it cannot hijack Escape from dialogs and
                        // menus; skipped while an IME composition is cancelling
                        // itself and while a composer popover (slash/add/sandbox)
                        // still expects Escape to close it first.
                        event.preventDefault();
                        void stopAgent();
                      }
                    }} />
                    {slashOpen && <div ref={slashMenuRef} className="slash-menu" role="listbox">{slashItems.length ? slashItems.map((item, index) => <button key={`${item.kind}-${item.id}`} type="button" className={`slash-menu-row${index === slashIndex ? " selected" : ""}`} onMouseDown={(event) => event.preventDefault()} onMouseEnter={() => setSlashIndex(index)} onClick={() => applySlashItem(item)}>
                      <span className={`slash-menu-icon is-${item.kind}`} aria-label={item.kind === "app" ? "应用工具" : item.kind === "tool" ? "本地工具" : "Skill"}>{item.kind === "app" ? <AppWindow size={13} /> : item.kind === "tool" ? <Wrench size={13} /> : <Sparkles size={13} />}</span>
                      <span className="slash-menu-copy"><b>{item.kind === "app" ? item.label : `/${item.id}`}</b><small>{item.description || item.label}</small></span>
                    </button>) : <div className="slash-menu-empty">没有匹配的 Skill 或工具</div>}</div>}
                    <div className="composer-toolbar">
                      <div className="composer-left-actions">
                        <ComposerAddMenu open={composerMenuOpen} onOpenChange={setComposerMenu}
                          projectIds={automationProjectIds} onProjectIdsChange={setAutomationProjectIds}
                          onFile={() => { void openFile("选择文件").then((path) => {
                              if (!path) return;
                              const name = path.split(/[\\/]/).filter(Boolean).pop() || path;
                              const kind = classifyDropped(name, false);
                              if (!kind) setDropNotice(`不支持的文件：${name}`);
                              else addSource(path, kind, name);
                            }); }}
                          onDirectory={() => { void chooseDirectory("选择工作目录").then((path) => { if (path) addSource(path, "directory"); }); }} />
                        <div className="composer-pop">
                        <button ref={sandboxTriggerRef} type="button" className={`sandbox-badge${sandboxMode === "full" ? " full" : ""}`} title="设置 Agent 权限模式（非沙盒：full 为完全信任，无强制隔离）" aria-expanded={sandboxOpen} onClick={() => { setSandboxOpen((value) => !value); setComposerMenuOpen(false); }}>
                          <ShieldCheck size={13} />{sandboxMode === "full" ? "完全信任" : sandboxMode === "risk" ? "替我审批" : "请求批准"}
                        </button>
                        {sandboxOpen && (
                          <AccessModeMenu value={sandboxMode} triggerRef={sandboxTriggerRef} onClose={() => setSandboxOpen(false)} onChange={(mode) => {
                            setSandboxOpen(false);
                            if (mode === "full") { if (sandboxMode !== "full") setFullAccessOpen(true); }
                            else setSandboxMode(mode);
                          }} />
                        )}
                        </div>
                        {sources.length > 0 && <span className="context-chip"><Check size={12} />{sources.length} 个本地来源</span>}
                      </div>
                      <div className="composer-actions">
                        <ContextUsageIndicator tokens={usedTokens} window={contextWindow}
                          systemTokens={measuredContext?.system} toolTokens={measuredContext?.tools}
                          anomaly={contextAnomaly} measuredTrigger={measuredContext?.compactionTriggerTokens}
                          readingModelId={measuredContext?.modelId}
                          checking={contextChecking} sending={sending} sessionId={sessionId}
                          onRefresh={() => { void refreshContext(); }} />
                        <AgentModelPicker
                          providers={providers}
                          selection={{ providerId: provider?.id || providerId, modelId: model?.id || modelId }}
                          inferenceLevel={inferenceLevel}
                          onChange={({ providerId: nextProviderId, modelId: nextModelId }) => {
                            modelSelectionRef.current = { providerId: nextProviderId, modelId: nextModelId };
                            setProviderId(nextProviderId);
                            setModelId(nextModelId);
                            // Model choice is per-session: remember it on the
                            // active conversation so reopening this chat
                            // restores its own model instead of the global one.
                            const currentId = activeSessionIdRef.current;
                            if (currentId) {
                              setSessions((previous) => updateSessionModelSelection(previous, currentId, {
                                providerId: nextProviderId, modelId: nextModelId,
                              }));
                            }
                          }}
                          onInferenceLevelChange={setInferenceLevel}
                          onOpenSettings={openProviderSettings}
                        />
                        {/* N3-A 改判：停止与发送共用同一个按钮位，运行时按钮变
                            成停止形态、空闲时回到发送形态。早先拆成"发送左侧再加
                            一个停止按钮"在两个方向上都错了——多占一个按钮位，
                            且与对话页顶栏的状态语义打架。共用按钮位仍然满足
                            N3-A 的初衷：草稿非空也不会把"停止"变成"发送/排队"，
                            因为运行时这个按钮只承担停止。 */}
                        <Button
                          type="button"
                          variant="primary"
                          size="sm"
                          className={`composer-send${stopAvailable ? " is-stop" : ""}`}
                          title={stopAvailable ? "停止当前任务" : submissionLocked && !sending && !activePermission ? "正在提交…" : activePermission ? "回答当前询问" : sending ? "加入队列" : "发送消息"}
                          aria-label={stopAvailable ? "停止当前任务" : submissionLocked && !sending && !activePermission ? "正在提交…" : activePermission ? "回答当前询问" : sending ? "加入队列" : "发送消息"}
                          disabled={!stopAvailable && (!draft.trim() || Boolean(composerActionPending) || (submissionLocked && !sending && !activePermission))}
                          icon={stopAvailable ? <Square size={11} strokeWidth={2} fill="currentColor" /> : <ArrowUp size={18} strokeWidth={2} />}
                          onClick={() => { if (stopAvailable) { void stopAgent(); } else { void sendMessage(); } }}
                        />
                      </div>
                    </div>
                    <ChatUsageSummary key={`usage:${sessionId}`} usage={visibleUsage} turnId={visibleTurnId} session={sessionTokens}
                      persistedUsage={session?.usage} persistedTurnId={session?.usageTurnId}
                      quota={displayedQuota} phase={displayedQuotaPhase} quotaError={quotaError} quotaCheckedAt={quotaCheckedAt}
                      onRefresh={() => { void refreshUsage(true); }}
                      pricing={visiblePricing} quotaChange={turnQuota.change} />
                  </div>
                </BorderGlow>
              </div>
              </div>
            </div>
            <ChatTerminalPanel visible={terminalOpen} target={target} workspacePath={workspacePath} sshConnections={sshConnections} wslProfiles={wslProfiles} onHide={() => setTerminalOpen(false)} resizable resizeBudget={workspaceRef} />
          </section>
          {(filePreviewPath || collaborationOpen) && <>
          <PaneResizeHandle className="chat-side-panel-resizer" side="right" label="调整会话工作区侧栏宽度" controls="chat-workspace-side-panel" width={sidePanelWidth.width} min={sidePanelWidth.bounds.min} max={sidePanelWidth.bounds.max} initial={WORKSPACE_PANE.initial} onResize={sidePanelWidth.resize} onCommit={sidePanelWidth.commit} />
          {filePreviewPath ? <aside id="chat-workspace-side-panel" className="collaboration-side-panel chat-file-preview-pane" style={{ width: sidePanelWidth.width, flexBasis: sidePanelWidth.width }} aria-label="文件改动预览">
            <ChatFilePreviewBoundary key={`${filePreviewPath}:${filePreviewOrigin?.sessionId}:${filePreviewOrigin?.turnId}`} onClose={closeFilePreview}><Suspense fallback={<aside className="chat-file-preview-panel" aria-label="文件预览"><div className="chat-file-preview-notice" role="status"><Spinner size={16} className="is-inline" />正在加载预览</div><Button variant="subtle" size="xs" icon={<X size={16} />} aria-label="关闭文件预览" onClick={closeFilePreview} /></aside>}><ChatFilePreview path={filePreviewPath} origin={filePreviewOrigin} previewTarget={previewTarget} previewTargets={previewTargets} onSelectPreviewTarget={setPreviewEnvironmentKey} onClose={closeFilePreview} /></Suspense></ChatFilePreviewBoundary>
          </aside> : <CollaborationSidePanel
            active={active}
            width={sidePanelWidth.width}
            view={collaborationSideView}
            threadId={sessionId}
            workspacePath={workspacePath || target.root}
            workspaceEnvironment={workspaceEnvironment}
            workspaceGitChanges={workspaceGitChanges}
            workspaceLoading={workspaceEnvironmentLoading}
          />}</>}
        </div>
      </div>
    </section>
    {pathMenu && <div ref={pathMenuRef} className="local-path-context-menu" style={{ left: pathMenu.x, top: pathMenu.y }}>
      <button type="button" onClick={() => { void openAttachment(pathMenu.path); setPathMenu(null); }}>打开文件</button>
      <button type="button" onClick={() => { void copyAttachmentText(pathMenu.path, "已复制文件路径"); setPathMenu(null); }}>复制路径</button>
      <button type="button" onClick={() => { void copyAttachmentContent(pathMenu.path); setPathMenu(null); }}>复制文件内容</button>
    </div>}
    <ConfirmDialog
      open={pendingRemoval !== null}
      title={pendingRemoval?.kind === "workspace" ? "移除工作区" : "删除对话"}
      message={pendingRemoval?.kind === "workspace" ? `确定移除“${pendingRemoval.title}”吗？工程源文件不会删除；勾选下方选项将永久删除关联对话及其缓存和备份。` : `确定删除“${pendingRemoval?.title || "此对话"}”吗？删除后无法恢复；正在执行的任务会先停止。`}
      confirmLabel={pendingRemoval?.kind === "workspace" ? "确认移除" : "确认删除"}
      busy={removalBusy}
      error={removalError}
      onCancel={() => { if (!removalExecuting.current) { setPendingRemoval(null); setRemovalError(""); } }}
      onConfirm={async () => {
        const removal = pendingRemoval;
        if (!removal || removalExecuting.current) return;
        removalExecuting.current = true;
        setRemovalBusy(true);
        setRemovalError("");
        try {
          if (removal.kind === "session") await removeChatSession(removal.id);
          else {
            if (removeWorkspaceSessions) {
              const summaries = isTauri() ? await listHistorySummaries() : [];
              const pathKey = (value: unknown) => typeof value === "string" ? value.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase() : "";
              const selectedPath = pathKey(removal.path);
              const ids = new Set([
                ...sessions.filter((item) => item.scope === "workspace" && pathKey(item.workspacePath) === selectedPath).map((item) => item.id),
                ...summaries.filter((item) => item.metadata?.scope === "workspace" && pathKey(item.metadata?.workspacePath) === selectedPath).map((item) => item.id),
              ]);
              for (const id of ids) await removeChatSession(id);
            }
            setWorkspaces((previous) => previous.filter((item) => workspaceKey(item.path) !== workspaceKey(removal.path)));
            const selected = sessionsRef.current.find((item) => item.id === activeSessionIdRef.current);
            const selectedPath = selected?.workspacePath || readChatNavSnapshot().workspacePath;
            if (selectedPath && workspaceKey(selectedPath) === workspaceKey(removal.path)) {
              // A removed workspace must not leave its selected conversation mounted
              // in the main pane. Keep the conversation history available unless the
              // user explicitly chose to delete associated sessions above.
              clearActiveConversation();
            }
          }
          setPendingRemoval(null);
        } catch (error) { setRemovalError(formatInvokeError(error)); }
        finally { removalExecuting.current = false; setRemovalBusy(false); }
      }}
    >
      {pendingRemoval?.kind === "workspace" && <label className="confirm-dialog-acknowledge">
        <input type="checkbox" checked={removeWorkspaceSessions} disabled={removalBusy} onChange={(event) => setRemoveWorkspaceSessions(event.target.checked)} />
        <span>同时删除关联对话（保留工程文件）</span>
      </label>}
    </ConfirmDialog>
    {/* 归档是不可撤销的整理动作（列表里看不见了，要去「设置 → 归档会话」找回来），
        所以和删除一样先要一次明确确认。 */}
    <ConfirmDialog
      open={pendingArchive !== null}
      title="归档会话"
      message={`归档“${pendingArchive?.title || "此对话"}”后，它会从侧栏和工作区列表里隐藏；对话本身不会删除，可以随时在「设置 → 归档会话」里恢复。`}
      confirmLabel="确认归档"
      cancelLabel="取消"
      busy={archiveBusy}
      onCancel={() => { if (!archiveBusy) setPendingArchive(null); }}
      onConfirm={async () => {
        const target = pendingArchive;
        if (!target || archiveBusy) return;
        setArchiveBusy(true);
        try {
          const item = sessionsRef.current.find((candidate) => candidate.id === target.id);
          if (item) await setHistoryArchived(item, true);
          setPendingArchive(null);
        } finally { setArchiveBusy(false); }
      }}
    />
    <ConfirmDialog
      open={fullAccessOpen}
      variant="danger"
      title="启用完全访问"
      message="完全访问后，Agent 将不再逐次询问命令、写入和 shell 操作。这个权限只受超时、输出和取消控制。"
      acknowledgeLabel="我已了解风险，仍然启用完全访问"
      confirmLabel="确认启用"
      cancelLabel="取消"
      onCancel={() => setFullAccessOpen(false)}
      onConfirm={() => { setSandboxMode("full"); setFullAccessOpen(false); }}
    />
    {attachmentPreview && <ChatImagePreview key={attachmentPreview.dataUrl} {...attachmentPreview} onClose={() => setAttachmentPreview(null)} />}
    </>
  );
});

export default ChatView;
