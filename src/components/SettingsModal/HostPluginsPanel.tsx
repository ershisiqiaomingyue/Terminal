import { PluginItem } from './PluginItem';
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { ArrowRight, ChevronRight, FileArchive, FolderOpen, Package, Plus, Puzzle, RefreshCw, Settings2, Trash2, X } from 'lucide-react';
import { Button, Input, Switch } from '@/components/ui';
import { useConfirmAction } from '@/components/ConfirmDialog/useConfirmAction';
import { checkEnvironmentTool, downloadEnvironmentTool, type EnvironmentToolDownloadProgress } from '@/platform/api';
import { installWithPluginEnvironments, missingPluginEnvironments } from '@/platform/pluginEnvironments';
import { discoverLocalPlugins, inspectHostPlugin, installHostPlugin, listHostPlugins, openPluginSettings, setHostPluginEnabled, uninstallHostPlugin, type HostPlugin, type LocalPlugin, type PluginInstallProgress } from '@/platform/plugins';
import { formatInvokeError, isTauri, listen, openDirectory, openFile } from '@/platform/runtime';
import { useSettingsNotification } from './SettingsNotification';
import { DownloadProgress } from './DownloadProgress';
import { PluginSettingsFrame } from '@/components/PluginHost/PluginSettingsFrame';
import { hasPluginUpdate, type PluginUpdate } from '@/platform/pluginUpdates';

const phaseLabels: Record<string, string> = {
  waiting: '正在等待安装…', preparing: '正在检查共享运行环境…',
  downloading: '正在下载插件与依赖…', verifying: '正在安装并校验插件…',
  copying: '正在复制本机插件与依赖…', 'downloading-host': '正在下载缺少的共享宿主依赖…',
  extracting: '正在解压本地插件包…',
  starting: '正在启动插件…', completed: '插件已安装并加载', failed: '插件安装失败',
};

export function HostPluginsPanel({ modelPlugins, availableModelPlugins, updates = {} }: { modelPlugins?: ReactNode; availableModelPlugins?: ReactNode; updates?: Record<string, PluginUpdate> }) {
  const [plugins, setPlugins] = useState<HostPlugin[]>([]), [local, setLocal] = useState<LocalPlugin[]>([]);
  const [source, setSource] = useState(''), [busy, setBusy] = useState(false), [progress, setProgress] = useState('');
  const [localSource, setLocalSource] = useState<{ path: string; kind: 'archive' | 'directory' }>();
  const [download, setDownload] = useState<PluginInstallProgress>();
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const notify = useSettingsNotification(), { confirmAction, confirmationDialog } = useConfirmAction();
  const mounted = useRef(true), executing = useRef(false), activeEnvironment = useRef('');
  const activeRequest = useRef(''), subscription = useRef<Promise<unknown>>();
  const environmentSample = useRef({ time: 0, bytes: 0 });
  const reload = async () => { const value = await listHostPlugins(); if (mounted.current) setPlugins(value); };
  useEffect(() => {
    mounted.current = true;
    if (!isTauri()) return;
    let active = true;
    const stops: (() => void)[] = [];
    void reload().catch(error => notify(formatInvokeError(error), 'error'));
    void discoverLocalPlugins().then(value => { if (mounted.current) setLocal(value); }).catch(error => notify(formatInvokeError(error), 'error'));
    for (const [event, callback] of [
      ['plugins-changed', () => { void reload().catch(() => {}); }],
      ['environment-tool-download-progress', (value: EnvironmentToolDownloadProgress) => {
        if (!active || activeEnvironment.current !== value.toolId) return;
        const now = performance.now(), previous = environmentSample.current;
        const rate = previous.time && value.downloadedBytes >= previous.bytes
          ? (value.downloadedBytes - previous.bytes) / Math.max((now - previous.time) / 1000, 0.001) : 0;
        environmentSample.current = { time: now, bytes: value.downloadedBytes };
        setProgress(value.phase === 'downloading' ? '正在下载共享运行环境…'
          : value.phase === 'failed' ? '运行环境安装失败' : '正在准备并校验运行环境…');
        setDownload(current => ({ requestId: '', ...value,
          downloadedBytes: value.phase === 'downloading' ? value.downloadedBytes : current?.downloadedBytes || value.downloadedBytes,
          totalBytes: value.totalBytes || current?.totalBytes,
          bytesPerSecond: value.phase === 'downloading' ? rate : 0,
        }));
      }],
    ] as const) void listen(event, callback as (value: unknown) => void).then(stop => { if (active) stops.push(stop); else stop(); }).catch(() => {});
    subscription.current = listen<PluginInstallProgress>('plugin-install-progress', value => {
      if (!active || !activeRequest.current || value.requestId !== activeRequest.current) return;
      setProgress(phaseLabels[value.phase] || '正在处理插件…');
      setDownload(value);
    }).then(stop => { if (active) stops.push(stop); else stop(); });
    void subscription.current.catch(() => {});
    return () => { active = false; mounted.current = false; stops.forEach(stop => stop()); };
  }, []);
  async function run(operation: () => Promise<unknown>, success: string) {
    if (executing.current) return;
    executing.current = true; setBusy(true); setDownload(undefined);
    try { await operation(); await reload(); if (mounted.current) notify(success, 'success'); }
    catch (error) { if (mounted.current) notify(formatInvokeError(error), 'error'); throw error; }
    finally { executing.current = false; activeEnvironment.current = ''; activeRequest.current = ''; if (mounted.current) { setBusy(false); setProgress(''); setDownload(undefined); } }
  }
  async function install(value: string, requirements?: string[], successMessage = '插件已安装并加载。') {
    if (!value.trim() || executing.current) return;
    try { requirements ??= (await inspectHostPlugin(value)).requiredEnvironmentTools; }
    catch (error) { notify(formatInvokeError(error), 'error'); return; }
    const perform = (allowDownload: boolean) => run(() => installWithPluginEnvironments(requirements, {
      check: checkEnvironmentTool, download: downloadEnvironmentTool, allowDownload,
      install: async () => {
        await subscription.current;
        activeRequest.current = crypto.randomUUID();
        const plugin = await installHostPlugin(value, requirements, activeRequest.current);
        if (plugin.status === 'error') throw new Error(plugin.message || '插件已安装，但启动失败');
        return plugin;
      },
      onDownload: runtime => {
        activeEnvironment.current = runtime.id; environmentSample.current = { time: performance.now(), bytes: 0 };
        setDownload(undefined); setProgress(`正在准备 ${runtime.name}…`);
      },
      onInstall: () => { activeEnvironment.current = ''; setDownload(undefined); setProgress('正在准备插件安装…'); },
    }), successMessage);
    try {
      const missing = await missingPluginEnvironments(requirements, checkEnvironmentTool);
      if (missing.length) confirmAction({ title: '安装插件运行环境',
        message: `缺少 ${missing.map(value => value.name).join('、')}。将下载到共享环境；安装或校验失败时不会启动插件。`,
        confirmLabel: '下载并安装', onConfirm: () => perform(true) });
      else await perform(false);
    } catch (error) { if (!executing.current && mounted.current) notify(formatInvokeError(error), 'error'); }
  }
  // 没有任何已安装插件时整块（标题 + 卡片组）都不渲染：空白卡片比「暂无」更吵。
  const installedModelCount = Array.isArray(modelPlugins) ? modelPlugins.length : modelPlugins ? 1 : 0;
  const hasInstalled = plugins.length + installedModelCount > 0;
  return <>
    {confirmationDialog}
    <details className="setting-card-group plugin-source-card" aria-label="添加插件">
      <summary className="plugin-summary">
        <span className="plugin-item-icon" aria-hidden="true"><Plus size={18} /></span>
        <span className="plugin-item-name">添加插件</span>
        <ChevronRight size={15} className="plugin-chevron" aria-hidden="true" />
      </summary>
      <div className="plugin-source-content">
        <div className="plugin-source-grid">
          <form className="plugin-source-method plugin-source-npm" onSubmit={event => { event.preventDefault(); if (!busy) void install(source); }}>
            <div className="plugin-source-heading"><Package size={16} aria-hidden="true" /><h4>从 npm 安装</h4><span className="plugin-source-badge">在线</span></div>
            <p className="plugin-source-description">输入插件包名，获取并安装插件。</p>
            <div className="plugin-add-row"><Input size="sm" aria-label="插件包名或本地路径" placeholder="@scope/plugin-name" value={source} onChange={event => setSource(event.target.value)} disabled={busy} />
              <Button type="submit" size="sm" variant="primary" disabled={busy || !source.trim()}>安装</Button></div>
            <p className="plugin-source-hint">也支持直接粘贴本地路径</p>
          </form>
          <section className="plugin-source-method plugin-source-local" aria-label="从本机导入">
            <div className="plugin-source-heading"><FolderOpen size={16} aria-hidden="true" /><h4>从本机导入</h4><span className="plugin-source-badge">本地</span></div>
            <div className="plugin-source-options">
              <button type="button" className="plugin-source-option" disabled={busy} onClick={() => { void openFile('选择插件压缩包', [{ name: '插件压缩包', extensions: ['tgz', 'tar.gz', 'zip'] }]).then(path => { if (path) setLocalSource({ path, kind: 'archive' }); }).catch(error => notify(formatInvokeError(error), 'error')); }}>
                <span className="plugin-source-option-icon"><FileArchive size={18} aria-hidden="true" /></span><span className="plugin-source-option-copy"><strong>选择压缩包</strong><span>.tgz / .tar.gz / .zip</span></span><ChevronRight size={14} aria-hidden="true" />
              </button>
              <button type="button" className="plugin-source-option" disabled={busy} onClick={() => { void openDirectory('选择插件目录').then(path => { if (path) setLocalSource({ path, kind: 'directory' }); }).catch(error => notify(formatInvokeError(error), 'error')); }}>
                <span className="plugin-source-option-icon"><FolderOpen size={18} aria-hidden="true" /></span><span className="plugin-source-option-copy"><strong>选择插件目录</strong><span>复制本机已有插件</span></span><ChevronRight size={14} aria-hidden="true" />
              </button>
            </div>
          </section>
        </div>
        {localSource && <div className="plugin-source-selection" role="status">
          {localSource.kind === 'archive' ? <FileArchive size={18} aria-hidden="true" /> : <FolderOpen size={18} aria-hidden="true" />}
          <div className="plugin-source-selected-file"><strong>{localSource.path.split(/[\\/]/).filter(Boolean).pop()}</strong><span title={localSource.path}>{localSource.path}</span></div>
          <Button size="sm" variant="primary" icon={<ArrowRight size={14} />} disabled={busy} onClick={() => { void install(localSource.path); }}>安装</Button>
          <Button size="sm" variant="subtle" icon={<X size={14} />} aria-label="取消选择本地插件" disabled={busy} onClick={() => setLocalSource(undefined)} />
        </div>}
        {progress && <DownloadProgress label={progress} downloadedBytes={download?.downloadedBytes || 0}
          totalBytes={download?.totalBytes} bytesPerSecond={download?.bytesPerSecond || 0}
          downloading={download?.phase === 'downloading' || download?.phase === 'downloading-host'} completed={download?.phase === 'completed'} />}
        {availableModelPlugins}
        {local.filter(value => !plugins.some(plugin => plugin.name === value.name)).map(value => <PluginItem key={value.name}
          name={value.name} icon={<Puzzle size={18} />} version={value.version} status="本机已安装">
          <p className="setting-desc">{value.description}</p>
          <div className="skills-actions plugin-actions"><Button size="sm" variant="secondary" disabled={busy} icon={<FolderOpen size={14} />} onClick={() => { void install(value.source, value.requiredEnvironmentTools); }}>加载</Button></div>
        </PluginItem>)}
      </div>
    </details>
    {hasInstalled && <h3 className="plugin-list-heading">已安装插件</h3>}
    {hasInstalled && <div className="setting-card-group plugin-items">{modelPlugins}{plugins.map(plugin => <PluginItem key={plugin.id} name={plugin.name} icon={<Puzzle size={18} />} version={plugin.version}
      onToggle={open => setExpanded(current => ({ ...current, [plugin.id]: open }))}
      status={plugin.status === 'running' ? '运行中' : plugin.status === 'disabled' ? '已停用' : '启动失败'} variant={plugin.status === 'error' ? 'danger' : plugin.status === 'running' ? 'success' : 'neutral'}>
      <p className="setting-desc">{plugin.description}</p>
        <div className="plugin-controls">
          {!plugin.settingsInline && <Button size="sm" variant="subtle" icon={<Settings2 size={14} />} disabled={plugin.status !== 'running' || !plugin.frameUrl} onClick={() => openPluginSettings(plugin.id)}>插件设置</Button>}
          <div className="plugin-control-tools">
          <Button size="sm" variant="subtle" className="skill-icon-button" icon={<RefreshCw size={14} />} aria-label={`重新加载 ${plugin.name}`} title="重新加载插件" disabled={busy} onClick={() => { void run(() => setHostPluginEnabled(plugin.id, true), '插件已重新加载。').catch(() => {}); }} />
          <Button size="sm" variant="subtle" className="skill-icon-button btn-danger-hover" icon={<Trash2 size={14} />} aria-label={`卸载 ${plugin.name}`} title="卸载插件" disabled={busy} onClick={() => confirmAction({ title: `卸载 ${plugin.name}`, message: '停止插件并删除本应用中的插件文件和配置。本机原插件保留。', confirmLabel: '卸载', onConfirm: () => run(() => uninstallHostPlugin(plugin.id), '插件已卸载。') })} />
          </div>
          <Switch className="plugin-control-start" checked={plugin.enabled} disabled={busy} aria-label={`启用 ${plugin.name}`} onChange={enabled => { void run(() => setHostPluginEnabled(plugin.id, enabled), enabled ? '插件已启用。' : '插件已停用。').catch(() => {}); }} />
        </div>
        {hasPluginUpdate(plugin.version, updates[plugin.id]?.version) && updates[plugin.id]?.source && <div className="plugin-links">
          <Button size="sm" variant="secondary" disabled={busy} onClick={() => { void install(updates[plugin.id].source!, plugin.requiredEnvironmentTools, '插件已更新。'); }}>更新 v{updates[plugin.id].version}</Button>
        </div>}
        {updates[plugin.id]?.error && <p className="setting-desc" title={updates[plugin.id].error!}>更新检查失败，下次打开时重试</p>}
        {plugin.message && <p className="setting-desc" role="alert">{plugin.message}</p>}
        {plugin.settingsInline && plugin.status === 'running' && plugin.frameUrl && expanded[plugin.id]
          && <PluginSettingsFrame key={plugin.bridgeKey} plugin={plugin} />}
    </PluginItem>)}</div>}
  </>;
}
