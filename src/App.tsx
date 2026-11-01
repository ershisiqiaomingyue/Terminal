import React, { Suspense, useCallback, useEffect, useMemo, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Info, AlertCircle, CheckCircle2 } from "lucide-react";
import { AppErrorBoundary } from "@/components/AppRecovery/AppRecovery";
import { SettingsModal, type SettingsSection } from "@/components/SettingsModal/SettingsModal";
import { TitleBar } from "@/components/Menubar/TitleBar";
import { setDiagnosticContext } from "@/platform/frontendHealth";
import { AI_TASK_ACTIVITY_EVENT, GLOBAL_NOTIFICATION_EVENT, type GlobalNotification } from "@/platform/notifications";
import { invoke, isTauri, listen } from "@/platform/runtime";
import { readAppTheme, subscribeAppTheme, type AppTheme } from "@/platform/theme";
import { useWindowDrag } from "@/platform/windowDrag";

const TerminalView = React.lazy(() =>
  import("@/views/Terminal").then((module) => ({ default: module.TerminalView })),
);

/** 应用里的一屏：终端，或设置页的某个分区。 */
type AppRoute = { view: "terminal" } | { view: "settings"; section: SettingsSection };

/**
 * 应用的唯一界面：终端。
 *
 * 左栏是已打开的终端与已保存服务器，中栏是本地 / WSL / SSH 的多面板终端，
 * 右栏是主机状态 / Git / 文件，底部是命令输入与 AI 命令栏。设置是独立整页：
 * 打开时整个视口切成设置页（终端状态在卸载时快照、返回时恢复）。标题栏的
 * 后退/前进沿视图历史移动，与 ChatGPT 桌面端一致。
 */
export const App: React.FC = () => {
  const [theme, setTheme] = useState<AppTheme>(readAppTheme);
  const [notifications, setNotifications] = useState<GlobalNotification[]>([]);

  // 视图历史（顶栏 ← → 用）：终端与设置（含分区）都算一条记录，前进/后退沿栈移动。
  const [nav, setNav] = useState<{ stack: AppRoute[]; index: number }>({ stack: [{ view: "terminal" }], index: 0 });
  const route = nav.stack[nav.index];
  const settingsOpen = route.view === "settings";

  // 无边框窗口：整窗可按在空白处拖动（终端内容区自己的拖拽会先截获）。
  const dragHandle = useMemo(() => (isTauri() ? getCurrentWindow() : null), []);
  const windowDrag = useWindowDrag(() => dragHandle);

  useEffect(() => subscribeAppTheme(setTheme), []);

  useEffect(() => {
    setDiagnosticContext({ view: "terminal" });
  }, []);

  // 把新路由压入历史栈（截掉当前位置之前的“前进”分支，同浏览器行为）。
  const pushRoute = useCallback((next: AppRoute) => {
    setNav(({ stack, index }) => {
      const current = stack[index];
      const same =
        current.view === "terminal" && next.view === "terminal"
          ? true
          : current.view === "settings" && next.view === "settings" && current.section === next.section;
      if (same) return { stack, index };
      const trimmed = [...stack.slice(0, index + 1), next];
      const capped = trimmed.length > 40 ? trimmed.slice(trimmed.length - 40) : trimmed;
      return { stack: capped, index: capped.length - 1 };
    });
  }, []);

  const navBack = useCallback(
    () => setNav(({ stack, index }) => ({ stack, index: Math.max(0, index - 1) })),
    [],
  );
  const navForward = useCallback(
    () => setNav(({ stack, index }) => ({ stack, index: Math.min(stack.length - 1, index + 1) })),
    [],
  );

  // 设置弹层入口：侧栏按钮、独立窗口回跳都发这个事件。
  useEffect(() => {
    const openSettings = (event: Event) => {
      const section = (event as CustomEvent<{ section?: SettingsSection }>).detail?.section ?? "terminal";
      pushRoute({ view: "settings", section });
    };
    window.addEventListener("android-tool-open-settings", openSettings);
    let disposed = false;
    let remove: (() => void) | undefined;
    if (isTauri()) {
      void listen<{ section: string }>("tool-window:open-settings", (detail) => {
        openSettings(new CustomEvent("android-tool-open-settings", { detail }));
      }).then((stop) => { if (disposed) stop(); else remove = stop; });
    }
    return () => {
      disposed = true;
      remove?.();
      window.removeEventListener("android-tool-open-settings", openSettings);
    };
  }, [pushRoute]);

  // 全局通知：终端里的 toast（连接失败、文件操作结果等）都走这个通道。
  useEffect(() => {
    const onNotification = (event: Event) => {
      const notification = (event as CustomEvent<GlobalNotification>).detail;
      if (!notification) return;
      setNotifications((current) => [...current, notification].slice(-4));
      window.setTimeout(
        () => setNotifications((current) => current.filter((item) => item.id !== notification.id)),
        notification.durationMs || 3600,
      );
    };
    window.addEventListener(AI_TASK_ACTIVITY_EVENT, () => {});
    window.addEventListener(GLOBAL_NOTIFICATION_EVENT, onNotification);
    return () => {
      window.removeEventListener(AI_TASK_ACTIVITY_EVENT, () => {});
      window.removeEventListener(GLOBAL_NOTIFICATION_EVENT, onNotification);
    };
  }, []);

  // 启动完成后统一关掉启动小窗（与上游一致：等首屏渲染过再关）。
  useEffect(() => {
    if (!isTauri()) return;
    const timer = window.setTimeout(() => {
      void invoke("finish_boot", { startupReady: true }).catch(() => undefined);
    }, 900);
    return () => window.clearTimeout(timer);
  }, []);

  const closeSettings = useCallback(() => pushRoute({ view: "terminal" }), [pushRoute]);

  return (
    <div className="terminal-app" data-theme={theme}>
      <div className="ambient-wallpaper" aria-hidden="true" />
      <TitleBar
        onMouseDown={windowDrag.onMouseDown}
        canBack={nav.index > 0}
        canForward={nav.index < nav.stack.length - 1}
        onBack={navBack}
        onForward={navForward}
      />
      <AppErrorBoundary>
        {route.view === "settings" ? (
          <Suspense fallback={<div className="app-view-loading">正在载入设置...</div>}>
            <SettingsModal isOpen section={route.section} onClose={closeSettings} />
          </Suspense>
        ) : (
          <Suspense fallback={<div className="app-view-loading">正在载入终端...</div>}>
            <TerminalView />
          </Suspense>
        )}
      </AppErrorBoundary>

      {notifications.length > 0 && (
        <div className="global-notification-stack" aria-live="polite">
          {notifications.map((notification) => (
            <div className={`global-notification ${notification.kind}`} key={notification.id}>
              {notification.kind === "success" ? <CheckCircle2 size={15} />
                : notification.kind === "error" || notification.kind === "warning" ? <AlertCircle size={15} />
                  : <Info size={15} />}
              <span>{notification.message}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
};

export default App;


