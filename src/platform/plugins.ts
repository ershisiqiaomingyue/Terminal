import { invoke } from './runtime';
import { listPlugins, installPlugin, setPluginEnabled, uninstallPlugin } from './pluginManager';

export interface HostPlugin {
  id: string;
  name: string;
  source: string;
  version: string;
  description: string;
  enabled: boolean;
  status: 'running' | 'disabled' | 'error';
  message: string;
  requiredEnvironmentTools: string[];
  frameUrl: string | null;
  bridgeKey: string | null;
  settingsInline?: boolean;
}
export interface LocalPlugin {
  name: string;
  source: string;
  version: string;
  description: string;
  requiredEnvironmentTools: string[];
}
export const listHostPlugins = () => listPlugins<HostPlugin & { protocol: 'dsh' }>().then(plugins => plugins.filter(plugin => plugin.protocol === 'dsh'));
export const discoverLocalPlugins = () => invoke<LocalPlugin[]>('discover_local_plugins');
export const inspectHostPlugin = (source: string) => invoke<LocalPlugin>('inspect_host_plugin', { source });
export interface PluginInstallProgress {
  requestId: string;
  phase: string;
  downloadedBytes: number;
  totalBytes?: number | null;
  bytesPerSecond: number;
  completedRequests?: number;
  activeRequests?: number;
}
export const installHostPlugin = (source: string, requiredEnvironmentTools = ['node'], requestId?: string) =>
  installPlugin<HostPlugin>({ source, requiredEnvironmentTools, requestId });
export const setHostPluginEnabled = (id: string, enabled: boolean) => setPluginEnabled<HostPlugin>(id, enabled);
export const uninstallHostPlugin = (id: string) => uninstallPlugin<void>(id);
export function openPluginSettings(id: string) {
  window.dispatchEvent(new CustomEvent('android-tool-plugin-settings', { detail: { id } }));
}
