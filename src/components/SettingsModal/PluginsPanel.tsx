import { PluginItem } from "./PluginItem";
import { useEffect, useRef, useState } from "react";
import { ChevronRight, Download, ExternalLink, FolderOpen, Puzzle, RefreshCw, Settings2, Trash2 } from "lucide-react";
import { Button, Input, Switch } from "@/components/ui";
import { ProviderIcon } from "@/components/ProviderIcon";
import { useConfirmAction } from "@/components/ConfirmDialog/useConfirmAction";
import { configureModelPlugin, installModelPlugin, listModelPlugins, probeModelPlugin, refreshModelPlugin, setModelPluginEnabled, uninstallModelPlugin, type ModelPlugin } from "@/platform/modelPlugins";
import { checkEnvironmentTool, downloadEnvironmentTool, type EnvironmentToolDownloadProgress } from "@/platform/api";
import { installWithPluginEnvironments, missingPluginEnvironments } from "@/platform/pluginEnvironments";
import { formatInvokeError, isTauri, listen, openFile } from "@/platform/runtime";
import { openExternalLink } from "@/platform/externalLinks";
import { useSettingsNotification } from "./SettingsNotification";
import "./plugins-panel.css";
import { HostPluginsPanel } from "./HostPluginsPanel";
import { checkPluginUpdates, hasPluginUpdate, type PluginUpdate } from "@/platform/pluginUpdates";

export function PluginsPanel() {
  const notify = useSettingsNotification();
  const { confirmAction, confirmationDialog } = useConfirmAction();
  const [plugins, setPlugins] = useState<ModelPlugin[]>([]);
  const [busy, setBusy] = useState("");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState("");
  const [installProgress, setInstallProgress] = useState("");
  const [updates, setUpdates] = useState<Record<string, PluginUpdate>>({});
  const [checkingUpdates, setCheckingUpdates] = useState(true);
  const [updateError, setUpdateError] = useState("");
  const activeRuntime = useRef<{ id: string; name: string } | null>(null);
  const executing = useRef(false);
  const mounted = useRef(true);
  const revision = useRef(0);
  const reload = async () => {
    const sequence = ++revision.current;
    const next = await listModelPlugins();
    if (mounted.current && sequence === revision.current) { setPlugins(next); setError(""); }
  };
  useEffect(() => {
    mounted.current = true;
    if (!isTauri()) { setLoading(false); setCheckingUpdates(false); return; }
    void checkPluginUpdates().then(results => {
      if (mounted.current) setUpdates(Object.fromEntries(results.map(result => [result.id, result])));
    }).catch(reason => { if (mounted.current) setUpdateError(formatInvokeError(reason)); })
      .finally(() => { if (mounted.current) setCheckingUpdates(false); });
    const unlisteners: (() => void)[] = [];
    void reload().catch((reason) => { if (mounted.current) setError(formatInvokeError(reason)); })
      .finally(() => { if (mounted.current) setLoading(false); });
    for (const event of ["model-plugins-changed", "model-plugin-state-changed"]) {
      void listen(event, () => { void reload().catch(() => {}); }).then((stop) => {
        if (!mounted.current) stop(); else unlisteners.push(stop);
      }).catch(() => {});
    }
    void listen<EnvironmentToolDownloadProgress>("environment-tool-download-progress", (progress) => {
      const runtime = activeRuntime.current;
      if (!mounted.current || runtime?.id !== progress.toolId) return;
      const phase = progress.phase;
      if (phase === "failed") { setInstallProgress(`${runtime.name} 安装失败，插件未安装。`); return; }
      if (phase === "completed") { setInstallProgress(`${runtime.name} 已安装，正在检查插件…`); return; }
      const downloaded = (progress.downloadedBytes / 1024 / 1024).toFixed(1);
      const total = progress.totalBytes ? ` / ${(progress.totalBytes / 1024 / 1024).toFixed(1)} MB` : " MB";
      setInstallProgress(phase === "resolving" ? `正在获取 ${runtime.name} 下载信息…`
        : phase === "installing" ? `正在校验并安装 ${runtime.name}…` : `正在下载 ${runtime.name}：${downloaded}${total}`);
    }).then((stop) => { if (!mounted.current) stop(); else unlisteners.push(stop); }).catch(() => {});
    return () => { mounted.current = false; ++revision.current; unlisteners.forEach((stop) => stop()); };
  }, []);
  const installing = plugins.some((plugin) => plugin.status === "installing");
  useEffect(() => {
    if (!busy && !installing) return;
    const timer = window.setInterval(() => { void reload().catch(() => {}); }, 1500);
    return () => window.clearInterval(timer);
  }, [busy, installing]);

  const run = async (key: string, operation: () => Promise<unknown>, message: string, rethrow = false) => {
    if (executing.current) return;
    executing.current = true; setBusy(key);
    try {
      await operation(); await reload();
      if (mounted.current) notify(message, "success");
    } catch (reason) { if (rethrow) throw reason; if (mounted.current) notify(formatInvokeError(reason), "error"); }
    finally { executing.current = false; activeRuntime.current = null; if (mounted.current) { setBusy(""); setInstallProgress(""); } }
  };
  const installPlugin = async (plugin: ModelPlugin, message: string) => {
    if (executing.current) return;
    executing.current = true;
    setBusy(`${plugin.id}:install`);
    let missing;
    try {
      missing = await missingPluginEnvironments(plugin.requiredEnvironmentTools, checkEnvironmentTool);
    } catch (reason) {
      if (mounted.current) notify(formatInvokeError(reason), "error");
      return;
    } finally {
      executing.current = false;
      if (mounted.current) setBusy("");
    }
    if (!mounted.current) return;
    const install = (allowDownload: boolean, rethrow = false) => run(`${plugin.id}:install`, () => installWithPluginEnvironments(plugin.requiredEnvironmentTools, {
      check: checkEnvironmentTool,
      download: downloadEnvironmentTool,
      install: () => installModelPlugin(plugin.id),
      allowDownload,
      onDownload: (runtime) => { activeRuntime.current = runtime; setInstallProgress(`正在准备 ${runtime.name}…`); },
      onInstall: () => { activeRuntime.current = null; setInstallProgress("正在下载并安装插件…"); },
    }), message, rethrow);
    if (missing.length === 0) {
      await install(false);
      return;
    }
    confirmAction({
      title: "安装插件运行环境",
      message: `${plugin.name} 需要安装 ${missing.map((runtime) => runtime.name).join("、")}。将从官方源下载便携包，统一保存在环境与工具中${missing.some((runtime) => runtime.id === "node") ? "；Node.js 下载约 35 MB" : ""}。环境下载或校验失败时，插件不会安装或启动。`,
      confirmLabel: "下载并安装插件",
      onConfirm: () => install(true, true),
    });
  };
  return <section className="settings-pane plugins-pane">
    {confirmationDialog}
    <header className="pane-header"><h2 className="pane-title">插件</h2></header>
    <div className="plugin-manager">
    {!isTauri() && <p className="setting-desc">请在桌面应用中管理插件。</p>}
    {loading && <p className="setting-desc" role="status">正在读取插件…</p>}
    {checkingUpdates && <p className="setting-desc" role="status">正在检查插件更新…</p>}
    {updateError && <p className="setting-desc" role="status" title={updateError}>插件更新检查失败，下次打开时重试。</p>}
    {error && <p className="setting-desc" role="alert">{error}<Button size="sm" onClick={() => void run("retry", reload, "插件列表已刷新。")}>重试</Button></p>}
    {isTauri() && <HostPluginsPanel updates={updates} availableModelPlugins={plugins.filter(plugin => !plugin.installedVersion).map(plugin => <PluginItem key={plugin.id} name={plugin.name} icon={<Puzzle size={18} />} status={plugin.status === "installing" ? "正在安装" : "可安装"}>
      <p className="setting-desc">{plugin.description}</p>
      <div className="plugin-actions"><Button size="sm" variant="primary" loading={busy === `${plugin.id}:install`} disabled={Boolean(busy) || installing} icon={<Download size={14} />} onClick={() => void installPlugin(plugin, "插件已安装并启用。")}>安装</Button></div>
      {(busy === `${plugin.id}:install` ? installProgress : plugin.message) && <p className="setting-desc" role={plugin.status === "error" ? "alert" : "status"}>{busy === `${plugin.id}:install` ? installProgress : plugin.message}</p>}
    </PluginItem>)} modelPlugins={plugins.some(plugin => plugin.installedVersion) ? plugins.filter(plugin => plugin.installedVersion).map((plugin) => <PluginEntry key={plugin.id} plugin={plugin} update={updates[plugin.id]} busy={busy} disabled={Boolean(busy) || installing} run={run} notify={notify}
      installPlugin={installPlugin}
      installProgress={busy.startsWith(`${plugin.id}:`) ? installProgress : ""}
      uninstall={() => confirmAction({ title: `卸载 ${plugin.name} 插件`, message: "停止插件并删除安装文件和插件配置。对话历史与原应用的登录状态保留。", confirmLabel: "卸载", onConfirm: () => run(`${plugin.id}:uninstall`, () => uninstallModelPlugin(plugin.id), "插件已卸载。", true) })} />) : undefined} />}
    </div>
  </section>;
}

type EntryProps = {
  plugin: ModelPlugin;
  update?: PluginUpdate;
  busy: string;
  disabled: boolean;
  run: (key: string, operation: () => Promise<unknown>, message: string, rethrow?: boolean) => Promise<void>;
  installPlugin: (plugin: ModelPlugin, message: string) => Promise<void>;
  installProgress: string;
  notify: ReturnType<typeof useSettingsNotification>;
  uninstall: () => void;
};
function PluginEntry({ plugin, update, busy, disabled, run, notify, installPlugin, installProgress, uninstall }: EntryProps) {
  const settings = useRef<HTMLDetailsElement>(null);
  const { confirmAction: confirmProbe, confirmationDialog: probeDialog } = useConfirmAction();
  const [draft, setDraft] = useState({ authFileCn: plugin.authFileCn, authFileAi: plugin.authFileAi, electronFileCn: plugin.electronFileCn || "", electronFileAi: plugin.electronFileAi || "" });
  useEffect(() => { setDraft({ authFileCn: plugin.authFileCn, authFileAi: plugin.authFileAi, electronFileCn: plugin.electronFileCn || "", electronFileAi: plugin.electronFileAi || "" }); }, [plugin.authFileCn, plugin.authFileAi, plugin.electronFileCn, plugin.electronFileAi]);
  // 字段列表由后端 spec 下发：key 只会是下面四个登录/程序路径字段，label 由后端提供。
  const fields = plugin.configFields as { key: keyof typeof draft; label: string }[];
  const action = (kind: string, operation: () => Promise<unknown>, message: string) => void run(`${plugin.id}:${kind}`, operation, message);
  const browse = async (key: keyof typeof draft, label: string) => {
    try { const path = await openFile(`选择 ${label}`); if (path) setDraft((value) => ({ ...value, [key]: path })); }
    catch (reason) { notify(formatInvokeError(reason), "error"); }
  };
  const ready = plugin.enabled && plugin.accounts.some((account) => account.state === "signed-in");
  const status = plugin.status === "installing" ? "正在安装" : !plugin.installedVersion ? "未安装" : !plugin.enabled ? "已停用" : ready ? "已连接" : "待连接";
  const availableVersion = update?.version || plugin.availableVersion;
  return <PluginItem name={plugin.name} icon={plugin.iconBrand ? <ProviderIcon brand={plugin.iconBrand} size={18} /> : <Puzzle size={18} />}
    version={plugin.installedVersion} status={status} variant={plugin.status === "error" ? "danger" : ready ? "success" : "neutral"}>
      {probeDialog}
      <p className="setting-desc">{plugin.description}</p>
      <div className="plugin-controls">
          <Button size="sm" variant="subtle" icon={<Settings2 size={14} />} onClick={() => { if (settings.current) { settings.current.open = !settings.current.open; if (settings.current.open) settings.current.scrollIntoView({ block: "nearest" }); } }}>插件设置</Button>
          <div className="plugin-control-tools">
          <Button size="sm" variant="subtle" className="skill-icon-button" aria-label={`刷新 ${plugin.name} 账号与额度`} title="刷新账号与额度" loading={busy === `${plugin.id}:refresh`} disabled={disabled} icon={<RefreshCw size={14} />} onClick={() => action("refresh", () => refreshModelPlugin(plugin.id), "刷新完成，请查看账号与额度状态。")}/>
          <Button size="sm" variant="subtle" className="skill-icon-button btn-danger-hover" aria-label={`卸载 ${plugin.name} 插件`} title="卸载插件" disabled={disabled} icon={<Trash2 size={14} />} onClick={uninstall}/>
          </div>
          <Switch className="plugin-control-start" checked={plugin.enabled} disabled={disabled} aria-label={`启用 ${plugin.name} 插件`} onChange={(enabled) => action("toggle", () => setModelPluginEnabled(plugin.id, enabled), enabled ? "插件已启用。" : "插件已停用。")} />
      </div>
      <div className="plugin-links">
          {(hasPluginUpdate(plugin.installedVersion, availableVersion) || plugin.status === "error") && <Button size="sm" variant="secondary" disabled={disabled} loading={busy === `${plugin.id}:install`} onClick={() => void installPlugin(plugin, "插件已更新。")}>{hasPluginUpdate(plugin.installedVersion, availableVersion) ? `更新 v${availableVersion}` : "重新安装"}</Button>}
        {update?.error && <span className="setting-desc" title={update.error}>更新检查失败，下次打开时重试</span>}
        <Button size="sm" variant="subtle" icon={<ExternalLink size={14} />} onClick={() => void openExternalLink(plugin.source).catch((reason) => notify(formatInvokeError(reason), "error"))}>来源</Button>
      </div>
      {installProgress ? <p className="setting-desc" role="status">{installProgress}</p> : (plugin.status === "installing" || plugin.status === "error") && <p className="setting-desc" role={plugin.status === "error" ? "alert" : "status"}>{plugin.message}</p>}
      {plugin.accounts.map((account) => <div className="plugin-account" key={account.id}>
        <div className="setting-label-block">
          <span className="setting-title">{account.name}<span className="skill-source">{account.state === "signed-in" ? account.account : account.state === "error" ? "连接异常" : "未登录"}</span></span>
          <span className="setting-desc">{account.message}</span>
          {account.creditError && <span className="setting-desc" role="alert">额度查询失败：{account.creditError}</span>}
        </div>
        {account.state === "signed-in" && <span className="setting-desc">{account.modelCount} 个模型{account.unlimited ? " · 不限量" : account.credits != null ? ` · ${account.credits.toLocaleString("zh-CN")} 积分` : ""}</span>}
      </div>)}
      {plugin.supportsReasoningProbe && plugin.accounts.some((account) => account.reasoningModels?.length) && <details className="plugin-advanced plugin-reasoning">
        <summary className="plugin-advanced-summary"><Settings2 size={14} /><span>推理档位检测</span><ChevronRight size={14} className="plugin-config-chevron" /></summary>
        <p className="setting-desc">检测会发送少量真实请求，可能消耗积分。结果按账号缓存 14 天；目录声明的档位优先使用。</p>
        {plugin.accounts.filter((account) => account.reasoningModels?.length).map((account) => <div key={account.id}>
          {plugin.accounts.filter((item) => item.reasoningModels?.length).length > 1 && <p className="setting-title">{account.name}</p>}
          {account.reasoningModels?.map((model) => {
            const key = `${plugin.id}:probe:${account.id}:${model.id}`;
            // providerId 只取后端下发的真实值（probe 命令依赖它定位账号），不再按插件名硬编码构造兜底；
            // 缺失时不渲染检测按钮，避免向后端发送伪造的 provider id。
            const providerId = account.providerId;
            return <div key={model.id} className="plugin-reasoning-row">
              <ProviderIcon brand={model.iconBrand} size={16} />
              <div className="plugin-reasoning-name"><span className="setting-title" title={model.name}>{model.name}</span>
                {model.probedAt && <span className="setting-desc">检测于 {new Date(model.probedAt).toLocaleString("zh-CN")}</span>}
                {model.message && <span className="setting-desc" role="alert">{model.message}</span>}
              </div>
              <span className={`plugin-reasoning-result${model.efforts.length ? " has-efforts" : ""}`}>
                {model.efforts.length ? model.efforts.join(" / ") : !model.supportsReasoning ? "不支持思考" : model.validation === "non-validating" ? "不校验档位 · 无法验证" : model.probedAt && model.validation === "validating" ? "无可用档位" : model.probedAt ? "检测未完成" : "档位未检测"}
              </span>
              {model.canProbe && providerId && <Button size="xs" variant="secondary" disabled={disabled || model.probing} loading={busy === key || model.probing}
                onClick={() => confirmProbe({ title: `检测 ${model.name} 推理档位`, message: "将发送少量测试请求，可能消耗账号积分。检测结果只用于当前账号。", confirmLabel: "开始检测",
                  onConfirm: () => run(key, () => probeModelPlugin(providerId, model.id), "检测完成，模型思考强度已更新。", true) })}>{model.probedAt ? "重新检测" : "开始检测"}</Button>}
            </div>;
          })}
        </div>)}
      </details>}
      <details ref={settings} className="plugin-advanced">
      <summary className="plugin-advanced-summary"><Settings2 size={14} /><span>高级配置</span><ChevronRight size={14} className="plugin-config-chevron" /></summary>
      <p className="setting-desc">{plugin.advancedHint}</p>
      {fields.map(({ key, label }) => <div className="plugin-config-row" key={key}>
        <label className="setting-desc" htmlFor={`${plugin.id}-${key}`}>{label}</label>
        <Input id={`${plugin.id}-${key}`} size="sm" value={draft[key]} placeholder="自动检测（留空）" disabled={disabled} onChange={(event) => setDraft((value) => ({ ...value, [key]: event.target.value }))}/>
        <Button size="sm" variant="subtle" className="skill-icon-button" aria-label={`选择 ${label}`} disabled={disabled} icon={<FolderOpen size={14}/>} onClick={() => void browse(key, label)}/>
      </div>)}
      <div className="skills-actions plugin-actions"><Button size="sm" variant="primary" disabled={disabled} loading={busy === `${plugin.id}:save`} onClick={() => action("save", () => configureModelPlugin(plugin.id, draft.authFileCn, draft.authFileAi, draft.electronFileCn, draft.electronFileAi), "插件配置已保存。")}>保存配置</Button></div>
      </details>
  </PluginItem>;
}
