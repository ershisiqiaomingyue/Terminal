import React, { createContext, useCallback, useContext, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { AlertCircle, CheckCircle2, Info } from "lucide-react";
import { formatInvokeError } from "@/platform/runtime";

type SettingsNotificationKind = "success" | "error" | "info";
type SettingsNotification = { id: string; message: string; kind: SettingsNotificationKind };
// 第一个参数放宽到 unknown：有的面板直接把后端返回的结构化错误对象塞进来。
type Notify = (message: unknown, kind?: SettingsNotificationKind) => void;

const SettingsNotificationContext = createContext<Notify>(() => undefined);

export function SettingsNotificationProvider({ children }: { children: React.ReactNode }) {
  const [notifications, setNotifications] = useState<SettingsNotification[]>([]);
  const timersRef = useRef<Record<string, number>>({});

  const notify = useCallback<Notify>((input, kind = "info") => {
    // 后端有的命令返回结构化错误（如 AgentCommandError { code, message }），
    // 调用方若直接把对象塞进来就会显示成「[object Object]」。这里统一归一化。
    const message = typeof input === "string" ? input : formatInvokeError(input);
    const id = `settings-notice-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
    setNotifications((previous) => [...previous.slice(-2), { id, message, kind }]);
    timersRef.current[id] = window.setTimeout(() => {
      delete timersRef.current[id];
      setNotifications((previous) => previous.filter((notice) => notice.id !== id));
    }, kind === "error" ? 4800 : 3000);
  }, []);

  useEffect(() => () => {
    Object.values(timersRef.current).forEach((timer) => window.clearTimeout(timer));
  }, []);

  return (
    <SettingsNotificationContext.Provider value={notify}>
      {children}
      {createPortal(<div className="settings-notification-stack" aria-live="polite">
        {notifications.map((notification) => (
          <div key={notification.id} className={`settings-notification is-${notification.kind}`} role="status">
            {notification.kind === "success" ? <CheckCircle2 size={15} /> : notification.kind === "error" ? <AlertCircle size={15} /> : <Info size={15} />}
            <span>{notification.message}</span>
          </div>
        ))}
      </div>, document.body)}
    </SettingsNotificationContext.Provider>
  );
}

export function useSettingsNotification() {
  return useContext(SettingsNotificationContext);
}
