import React from "react";
import { Select, Button, Input } from "@/components/ui";
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
import { discoverRemoteThemes, importRemoteThemeFamily, importLocalTheme, readImportedThemes, readThemeSelection, removeImportedTheme, selectImportedTheme, setAppTheme, subscribeAppTheme } from "@/platform/theme";
import type { ThemeCandidate } from "@/platform/themeImport";
import { groupThemeOptions } from "@/platform/themeNames";
import { formatInvokeError } from "@/platform/runtime";
import { Download, Upload } from "lucide-react";
import { useSettingsNotification } from "./SettingsNotification";
import { FileTransferSettings } from "./FileTransferSettings";
import { ArchivedSessionsDialog } from "@/components/ArchivedSessionsDialog/ArchivedSessionsDialog";

const readThemeOptions = () => groupThemeOptions(readImportedThemes());

export const GeneralPanel: React.FC = () => {
  const [appLanguage, setAppLanguage] = React.useState("zh-CN");
  const [appTheme, setThemeSelection] = React.useState(readThemeSelection);
  const [importedThemes, setImportedThemes] = React.useState(readThemeOptions);
  const [appFont, setFontSelection] = React.useState<AppFont>(readAppFont);
  const [appMonoFont, setMonoFontSelection] = React.useState<AppMonoFont>(readAppMonoFont);
  const [themeUrl, setThemeUrl] = React.useState("");
  const [remoteOpen, setRemoteOpen] = React.useState(false);
  const [remoteThemes, setRemoteThemes] = React.useState<ThemeCandidate[]>([]);
  const [themeProgress, setThemeProgress] = React.useState("");
  const [themeBusy, setThemeBusy] = React.useState(false);
  const [archivedOpen, setArchivedOpen] = React.useState(false);
  const themeBusyRef = React.useRef(false);
  const notify = useSettingsNotification();

  React.useEffect(() => subscribeAppTheme(() => {
    setThemeSelection(readThemeSelection());
    setImportedThemes(readThemeOptions());
  }), []);

  const runThemeAction = async (action: () => Promise<void>) => {
    if (themeBusyRef.current) return;
    themeBusyRef.current = true;
    setThemeBusy(true);
    try { await action(); }
    catch (reason) { notify(formatInvokeError(reason), "error"); }
    finally { themeBusyRef.current = false; setThemeBusy(false); setThemeProgress(""); }
  };

  const fetchThemes = () => runThemeAction(async () => {
    setRemoteThemes([]);
    const themes = await discoverRemoteThemes(themeUrl);
    setRemoteThemes(themes);
    notify(`已识别 ${themes.length} 个主题。`, "success");
  });

  return (
    <section className="settings-pane">
      <header className="pane-header">
        <h2 className="pane-title">常规偏好</h2>
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
              <span className="setting-title">外观主题</span>
              <span className="setting-desc">应用色彩与亮暗模式</span>
            </div>
            <div className="setting-control-block theme-setting-control">
              <Select
                size="sm"
                value={appTheme}
                disabled={themeBusy}
                onChange={(val) => {
                  if (val === "light" || val === "dark") setAppTheme(val);
                  else selectImportedTheme(String(val));
                }}
                options={[
                  { label: "亮色", value: "light" },
                  { label: "暗色", value: "dark" },
                  ...importedThemes.map((theme) => ({ label: theme.name, value: theme.id, group: { id: theme.family.id, label: theme.family.name }, removeLabel: `删除主题 ${theme.name}`, onRemove: () => {
                    try { removeImportedTheme(theme.id); notify(`已删除主题：${theme.name}`, "success"); }
                    catch (reason) { notify(formatInvokeError(reason), "error"); }
                  } })),
                ]}
                style={{ width: "100%" }}
              />
              <div className="theme-source-actions">
                <Button size="xs" variant="secondary" icon={<Download size={13} />} disabled={themeBusy} aria-expanded={remoteOpen} onClick={() => setRemoteOpen((open) => !open)}>远程拉取</Button>
                <Button size="xs" variant="secondary" icon={<Upload size={13} />} disabled={themeBusy} onClick={() => void runThemeAction(async () => {
                  const theme = await importLocalTheme();
                  if (theme) notify(`已导入主题：${theme.name}`, "success");
                })}>本地添加</Button>
              </div>
              {remoteOpen && <div className="theme-remote-controls">
                <div className="theme-remote-row">
                  <Input size="sm" value={themeUrl} disabled={themeBusy} aria-label="GitHub 主题地址" onChange={(event) => { setThemeUrl(event.target.value); setRemoteThemes([]); }} onKeyDown={(event) => { if (event.key === "Enter" && themeUrl.trim() && !themeBusy) void fetchThemes(); }} placeholder="GitHub 仓库或主题 JSON 地址" />
                  <Button size="xs" variant="secondary" loading={themeBusy} disabled={!themeUrl.trim()} onClick={() => void fetchThemes()}>拉取</Button>
                </div>
                {remoteThemes.length > 0 && <div className="theme-remote-row">
                  <div className="theme-family-preview"><span className="setting-title">{remoteThemes[0].family?.name}</span><span className="setting-desc">{remoteThemes.map(theme => theme.name).join("、")}</span></div>
                  <Button size="xs" variant="primary" loading={themeBusy} disabled={themeBusy} onClick={() => void runThemeAction(async () => {
                    setThemeProgress(`0/${remoteThemes.length}`);
                    const themes = await importRemoteThemeFamily(remoteThemes, (done, total) => setThemeProgress(`${done}/${total}`));
                    setRemoteOpen(false);
                    notify(`已添加 ${themes[0].family?.name || "主题"}，共 ${themes.length} 个子主题。`, "success");
                  })}>{themeProgress ? `添加中 ${themeProgress}` : "添加整套主题"}</Button>
                </div>}
              </div>}
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
              <span className="setting-desc">用于终端、Logcat、命令输出和 Markdown 代码；中文字形自动回退到等宽中文字体，不会与正文错位。</span>
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
              <span className="font-setting-preview font-setting-preview-mono">~/workspace $ adb devices</span>
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
