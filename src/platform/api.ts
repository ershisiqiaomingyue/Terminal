import { invoke, openDirectory } from "@/platform/runtime";
import { notifyAgentProvidersChanged, ensureAgentProviderEventsBridged } from "./agentProviderEvents";
import { normalizeAgentProviders, type AgentProviderResponse } from "./agentProviders";
import { findModelCapability, resolveContextWindow, selectableReasoningEfforts, type ModelCapability, type ReasoningEffort } from "./modelCapabilities";

export interface SshConnection {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  privateKeyPath?: string | null;
  strictHostKey?: boolean;
  defaultRoot?: string | null;
}

export interface SshConnectionProgress {
  connectionId: string;
  taskId?: string | null;
  status: "disconnected" | "connecting" | "connected" | "failed" | string;
  progress: number;
  message: string;
  updatedAt: number;
  remoteStatus?: Record<string, unknown> | null;
  error?: string | null;
}

export interface WslProfile {
  id: string;
  name: string;
  distribution: string;
  defaultRoot?: string | null;
  shell: string;
}

export interface WslDistribution {
  name: string;
  version?: number | null;
  state?: string | null;
  isDefault: boolean;
}

export interface AgentModelSettings {
  iconBrand?: string | null;
  billing?: { multiplier?: number | null; badges: string[]; free: boolean; rateUnknown?: boolean } | null;
  protocol?: string | null;
  protocols?: string[];
  id: string;
  name: string;
  contextWindow?: number | null;
  /** 单轮输出上限（tokens）。留空表示沿用按上下文窗口推算的内置输出预算。 */
  maxOutputTokens?: number | null;
  supportsReasoning?: boolean | null;
  reasoningEfforts?: ReasoningEffort[];
  capabilitySource?: string | null;
}

export interface AgentProviderCheckInput {
  id: string;
  name: string;
  kind: string;
  brand?: string | null;
  baseUrl: string;
  model: string;
  apiKey?: string | null;
  inputPricePerMillion?: number | null;
  outputPricePerMillion?: number | null;
  currency?: string | null;
  quotaUrls?: AgentProvider["quotaUrls"];
  accessToken?: string | null;
}

export interface AgentConnectionView {
  id: string;
  name: string;
  status: {
    status: "not_configured" | "checking" | "connected" | "error" | string;
    message?: string;
  };
}

export interface AgentProvider {
  promptCacheRetention?: "default" | "extended";
  id: string;
  name: string;
  kind: string;
  brand?: string | null;
  baseUrl: string;
  model: string;
  contextWindow?: number | null;
  inputPricePerMillion?: number | null;
  outputPricePerMillion?: number | null;
  quotaUrls?: { balanceUrl?: string | null; spendingUrl?: string | null } | null;
  currency?: string | null;
  models: AgentModelSettings[];
}

export interface SkillRepository {
  path: string;
  status: string;
  message: string;
  skillCount: number;
  builtin: boolean;
}

export interface SkillCatalog {
  repositories: SkillRepository[];
  skills: SkillSummary[];
}

export interface SkillSummary {
  id: string; name: string; description: string; repository: string; path: string;
  builtin: boolean; enabled: boolean; runtime: string; status: string; statusMessage: string;
  entrypoint?: string | null; dependencies: string[];
}

export interface SkillRemotePreview {
  owner: string;
  name: string;
  branch: string;
  url: string;
  destination: string;
  skills: Array<{ sourcePath: string; id: string; name: string; description?: string; conflict: boolean }>;
}

export const SKILLS_CHANGED_EVENT = "android-tool-skills-changed";

export interface McpServerView {
  id: string;
  enabled: boolean;
  status: unknown;
  tools: unknown[];
  error?: string | null;
  lastCheckedAt?: number | null;
}

export interface EnvironmentToolStatus {
  id: string;
  name: string;
  description: string;
  localPath?: string | null;
  localVersion?: string | null;
  localStatus: "ready" | "available" | "missing" | string;
  localMessage: string;
  remoteVersion?: string | null;
  remoteMessage?: string | null;
  supportsDownload: boolean;
  selectionHint: string;
}

export interface EnvironmentToolDownloadProgress {
  toolId: string;
  phase: "resolving" | "downloading" | "installing" | "completed" | string;
  downloadedBytes: number;
  totalBytes?: number | null;
}

export interface EnvironmentNetworkSettings {
  proxyUrl?: string | null;
  proxyEnabled: boolean;
  proxyChildProcesses: boolean;
}

export const listSshConnections = () => invoke<SshConnection[]>("list_ssh_connections");

export const saveSshConnection = (config: SshConnection, password?: string) =>
  invoke<SshConnection>("save_ssh_connection", { config, password: password || null });

export const deleteSshConnection = (id: string) => invoke<void>("delete_ssh_connection", { id });
export const countActiveSshTerminals = (id: string) => invoke<number>("count_active_ssh_terminals", { id });

export const testSshConnection = (id: string) => invoke<void>("test_ssh_connection_command", { id });

export const testSshConnectionConfig = (config: SshConnection, password?: string) =>
  invoke<void>("test_ssh_connection_config", { config, password: password || null });

export const startSshConnection = (id: string, path?: string) =>
  invoke<SshConnectionProgress>("start_ssh_connection", { id, path: path || null });

export const getSshConnectionProgress = (id: string) =>
  invoke<SshConnectionProgress>("get_ssh_connection_progress", { id });

export const disconnectSshConnection = (id: string) =>
  invoke<SshConnectionProgress>("disconnect_ssh_connection_command", { id });

export const listWslDistributions = () => invoke<WslDistribution[]>("list_wsl_distributions");

export const listWslProfiles = () => invoke<WslProfile[]>("list_wsl_profiles");

export const saveWslProfile = (profile: WslProfile) => invoke<WslProfile>("save_wsl_profile", { profile });

export const deleteWslProfile = (id: string) => invoke<void>("delete_wsl_profile", { id });

export const testWslProfile = (id: string) => invoke<void>("test_wsl_profile", { id });

export const listAgentProviders = async () => {
  ensureAgentProviderEventsBridged();
  const [response, capabilities] = await Promise.all([
    invoke<AgentProviderResponse[]>("list_agent_providers"),
    invoke<ModelCapability[]>("get_agent_model_capabilities"),
  ]);
  return normalizeAgentProviders(response).map((provider) => ({ ...provider,
    models: provider.models.map((model) => {
      if (provider.id.startsWith("plugin:")) return model;
      const capability = findModelCapability(capabilities, model.id, provider.brand);
      // 与后端 resolved_model_context_window 同一条优先级（含 provider 层级的适用规则）：
      // 只暴露后端真正会用的窗口，避免面板显示一个运行时不会采用的数字。
      const applicableProviderWindow = (provider.models.filter((entry) => entry.id.trim()).length <= 1
        || model.id.trim() === provider.model.trim())
        ? provider.contextWindow
        : null;
      const contextWindow = resolveContextWindow(model.contextWindow ?? applicableProviderWindow, capability?.contextWindow);
      if (!capability) return contextWindow === model.contextWindow ? model : { ...model, contextWindow };
      return { ...model, contextWindow,
        supportsReasoning: capability.supportsReasoning ?? model.supportsReasoning,
        reasoningEfforts: selectableReasoningEfforts(capability, model.protocol || provider.kind),
        capabilitySource: capability.source };
    }),
  }));
};

export const saveAgentProvider = async (provider: {
  promptCacheRetention?: "default" | "extended";
  id: string;
  name: string;
  kind: string;
  brand?: string;
  baseUrl: string;
  model: string;
  contextWindow?: number;
  apiKey?: string;
  inputPricePerMillion?: number;
  outputPricePerMillion?: number;
  quotaUrls?: AgentProvider["quotaUrls"];
  usageAccessToken?: string;
  currency?: string;
  models?: AgentModelSettings[];
}) => {
  const saved = await invoke<AgentProvider>("save_agent_provider", provider);
  notifyAgentProvidersChanged();
  return saved;
};

export const deleteAgentProvider = async (id: string) => {
  await invoke<void>("delete_agent_provider", { id });
  notifyAgentProvidersChanged();
};

export const getAgentProviderApiKey = (id: string) => invoke<string | null>("get_agent_provider_api_key", { id });

export const checkAgentProvider = (request: AgentProviderCheckInput) =>
  invoke<AgentConnectionView>("check_agent_provider", { request });

export const discoverAgentModels = (request: Omit<AgentProviderCheckInput, "model" | "inputPricePerMillion" | "outputPricePerMillion">) =>
  invoke<AgentModelSettings[]>("discover_agent_models", { request });

export const testAgentProviderChat = (request: {
  id: string;
  name: string;
  kind: string;
  baseUrl: string;
  model: string;
  apiKey?: string | null;
  message: string;
}) => invoke<{ text: string }>("test_agent_provider_chat", { request });

export const listSkills = () => invoke<SkillCatalog>("list_skills");

export const readSkillDetails = (skill: SkillSummary) =>
  invoke<string>("read_skill_details", { repository: skill.repository, path: skill.path });

export const setSkillEnabled = (skill: SkillSummary, enabled: boolean) =>
  invoke<void>("set_skill_enabled", { repository: skill.repository, path: skill.path, enabled }).then(() => {
    window.dispatchEvent(new Event(SKILLS_CHANGED_EVENT));
  });

export const deleteSkill = (skill: SkillSummary) =>
  invoke<void>("delete_skill", { repository: skill.repository, path: skill.path }).then(() => {
    window.dispatchEvent(new Event(SKILLS_CHANGED_EVENT));
  });

export const saveSkillRepositories = (repositories: string[]) =>
  invoke<SkillCatalog>("save_skill_repositories", { repositories }).then((catalog) => {
    window.dispatchEvent(new Event(SKILLS_CHANGED_EVENT));
    return catalog;
  });

export const previewSkillRepository = (url: string, branch: string, destination: string) =>
  invoke<SkillRemotePreview>("preview_skill_repository", {
    request: { url, branch, destination },
  });

export const importSkillRepository = (url: string, branch: string, destination: string,
  selections: Array<{ sourcePath: string; resolution: "skip" | "keep" | "overwrite" | "rename"; renamedId?: string }>) =>
  invoke<{ importedIds: string[]; skipped: string[] }>("import_skill_repository", {
    request: { url, branch, destination }, selections,
  }).then((result) => {
    window.dispatchEvent(new Event(SKILLS_CHANGED_EVENT));
    return result;
  });

export const listMcpServers = () => invoke<McpServerView[]>("list_mcp_servers");

export const connectMcpServer = (input: {
  id: string;
  transport: { transport: "stdio"; command: string; args: string[]; env: Record<string, string> } | { transport: "http"; url: string; headers: Record<string, string> };
  enabled: boolean;
  timeoutSecs: number;
}) => invoke<McpServerView>("connect_mcp_server", { input });

export const disconnectMcpServer = (serverId: string) => invoke<void>("disconnect_mcp_server", { serverId });

export const removeMcpServer = (serverId: string) => invoke<void>("remove_mcp_server", { serverId });

export const listEnvironmentTools = (includeRemote = true) =>
  invoke<EnvironmentToolStatus[]>("list_environment_tools", { request: { includeRemote } });

export const checkEnvironmentTool = (toolId: string) =>
  invoke<EnvironmentToolStatus>("check_environment_tool", { toolId });

export const saveEnvironmentToolPath = (toolId: string, path: string) =>
  invoke<EnvironmentToolStatus>("save_environment_tool_path", { request: { toolId, path } }).then((status) => {
    if (toolId === "python") window.dispatchEvent(new Event(SKILLS_CHANGED_EVENT));
    return status;
  });

export const downloadEnvironmentTool = (toolId: string) =>
  invoke<EnvironmentToolStatus>("download_environment_tool", { request: { toolId } }).then((status) => {
    if (toolId === "python") window.dispatchEvent(new Event(SKILLS_CHANGED_EVENT));
    return status;
  });

export const getEnvironmentNetworkSettings = () =>
  invoke<EnvironmentNetworkSettings>("get_environment_network_settings");

export const saveEnvironmentNetworkSettings = (request: EnvironmentNetworkSettings) =>
  invoke<EnvironmentNetworkSettings>("save_environment_network_settings", { request });

export interface LspServerView {
  id: string;
  command: string;
  args: string[];
  languages: Record<string, string>;
  filenames: Record<string, string>;
  disabled: boolean;
  /** Keys of the entry the app does not own; a save keeps them untouched. */
  extraFields: string[];
  status: "ready" | "missing" | "unlaunchable" | "disabled" | "invalid" | string;
  /** How the command was resolved: 绝对路径 / 工作区 node_modules/.bin / PATH. */
  source?: string | null;
  resolvedPath?: string | null;
  message: string;
}

export interface LspConfigView {
  configPath: string;
  /** True while the file does not exist yet: the list is the built-in default. */
  usingBundledDefault: boolean;
  /**
   * Set when the file exists but cannot be read or parsed. `servers` is empty
   * then and every save keeps failing, so the panel shows this and only
   * overwrites the file on an explicit「重置为默认」.
   */
  configError?: string | null;
  servers: LspServerView[];
}

export interface LspServerEdit {
  id: string;
  command: string;
  args: string[];
  languages: Record<string, string>;
}

/**
 * `workspace` is the directory whose `node_modules/.bin` is searched before
 * `PATH`; without it a project-local server cannot be discovered.
 */
export const listLspServers = (workspace?: string | null) =>
  invoke<LspConfigView>("list_lsp_servers", { workspace: workspace || null });

export const saveLspServer = (server: LspServerEdit, workspace?: string | null) =>
  invoke<LspConfigView>("save_lsp_server", { server, workspace: workspace || null });

/**
 * Overwrite the config file with the built-in default. Only the panel's
 * explicit「重置为默认」action calls it: a damaged file is never replaced
 * automatically, because that would discard whatever the user wrote.
 */
export const resetLspServers = (workspace?: string | null) =>
  invoke<LspConfigView>("reset_lsp_servers", { workspace: workspace || null });

export const chooseDirectory = (title: string) => openDirectory(title);
