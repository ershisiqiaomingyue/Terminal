import React from "react";
import { Button } from "rain-ui";
import { ArrowLeft, ArrowRight } from "lucide-react";
import { AppMenubar } from "./AppMenubar";
import "./titlebar.css";

/**
 * 应用工具栏：后退/前进、终端侧栏折叠与应用菜单。
 * 最小化、最大化、关闭由 Rust/Tauri 原生窗口标题栏提供。
 */
export interface TitleBarProps {
  onMouseDown?: (event: React.MouseEvent) => void;
  canBack?: boolean;
  canForward?: boolean;
  onBack?: () => void;
  onForward?: () => void;
}

export const TitleBar: React.FC<TitleBarProps> = ({ onMouseDown, canBack = false, canForward = false, onBack, onForward }) => {
  return (
    <header className="titlebar" onMouseDown={onMouseDown}>
      <div className="titlebar-nav" onMouseDown={(event) => event.stopPropagation()}>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          icon={<ArrowLeft size={15} />}
          disabled={!canBack}
          title="后退"
          aria-label="后退"
          onClick={onBack}
        />
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          icon={<ArrowRight size={15} />}
          disabled={!canForward}
          title="前进"
          aria-label="前进"
          onClick={onForward}
        />
      </div>

      <div className="titlebar-menu" onMouseDown={(event) => event.stopPropagation()}>
        <AppMenubar />
      </div>

      <div className="titlebar-spacer" />
    </header>
  );
};

export default TitleBar;
