import React from "react";
import { Plus, Terminal as TerminalIcon } from "lucide-react";
import type { TerminalTab } from "./types";

/**
 * 空闲面板上的「新建终端」悬浮选择器。
 *
 * 为什么放在面板里而不是顶栏下拉：用户要求「进入界面默认是空闲面板」，那么
 * 打开第一个终端的动作就必须发生在空闲面板上——鼠标已经在面板中央，再让用户
 * 跑到左上角点 + 是反直觉的。
 *
 * 性能约定：这个组件只在 `data-empty="true"` 的面板上渲染，且 `onCreate` 只在
 * 点击时触发一次真实的开会话；不注册任何全局监听、不做轮询。
 */
export interface EmptyPanePickerProps {
  /** 用户选了一种终端类型。 */
  onCreate: (type: TerminalTab["type"]) => void;
}

interface PickerOption {
  type: TerminalTab["type"];
  label: string;
  hint: string;
}

const OPTIONS: PickerOption[] = [
  { type: "local", label: "本地终端", hint: "PowerShell" },
  { type: "wsl", label: "WSL", hint: "默认发行版" },
  { type: "ssh", label: "SSH 远程", hint: "选择已保存的服务器" },
];

export const EmptyPanePicker: React.FC<EmptyPanePickerProps> = ({ onCreate }) => {
  const [open, setOpen] = React.useState(false);

  return (
    <div className="terminal-pane-empty" data-picker-open={open ? "true" : "false"}>
      <TerminalIcon size={20} />
      <span>空闲面板</span>
      <small>点下面的按钮在这块面板里开一个终端</small>
      <button
        type="button"
        className="terminal-pane-empty-add"
        onClick={(event) => {
          event.stopPropagation();
          setOpen((value) => !value);
        }}
        aria-haspopup="menu"
        aria-expanded={open}
      >
        <Plus size={13} />
        新建终端
      </button>
      {open && (
        <div className="terminal-pane-picker" role="menu" onMouseDown={(event) => event.stopPropagation()}>
          {OPTIONS.map((option) => (
            <button
              key={option.type}
              type="button"
              role="menuitem"
              className="terminal-pane-picker-item"
              onClick={(event) => {
                event.stopPropagation();
                setOpen(false);
                onCreate(option.type);
              }}
            >
              <span className="terminal-pane-picker-label">{option.label}</span>
              <span className="terminal-pane-picker-hint">{option.hint}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
};
