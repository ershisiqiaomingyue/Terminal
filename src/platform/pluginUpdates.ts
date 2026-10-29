import { invoke } from './runtime';

export interface PluginUpdate {
  id: string;
  version: string | null;
  source: string | null;
  error: string | null;
}
export const checkPluginUpdates = () => invoke<PluginUpdate[]>('check_plugin_updates');

export { hasPluginUpdate } from './pluginVersion';