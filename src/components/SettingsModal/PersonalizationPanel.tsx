import React from "react";
import { Button, Card, CardBody, CardHeader, Input, Select } from "rain-ui";
import { Download, Upload } from "lucide-react";
import {
  discoverRemoteThemes,
  importRemoteThemeFamily,
  importLocalTheme,
  readImportedThemes,
  readThemeSelection,
  removeImportedTheme,
  selectImportedTheme,
  setAppTheme,
  subscribeAppTheme,
} from "@/platform/theme";
import type { ThemeCandidate } from "@/platform/themeImport";
import { groupThemeOptions } from "@/platform/themeNames";
import { formatInvokeError } from "@/platform/runtime";
import { useSettingsNotification } from "./SettingsNotification";

const readThemeOptions = () => groupThemeOptions(readImportedThemes());

/** 个性化：从 GeneralPanel 抽出的主题选择 + 本地/远程主题导入。 */
export const PersonalizationPanel: React.FC = () => {
  const [appTheme, setThemeSelection] = React.useState(readThemeSelection);
  const [importedThemes, setImportedThemes] = React.useState(readThemeOptions);
  const [themeUrl, setThemeUrl] = React.useState("");
  const [remoteOpen, setRemoteOpen] = React.useState(false);
  const [remoteThemes, setRemoteThemes] = React.useState<ThemeCandidate[]>([]);
  const [themeProgress, setThemeProgress] = React.useState("");
  const [themeBusy, setThemeBusy] = React.useState(false);
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
        <h2 className="pane-title">个性化</h2>
      </header>

      <Card tone="surface" className="settings-rain-card">
        <CardHeader>
          <div>
            <div className="card-title">外观主题</div>
            <div className="card-subtitle">应用色彩与亮暗模式，支持导入本地或远程主题包</div>
          </div>
        </CardHeader>
        <CardBody>
          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">界面主题</span>
              <span className="setting-desc">应用色彩与亮暗模式</span>
            </div>
            <div className="setting-control-block theme-setting-control">
              <Select
                value={appTheme}
                disabled={themeBusy}
                searchable
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
              />
              <div className="theme-source-actions">
                <Button variant="secondary" icon={<Download size={13} />} disabled={themeBusy} aria-expanded={remoteOpen} onClick={() => setRemoteOpen((open) => !open)}>远程拉取</Button>
                <Button variant="secondary" icon={<Upload size={13} />} disabled={themeBusy} onClick={() => void runThemeAction(async () => {
                  const theme = await importLocalTheme();
                  if (theme) notify(`已导入主题：${theme.name}`, "success");
                })}>本地添加</Button>
              </div>
              {remoteOpen && <div className="theme-remote-controls">
                <div className="theme-remote-row">
                  <Input value={themeUrl} disabled={themeBusy} aria-label="GitHub 主题地址" onChange={(event) => { setThemeUrl(event.target.value); setRemoteThemes([]); }} onKeyDown={(event) => { if (event.key === "Enter" && themeUrl.trim() && !themeBusy) void fetchThemes(); }} placeholder="GitHub 仓库或主题 JSON 地址" />
                  <Button variant="secondary" loading={themeBusy} disabled={!themeUrl.trim()} onClick={() => void fetchThemes()}>拉取</Button>
                </div>
                {remoteThemes.length > 0 && <div className="theme-remote-row">
                  <div className="theme-family-preview"><span className="setting-title">{remoteThemes[0].family?.name}</span><span className="setting-desc">{remoteThemes.map(theme => theme.name).join("、")}</span></div>
                  <Button variant="primary" loading={themeBusy} disabled={themeBusy} onClick={() => void runThemeAction(async () => {
                    setThemeProgress(`0/${remoteThemes.length}`);
                    const themes = await importRemoteThemeFamily(remoteThemes, (done, total) => setThemeProgress(`${done}/${total}`));
                    setRemoteOpen(false);
                    notify(`已添加 ${themes[0].family?.name || "主题"}，共 ${themes.length} 个子主题。`, "success");
                  })}>{themeProgress ? `添加中 ${themeProgress}` : "添加整套主题"}</Button>
                </div>}
              </div>}
            </div>
          </div>
        </CardBody>
      </Card>
    </section>
  );
};

export default PersonalizationPanel;
