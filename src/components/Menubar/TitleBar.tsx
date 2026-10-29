import React, { useCallback, useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Minus, Square, X, Copy } from "lucide-react";
import { isTauri } from "@/platform/runtime";
import { readAppTheme, subscribeAppTheme, type AppTheme } from "@/platform/theme";
import { AppMenubar } from "./AppMenubar";
import "./titlebar.css";

/** 菜单起点：左侧留 100px 空白（不放前进/后退箭头）。 */
const MENU_LEFT_OFFSET = 100;

/**
 * 应用标题栏。
 *
 * 布局照桌面编辑器的习惯：左边一段空白的可拖动区（不放前进/后退箭头），
 * 从 100px 处开始内嵌菜单栏，右侧是原生风格的最小化 / 最大化 / 关闭。
 * 整条栏都是窗口拖动区，只有菜单与按钮自己吃掉鼠标事件。
 */
export const TitleBar: React.FC<{ onMouseDown?: (event: React.MouseEvent) => void }> = ({ onMouseDown }) => {
  const [theme, setTheme] = useState<AppTheme>(readAppTheme);
  const [maximized, setMaximized] = useState(false);
  useEffect(() => subscribeAppTheme(setTheme), []);

  const win = React.useMemo(() => (isTauri() ? getCurrentWindow() : null), []);

  useEffect(() => {
    if (!win) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void win.isMaximized().then((value) => { if (!disposed) setMaximized(value); }).catch(() => undefined);
    void win.onResized(() => {
      void win.isMaximized().then((value) => { if (!disposed) setMaximized(value); }).catch(() => undefined);
    }).then((stop) => { if (disposed) stop(); else unlisten = stop; }).catch(() => undefined);
    return () => { disposed = true; unlisten?.(); };
  }, [win]);

  const run = useCallback(
    (command: "minimize" | "toggleMaximize" | "close") => {
      if (!win) return;
      if (command === "minimize") void win.minimize();
      else if (command === "toggleMaximize") void win.toggleMaximize();
      else void win.close();
    },
    [win],
  );

  return (
    <header className="titlebar" data-theme={theme} onMouseDown={onMouseDown}>
      {/* 左侧留白：只是可拖动区，不放任何按钮 */}
      <div className="titlebar-gutter" style={{ width: MENU_LEFT_OFFSET }} aria-hidden="true" />

      <div className="titlebar-menu" onMouseDown={(event) => event.stopPropagation()}>
        <AppMenubar />
      </div>

      <div className="titlebar-spacer" />

      <div className="titlebar-controls" onMouseDown={(event) => event.stopPropagation()}>
        <button type="button" className="titlebar-btn" title="最小化" aria-label="最小化" onClick={() => run("minimize")}>
          <Minus size={14} />
        </button>
        <button
          type="button"
          className="titlebar-btn"
          title={maximized ? "还原" : "最大化"}
          aria-label={maximized ? "还原" : "最大化"}
          onClick={() => run("toggleMaximize")}
        >
          {maximized ? <Copy size={11} /> : <Square size={11} />}
        </button>
        <button type="button" className="titlebar-btn titlebar-btn-close" title="关闭" aria-label="关闭" onClick={() => run("close")}>
          <X size={14} />
        </button>
      </div>
    </header>
  );
};

export default TitleBar;
