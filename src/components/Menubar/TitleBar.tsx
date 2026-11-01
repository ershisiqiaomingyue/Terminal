import React, { useCallback, useEffect, useState } from "react";
import { Button } from "rain-ui";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ArrowLeft, ArrowRight, Minus, Square, X, Copy } from "lucide-react";
import { isTauri } from "@/platform/runtime";
import { readAppTheme, subscribeAppTheme, type AppTheme } from "@/platform/theme";
import { AppMenubar } from "./AppMenubar";
import "./titlebar.css";

/**
 * 应用标题栏（对齐 ChatGPT 桌面端）：
 * 最左是后退/前进箭头，随后是内嵌菜单栏（终端侧栏折叠 + 「设置 / 帮助」两个菜单），
 * 最右是 Windows 原生位置的最小化 / 最大化 / 关闭（贴住窗口右缘）。
 * 整条栏都是窗口拖动区，只有菜单与按钮自己吃掉鼠标事件。
 */
export interface TitleBarProps {
  onMouseDown?: (event: React.MouseEvent) => void;
  canBack?: boolean;
  canForward?: boolean;
  onBack?: () => void;
  onForward?: () => void;
}

export const TitleBar: React.FC<TitleBarProps> = ({ onMouseDown, canBack = false, canForward = false, onBack, onForward }) => {
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
      {/* 后退 / 前进：跟随应用视图历史（终端 ↔ 设置），到头即禁用 */}
      <div className="titlebar-nav" onMouseDown={(event) => event.stopPropagation()}>
        <Button
          type="button" variant="bare"
          className="titlebar-btn"
          disabled={!canBack}
          title="后退"
          aria-label="后退"
          onClick={onBack}
        >
          <ArrowLeft size={15} />
        </Button>
        <Button
          type="button" variant="bare"
          className="titlebar-btn"
          disabled={!canForward}
          title="前进"
          aria-label="前进"
          onClick={onForward}
        >
          <ArrowRight size={15} />
        </Button>
      </div>

      <div className="titlebar-menu" onMouseDown={(event) => event.stopPropagation()}>
        <AppMenubar />
      </div>

      <div className="titlebar-spacer" />

      <div className="titlebar-controls" onMouseDown={(event) => event.stopPropagation()}>
        <Button type="button" variant="bare" className="titlebar-btn" title="最小化" aria-label="最小化" onClick={() => run("minimize")}>
          <Minus size={14} />
        </Button>
        <Button
          type="button" variant="bare"
          className="titlebar-btn"
          title={maximized ? "还原" : "最大化"}
          aria-label={maximized ? "还原" : "最大化"}
          onClick={() => run("toggleMaximize")}
        >
          {maximized ? <Copy size={11} /> : <Square size={11} />}
        </Button>
        <Button type="button" variant="bare" className="titlebar-btn titlebar-btn-close" title="关闭" aria-label="关闭" onClick={() => run("close")}>
          <X size={14} />
        </Button>
      </div>
    </header>
  );
};

export default TitleBar;
