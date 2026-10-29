import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { Activity, Check, Monitor, RefreshCw, Server, Terminal, Unplug } from "lucide-react";
import { Button } from "@/components/ui";
import type { SshConnection, WslProfile } from "@/platform/api";
import { getSshStatus } from "@/platform/chatApi";
import { formatInvokeError, isTauri } from "@/platform/runtime";
import { disconnectGlobalSsh, getGlobalSshSnapshot } from "@/platform/globalSsh";
import { environmentTargetForSsh, environmentTargetForWsl } from "@/platform/executionEnvironment";
import { OPEN_GLOBAL_ENVIRONMENT_EVENT, refreshGlobalEnvironment, refreshGlobalWslStatus, selectGlobalLocal, selectGlobalSsh, selectGlobalWsl, useGlobalEnvironment } from "@/platform/globalEnvironment";
import { CHAT_NAV_EVENT, readChatNavSnapshot, readChatSessions } from "@/views/Chat/chatNavStore";
import { dispatchBoundEnvironmentChange, isSessionEnvironmentBound } from "@/views/Chat/conversationEnvironments";
import type { ChatSession, ExecutionTarget } from "@/views/Chat/types";
import { SystemStatusTarget } from "./SystemStatusTarget";
import { SystemStatusResources } from "./SystemStatusResources";
import { systemStatusMetrics } from "./systemStatusMetrics";
import "./system-status.css";

type RemoteStatus = Record<string, unknown>;

export function SystemStatusIndicator({ placement = "sidebar" }: { placement?: "sidebar" | "topbar" }) {
  const environment = useGlobalEnvironment();
  const globalSsh = environment.ssh;
  const { connections, progress, connection: activeConnection } = globalSsh;
  // 绑定针对「当前对话」，因此跟随对话导航事件更新。
  const [activeConversation, setActiveConversation] = useState<ChatSession | null>(null);
  useEffect(() => {
    const sync = () => {
      const snapshot = readChatNavSnapshot();
      setActiveConversation(readChatSessions().find((session) => session.id === snapshot.sessionId) || null);
    };
    sync();
    window.addEventListener(CHAT_NAV_EVENT, sync);
    return () => window.removeEventListener(CHAT_NAV_EVENT, sync);
  }, []);
  const activeSessionId = activeConversation?.id || "";
  const bindToggle = useCallback((target: ExecutionTarget) => {
    if (!activeSessionId) return undefined;
    return () => dispatchBoundEnvironmentChange({
      sessionId: activeSessionId,
      target,
      bound: !isSessionEnvironmentBound(activeConversation, target),
    });
  }, [activeConversation, activeSessionId]);
  const [remoteStatus, setRemoteStatus] = useState<RemoteStatus | null>(null);
  const [error, setError] = useState("");
  const [open, setOpen] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [acting, setActing] = useState(false);
  const actionPending = useRef(false);
  const [tab, setTab] = useState<"ssh" | "wsl">(environment.selection === "wsl" ? "wsl" : "ssh");
  const tabRef = useRef(tab);
  tabRef.current = tab;
  const pendingRefresh = useRef<Promise<void> | null>(null);
  const refreshRef = useRef<() => Promise<void>>(async () => undefined);
  const closeTimerRef = useRef<number | null>(null);
  const popoverRef = useRef<HTMLElement>(null);

  const selectedWsl = environment.selection === "wsl";
  const selectedSsh = environment.selection === "ssh" || (environment.selection === "auto" && progress?.status === "connected");
  const currentStatus = selectedWsl ? environment.status : selectedSsh ? progress?.status : "idle";
  const tone = currentStatus === "connected" ? "connected" : currentStatus === "connecting" ? "checking" : currentStatus === "failed" ? "error" : "idle";
  const label = selectedWsl ? `WSL · ${environment.profile?.name || "未选择"}` : selectedSsh ? `SSH · ${activeConnection?.name || "未选择"}` : "本机工作区";
  const detail = systemStatusMetrics(tab === "wsl" ? environment.remoteStatus : remoteStatus || progress?.remoteStatus || null);
  const viewingWsl = tab === "wsl";
  const hasTarget = viewingWsl ? Boolean(environment.profile) : Boolean(activeConnection);
  const displayedStatus = viewingWsl ? environment.status : progress?.status;
  const displayError = viewingWsl ? environment.profilesError || (selectedWsl ? environment.error : "") : error || globalSsh.error || progress?.error || "";

  const refresh = useCallback(async () => {
    if (!isTauri() || actionPending.current) return;
    if (pendingRefresh.current) return pendingRefresh.current;
    const request = (async () => {
      setRefreshing(true);
      try {
        await refreshGlobalEnvironment();
        if (tabRef.current === "wsl") {
          await refreshGlobalWslStatus();
          return;
        }
        const selected = getGlobalSshSnapshot();
        setError(selected.error || selected.progress?.error || "");
        if (!selected.connection || selected.progress?.status !== "connected") {
          setRemoteStatus(null);
          return;
        }
        const latest = await getSshStatus(selected.connection.id, selected.connection.defaultRoot || undefined);
        const current = getGlobalSshSnapshot();
        if (current.connection?.id === selected.connection.id && current.progress?.status === "connected") setRemoteStatus(latest);
      } catch (nextError) {
        setError(formatInvokeError(nextError));
      } finally {
        setRefreshing(false);
      }
    })();
    pendingRefresh.current = request;
    try { await request; } finally { pendingRefresh.current = null; }
  }, []);
  refreshRef.current = refresh;

  useEffect(() => {
    setRemoteStatus(null);
    setError(globalSsh.error || progress?.error || "");
  }, [activeConnection?.id, progress?.status, progress?.error, globalSsh.error]);

  useEffect(() => {
    void refreshRef.current();
    const resume = () => { if (document.visibilityState === "visible") void refreshRef.current(); };
    const timer = window.setInterval(resume, open ? 5_000 : 15_000);
    document.addEventListener("visibilitychange", resume);
    return () => { window.clearInterval(timer); document.removeEventListener("visibilitychange", resume); };
  }, [open, tab, activeConnection?.id, progress?.status, environment.profileId]);

  useEffect(() => () => {
    if (closeTimerRef.current !== null) window.clearTimeout(closeTimerRef.current);
  }, []);

  useLayoutEffect(() => {
    if (!open) return;
    const reposition = () => {
      const popover = popoverRef.current;
      if (!popover) return;
      popover.style.setProperty("--environment-offset", "0px");
      const bounds = popover.getBoundingClientRect();
      const left = Math.max(12, Math.min(bounds.left, window.innerWidth - bounds.width - 12));
      popover.style.setProperty("--environment-offset", `${left - bounds.left}px`);
    };
    reposition();
    window.addEventListener("resize", reposition);
    return () => window.removeEventListener("resize", reposition);
  }, [open]);

  useEffect(() => {
    if (placement !== "topbar") return;
    const showMenu = () => {
      if (closeTimerRef.current !== null) window.clearTimeout(closeTimerRef.current);
      closeTimerRef.current = null;
      setTab("ssh");
      setOpen(true);
    };
    window.addEventListener(OPEN_GLOBAL_ENVIRONMENT_EVENT, showMenu);
    return () => window.removeEventListener(OPEN_GLOBAL_ENVIRONMENT_EVENT, showMenu);
  }, [placement]);

  const keepOpen = () => {
    if (closeTimerRef.current !== null) {
      window.clearTimeout(closeTimerRef.current);
      closeTimerRef.current = null;
    }
    setOpen(true);
  };

  const scheduleClose = () => {
    if (closeTimerRef.current !== null) window.clearTimeout(closeTimerRef.current);
    closeTimerRef.current = window.setTimeout(() => {
      closeTimerRef.current = null;
      setOpen(false);
    }, 180);
  };

  const disconnect = async () => {
    if (actionPending.current) return;
    if (viewingWsl) { selectGlobalLocal(); return; }
    if (!activeConnection) return;
    actionPending.current = true;
    setActing(true);
    try {
      await pendingRefresh.current;
      await disconnectGlobalSsh(activeConnection);
      setRemoteStatus(null);
      setError("");
    } catch (nextError) {
      setError(formatInvokeError(nextError));
    } finally {
      actionPending.current = false;
      setActing(false);
    }
  };

  const selectConnection = async (connection: SshConnection) => {
    if (actionPending.current) return;
    const current = getGlobalSshSnapshot();
    if (selectedSsh && current.connection?.id === connection.id
      && (current.progress?.status === "connected" || current.progress?.status === "connecting")) return;
    actionPending.current = true;
    setActing(true);
    try {
      await pendingRefresh.current;
      const next = await selectGlobalSsh(connection);
      setRemoteStatus(next.remoteStatus || null);
      setError("");
    } catch (nextError) {
      setRemoteStatus(null);
      setError(formatInvokeError(nextError));
    } finally {
      actionPending.current = false;
      setActing(false);
    }
  };

  const selectWsl = async (profile: WslProfile) => {
    if (actionPending.current || (selectedWsl && environment.profileId === profile.id
      && (environment.status === "connected" || environment.status === "connecting"))) return;
    actionPending.current = true;
    setActing(true);
    try { await pendingRefresh.current; await selectGlobalWsl(profile); }
    finally { actionPending.current = false; setActing(false); }
  };

  const targetDetails = hasTarget && <>
    {displayedStatus === "connecting" && <div className="sidebar-system-status-progress" role="status">
      <span>{viewingWsl ? "正在连接 WSL" : progress?.message || "正在连接 SSH"}</span>
      <progress aria-label="连接进度" max="100" value={viewingWsl ? undefined : progress?.progress} />
    </div>}
    {displayedStatus === "connected" && <>
      <SystemStatusResources metrics={detail} />
    </>}
  </>;
  const canDisconnect = hasTarget && displayedStatus === "connected" && (!viewingWsl || selectedWsl);

  return <div className={`sidebar-system-status ${placement === "topbar" ? "is-topbar" : ""}`} onMouseEnter={keepOpen} onMouseLeave={scheduleClose}>
    <button type="button" className={`sidebar-system-status-trigger ${tone}`} aria-label={`全局执行环境：${label}`} aria-expanded={open} title={`全局执行环境：${label}`} onClick={keepOpen} onFocus={keepOpen} onKeyDown={(event) => { if (event.key === "Escape") setOpen(false); }}>
      <span className="sidebar-system-status-dot" aria-hidden="true" />
    </button>
    {open && <aside ref={popoverRef} className="sidebar-system-status-popover" aria-label="全局执行环境" onKeyDown={(event) => { if (event.key === "Escape") setOpen(false); }}>
      <div className="sidebar-system-status-heading">
        <span><Activity size={14} />全局执行环境</span>
        <Button type="button" size="xs" variant="subtle" className="sidebar-system-status-refresh" title="刷新状态" aria-label="刷新状态" icon={<RefreshCw size={14} className={refreshing ? "is-spinning" : ""} />} onClick={() => void refreshRef.current()} disabled={refreshing || acting} />
      </div>
      <Button size="xs" variant="surface" icon={<Monitor size={14} />} onClick={selectGlobalLocal} disabled={acting} aria-pressed={!selectedSsh && !selectedWsl} className="sidebar-system-status-local">本机工作区{!selectedSsh && !selectedWsl && <Check size={13} />}</Button>
      <div className="segmented-glass sidebar-system-status-tabs" role="tablist" aria-label="远程环境类型" onKeyDown={(event) => {
        if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
        event.preventDefault();
        const next = event.key === "Home" ? "ssh" : event.key === "End" ? "wsl" : tab === "ssh" ? "wsl" : "ssh";
        setTab(next);
        event.currentTarget.querySelector<HTMLButtonElement>(`[data-environment="${next}"]`)?.focus();
      }}>
        <Button size="xs" variant="subtle" icon={<Server size={13} />} className={`seg-btn${tab === "ssh" ? " active" : ""}`} role="tab" data-environment="ssh" aria-selected={tab === "ssh"} tabIndex={tab === "ssh" ? 0 : -1} onClick={() => setTab("ssh")}>SSH</Button>
        <Button size="xs" variant="subtle" icon={<Terminal size={13} />} className={`seg-btn${tab === "wsl" ? " active" : ""}`} role="tab" data-environment="wsl" aria-selected={tab === "wsl"} tabIndex={tab === "wsl" ? 0 : -1} onClick={() => setTab("wsl")}>WSL</Button>
      </div>
      <div className="sidebar-system-status-options">
        {viewingWsl ? environment.profiles.map((profile) => <SystemStatusTarget key={profile.id} title={profile.name}
          subtitle={`${profile.distribution} · ${profile.defaultRoot || "/"}`} icon={<Terminal size={15} />}
          selected={selectedWsl && environment.profileId === profile.id} status={environment.profileId === profile.id ? environment.status : undefined}
          latency={environment.profileId === profile.id ? detail.latency : null}
          bound={isSessionEnvironmentBound(activeConversation, environmentTargetForWsl(profile))}
          onToggleBind={bindToggle(environmentTargetForWsl(profile))}
          onSelect={() => void selectWsl(profile)} disabled={acting}>
          {environment.profileId === profile.id && (displayedStatus === "connecting" || displayedStatus === "connected") && targetDetails}
        </SystemStatusTarget>) : connections.map((connection) => <SystemStatusTarget key={connection.id} title={connection.name}
          subtitle={connection.name.trim() === `${connection.username}@${connection.host}` ? undefined
            : `${connection.username}@${connection.host}${connection.port !== 22 ? `:${connection.port}` : ""}`}
          description={`${connection.name}\n${connection.username}@${connection.host}:${connection.port}`} icon={<Server size={15} />}
          selected={selectedSsh && activeConnection?.id === connection.id} status={activeConnection?.id === connection.id ? progress?.status : undefined}
          latency={activeConnection?.id === connection.id ? detail.latency : null}
          bound={isSessionEnvironmentBound(activeConversation, environmentTargetForSsh(connection))}
          onToggleBind={bindToggle(environmentTargetForSsh(connection))}
          onSelect={() => void selectConnection(connection)} disabled={acting}>
          {activeConnection?.id === connection.id && (displayedStatus === "connecting" || displayedStatus === "connected") && targetDetails}
        </SystemStatusTarget>)}
        {!(viewingWsl ? environment.profiles.length : connections.length) && <p className="sidebar-system-status-empty">{viewingWsl ? "暂无 WSL 配置" : "暂无 SSH 连接"}</p>}
        <p className="sidebar-system-status-bind-hint">{activeSessionId
          ? "勾选 = 绑定到当前对话；首个绑定即该对话的默认执行环境，其余主机由带 target 参数的工具按需调用。"
          : "打开一个对话后，即可把环境绑定给它。"}</p>
        {displayError && <p className="sidebar-system-status-message error" role="status">{displayError}</p>}
      </div>
      <div className="sidebar-system-status-actions">
        {canDisconnect && <Button size="xs" variant="surface" icon={<Unplug size={13} />} onClick={() => void disconnect()} disabled={acting}>{viewingWsl ? "取消选择" : "断开连接"}</Button>}
        <Button size="xs" variant="surface" icon={viewingWsl ? <Terminal size={13} /> : <Server size={13} />} onClick={() => window.dispatchEvent(new CustomEvent("android-tool-open-settings", { detail: { section: tab } }))}>{viewingWsl ? "配置 WSL" : "配置 SSH"}</Button>
      </div>
    </aside>}
  </div>;
}
