import React, { useCallback, useEffect, useState } from "react";
import { AppErrorBoundary } from "@/components/AppRecovery/AppRecovery";
import { LoaderCircle, Minus, Monitor, Plus, Square, X } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { setDiagnosticContext } from "@/platform/frontendHealth";
import { ToolWindowView, type SupportedToolKey } from "@/views/ToolWindows";
import { HOME_TOOLS } from "@/views/Home";
import { SystemStatusIndicator } from "@/components/Sidebar/SystemStatusIndicator";
import { GLOBAL_NOTIFICATION_EVENT, notify, type GlobalNotification } from "@/platform/notifications";
import { formatInvokeError, invoke, isTauri } from "@/platform/runtime";
import { Button } from "rain-ui";
import { openToolWindow, type ToolWindowInfo } from "@/platform/toolWindows";
import { useWindowDrag } from "@/platform/windowDrag";
import "./tool-window.css";

const QuickToolPalette = React.lazy(async () => {
  const module = await import("@/components/QuickToolPalette/QuickToolPalette");
  return { default: module.QuickToolPalette };
});

export function ToolWindowApp({ info }: { info: ToolWindowInfo }) {
  useEffect(() => { setDiagnosticContext({ view: info.toolKey }); }, [info.toolKey]);
  const [notifications, setNotifications] = useState<GlobalNotification[]>([]);
  const [creating, setCreating] = useState(false);
  const [searchOpen, setSearchOpen] = useState(false);
  useEffect(() => {
    document.title = `${info.title} · AndroidToolkit`;
    const timer = setTimeout(() => {
      void invoke("ready_tool_window").catch((error) => notify(formatInvokeError(error), "error"));
    }, 0);
    const receive = (event: Event) => {
      const value = (event as CustomEvent<GlobalNotification>).detail;
      setNotifications((previous) => [...previous, value].slice(-3));
      setTimeout(() => setNotifications((previous) => previous.filter((item) => item.id !== value.id)), value.durationMs || 3600);
    };
    const settings = (event: Event) => {
      const section = (event as CustomEvent<{ section?: string }>).detail?.section;
      void invoke("focus_main_window", { settingsSection: section || "general" }).catch((error) => notify(formatInvokeError(error), "error"));
    };
    window.addEventListener("android-tool-open-settings", settings);
    window.addEventListener(GLOBAL_NOTIFICATION_EVENT, receive);
    return () => {
      clearTimeout(timer);
      window.removeEventListener(GLOBAL_NOTIFICATION_EVENT, receive);
      window.removeEventListener("android-tool-open-settings", settings);
    };
  }, [info.title]);

  const openTool = async (toolKey: SupportedToolKey) => {
    if (creating) return;
    setCreating(true);
    const title = HOME_TOOLS.find((tool) => tool.toolKey === toolKey)?.title || toolKey;
    try { await openToolWindow(toolKey, title); }
    catch (error) { notify(formatInvokeError(error), "error"); }
    finally { setCreating(false); }
  };
  const run = (operation: "minimize" | "toggleMaximize" | "close") => {
    void getCurrentWindow()[operation]().catch((error) => notify(formatInvokeError(error), "error"));
  };
  // 与主窗口共用同一套拖动语义：指针移动确认后才发起系统拖动，双击最大化由前端自行计时。
  const getWindowHandle = useCallback(() => (isTauri() ? getCurrentWindow() : null), []);
  const windowDragHandlers = useWindowDrag(getWindowHandle, (error) => notify(formatInvokeError(error), "error"));

  return <div className="standalone-tool-shell">
    <header className="standalone-tool-topbar" {...windowDragHandlers}>
      <span className="standalone-tool-title">{info.title}</span>
      <div className="topbar-window-controls" data-tauri-drag-region="false">
        <Button variant="bare" className="topbar-theme-toggle" title="打开工具" aria-label="打开工具" aria-haspopup="dialog" aria-expanded={searchOpen} disabled={creating} onClick={() => setSearchOpen(true)} icon={creating ? <LoaderCircle size={15} /> : <Plus size={15} />} />
        <Button variant="bare" className="topbar-theme-toggle" title="显示主窗口" aria-label="显示主窗口" onClick={() => void invoke("focus_main_window").catch((error) => notify(formatInvokeError(error), "error"))} icon={<Monitor size={15} />} />
        <SystemStatusIndicator placement="topbar" />
        <Button variant="bare" className="topbar-theme-toggle" title="最小化" aria-label="最小化" onClick={() => run("minimize")} icon={<Minus size={14} />} />
        <Button variant="bare" className="topbar-theme-toggle" title="最大化" aria-label="最大化" onClick={() => run("toggleMaximize")} icon={<Square size={13} />} />
        <Button variant="bare" className="topbar-theme-toggle" title="关闭窗口" aria-label="关闭窗口" onClick={() => run("close")} icon={<X size={15} />} />
      </div>
    </header>
    <main className="standalone-tool-content"><AppErrorBoundary><ToolWindowView type={info.toolKey} active onOpenTool={(key) => void openTool(key)} onBack={() => void invoke("focus_main_window")} /></AppErrorBoundary></main>
    {searchOpen && <React.Suspense fallback={null}><QuickToolPalette
      open tools={HOME_TOOLS} allowAi={false}
      onClose={() => setSearchOpen(false)} onMinimize={() => setSearchOpen(false)}
      onOpenTool={(key) => void openTool(key)} onExecuteAction={() => undefined}
    /></React.Suspense>}
    {!!notifications.length && <div className="standalone-tool-notifications" role="status">{notifications.map((item) => <div key={item.id}>{item.message}</div>)}</div>}
  </div>;
}
