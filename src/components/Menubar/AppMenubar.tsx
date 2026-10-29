import React, { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { notify } from "@/platform/notifications";
import { readAppTheme, setAppTheme, subscribeAppTheme, type AppTheme } from "@/platform/theme";
import { isTauri } from "@/platform/runtime";
import type { SettingsSection } from "@/components/SettingsModal/SettingsModal";
import {
  Menubar,
  MenubarCheckboxItem,
  MenubarContent,
  MenubarItem,
  MenubarLabel,
  MenubarMenu,
  MenubarRadioGroup,
  MenubarRadioItem,
  MenubarSeparator,
  MenubarShortcut,
  MenubarTrigger,
} from "./menubar";

import { sendTerminalMenu as send } from "@/platform/terminalMenu";

function openSettings(section: SettingsSection) {
  window.dispatchEvent(new CustomEvent("android-tool-open-settings", { detail: { section } }));
}

/**
 * 应用顶部菜单栏。
 *
 * 结构参考 ../rain-ui 的 Radix Menubar 组件（见 ./menubar.tsx），样式改成本项目
 * 的设计令牌。这里负责把菜单项接到真实动作上：终端会话、布局、右侧面板、主题、
 * 设置与退出。
 */
export const AppMenubar: React.FC = () => {
  const [theme, setTheme] = useState<AppTheme>(readAppTheme);
  useEffect(() => subscribeAppTheme(setTheme), []);

  const chooseTheme = (next: AppTheme) => setAppTheme(next);

  const quit = () => {
    if (!isTauri()) return;
    void getCurrentWindow().close();
  };

  return (
    <Menubar className="terminal-menubar" data-tauri-drag-region>
      <MenubarMenu>
        <MenubarTrigger>终端</MenubarTrigger>
        <MenubarContent>
          <MenubarItem onSelect={() => send({ action: "new", value: "pwsh" })}>
            PowerShell 7<MenubarShortcut>Ctrl+Shift+1</MenubarShortcut>
          </MenubarItem>
          <MenubarItem onSelect={() => send({ action: "new", value: "powershell" })}>
            Windows PowerShell
          </MenubarItem>
          <MenubarItem onSelect={() => send({ action: "new", value: "cmd" })}>
            命令提示符
          </MenubarItem>
          <MenubarSeparator />
          <MenubarItem onSelect={() => send({ action: "new-wsl" })}>新建 WSL 会话</MenubarItem>
          <MenubarItem onSelect={() => send({ action: "new-ssh" })}>新建 SSH 连接…</MenubarItem>
          <MenubarItem onSelect={() => send({ action: "new-adb" })}>新建 ADB Shell</MenubarItem>
          <MenubarSeparator />
          <MenubarItem onSelect={() => send({ action: "close-current" })}>
            关闭当前终端<MenubarShortcut>Ctrl+W</MenubarShortcut>
          </MenubarItem>
        </MenubarContent>
      </MenubarMenu>

      <MenubarMenu>
        <MenubarTrigger>布局</MenubarTrigger>
        <MenubarContent>
          <MenubarItem onSelect={() => send({ action: "layout", value: "1x1" })}>单面板</MenubarItem>
          <MenubarItem onSelect={() => send({ action: "layout", value: "1x2" })}>左右两栏</MenubarItem>
          <MenubarItem onSelect={() => send({ action: "layout", value: "2v" })}>上下两块</MenubarItem>
          <MenubarItem onSelect={() => send({ action: "layout", value: "2x2" })}>四格（田字）</MenubarItem>
          <MenubarSeparator />
          <MenubarLabel>右侧面板</MenubarLabel>
          <MenubarItem onSelect={() => send({ action: "pane", value: "host" })}>主机状态</MenubarItem>
          <MenubarItem onSelect={() => send({ action: "pane", value: "git" })}>版本控制</MenubarItem>
          <MenubarItem onSelect={() => send({ action: "pane", value: "files" })}>文件浏览</MenubarItem>
        </MenubarContent>
      </MenubarMenu>

      <MenubarMenu>
        <MenubarTrigger>外观</MenubarTrigger>
        <MenubarContent>
          <MenubarRadioGroup value={theme} onValueChange={(value) => chooseTheme(value as AppTheme)}>
            <MenubarRadioItem value="light">浅色主题</MenubarRadioItem>
            <MenubarRadioItem value="dark">深色主题</MenubarRadioItem>
          </MenubarRadioGroup>
          <MenubarSeparator />
          <MenubarCheckboxItem checked disabled>
            终端画布渲染
          </MenubarCheckboxItem>
        </MenubarContent>
      </MenubarMenu>

      <MenubarMenu>
        <MenubarTrigger>设置</MenubarTrigger>
        <MenubarContent>
          <MenubarItem onSelect={() => openSettings("terminal")}>终端外观…</MenubarItem>
          <MenubarItem onSelect={() => openSettings("ssh")}>SSH 连接…</MenubarItem>
          <MenubarItem onSelect={() => openSettings("wsl")}>WSL 配置…</MenubarItem>
        </MenubarContent>
      </MenubarMenu>

      <MenubarMenu>
        <MenubarTrigger>帮助</MenubarTrigger>
        <MenubarContent>
          <MenubarItem
            onSelect={() =>
              notify("Terminal · 支持本地 / WSL / SSH / ADB 多面板终端", "info", 4200)
            }
          >
            关于 Terminal
          </MenubarItem>
          <MenubarItem
            onSelect={() =>
              notify("Ctrl+W 关闭当前终端 · 拖拽页签到面板可交换位置", "info", 4200)
            }
          >
            快捷键提示
          </MenubarItem>
          <MenubarSeparator />
          <MenubarItem onSelect={quit}>退出</MenubarItem>
        </MenubarContent>
      </MenubarMenu>
    </Menubar>
  );
};

export default AppMenubar;
