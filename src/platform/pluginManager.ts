import { invoke } from './runtime';

export interface PluginEntry { id: string; protocol: 'dsh' | 'model-provider' }
let pendingList: Promise<PluginEntry[]> | undefined;
export function listPlugins<T extends PluginEntry>() {
  // Both settings sections and the overlay share an in-flight inventory read.
  pendingList ??= invoke<PluginEntry[]>('list_plugins').finally(() => { pendingList = undefined; });
  return pendingList as Promise<T[]>;
}
export const installPlugin = <T>(request: { id?: string; source?: string; requiredEnvironmentTools?: string[]; requestId?: string }) => invoke<T>('install_plugin', request);
export const setPluginEnabled = <T>(id: string, enabled: boolean) => invoke<T>('set_plugin_enabled', { id, enabled });
export const refreshPlugin = <T>(id: string) => invoke<T>('refresh_plugin', { id });
export const uninstallPlugin = <T>(id: string) => invoke<T>('uninstall_plugin', { id });
