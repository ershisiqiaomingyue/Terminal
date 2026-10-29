import React, { useEffect, useMemo, useRef, useState } from "react";
import {
  Eye,
  EyeOff,
  ChevronDown,
  ChevronRight,
  RefreshCw,
  Plug,
  Save,
  Trash2,
  Pencil,
  Unplug,
  Check,
  Eraser,
} from "lucide-react";
import { Switch, Select, Input, Button, Badge, Card } from "@/components/ui";
import { ProviderIcon } from "@/components/ProviderIcon";
import { ConfirmDialog } from "@/components/ConfirmDialog/ConfirmDialog";
import {
  checkAgentProvider,
  connectMcpServer,
  deleteAgentProvider,
  disconnectMcpServer,
  discoverAgentModels,
  getAgentProviderApiKey,
  listAgentProviders,
  listMcpServers,
  saveAgentProvider,
  type AgentModelSettings,
  type AgentProvider,
  type AgentProviderCheckInput,
} from "@/platform/api";
import { formatInvokeError, isTauri } from "@/platform/runtime";
import { getProviderQuota, type AgentQuota } from "@/platform/chatApi";
import { formatQuotaBalance, formatQuotaSpent } from "@/platform/providerQuota";
import { useSettingsNotification } from "./SettingsNotification";
import { modelsForProviderProtocol } from "./providerProtocol";
import { formatContextWindow, parseTokenCount, tokenCountInputValue } from "./modelWindowFormat";
import { isDeclaredContextWindow } from "@/platform/modelCapabilities";
import { AgentThinkingSettings } from "./AgentThinkingSettings";
import { AgentRuntimeSettings } from "./AgentRuntimeSettings";
import { ProviderEditDialog } from "./ProviderEditDialog";
import { ModelDiscoveryDialog } from "./ModelDiscoveryDialog";

interface PresetItem {
  label: string;
  brand: string;
  id: string;
  name: string;
  kind: string;
  baseUrl: string;
}

const PRESETS: PresetItem[] = [
  { label: "GPT / OpenAI", brand: "openai", id: "openai", name: "OpenAI / GPT", kind: "open_ai_responses", baseUrl: "https://api.openai.com/v1" },
  { label: "Grok", brand: "grok", id: "grok", name: "Grok / xAI", kind: "open_ai_responses", baseUrl: "https://api.x.ai/v1" },
  { label: "Claude", brand: "anthropic", id: "anthropic", name: "Anthropic / Claude", kind: "anthropic", baseUrl: "https://api.anthropic.com/v1" },
  { label: "DeepSeek", brand: "deepseek", id: "deepseek", name: "DeepSeek", kind: "anthropic", baseUrl: "https://api.deepseek.com/anthropic" },
  { label: "Kimi", brand: "kimi", id: "kimi", name: "Kimi / Moonshot", kind: "anthropic", baseUrl: "https://api.moonshot.cn/anthropic" },
  { label: "GLM", brand: "glm", id: "glm", name: "GLM / 智谱", kind: "anthropic", baseUrl: "https://open.bigmodel.cn/api/anthropic" },
  { label: "Qwen", brand: "qwen", id: "qwen", name: "Qwen / 通义千问", kind: "anthropic", baseUrl: "https://token-plan.cn-beijing.maas.aliyuncs.com/apps/anthropic" },
  { label: "MiMo", brand: "mimo", id: "mimo", name: "MiMo", kind: "anthropic", baseUrl: "https://api.xiaomimimo.com/anthropic" },
  { label: "其他", brand: "custom", id: "custom-provider", name: "自定义 provider", kind: "open_ai_compatible", baseUrl: "" },
];

function mergeModels(...groups: AgentModelSettings[][]): AgentModelSettings[] {
  const unique = new Map<string, AgentModelSettings>();
  groups.flat().forEach((model) => {
    const id = model.id.trim();
    if (!id || unique.has(id)) return;
    unique.set(id, { ...model, id, name: model.name.trim() || id });
  });
  return [...unique.values()];
}

function connectionNotice(status: { status: string; message?: string }) {
  const message = status.message || "未返回状态说明";
  return status.status === "connected"
    ? { message: `连接测试通过：${message}`, kind: "success" as const }
    : { message: `连接测试失败：${message}`, kind: "error" as const };
}

function priceValue(value: string): number | undefined {
  const price = value.trim() ? Number(value) : NaN;
  return Number.isFinite(price) && price >= 0 ? price : undefined;
}

export const AgentPanel: React.FC = () => {
  const notify = useSettingsNotification();
  const [selectedBrand, setSelectedBrand] = useState("deepseek");
  const [providerId, setProviderId] = useState("deepseek");
  const [providerName, setProviderName] = useState("DeepSeek");
  const [providerKind, setProviderKind] = useState("anthropic");
  const [providerCacheRetention, setProviderCacheRetention] = useState<"default" | "extended">("default");
  const [providerBaseUrl, setProviderBaseUrl] = useState("https://api.deepseek.com/anthropic");
  const [providerApiKey, setProviderApiKey] = useState("");
  const [providerApiKeyVisible, setProviderApiKeyVisible] = useState(false);
  const [loadingApiKey, setLoadingApiKey] = useState(false);
  const apiKeyReadRevision = useRef(0);
  const [providerModel, setProviderModel] = useState("");
  const [providerModels, setProviderModels] = useState<AgentModelSettings[]>([]);
  const [manualModelInput, setManualModelInput] = useState("");
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [providerContextWindow, setProviderContextWindow] = useState("");
  const [providerInputPrice, setProviderInputPrice] = useState("1.00");
  const [providerOutputPrice, setProviderOutputPrice] = useState("2.00");
  const [providerBalanceUrl, setProviderBalanceUrl] = useState("");
  const [providerSpendingUrl, setProviderSpendingUrl] = useState("");
  const [providerUsageToken, setProviderUsageToken] = useState("");
  const [providerCurrency, setProviderCurrency] = useState("");
  const [queryingQuota, setQueryingQuota] = useState(false);
  const [queriedQuota, setQueriedQuota] = useState<{ key: string; value: AgentQuota } | null>(null);
  const quotaQueryRevision = useRef(0);
  useEffect(() => () => { quotaQueryRevision.current += 1; }, []);
  const quotaQueryKey = JSON.stringify([providerId, providerKind, selectedBrand, providerBaseUrl, providerApiKey, providerBalanceUrl, providerSpendingUrl, providerUsageToken, providerCurrency]);
  const [editingProviderId, setEditingProviderId] = useState<string | null>(null);
  const [savingProvider, setSavingProvider] = useState(false);
  const providerFormRef = useRef<HTMLDivElement>(null);
  const [providerFormHeight, setProviderFormHeight] = useState(0);
  const readDraft = () => ({ selectedBrand, providerId, providerName, providerKind, providerBaseUrl, providerCacheRetention, providerApiKey, providerApiKeyVisible, providerModel, providerModels, manualModelInput, advancedOpen, providerContextWindow, providerInputPrice, providerOutputPrice, providerBalanceUrl, providerSpendingUrl, providerUsageToken, providerCurrency });
  const draftBeforeEdit = useRef<ReturnType<typeof readDraft> | null>(null);

  const [savedProviders, setSavedProviders] = useState<AgentProvider[]>([]);
  const [pendingDeletion, setPendingDeletion] = useState<AgentProvider | null>(null);
  const [deletingProviderId, setDeletingProviderId] = useState<string | null>(null);
  const [discoveringModels, setDiscoveringModels] = useState(false);
  const [discoveredModels, setDiscoveredModels] = useState<AgentModelSettings[] | null>(null);
  const [testingProviderId, setTestingProviderId] = useState<string | null>(null);
  const [expandedProviderModels, setExpandedProviderModels] = useState<Record<string, boolean>>({});
  const [switchingDefaultModel, setSwitchingDefaultModel] = useState<{ providerId: string; modelId: string } | null>(null);

  const [mcpTransport, setMcpTransport] = useState<"stdio" | "http">("stdio");
  const [mcpId, setMcpId] = useState("local-tools");
  const [mcpCommand, setMcpCommand] = useState("npx");
  const [mcpArgs, setMcpArgs] = useState("-y @modelcontextprotocol/server-android-tools");
  const [mcpUrl, setMcpUrl] = useState("http://localhost:8000/mcp");
  const [mcpServers, setMcpServers] = useState<Array<{ id: string; status: string; toolsCount: number; enabled: boolean }>>([]);

  const reloadProviders = async () => {
    if (!isTauri()) return;
    const [providers, servers] = await Promise.all([listAgentProviders(), listMcpServers()]);
    setSavedProviders(providers.filter((provider) => !provider.id.startsWith("plugin:")));
    setMcpServers(servers.map((server) => {
      const health = server.status && typeof server.status === "object" && "status" in server.status
        ? String((server.status as { status?: unknown }).status || "")
        : "";
      const status = server.error
        ? "连接失败"
        : health === "connected"
          ? "已连接"
          : health === "checking"
            ? "正在检查"
            : server.enabled ? "未连接" : "已停用";
      return { id: server.id, status, toolsCount: server.tools.length, enabled: server.enabled };
    }));
  };

  useEffect(() => {
    void reloadProviders().catch((reason) => notify(formatInvokeError(reason), "error"));
  }, []);

  const selectableModels = useMemo(
    () => mergeModels(providerModels, providerModel ? [{ id: providerModel, name: providerModel }] : []),
    [providerModel, providerModels],
  );

  // 当前选中模型的内置/目录窗口（已按后端优先级解析），用于说明输入框留空时真正生效的值。
  const selectedModelWindow = useMemo(
    () => providerModels.find((entry) => entry.id === providerModel.trim())?.contextWindow ?? null,
    [providerModel, providerModels],
  );

  const closeProviderEditor = () => {
    setDiscoveredModels(null);
    apiKeyReadRevision.current += 1;
    setLoadingApiKey(false);
    const draft = draftBeforeEdit.current;
    if (draft) {
      setSelectedBrand(draft.selectedBrand);
      setProviderId(draft.providerId);
      setProviderName(draft.providerName);
      setProviderKind(draft.providerKind);
      setProviderBaseUrl(draft.providerBaseUrl);
      setProviderCacheRetention(draft.providerCacheRetention);
      setProviderApiKey(draft.providerApiKey);
      setProviderApiKeyVisible(draft.providerApiKeyVisible);
      setProviderModel(draft.providerModel);
      setProviderModels(draft.providerModels);
      setManualModelInput(draft.manualModelInput);
      setAdvancedOpen(draft.advancedOpen);
      setProviderContextWindow(draft.providerContextWindow);
      setProviderInputPrice(draft.providerInputPrice);
      setProviderOutputPrice(draft.providerOutputPrice);
      setProviderBalanceUrl(draft.providerBalanceUrl);
      setProviderSpendingUrl(draft.providerSpendingUrl);
      setProviderUsageToken(draft.providerUsageToken);
      setProviderCurrency(draft.providerCurrency);
    }
    draftBeforeEdit.current = null;
    setEditingProviderId(null);
  };

  const editProvider = (provider: AgentProvider, model = provider.model) => {
    setProviderFormHeight(providerFormRef.current?.getBoundingClientRect().height || 0);
    apiKeyReadRevision.current += 1;
    setLoadingApiKey(false);
    draftBeforeEdit.current = readDraft();
    setEditingProviderId(provider.id);
    setProviderId(provider.id);
    setProviderName(provider.name);
    setProviderKind(provider.kind);
    setProviderBaseUrl(provider.baseUrl);
    setProviderCacheRetention(provider.promptCacheRetention || "default");
    setProviderModel(model);
    setSelectedBrand(provider.brand || "custom");
    // 只回填用户真正声明过的窗口：面板预填值（65,536）不是声明，展示它只会和模型行的内置窗口自相矛盾。
    setProviderContextWindow(isDeclaredContextWindow(provider.contextWindow) ? String(provider.contextWindow) : "");
    setProviderInputPrice(String(provider.inputPricePerMillion ?? ""));
    setProviderOutputPrice(String(provider.outputPricePerMillion ?? ""));
    setProviderModels(mergeModels(provider.models, model ? [{ id: model, name: model }] : []));
    setProviderBalanceUrl(provider.quotaUrls?.balanceUrl || "");
    setProviderSpendingUrl(provider.quotaUrls?.spendingUrl || "");
    setProviderUsageToken("");
    setProviderCurrency(provider.currency || "");
    setQueriedQuota(null);
    setProviderApiKey("");
    setProviderApiKeyVisible(false);
    setManualModelInput("");
    setAdvancedOpen(false);
  };

  useEffect(() => () => { apiKeyReadRevision.current += 1; }, []);

  const toggleApiKeyVisibility = async () => {
    if (loadingApiKey) return;
    if (providerApiKeyVisible || providerApiKey || !editingProviderId) {
      setProviderApiKeyVisible((visible) => !visible);
      return;
    }
    const revision = ++apiKeyReadRevision.current;
    setLoadingApiKey(true);
    try {
      const key = await getAgentProviderApiKey(editingProviderId);
      if (apiKeyReadRevision.current !== revision) return;
      if (!key) {
        notify("该供应商没有已保存的 API Key。", "info");
        return;
      }
      setProviderApiKey(key);
      setProviderApiKeyVisible(true);
    } catch (reason) {
      if (apiKeyReadRevision.current === revision) notify(formatInvokeError(reason), "error");
    } finally {
      if (apiKeyReadRevision.current === revision) setLoadingApiKey(false);
    }
  };

  const confirmProviderDeletion = async () => {
    if (!pendingDeletion || deletingProviderId) return;
    const provider = pendingDeletion;
    setPendingDeletion(null);
    setDeletingProviderId(provider.id);
    try {
      await deleteAgentProvider(provider.id);
      await reloadProviders();
      notify(`供应商「${provider.name}」已删除。`, "success");
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    } finally {
      setDeletingProviderId(null);
    }
  };

  const buildProviderCheckRequest = (): AgentProviderCheckInput => ({
    id: providerId.trim(),
    name: providerName.trim(),
    kind: providerKind,
    brand: selectedBrand,
    baseUrl: providerBaseUrl.trim(),
    model: providerModel.trim(),
    apiKey: providerApiKey.trim() || undefined,
    inputPricePerMillion: priceValue(providerInputPrice),
    outputPricePerMillion: priceValue(providerOutputPrice),
    currency: providerCurrency.trim() || null,
  });

  const ensureProviderDraft = (requireModel = true) => {
    const draft = buildProviderCheckRequest();
    if (!draft.id || !draft.name || !draft.baseUrl || (requireModel && !draft.model)) {
      const message = requireModel ? "请先填写配置 ID、名称、Base URL 和默认模型。" : "请先填写配置 ID、名称和 Base URL。";
      notify(message, "error");
      return null;
    }
    return draft;
  };

  const testProvider = async (draft: AgentProviderCheckInput, loadingId: string) => {
    setTestingProviderId(loadingId);
    try {
      const result = await checkAgentProvider(draft);
      const notice = connectionNotice(result.status);
      notify(notice.message, notice.kind);
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    } finally {
      setTestingProviderId((current) => current === loadingId ? null : current);
    }
  };

  const testDraftProvider = async () => {
    const draft = ensureProviderDraft();
    if (draft) await testProvider(draft, "draft");
  };

  const queryDraftQuota = async () => {
    if (queryingQuota) return;
    const revision = ++quotaQueryRevision.current;
    const key = quotaQueryKey;
    setQueryingQuota(true);
    setQueriedQuota(null);
    try {
      if (!isTauri()) throw new Error("请在桌面应用中查询供应商余额。");
      if (!providerId.trim() || !providerBaseUrl.trim()) throw new Error("请填写配置 ID 和 Base URL。");
      const value = await getProviderQuota({ ...buildProviderCheckRequest(),
        quotaUrls: { balanceUrl: providerBalanceUrl.trim() || null, spendingUrl: providerSpendingUrl.trim() || null },
        accessToken: providerUsageToken.trim() || null,
      });
      if (quotaQueryRevision.current === revision) {
        setQueriedQuota({ key, value });
        notify(value.message || (value.status === "available" ? "供应商余额已更新。" : "供应商余额暂不可用。"), value.status === "available" ? "success" : value.status === "error" ? "error" : "info");
      }
    } catch (reason) {
      if (quotaQueryRevision.current === revision) notify(formatInvokeError(reason), "error");
    } finally {
      if (quotaQueryRevision.current === revision) setQueryingQuota(false);
    }
  };

  const testSavedProvider = async (provider: AgentProvider) => {
    await testProvider({
      id: provider.id,
      name: provider.name,
      kind: provider.kind,
      brand: provider.brand,
      baseUrl: provider.baseUrl,
      model: provider.model,
      inputPricePerMillion: provider.inputPricePerMillion,
      outputPricePerMillion: provider.outputPricePerMillion,
    }, provider.id);
  };

  const switchDefaultModel = async (provider: AgentProvider, model: AgentModelSettings) => {
    if (switchingDefaultModel || provider.model === model.id) return;
    setSwitchingDefaultModel({ providerId: provider.id, modelId: model.id });
    try {
      await saveAgentProvider({
        id: provider.id,
        name: provider.name,
        kind: provider.kind,
        brand: provider.brand ?? undefined,
        baseUrl: provider.baseUrl,
        model: model.id,
        models: mergeModels(provider.models, provider.model ? [{ id: provider.model, name: provider.model }] : []),
        contextWindow: provider.contextWindow ?? undefined,
        inputPricePerMillion: provider.inputPricePerMillion ?? undefined,
        outputPricePerMillion: provider.outputPricePerMillion ?? undefined,
        quotaUrls: provider.quotaUrls,
        currency: provider.currency ?? undefined,
        promptCacheRetention: provider.promptCacheRetention,
      });
      setSavedProviders((current) => current.map((entry) => entry.id === provider.id ? { ...entry, model: model.id } : entry));
      notify(`默认模型已切换为 ${model.name}。`, "success");
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    } finally {
      setSwitchingDefaultModel(null);
    }
  };

  // 从默认模型下拉框直接输入 ID 添加，不弹窗；添加后立即设为默认。
  const addManualModel = () => {
    const next = manualModelInput.trim();
    if (!next) return;
    if (providerModels.some((model) => model.id === next)) {
      setProviderModel(next);
      setManualModelInput("");
      notify(`模型 ${next} 已在目录中，已设为默认。`, "info");
      return;
    }
    setProviderModels((models) => mergeModels(models, [{ id: next, name: next }]));
    setProviderModel(next);
    setManualModelInput("");
    notify(`已添加模型 ${next} 并设为默认。`, "success");
  };

  // 清空弹窗内已插入的模型目录，重新发现后可干净重选。
  // 只影响当前草稿状态，不写磁盘；保存动作仍然由用户主动触发。
  const clearProviderModels = async () => {
    if (!providerModels.length) return;
    setProviderModels([]);
    setProviderModel("");
    notify(`已清空模型目录（${providerModels.length} 项），可重新发现模型后重选。`, "success");
  };

  const discoverModels = async () => {
    const draft = ensureProviderDraft(false);
    if (!draft) return;
    setDiscoveringModels(true);
    try {
      const models = await discoverAgentModels({
        id: draft.id,
        name: draft.name,
        kind: draft.kind,
        brand: draft.brand,
        baseUrl: draft.baseUrl,
        apiKey: draft.apiKey,
      });
      if (models.length) setDiscoveredModels(mergeModels(models));
      else notify("Provider 未公开模型列表；可继续手动填写模型 ID。", "info");
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    } finally {
      setDiscoveringModels(false);
    }
  };

  const saveProvider = async () => {
    if (savingProvider) return;
    if (!ensureProviderDraft()) return;
    const ids = providerModels.map((model) => model.id.trim());
    if (ids.some((id) => !id) || new Set(ids).size !== ids.length) {
      notify("模型 ID 不能为空或重复。", "error");
      return;
    }
    setSavingProvider(true);
    try {
      await saveAgentProvider({
        promptCacheRetention: providerCacheRetention,
        id: providerId.trim(),
        name: providerName.trim(),
        kind: providerKind,
        brand: selectedBrand,
        baseUrl: providerBaseUrl.trim(),
        model: providerModel.trim(),
        contextWindow: Number(providerContextWindow) || undefined,
        apiKey: providerApiKey.trim() || undefined,
        inputPricePerMillion: priceValue(providerInputPrice),
        outputPricePerMillion: priceValue(providerOutputPrice),
        currency: providerCurrency.trim() || undefined,
        quotaUrls: { balanceUrl: providerBalanceUrl.trim() || null, spendingUrl: providerSpendingUrl.trim() || null },
        usageAccessToken: providerUsageToken.trim() || undefined,
        models: modelsForProviderProtocol(mergeModels(providerModels, providerModel ? [{ id: providerModel, name: providerModel }] : []), providerKind),
      });
      setProviderApiKey("");
      setProviderUsageToken("");
      await reloadProviders();
      notify("Provider 已保存到本地。", "success");
      if (editingProviderId) closeProviderEditor();
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    } finally {
      setSavingProvider(false);
    }
  };

  const connectMcp = async () => {
    try {
      await connectMcpServer({
        id: mcpId.trim(),
        enabled: true,
        timeoutSecs: 30,
        transport: mcpTransport === "stdio"
          ? { transport: "stdio", command: mcpCommand.trim(), args: mcpArgs.split(/\s+/).filter(Boolean), env: {} }
          : { transport: "http", url: mcpUrl.trim(), headers: {} },
      });
      await reloadProviders();
      notify("MCP 服务已连接。", "success");
    } catch (reason) {
      notify(formatInvokeError(reason), "error");
    }
  };

  const handleApplyPreset = (preset: PresetItem) => {
    apiKeyReadRevision.current += 1;
    setLoadingApiKey(false);
    setProviderApiKey("");
    setProviderApiKeyVisible(false);
    setSelectedBrand(preset.brand);
    setProviderId(preset.id);
    setProviderName(preset.name);
    setProviderKind(preset.kind);
    setProviderBaseUrl(preset.baseUrl);
    setProviderCacheRetention("default");
    setProviderModel("");
    setProviderModels([]);
    setManualModelInput("");
    setProviderBalanceUrl("");
    setProviderSpendingUrl("");
    setProviderUsageToken("");
    setProviderCurrency("");
    setQueriedQuota(null);
  };

  const providerEditorBusy = savingProvider || discoveringModels || testingProviderId === "draft" || queryingQuota || loadingApiKey || Boolean(switchingDefaultModel);
  const quotaResult = queriedQuota?.key === quotaQueryKey ? queriedQuota : null;
  const providerForm = (
      <div>
        {!editingProviderId && <div className="setting-group-title">供应商配置 / provider 配置</div>}
        <fieldset className="setting-card-group provider-config-form" disabled={providerEditorBusy}>
          {!editingProviderId && <div className="preset-chips-row">
            {PRESETS.map((preset) => (
              <button
                key={preset.id}
                type="button"
                className={`preset-chip ${selectedBrand === preset.brand ? "active" : ""}`}
                data-brand={preset.brand}
                onClick={() => handleApplyPreset(preset)}
              >
                <ProviderIcon brand={preset.brand} size={16} />
                <span>{preset.label}</span>
              </button>
            ))}
          </div>}

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">配置 ID</span>
              <span className="setting-desc">provider 唯一标识</span>
            </div>
            <div className="setting-control-block">
              <Input
                size="sm"
                value={providerId}
                disabled={Boolean(editingProviderId)}
                aria-label="配置 ID"
                onChange={(e) => setProviderId(e.target.value)}
                placeholder="default-provider"
                style={{ width: "100%" }}
              />
            </div>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">显示名称</span>
              <span className="setting-desc">在界面上显示的名称</span>
            </div>
            <div className="setting-control-block">
              <Input
                size="sm"
                value={providerName}
                aria-label="显示名称"
                onChange={(e) => setProviderName(e.target.value)}
                placeholder="我的 provider"
                style={{ width: "100%" }}
              />
            </div>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">传输协议</span>
              <span className="setting-desc">API 访问协议规范</span>
            </div>
            <div className="setting-control-block">
              <Select
                size="sm"
                value={providerKind}
                onChange={(val) => setProviderKind(val as string)}
                options={[
                  { label: "OpenAI 兼容", value: "open_ai_compatible" },
                  { label: "OpenAI Chat Completions", value: "open_ai" },
                  { label: "OpenAI Responses", value: "open_ai_responses" },
                  { label: "Anthropic", value: "anthropic" },
                ]}
                style={{ width: "100%" }}
              />
            </div>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">Base URL</span>
              <span className="setting-desc">API 基础访问地址</span>
            </div>
            <div className="setting-control-block">
              <Input
                size="sm"
                value={providerBaseUrl}
                aria-label="Base URL"
                onChange={(e) => setProviderBaseUrl(e.target.value)}
                placeholder="https://api.openai.com/v1"
                style={{ width: "100%" }}
              />
            </div>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">API Key</span>
              <span className="setting-desc">加密保存到本地安全凭据库 (留空沿用系统凭据)</span>
            </div>
            <div className="setting-control-block">
              <Input
                size="sm"
                type={providerApiKeyVisible ? "text" : "password"}
                value={providerApiKey}
                aria-label="API Key"
                autoComplete="off"
                onChange={(e) => setProviderApiKey(e.target.value)}
                placeholder={editingProviderId ? "********" : "保存到系统凭据库"}
                style={{ flex: 1 }}
                suffixAction={
                  <button
                    type="button"
                    className="input-clear-pill"
                    onClick={() => void toggleApiKeyVisibility()}
                    title={providerApiKeyVisible ? "隐藏 API Key" : "显示 API Key"}
                    aria-label={providerApiKeyVisible ? "隐藏 API Key" : "显示 API Key"}
                    aria-pressed={providerApiKeyVisible}
                  >
                    {providerApiKeyVisible ? <EyeOff size={13} /> : <Eye size={13} />}
                  </button>
                }
              />
            </div>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">默认模型</span>
              <span className="setting-desc">当前 provider 默认使用的模型</span>
            </div>
            <div className="setting-control-block provider-model-setting">
              <Select
                size="sm"
                value={providerModel}
                searchable
                searchPlaceholder="搜索模型名称或 ID"
                placeholder="请先发现模型或手动添加"
                disabled={providerEditorBusy}
                onChange={(val) => setProviderModel(val as string)}
                options={selectableModels.map((model) => ({
                  label: model.name,
                  value: model.id,
                  // Single-row layout: the window badge rides on the right edge
                  // instead of forcing a second description line.
                  trailing: model.contextWindow && model.contextWindow > 0
                    ? formatContextWindow(model.contextWindow)
                    : "未返回",
                  onRemove: model.id === providerModel ? undefined : () => {
                    setProviderModels((models) => models.filter((entry) => entry.id !== model.id));
                  },
                  removeLabel: `从目录移除 ${model.name}`,
                }))}
                keepOpenOnSelect
                footer={
                  <div className="select-inline-add">
                    <Input
                      size="sm"
                      value={manualModelInput}
                      onChange={(e) => setManualModelInput(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key !== "Enter") return;
                        e.preventDefault();
                        addManualModel();
                      }}
                      placeholder="输入模型 ID 后回车添加"
                      aria-label="手动添加模型 ID"
                    />
                    <Button
                      size="sm"
                      variant="secondary"
                      disabled={!manualModelInput.trim()}
                      onClick={() => addManualModel()}
                    >
                      添加
                    </Button>
                  </div>
                }
              />
            </div>
          </div>

          {providerModels.length > 0 && <div className="provider-model-catalog">
            <div className="provider-model-catalog-heading">模型目录 <span>{providerModels.length}</span><span className="provider-model-catalog-hint" title="上游接口没返回模型窗口时在这里自己填：上下文长度（输入）与单轮输出上限，留空表示自动">上下文 / 输出上限，留空=自动</span></div>
            <Card tone="muted" className="provider-model-catalog-card">
            <div className="provider-model-catalog-list">
            {providerModels.map((model, index) => <div key={index} className="provider-model-catalog-row">
              <Input size="sm" value={model.id} aria-label={`模型 ${index + 1} ID`} onChange={(event) => {
                const id = event.target.value;
                setProviderModels((models) => models.map((item, position) => position === index ? { ...item, id } : item));
                if (providerModel === model.id) setProviderModel(id);
              }} />
              <Input size="sm" value={model.name} aria-label={`模型 ${index + 1}名称`} onChange={(event) => setProviderModels((models) => models.map((item, position) => position === index ? { ...item, name: event.target.value } : item))} />
              <Input size="sm" inputMode="numeric" className="provider-model-catalog-limit" aria-label={`模型 ${index + 1} 上下文长度`} title="上下文长度（输入 tokens）：留空按已知窗口或 64K 估算" placeholder="上下文" value={tokenCountInputValue(model.contextWindow)} onChange={(event) => {
                const contextWindow = parseTokenCount(event.target.value);
                setProviderModels((models) => models.map((item, position) => position === index ? { ...item, contextWindow } : item));
              }} />
              <Input size="sm" inputMode="numeric" className="provider-model-catalog-limit" aria-label={`模型 ${index + 1} 输出上限`} title="单轮输出上限（tokens）：留空按上下文窗口推算" placeholder="输出上限" value={tokenCountInputValue(model.maxOutputTokens)} onChange={(event) => {
                const maxOutputTokens = parseTokenCount(event.target.value);
                setProviderModels((models) => models.map((item, position) => position === index ? { ...item, maxOutputTokens } : item));
              }} />
              <Button type="button" size="xs" variant="subtle" icon={<Trash2 size={13} />} aria-label={`移除模型 ${model.id}`} title="移除模型" onClick={() => {
                const remaining = providerModels.filter((_, position) => position !== index);
                setProviderModels(remaining);
                if (providerModel === model.id) setProviderModel(remaining[0]?.id || "");
              }} />
            </div>)}
            </div>
            </Card>
          </div>}

          {/* 高级配置 */}
          <div
            className="setting-row-item"
            style={{ background: "var(--bg-surface)", cursor: "pointer" }}
            onClick={() => setAdvancedOpen(!advancedOpen)}
          >
            <div style={{ display: "flex", alignItems: "center", gap: 6, fontSize: 12.5, fontWeight: 600, color: "var(--text-secondary)" }}>
              <span>可选高级配置 (缓存、上下文与费用查询)</span>
              <ChevronDown size={14} style={{ transform: advancedOpen ? "rotate(180deg)" : "none", transition: "transform 0.2s" }} />
            </div>
          </div>

          {advancedOpen && (
            <div style={{ background: "var(--bg-surface)", borderTop: "1px solid var(--divider-color)" }}>
              <div className="setting-row-item">
                <div className="setting-label-block">
                  <span className="setting-title">余额 / 额度查询 URL</span>
                  <span className="setting-desc">两项均留空时自动查询；支持完整 URL 或 / 开头路径</span>
                </div>
                <div className="setting-control-block">
                  <Input size="sm" aria-label="余额 / 额度查询 URL" value={providerBalanceUrl} onChange={(e) => setProviderBalanceUrl(e.target.value)} placeholder="/user/balance" style={{ width: "100%" }} />
                </div>
              </div>
              <div className="setting-row-item">
                <div className="setting-label-block">
                  <span className="setting-title">消费查询 URL</span>
                  <span className="setting-desc">消费累计金额的 JSON 接口</span>
                </div>
                <div className="setting-control-block">
                  <Input size="sm" aria-label="消费查询 URL" value={providerSpendingUrl} onChange={(e) => setProviderSpendingUrl(e.target.value)} placeholder="https://example.com/api/usage" style={{ width: "100%" }} />
                </div>
              </div>
              <div className="setting-row-item">
                <div className="setting-label-block">
                  <span className="setting-title">查询访问令牌</span>
                  <span className="setting-desc">同域可使用 API Key；跨域必填。留空保留已保存令牌</span>
                </div>
                <div className="setting-control-block">
                  <Input size="sm" type="password" autoComplete="new-password" aria-label="查询访问令牌" value={providerUsageToken} onChange={(e) => setProviderUsageToken(e.target.value)} placeholder="Bearer Token" style={{ width: "100%" }} />
                </div>
              </div>
              <div className="setting-row-item">
                <div className="setting-label-block">
                  <span className="setting-title">缓存保留策略（可选）</span>
                  <span className="setting-desc">缓存按协议自动启用，无需配置。这里只选择保留时间：Claude 可延长至 1 小时（写入费用更高），支持的 GPT 模型最长 24 小时；其他接口沿用默认值</span>
                </div>
                <div className="setting-control-block">
                  <Select size="sm" aria-label="缓存保留策略" value={providerCacheRetention}
                    disabled={providerEditorBusy} onChange={(value) => setProviderCacheRetention(value as "default" | "extended")}
                    options={[{label:"自动（接口默认时长）", value:"default"}, {label:"延长保留时间", value:"extended"}]} style={{width:"100%"}} />
                </div>
              </div>

              <div className="setting-row-item">
                <div className="setting-label-block">
                  <span className="setting-title">上下文窗口</span>
                  <span className="setting-desc">
                    {selectedModelWindow
                      ? `留空则使用模型内置窗口（当前生效 ${formatContextWindow(selectedModelWindow)} tokens）；填写后按你的值覆盖`
                      : "留空则使用模型内置窗口；未知模型按 64K 保守估算"}
                    <br />模型目录里逐模型填写的长度优先于这里的统一设置；单轮输出上限只在模型目录里按模型填写
                  </span>
                </div>
                <div className="setting-control-block">
                  <Input
                    size="sm"
                    value={providerContextWindow}
                    onChange={(e) => setProviderContextWindow(e.target.value)}
                    placeholder="留空 = 使用内置窗口"
                    style={{ width: "100%" }}
                  />
                </div>
              </div>

              <div className="setting-row-item">
                <div className="setting-label-block"><span className="setting-title">计价币种</span><span className="setting-desc">输入、输出价格的币种</span></div>
                <div className="setting-control-block"><Input size="sm" aria-label="计价币种" value={providerCurrency} onChange={(e) => setProviderCurrency(e.target.value)} placeholder="CNY / USD" style={{ width: "100%" }} /></div>
              </div>

              <div className="setting-row-item">
                <div className="setting-label-block">
                  <span className="setting-title">输入价格 / 1M Tokens</span>
                </div>
                <div className="setting-control-block">
                  <Input
                    size="sm"
                    value={providerInputPrice}
                    onChange={(e) => setProviderInputPrice(e.target.value)}
                    placeholder="1.00"
                    style={{ width: "100%" }}
                  />
                </div>
              </div>

              <div className="setting-row-item">
                <div className="setting-label-block">
                  <span className="setting-title">输出价格 / 1M Tokens</span>
                </div>
                <div className="setting-control-block">
                  <Input
                    size="sm"
                    value={providerOutputPrice}
                    onChange={(e) => setProviderOutputPrice(e.target.value)}
                    placeholder="2.00"
                    style={{ width: "100%" }}
                  />
                </div>
              </div>
              <div className="setting-row-item provider-quota-query">
                <div className="setting-label-block"><span className="setting-title">当前余额</span></div>
                <div className="provider-quota-query-content">
                  <div className="provider-quota-values" aria-live="polite">
                    <span>余额 <b>{queryingQuota ? "查询中" : quotaResult ? formatQuotaBalance(quotaResult.value || null) : "待查询"}</b></span>
                    <span>已消费 <b>{quotaResult ? formatQuotaSpent(quotaResult.value || null) : "待查询"}</b></span>
                  </div>
                  <Button size="sm" variant="secondary" loading={queryingQuota} onClick={() => void queryDraftQuota()}><RefreshCw size={13} /><span>查询</span></Button>
                </div>
              </div>
            </div>
          )}

          <div className="setting-row-item" style={{ background: "var(--bg-surface)", justifyContent: "flex-end", gap: 10 }}>
            <Button size="sm" variant="secondary" style={{ gap: 6 }} disabled={!providerModels.length} onClick={() => void clearProviderModels()}>
              <Eraser size={13} />
              <span>清空目录</span>
            </Button>
            <Button size="sm" variant="secondary" style={{ gap: 6 }} loading={discoveringModels} onClick={() => void discoverModels()}>
              <RefreshCw size={13} />
              <span>发现模型</span>
            </Button>
            <Button size="sm" variant="secondary" style={{ gap: 6 }} loading={testingProviderId === "draft"} onClick={() => void testDraftProvider()}>
              <Plug size={13} />
              <span>测试连接</span>
            </Button>
            {editingProviderId && <Button size="sm" variant="secondary" onClick={closeProviderEditor}>取消</Button>}
            <Button size="sm" variant="primary" loading={savingProvider} style={{ gap: 6 }} onClick={() => void saveProvider()}>
              <Save size={13} />
              <span>保存</span>
            </Button>
          </div>
        </fieldset>
      </div>
  );

  return (
    <section className="settings-pane">
      <header className="pane-header"><h2 className="pane-title">Agent 接入</h2></header>
      <div ref={providerFormRef}>{editingProviderId ? <div style={{ height: providerFormHeight }} aria-hidden="true" /> : providerForm}</div>
      {editingProviderId && <ProviderEditDialog busy={providerEditorBusy} onClose={closeProviderEditor}>{providerForm}</ProviderEditDialog>}
      {discoveredModels && <ModelDiscoveryDialog models={discoveredModels} savedIds={selectableModels.map((model) => model.id)} onClose={() => setDiscoveredModels(null)} onAdd={(selected) => {
        setProviderModels((models) => mergeModels(models, selected));
        if (!providerModel && selected[0]) setProviderModel(selected[0].id);
        setDiscoveredModels(null);
      }} />}
      <ConfirmDialog
        open={Boolean(pendingDeletion)}
        title="删除供应商"
        message={`确认删除供应商「${pendingDeletion?.name || ""}」及其已保存的密钥？历史对话不会删除。`}
        confirmLabel="确认删除"
        onCancel={() => setPendingDeletion(null)}
        onConfirm={() => void confirmProviderDeletion()}
      />

      {/* 已保存的 provider 列表 */}
      <div>
        <div className="setting-group-title">
          <span>已保存的 provider</span>
          <small style={{ marginLeft: 8, fontWeight: 500, color: "var(--text-tertiary)" }}>
            {savedProviders.length} 个接入
          </small>
        </div>

        {savedProviders.length > 0 && <div className="setting-card-group provider-saved-list">
          {savedProviders.map((p) => {
            const savedModels = mergeModels(p.models, p.model ? [{ id: p.model, name: p.model }] : []);
            const modelsExpanded = Boolean(expandedProviderModels[p.id]);
            return <Card key={p.id} tone="muted" className="provider-saved-card">
              <div className="provider-saved-header">
                <div className="provider-saved-identity">
                  <ProviderIcon brand={p.brand || "custom"} size={22} />
                  <div className="provider-saved-details">
                    <div className="provider-saved-title">{p.name}</div>
                    <div className="provider-saved-meta">{p.baseUrl}</div>
                  </div>
                </div>
                <div className="provider-saved-actions">
                  <Button size="xs" variant="secondary" loading={testingProviderId === p.id} onClick={() => void testSavedProvider(p)}>测试</Button>
                  <Button type="button" size="xs" variant="subtle" icon={<Pencil size={13} />} title="编辑 provider" aria-label={`编辑 ${p.name}`} disabled={providerEditorBusy || Boolean(deletingProviderId)} onClick={() => editProvider(p)} />
                  <Button type="button" size="xs" variant="subtle" className="btn-danger-hover" icon={<Trash2 size={13} />} title="删除 provider" aria-label={`删除 ${p.name}`} disabled={providerEditorBusy || Boolean(deletingProviderId)} loading={deletingProviderId === p.id} onClick={() => setPendingDeletion(p)} />
                </div>
              </div>
              <div className="provider-default-model-row">
                <span className="provider-default-model"><Badge variant="primary">默认模型</Badge><code title={p.model || "未设置"}>{p.model || "未设置"}</code></span>
                {savedModels.length > 0 && <Button size="xs" variant="subtle"
                  type="button"
                  title={modelsExpanded ? "收起模型列表" : `展开全部 ${savedModels.length} 个模型`}
                  aria-label={modelsExpanded ? "收起模型列表" : "展开全部模型"}
                  aria-expanded={modelsExpanded}
                  aria-controls={`provider-models-${p.id}`}
                  onClick={() => setExpandedProviderModels((current) => ({ ...current, [p.id]: !modelsExpanded }))}
                  icon={modelsExpanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                >
                  {savedModels.length} 个模型
                </Button>}
              </div>
              {modelsExpanded && savedModels.length > 0 && <div className="provider-model-row" id={`provider-models-${p.id}`}>
                <div className="provider-model-list">
                {savedModels.map((model) => (
                  <Button
                    key={model.id}
                    type="button"
                    size="xs"
                    variant={model.id === p.model ? "secondary" : "subtle"}
                    className="provider-model-chip"
                    title={model.id === p.model ? `当前默认模型：${model.name}` : `设为默认模型：${model.name}`}
                    aria-pressed={model.id === p.model}
                    icon={model.id === p.model ? <Check size={12} /> : undefined}
                    loading={switchingDefaultModel?.providerId === p.id && switchingDefaultModel.modelId === model.id}
                    disabled={providerEditorBusy || Boolean(deletingProviderId)}
                    onClick={() => void switchDefaultModel(p, model)}
                  >
                    {model.name}
                  </Button>
                ))}
                </div>
              </div>}
            </Card>;
          })}
        </div>}
      </div>

      <AgentThinkingSettings />

      <AgentRuntimeSettings />

      {/* MCP 服务配置 */}
      <div>
        <div className="setting-group-title">模型上下文协议 (MCP)</div>
        <div className="setting-card-group">
          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">服务标识 (ID)</span>
              <span className="setting-desc">MCP 服务唯一名称</span>
            </div>
            <div className="setting-control-block">
              <Input
                size="sm"
                value={mcpId}
                onChange={(e) => setMcpId(e.target.value)}
                placeholder="local-tools"
                style={{ width: "100%" }}
              />
            </div>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">传输协议</span>
              <span className="setting-desc">本地子进程 (stdio) 或远程 HTTP SSE</span>
            </div>
            <div className="setting-control-block">
              <Select
                size="sm"
                value={mcpTransport}
                onChange={(val) => setMcpTransport(val as "stdio" | "http")}
                options={[
                  { label: "本地 stdio (子进程)", value: "stdio" },
                  { label: "HTTP (SSE / Stream)", value: "http" },
                ]}
                style={{ width: "100%" }}
              />
            </div>
          </div>

          {mcpTransport === "stdio" ? (
            <>
              <div className="setting-row-item">
                <div className="setting-label-block">
                  <span className="setting-title">可执行命令</span>
                  <span className="setting-desc">stdio 启动程序</span>
                </div>
                <div className="setting-control-block">
                  <Input
                    size="sm"
                    value={mcpCommand}
                    onChange={(e) => setMcpCommand(e.target.value)}
                    placeholder="npx / uvx / python"
                    style={{ width: "100%" }}
                  />
                </div>
              </div>
              <div className="setting-row-item">
                <div className="setting-label-block">
                  <span className="setting-title">命令行参数</span>
                  <span className="setting-desc">空格分隔参数</span>
                </div>
                <div className="setting-control-block">
                  <Input
                    size="sm"
                    value={mcpArgs}
                    onChange={(e) => setMcpArgs(e.target.value)}
                    placeholder="-y @modelcontextprotocol/server-android-tools"
                    style={{ width: "100%" }}
                  />
                </div>
              </div>
            </>
          ) : (
            <div className="setting-row-item">
              <div className="setting-label-block">
                <span className="setting-title">HTTP MCP URL</span>
                <span className="setting-desc">MCP 服务 URL 地址</span>
              </div>
              <div className="setting-control-block">
                <Input
                  size="sm"
                  value={mcpUrl}
                  onChange={(e) => setMcpUrl(e.target.value)}
                  placeholder="http://localhost:8000/mcp"
                  style={{ width: "100%" }}
                />
              </div>
            </div>
          )}

          <div className="setting-row-item" style={{ background: "var(--bg-surface)", justifyContent: "flex-end" }}>
            <Button size="sm" variant="primary" style={{ gap: 6 }} onClick={() => void connectMcp()}>
              <Plug size={13} />
              <span>连接并发现工具</span>
            </Button>
          </div>

          {mcpServers.length > 0 ? (
            <div style={{ padding: "10px 16px", display: "flex", flexDirection: "column", gap: 6, borderTop: "1px solid var(--divider-color)" }}>
              {mcpServers.map((srv) => (
                <div key={srv.id} className="mcp-row-item">
                  <span className="mcp-status-dot" />
                  <div style={{ flex: 1 }}>
                    <div style={{ fontSize: 13, fontWeight: 700, color: "var(--text-primary)" }}>{srv.id}</div>
                    <div style={{ fontSize: 11, color: "var(--text-tertiary)" }}>{srv.status}{srv.status === "已连接" ? ` · ${srv.toolsCount} 个工具已就绪` : ""}</div>
                  </div>
                  <button
                    type="button"
                    className="icon-action-btn danger"
                    title="断开 MCP 连接"
                    onClick={() => void disconnectMcpServer(srv.id).then(reloadProviders).then(() => notify("MCP 服务已断开。", "success")).catch((reason) => notify(formatInvokeError(reason), "error"))}
                  >
                    <Unplug size={13} />
                  </button>
                </div>
              ))}
            </div>
          ) : null}
        </div>
      </div>
    </section>
  );
};
