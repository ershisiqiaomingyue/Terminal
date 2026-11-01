import React, { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { PanelLeft } from "lucide-react";
import { notify } from "@/platform/notifications";
import { sendTerminalMenu } from "@/platform/terminalMenu";
import type { SettingsSection } from "@/components/SettingsModal/SettingsModal";
import { Button, Divider, Modal } from "rain-ui";
import {
  Menubar,
  MenubarMenu,
  MenubarTrigger,
  MenubarContent,
  MenubarItem,
} from "./menubar";

const RELEASES_API = "https://api.github.com/repos/ershisiqiaomingyue/Terminal/releases/latest";

function openSettings(section: SettingsSection) {
  window.dispatchEvent(new CustomEvent("android-tool-open-settings", { detail: { section } }));
}

/**
 * 应用顶部菜单栏。
 *
 * 最左是终端侧栏折叠按钮。设置是直达入口，不再展开二级菜单；
 * 帮助只保留关于。窗口控制按钮由原生窗口装饰提供。
 */
export const AppMenubar: React.FC = () => {
  const [aboutOpen, setAboutOpen] = useState(false);
  const [version, setVersion] = useState("");
  const [checkingUpdate, setCheckingUpdate] = useState(false);

  useEffect(() => {
    if (!aboutOpen || version) return;
    void getVersion().then(setVersion).catch(() => setVersion(""));
  }, [aboutOpen, version]);

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

  return (
    <Menubar className="terminal-menubar" data-tauri-drag-region>
      <Button
        type="button"
        variant="ghost"
        size="icon-sm"
        icon={<PanelLeft size={15} strokeWidth={1.7} />}
        title="收起 / 展开终端列表"
        aria-label="收起 / 展开终端列表"
        onMouseDown={(event) => event.stopPropagation()}
        onClick={() => sendTerminalMenu({ action: "toggle-sidebar" })}
      />

      <Button
        type="button"
        variant="ghost"
        size="sm"
        onMouseDown={(event) => event.stopPropagation()}
        onClick={() => openSettings("general")}
      >
        设置
      </Button>

      <MenubarMenu>
        <MenubarTrigger>帮助</MenubarTrigger>
        <MenubarContent>
          <MenubarItem onSelect={() => setAboutOpen(true)}>关于 Terminal</MenubarItem>
        </MenubarContent>
      </MenubarMenu>

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
