import React from "react";
import { Select, Button } from "@/components/ui";
import {
  APP_FONT_OPTIONS,
  APP_MONO_FONT_OPTIONS,
  readAppFont,
  readAppMonoFont,
  setAppFont,
  setAppMonoFont,
  type AppFont,
  type AppMonoFont,
} from "@/platform/font";
import { FileTransferSettings } from "./FileTransferSettings";
import { ArchivedSessionsDialog } from "@/components/ArchivedSessionsDialog/ArchivedSessionsDialog";

export const GeneralPanel: React.FC = () => {
  const [appLanguage, setAppLanguage] = React.useState("zh-CN");
  const [appFont, setFontSelection] = React.useState<AppFont>(readAppFont);
  const [appMonoFont, setMonoFontSelection] = React.useState<AppMonoFont>(readAppMonoFont);
  const [archivedOpen, setArchivedOpen] = React.useState(false);

  return (
    <section className="settings-pane">
      <header className="pane-header">
        <h2 className="pane-title">常规</h2>
      </header>

      <div>
        <div className="setting-group-title">界面首选项</div>
        <div className="setting-card-group">
          <FileTransferSettings />
          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">界面语言 (Language)</span>
              <span className="setting-desc">应用操作界面显示语言</span>
            </div>
            <div className="setting-control-block">
              <Select
                size="sm"
                value={appLanguage}
                onChange={(val) => setAppLanguage(val as string)}
                options={[
                  { label: "简体中文", value: "zh-CN" },
                  { label: "English (US)", value: "en-US" },
                ]}
                style={{ width: "100%" }}
              />
            </div>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">界面字体</span>
              <span className="setting-desc">默认使用系统字体；Windows 使用 Segoe UI 与微软雅黑，macOS 使用系统字体与苹方。思源、霞鹜文楷、Inter、Lato、Ubuntu 随安装包分发；标注「需已安装」的读取系统里装的版本，没装会自动退化为系统字体。</span>
            </div>
            <div className="setting-control-block font-setting-control">
              <Select
                size="sm"
                value={appFont}
                onChange={(value) => {
                  const font = value as AppFont;
                  setAppFont(font);
                  setFontSelection(font);
                }}
                options={[...APP_FONT_OPTIONS]}
                searchable
                searchPlaceholder="搜索界面字体"
                style={{ width: "100%" }}
              />
              <span className="font-setting-preview">Aa 中文界面，清晰易读 0123456789</span>
            </div>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">代码与终端字体</span>
              <span className="setting-desc">用于终端、命令输出和 Markdown 代码；中文字形自动回退到等宽中文字体，不会与正文错位。</span>
            </div>
            <div className="setting-control-block font-setting-control">
              <Select
                size="sm"
                value={appMonoFont}
                onChange={(value) => {
                  const font = value as AppMonoFont;
                  setAppMonoFont(font);
                  setMonoFontSelection(font);
                }}
                options={[...APP_MONO_FONT_OPTIONS]}
                searchable
                searchPlaceholder="搜索代码字体"
                style={{ width: "100%" }}
              />
              <span className="font-setting-preview font-setting-preview-mono">~/workspace $ cargo check</span>
            </div>
          </div>

          {/* 归档会话原先没有任何入口：归档完就再也找不回来。这一版先用悬浮窗
              列出来（下一版再做独立界面），恢复就是清掉 archived 标记。 */}
          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">归档会话</span>
              <span className="setting-desc">查看侧栏里被归档的对话，随时恢复到原工作区，或彻底删除。</span>
            </div>
            <div className="setting-control-block">
              <Button size="sm" variant="subtle" onClick={() => setArchivedOpen(true)}>查看归档</Button>
            </div>
          </div>

        </div>
      </div>
      {archivedOpen && <ArchivedSessionsDialog onClose={() => setArchivedOpen(false)} />}
    </section>
  );
};
