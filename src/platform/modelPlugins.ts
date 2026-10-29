import { invoke } from "./runtime";
import { listPlugins, installPlugin, refreshPlugin, setPluginEnabled, uninstallPlugin } from './pluginManager';

export interface ModelPluginAccount {
  providerId?: string;
  reasoningModels?: ModelPluginReasoningModel[];
  id: string;
  name: string;
  state: "signed-in" | "signed-out" | "error";
  account: string;
  modelCount: number;
  credits: number | null;
  unlimited: boolean;
  message: string;
  creditError: string;
}
export interface ModelPluginReasoningModel {
  id: string;
  name: string;
  iconBrand: string;
  supportsReasoning: boolean;
  efforts: string[];
  source: "catalog" | "probe" | "unknown";
  validation: "validating" | "non-validating" | "unknown" | "";
  probedAt: number | null;
  canProbe: boolean;
  probing: boolean;
  message: string;
}
export interface ModelPlugin {
  id: string;
  name: string;
  description: string;
  source: string;
  installedVersion: string | null;
  availableVersion: string;
  enabled: boolean;
  status: "not-installed" | "installing" | "running" | "disabled" | "error";
  message: string;
  authFileCn: string;
  authFileAi: string;
  electronFileCn: string;
  electronFileAi: string;
  iconBrand: string;
  advancedHint: string;
  configFields: { key: string; label: string }[];
  supportsReasoningProbe: boolean;
  accounts: ModelPluginAccount[];
  requiredEnvironmentTools: string[];
}
export const listModelPlugins = () => listPlugins<ModelPlugin & { protocol: 'model-provider' }>().then(plugins => plugins.filter(plugin => plugin.protocol === 'model-provider'));
export const installModelPlugin = (id: string) => installPlugin<ModelPlugin>({ id });
export const refreshModelPlugin = (id: string) => refreshPlugin<ModelPlugin>(id);
export const probeModelPlugin = (providerId: string, modelId: string) => invoke<ModelPlugin>("probe_model_plugin", { providerId, modelId });
export const setModelPluginEnabled = (id: string, enabled: boolean) => setPluginEnabled<ModelPlugin>(id, enabled);
export const uninstallModelPlugin = (id: string) => uninstallPlugin<ModelPlugin>(id);
export const configureModelPlugin = (id: string, authFileCn: string, authFileAi: string, electronFileCn = "", electronFileAi = "") =>
  invoke<ModelPlugin>("configure_model_plugin", { id, authFileCn, authFileAi, electronFileCn, electronFileAi });
