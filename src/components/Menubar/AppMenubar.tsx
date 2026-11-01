import React, { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getVersion } from "@tauri-apps/api/app";
import { PanelLeft } from "lucide-react";
import { notify } from "@/platform/notifications";
import { readAppTheme, setAppTheme, subscribeAppTheme, type AppTheme } from "@/platform/theme";
import { isTauri } from "@/platform/runtime";
import { sendTerminalMenu } from "@/platform/terminalMenu";
import type { SettingsSection } from "@/components/SettingsModal/SettingsModal";
import { Button, Divider, Modal } from "rain-ui";
import {
  Menubar,
  MenubarMenu,
  MenubarTrigger,
  MenubarContent,
  MenubarItem,
  MenubarSeparator,
  MenubarRadioGroup,
  MenubarRadioItem,
} from "./menubar";

import { BackgroundTasksDialog } from "@/components/BackgroundTasks/BackgroundTasksDialog";

const RELEASES_API = "https://api.github.com/repos/ershisiqiaomingyue/Terminal/releases/latest";

function openSettings(section: SettingsSection) {
  window.dispatchEvent(new CustomEvent("android-tool-open-settings", { detail: { section } }));
}

/**
 * 应用顶部菜单栏。
 *
 * 按需求定稿：最左是终端侧栏折叠按钮（后退/前进箭头在 TitleBar 里），
 * 菜单只有「设置」与「帮助」两项；后台任务入口收进「设置」菜单的弹窗。
 * 折叠动作经 terminal:menu 事件送进终端视图（菜单栏在终端视图之外，见
 * App.tsx）。帮助菜单的「关于」用 rain-ui Modal 展示版本并支持检查更新。
 */
export const AppMenubar: React.FC = () => {
  const [theme, setTheme] = useState<AppTheme>(readAppTheme);
  const [aboutOpen, setAboutOpen] = useState(false);
  const [tasksOpen, setTasksOpen] = useState(false);
  const [version, setVersion] = useState("");
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  useEffect(() => subscribeAppTheme(setTheme), []);
  useEffect(() => {
    if (!aboutOpen || version) return;
    void getVersion().then(setVersion).catch(() => setVersion(""));
  }, [aboutOpen, version]);

  const chooseTheme = (next: AppTheme) => setAppTheme(next);

  const checkUpdate = async () => {
    setCheckingUpdate(true);
    try {
      const response = await fetch(RELEASES_API, { headers: { Accept: "application/vnd.github+json" } });
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const release = (await response.json()) as { tag_name?: string };
      const latest = String(release.tag_name || "").replace(/^v/i, "");
      const current = version || (await getVersion());
      if (latest && latest !== current) notify(`发现新版本 v${latest}（当前 v${current}），请到 GitHub 下载。`, "info", 6000);
      else notify(`已是最新版本（v${current}）。`, "success", 4200);
    } catch (error) {
      notify(`检查更新失败：${error instanceof Error ? error.message : String(error)}`, "error", 5200);
    } finally {
      setCheckingUpdate(false);
    }
  };

  const quit = () => {
    if (!isTauri()) return;
    void getCurrentWindow().close();
  };

  return (
    <Menubar className="terminal-menubar" data-tauri-drag-region>
      <Button
        type="button" variant="bare"
        className="terminal-menubar-toggle"
        title="收起 / 展开终端列表"
        aria-label="收起 / 展开终端列表"
        onMouseDown={(event) => event.stopPropagation()}
        onClick={() => sendTerminalMenu({ action: "toggle-sidebar" })}
      >
        <PanelLeft size={15} strokeWidth={1.7} />
      </Button>

      <MenubarMenu>
        <MenubarTrigger>设置</MenubarTrigger>
        <MenubarContent>
          <MenubarItem onSelect={() => openSettings("general")}>打开设置…</MenubarItem>
          <MenubarItem onSelect={() => openSettings("appearance")}>终端…</MenubarItem>
          <MenubarItem onSelect={() => openSettings("environment")}>环境…</MenubarItem>
          <MenubarSeparator />
          <MenubarItem onSelect={() => setTasksOpen(true)}>后台任务…</MenubarItem>
          <MenubarSeparator />
          <MenubarRadioGroup value={theme} onValueChange={(value) => chooseTheme(value as AppTheme)}>
            <MenubarRadioItem value="light">浅色主题</MenubarRadioItem>
            <MenubarRadioItem value="dark">深色主题</MenubarRadioItem>
          </MenubarRadioGroup>
        </MenubarContent>
      </MenubarMenu>

      <MenubarMenu>
        <MenubarTrigger>帮助</MenubarTrigger>
        <MenubarContent>
          <MenubarItem onSelect={() => setAboutOpen(true)}>关于 Terminal</MenubarItem>
          <MenubarSeparator />
          <MenubarItem onSelect={quit}>退出</MenubarItem>
        </MenubarContent>
      </MenubarMenu>

      <BackgroundTasksDialog open={tasksOpen} onClose={() => setTasksOpen(false)} />

      <Modal
        open={aboutOpen}
        title="关于 Terminal"
        onClose={() => setAboutOpen(false)}
        footer={
          <>
            <Button variant="secondary" loading={checkingUpdate} onClick={() => void checkUpdate()}>
              检查更新
            </Button>
            <Button variant="default" onClick={() => setAboutOpen(false)}>
              关闭
            </Button>
          </>
        }
      >
        <div className="about-modal-body">
          <p className="setting-title">Terminal</p>
          <p className="setting-desc">版本 {version || "…"}</p>
          <p className="setting-desc">本地 / WSL / SSH 多面板终端，内置 AI 命令助手。</p>
          <Divider dense />
          <p className="setting-desc">Ctrl+W 关闭当前终端 · 拖拽页签到面板可交换位置 · 拖拽文件到终端可粘贴路径</p>
        </div>
      </Modal>
    </Menubar>
  );
};

export default AppMenubar;

