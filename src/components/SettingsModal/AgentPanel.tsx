import React, { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Eye,
  EyeOff,
  KeyRound,
  Pencil,
  Plug,
  PlugZap,
  RefreshCw,
  Save,
  SendHorizontal,
  Trash2,
} from "lucide-react";
import {
  Accordion,
  AccordionContent,
  AccordionItem,
  AccordionTrigger,
  Button,
  ConfirmDialog,
  Input,
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
  InputGroupText,
  Select,
  cn,
} from "rain-ui";
import { ProviderIcon } from "@/components/ProviderIcon";
import {
  deleteAgentProvider,
  discoverAgentModels,
  getAgentProviderApiKey,
  listAgentProviders,
  saveAgentProvider,
  testAgentProviderChat,
  type AgentProvider,
  type AgentModelSettings,
  type AgentProviderCheckInput,
} from "@/platform/api";
import { formatInvokeError, isTauri } from "@/platform/runtime";
import { AGENT_PROVIDERS_CHANGED_EVENT } from "@/platform/agentProviderEvents";
import { getProviderQuota, type AgentQuota } from "@/platform/chatApi";
import { formatQuotaBalance, formatQuotaSpent } from "@/platform/providerQuota";
import { usePortal } from "rain-ui";
import { modelsForProviderProtocol } from "./providerProtocol";
import { formatContextWindow, parseTokenCount, tokenCountInputValue } from "./modelWindowFormat";
import { ModelDiscoveryDialog } from "./ModelDiscoveryDialog";
import { SettingsEditDialog } from "./SettingsEditDialog";
import { SettingRow, SettingsActions, SettingsGroup, SettingsPane } from "./settingsUI";

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

const PROTOCOL_OPTIONS = [
  { label: "OpenAI 兼容", value: "open_ai_compatible" },
  { label: "OpenAI Chat Completions", value: "open_ai" },
  { label: "OpenAI Responses", value: "open_ai_responses" },
  { label: "Anthropic", value: "anthropic" },
];

const CONTROL_URL = "w-[216px] max-w-full";
const CONTROL_SECRET = "w-[216px] max-w-full";
const CONTROL_MODEL = "w-[192px] max-w-full";
const CONTROL_COMPACT = "w-[200px] max-w-full";
const CONTROL_NUMERIC = "w-[160px] max-w-full";
const TEST_MESSAGE_DEFAULT = "请只回复：连接正常。";

type KnownModelLimits = Record<string, {
  contextWindow?: number;
  maxOutputTokens?: number;
}>;

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

/** Agent 配置行沿用设置页统一结构：左侧标题/说明，右侧控件靠右。 */
const AgentFormRow: React.FC<{
  title: React.ReactNode;
  desc?: React.ReactNode;
  children: React.ReactNode;
  controlClassName?: string;
  className?: string;
}> = ({ title, desc, children, controlClassName, className }) => (
  <SettingRow title={title} desc={desc} controlClassName={controlClassName} className={cn("min-h-[62px] py-3", className)}>
    {children}
  </SettingRow>
);

export const AgentPanel: React.FC = () => {
  const portal = usePortal();
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
  const [knownModelLimits, setKnownModelLimits] = useState<KnownModelLimits>({});
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

  const [editingProviderId, setEditingProviderId] = useState<string | null>(null);
  const [savingProvider, setSavingProvider] = useState(false);
  const [discoveringModels, setDiscoveringModels] = useState(false);
  const [discoveredModels, setDiscoveredModels] = useState<AgentModelSettings[] | null>(null);
  const [pendingDeletion, setPendingDeletion] = useState<AgentProvider | null>(null);
  const [deletingProviderId, setDeletingProviderId] = useState<string | null>(null);
  const [testTarget, setTestTarget] = useState<AgentProvider | null>(null);
  const [testDiscovering, setTestDiscovering] = useState(false);
  const [testProtocol, setTestProtocol] = useState("");
  const [testModel, setTestModel] = useState("");
  const [testMessage, setTestMessage] = useState(TEST_MESSAGE_DEFAULT);
  const [testStatus, setTestStatus] = useState<{ kind: "success" | "warning" | "error"; message: string } | null>(null);
  const [testingProvider, setTestingProvider] = useState(false);
  const testOpenRevision = useRef(0);
  const [savedProviders, setSavedProviders] = useState<AgentProvider[]>([]);
  const [savedProvidersLoading, setSavedProvidersLoading] = useState(() => isTauri());
  const [savedProviderError, setSavedProviderError] = useState<string | null>(null);

  useEffect(() => () => {
    quotaQueryRevision.current += 1;
    apiKeyReadRevision.current += 1;
    testOpenRevision.current += 1;
  }, []);

  const refreshSavedProviders = useCallback(async () => {
    if (!isTauri()) {
      setSavedProviders([]);
      setSavedProvidersLoading(false);
      return;
    }
    setSavedProvidersLoading(true);
    try {
      const providers = await listAgentProviders();
      setSavedProviders(providers.filter((provider) => !provider.id.startsWith("plugin:")));
      setSavedProviderError(null);
    } catch (reason) {
      setSavedProviderError(formatInvokeError(reason));
    } finally {
      setSavedProvidersLoading(false);
    }
  }, []);

  useEffect(() => {
    void refreshSavedProviders();
    const handleProvidersChanged = () => void refreshSavedProviders();
    window.addEventListener(AGENT_PROVIDERS_CHANGED_EVENT, handleProvidersChanged);
    return () => window.removeEventListener(AGENT_PROVIDERS_CHANGED_EVENT, handleProvidersChanged);
  }, [refreshSavedProviders]);

  const quotaQueryKey = JSON.stringify([
    providerId, providerKind, selectedBrand, providerBaseUrl, providerApiKey,
    providerBalanceUrl, providerSpendingUrl, providerUsageToken, providerCurrency,
  ]);

  const selectableModels = useMemo(
    () => mergeModels(providerModels, providerModel ? [{ id: providerModel, name: providerModel }] : []),
    [providerModel, providerModels],
  );

  const selectedModel = useMemo(
    () => providerModels.find((entry) => entry.id === providerModel.trim()) ?? null,
    [providerModel, providerModels],
  );

  const selectedModelWindow = selectedModel?.contextWindow ?? null;
  const selectedPreset = PRESETS.find((preset) => preset.brand === selectedBrand);
  const selectedProtocol = PROTOCOL_OPTIONS.find((option) => option.value === providerKind);
  const providerEditorBusy = savingProvider || discoveringModels || testingProvider;

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
      portal(message, "error");
      return null;
    }
    return draft;
  };

  const validateModelLimits = () => {
    const providerKnown = knownModelLimits[providerModel.trim()];
    const providerWindow = Number(providerContextWindow) || 0;
    if (providerKnown?.contextWindow && providerWindow > providerKnown.contextWindow) {
      portal(`Provider 上下文不能超过模型已知上限 ${formatContextWindow(providerKnown.contextWindow)}。`, "error");
      return false;
    }
    for (const model of providerModels) {
      const known = knownModelLimits[model.id];
      if (known?.contextWindow && (model.contextWindow ?? 0) > known.contextWindow) {
        portal(`模型 ${model.name || model.id} 的上下文不能超过已知上限 ${formatContextWindow(known.contextWindow)}。`, "error");
        return false;
      }
      if (known?.maxOutputTokens && (model.maxOutputTokens ?? 0) > known.maxOutputTokens) {
        portal(`模型 ${model.name || model.id} 的输出上限不能超过已知上限 ${formatContextWindow(known.maxOutputTokens)}。`, "error");
        return false;
      }
    }
    return true;
  };

  const closeConnectionTest = () => {
    if (testingProvider) return;
    testOpenRevision.current += 1;
    setTestDiscovering(false);
    setTestTarget(null);
  };

  const openProviderTest = async (provider: AgentProvider) => {
    const revision = ++testOpenRevision.current;
    const existingModels = mergeModels(
      provider.models,
      provider.model ? [{ id: provider.model, name: provider.model }] : [],
    );
    const initialModel = provider.model || existingModels[0]?.id || "";

    setTestTarget({ ...provider, models: existingModels });
    setTestProtocol(provider.kind);
    setTestModel(initialModel);
    setTestMessage(TEST_MESSAGE_DEFAULT);
    setTestStatus(null);
    setTestDiscovering(true);

    try {
      const discovered = await discoverAgentModels({
        id: provider.id,
        name: provider.name,
        kind: provider.kind,
        brand: provider.brand || null,
        baseUrl: provider.baseUrl,
        apiKey: undefined,
      });
      if (testOpenRevision.current !== revision) return;

      const models = mergeModels(existingModels, discovered);
      setTestTarget((current) => current ? { ...current, models } : current);
      setTestModel((current) => current.trim() || provider.model || models[0]?.id || "");
      if (!models.length) {
        setTestStatus({
          kind: "warning",
          message: "没有发现可用模型。请检查 Provider 配置，或先在供应商编辑弹窗中添加模型。",
        });
      }
    } catch (reason) {
      if (testOpenRevision.current !== revision) return;
      setTestStatus({
        kind: "warning",
        message: `自动发现模型失败：${formatInvokeError(reason)} 仍可使用已保存的模型继续测试。`,
      });
    } finally {
      if (testOpenRevision.current === revision) setTestDiscovering(false);
    }
  };

  const runRealConnectionTest = async (provider: AgentProvider, model = testModel, protocol = testProtocol) => {
    const selected = model.trim();
    const message = testMessage.trim() || TEST_MESSAGE_DEFAULT;
    if (!selected) {
      setTestStatus({ kind: "error", message: "请先选择一个模型。" });
      return;
    }
    if (!provider.model && !provider.models.length) {
      setTestStatus({ kind: "error", message: "该 Provider 没有可用模型，请先发现或添加模型。" });
      return;
    }

    setTestingProvider(true);
    setTestStatus(null);
    try {
      // A connection test must not mutate the saved provider configuration.
      const testProvider: AgentProvider = protocol && protocol !== provider.kind
        ? { ...provider, kind: protocol }
        : provider;

      const response = await testAgentProviderChat({
        id: testProvider.id,
        name: testProvider.name,
        kind: protocol || testProvider.kind,
        baseUrl: testProvider.baseUrl,
        model: selected,
        message,
      });
      const text = response.text.trim();
      if (!text) {
        const statusMessage = "请求已完成，但模型返回了空内容。请检查模型或接口兼容性。";
        setTestStatus({ kind: "warning", message: statusMessage });
        portal(statusMessage, "warning");
      } else {
        const statusMessage = `连接成功，模型返回：${text.slice(0, 160)}${text.length > 160 ? "…" : ""}`;
        setTestStatus({ kind: "success", message: statusMessage });
        portal("模型连接测试成功。", "success");
      }
    } catch (reason) {
      const message = formatInvokeError(reason);
      setTestStatus({ kind: "error", message });
      portal(message, "error");
    } finally {
      setTestingProvider(false);
    }
  };

  const openDraftTest = async () => {
    if (savingProvider || testingProvider) return;
    setSavingProvider(true);
    try {
      const saved = await persistProviderDraft();
      if (!saved) return;
      portal("Provider 已保存，正在自动发现模型。", "info");
      await openProviderTest(saved);
    } catch (reason) {
      portal(formatInvokeError(reason), "error");
    } finally {
      setSavingProvider(false);
    }
  };

  const openSavedProviderTest = (provider: AgentProvider) => {
    void openProviderTest(provider);
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
      const value = await getProviderQuota({
        ...buildProviderCheckRequest(),
        quotaUrls: { balanceUrl: providerBalanceUrl.trim() || null, spendingUrl: providerSpendingUrl.trim() || null },
        accessToken: providerUsageToken.trim() || null,
      });
      if (quotaQueryRevision.current === revision) {
        setQueriedQuota({ key, value });
        portal(value.message || (value.status === "available" ? "供应商余额已更新。" : "供应商余额暂不可用。"), value.status === "available" ? "success" : value.status === "error" ? "error" : "info");
      }
    } catch (reason) {
      if (quotaQueryRevision.current === revision) portal(formatInvokeError(reason), "error");
    } finally {
      if (quotaQueryRevision.current === revision) setQueryingQuota(false);
    }
  };

  const addManualModel = () => {
    const next = manualModelInput.trim();
    if (!next) return;
    if (providerModels.some((model) => model.id === next)) {
      setProviderModel(next);
      setManualModelInput("");
      portal(`模型 ${next} 已在目录中，已设为默认。`, "info");
      return;
    }
    setProviderModels((models) => mergeModels(models, [{ id: next, name: next }]));
    setProviderModel(next);
    setManualModelInput("");
    portal(`已添加模型 ${next} 并设为默认。`, "success");
  };

  const clearProviderModels = () => {
    if (!providerModels.length) return;
    setProviderModels([]);
    setProviderModel("");
    setKnownModelLimits({});
    portal(`已清空模型目录（${providerModels.length} 项）。`, "success");
  };

  const discoverModels = async () => {
    const draft = ensureProviderDraft(false);
    if (!draft) return;
    setDiscoveringModels(true);
    try {
      const models = await discoverAgentModels({
        id: draft.id, name: draft.name, kind: draft.kind, brand: draft.brand,
        baseUrl: draft.baseUrl, apiKey: draft.apiKey,
      });
      if (models.length) {
        setDiscoveredModels(mergeModels(models));
        setKnownModelLimits((current) => ({
          ...current,
          ...Object.fromEntries(models.map((model) => [model.id, {
            contextWindow: model.contextWindow || undefined,
            maxOutputTokens: model.maxOutputTokens || undefined,
          }])),
        }));
      } else {
        portal("Provider 未公开模型列表；可以继续手动填写模型 ID。", "info");
      }
    } catch (reason) {
      portal(formatInvokeError(reason), "error");
    } finally {
      setDiscoveringModels(false);
    }
  };

  const persistProviderDraft = async () => {
    if (!ensureProviderDraft() || !validateModelLimits()) return null;
    const ids = providerModels.map((model) => model.id.trim());
    if (ids.some((id) => !id) || new Set(ids).size !== ids.length) {
      portal("模型 ID 不能为空或重复。", "error");
      return null;
    }
    const saved = await saveAgentProvider({
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
    await refreshSavedProviders();
    return saved;
  };

  const saveProvider = async () => {
    if (savingProvider) return;
    setSavingProvider(true);
    try {
      const saved = await persistProviderDraft();
      if (!saved) return;
      portal("Provider 已保存到本地。", "success");
      if (editingProviderId) closeProviderEditor();
    } catch (reason) {
      portal(formatInvokeError(reason), "error");
    } finally {
      setSavingProvider(false);
    }
  };

  const readDraft = () => ({
    selectedBrand, providerId, providerName, providerKind, providerBaseUrl,
    providerCacheRetention, providerApiKey, providerApiKeyVisible, providerModel,
    providerModels, knownModelLimits, manualModelInput, advancedOpen,
    providerContextWindow, providerInputPrice, providerOutputPrice,
    providerBalanceUrl, providerSpendingUrl, providerUsageToken, providerCurrency,
  });
  const draftBeforeEdit = useRef<ReturnType<typeof readDraft> | null>(null);

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
      setKnownModelLimits(draft.knownModelLimits);
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

  const editProvider = (provider: AgentProvider) => {
    apiKeyReadRevision.current += 1;
    setLoadingApiKey(false);
    draftBeforeEdit.current = readDraft();
    setEditingProviderId(provider.id);
    setProviderId(provider.id);
    setProviderName(provider.name);
    setProviderKind(provider.kind);
    setProviderBaseUrl(provider.baseUrl);
    setProviderCacheRetention(provider.promptCacheRetention || "default");
    setProviderModel(provider.model);
    setSelectedBrand(provider.brand || "custom");
    setProviderContextWindow(provider.contextWindow ? String(provider.contextWindow) : "");
    setProviderInputPrice(String(provider.inputPricePerMillion ?? ""));
    setProviderOutputPrice(String(provider.outputPricePerMillion ?? ""));
    const models = mergeModels(provider.models, provider.model ? [{ id: provider.model, name: provider.model }] : []);
    setProviderModels(models);
    setKnownModelLimits(Object.fromEntries(models.map((model) => [model.id, {
      contextWindow: model.contextWindow || undefined,
      maxOutputTokens: model.maxOutputTokens || undefined,
    }])));
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
        portal("该供应商没有已保存的 API Key。", "info");
        return;
      }
      setProviderApiKey(key);
      setProviderApiKeyVisible(true);
    } catch (reason) {
      if (apiKeyReadRevision.current === revision) portal(formatInvokeError(reason), "error");
    } finally {
      if (apiKeyReadRevision.current === revision) setLoadingApiKey(false);
    }
  };

  const confirmProviderDeletion = async () => {
    if (!pendingDeletion || deletingProviderId) return;
    const provider = pendingDeletion;
    setDeletingProviderId(provider.id);
    try {
      await deleteAgentProvider(provider.id);
      await refreshSavedProviders();
      setPendingDeletion(null);
      portal(`供应商「${provider.name}」已删除。`, "success");
    } catch (reason) {
      portal(formatInvokeError(reason), "error");
    } finally {
      setDeletingProviderId(null);
    }
  };

  const handleApplyPreset = (brand: string) => {
    const preset = PRESETS.find((entry) => entry.brand === brand);
    if (!preset) return;
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
    setKnownModelLimits({});
    setManualModelInput("");
    setProviderBalanceUrl("");
    setProviderSpendingUrl("");
    setProviderUsageToken("");
    setProviderCurrency("");
    setQueriedQuota(null);
    setAdvancedOpen(false);
    setProviderContextWindow("");
    setProviderInputPrice("1.00");
    setProviderOutputPrice("2.00");
  };

  const quotaResult = queriedQuota?.key === quotaQueryKey ? queriedQuota : null;

  const renderProviderForm = () => (
      <SettingsGroup title="Provider 配置" className={cn("min-w-0", editingProviderId ? "mb-0" : "mb-10")}>
            
                    <AgentFormRow title="供应商" desc="选择预设并填入基础配置">
                      <Select
                        aria-label="供应商"
                        value={selectedBrand}
                        disabled={Boolean(editingProviderId)}
                        searchable
                        searchPlaceholder="搜索供应商"
                        className="w-[220px]"
                        onChange={handleApplyPreset}
                        options={PRESETS.map((preset) => ({
                          value: preset.brand,
                          label: preset.label,
                          leading: <ProviderIcon brand={preset.brand} size={14} />,
                        }))}
                      />
                    </AgentFormRow>
            
                    <AgentFormRow title="配置 ID" desc="Provider 的唯一标识">
                      <Input
                        className={CONTROL_COMPACT}
                        value={providerId}
                        aria-label="配置 ID"
                        disabled={Boolean(editingProviderId)}
                        onChange={(event) => setProviderId(event.target.value)}
                        placeholder="default-provider"
                      />
                    </AgentFormRow>
            
                    <AgentFormRow title="显示名称" desc="用于界面展示">
                      <Input
                        className={CONTROL_COMPACT}
                        value={providerName}
                        aria-label="显示名称"
                        onChange={(event) => setProviderName(event.target.value)}
                        placeholder="我的 Provider"
                      />
                    </AgentFormRow>
            
                    <AgentFormRow title="传输协议" desc="接口协议规范">
                      <Select
                        aria-label="传输协议"
                contentClassName="z-[13000]"
                        value={providerKind}
                        onChange={(value) => setProviderKind(value)}
                        options={PROTOCOL_OPTIONS}
                      />
                    </AgentFormRow>
            
                    <AgentFormRow title="Base URL" desc="API 基础地址">
                      <Input
                        className={CONTROL_URL}
                        value={providerBaseUrl}
                        aria-label="Base URL"
                        onChange={(event) => setProviderBaseUrl(event.target.value)}
                        placeholder="https://api.openai.com/v1"
                      />
                    </AgentFormRow>
            
                    <AgentFormRow title="API Key" desc="留空沿用已有凭据">
                      <InputGroup className={CONTROL_SECRET}>
                        <InputGroupAddon align="start">
                          <InputGroupText>
                            <KeyRound aria-hidden="true" />
                          </InputGroupText>
                        </InputGroupAddon>
                        <InputGroupInput
                          type="text"
                          className="[&::-ms-clear]:hidden [&::-ms-reveal]:hidden"
                          style={{ WebkitTextSecurity: providerApiKeyVisible ? "none" : "disc" } as React.CSSProperties}
                          value={providerApiKey}
                          aria-label="API Key"
                          autoComplete="off"
                          onChange={(event) => setProviderApiKey(event.target.value)}
                          placeholder="保存到系统凭据库"
                        />
                        <InputGroupAddon align="end">
                          <InputGroupButton
                            aria-label={providerApiKeyVisible ? "隐藏 API Key" : "显示 API Key"}
                            title={providerApiKeyVisible ? "隐藏 API Key" : "显示 API Key"}
                            aria-pressed={providerApiKeyVisible}
                            type="button"
                            disabled={loadingApiKey}
                            onClick={() => void toggleApiKeyVisibility()}
                          >
                            {providerApiKeyVisible ? <EyeOff aria-hidden="true" /> : <Eye aria-hidden="true" />}
                          </InputGroupButton>
                        </InputGroupAddon>
                      </InputGroup>
                    </AgentFormRow>
            
                    <AgentFormRow title="默认模型" desc="用于新对话和 Agent 任务">
                      <Select
                        aria-label="默认模型"
                contentClassName="z-[13000]"
                        className={CONTROL_MODEL}
                        value={providerModel}
                        searchable
                        searchPlaceholder="搜索模型名称或 ID"
                        placeholder="发现模型或手动添加"
                        disabled={providerEditorBusy}
                        onChange={(value) => setProviderModel(value)}
                        options={selectableModels.map((model) => ({
                          label: model.name,
                          value: model.id,
                          trailing: model.contextWindow && model.contextWindow > 0 ? formatContextWindow(model.contextWindow) : "未返回",
                          onRemove: model.id === providerModel ? undefined : () => {
                            setProviderModels((models) => models.filter((entry) => entry.id !== model.id));
                          },
                          removeLabel: `从目录移除 ${model.name}`,
                        }))}
                        keepOpenOnSelect
                        footer={
                          <div className="flex items-center gap-2 p-2">
                            <Input
                              value={manualModelInput}
                              onChange={(event) => setManualModelInput(event.target.value)}
                              onKeyDown={(event) => {
                                if (event.key !== "Enter") return;
                                event.preventDefault();
                                addManualModel();
                              }}
                              placeholder="输入模型"
                              aria-label="输入模型"
                            />
                            <Button variant="outline" disabled={!manualModelInput.trim()} onClick={addManualModel}>添加</Button>
                          </div>
                        }
                      />
                    </AgentFormRow>
            
                    <AgentFormRow title="模型目录" desc={providerModels.length ? `已配置 ${providerModels.length} 个模型` : "发现或手动添加模型"}>
                      <div className="flex flex-wrap items-center gap-2">
                        <Button variant="outline" icon={<RefreshCw size={13} />} loading={discoveringModels} disabled={providerEditorBusy && !discoveringModels} onClick={() => void discoverModels()}>发现模型</Button>
                        <Button variant="outline" icon={<Trash2 size={13} />} disabled={!providerModels.length || providerEditorBusy} onClick={clearProviderModels}>清空</Button>
                      </div>
                    </AgentFormRow>
            
                    {providerModels.length > 0 ? (
                      <div className="settings-row border-t border-border bg-muted/10 px-[17px] py-3.5">
                        <div className="mb-2.5 flex items-center justify-between gap-4">
                          <span className="text-[12px] font-medium text-foreground">模型明细</span>
                          <span className="text-[10.5px] text-muted-foreground">留空时自动推断，已知上限不可超过</span>
                        </div>
                        <div className="min-w-0 overflow-hidden rounded-xl border border-border/70 bg-background/60">
                          <div className="grid min-w-0 grid-cols-[minmax(0,1.2fr)_minmax(0,1fr)_74px_74px_28px] items-center gap-1.5 border-b border-border/60 bg-muted/25 px-2 py-1.5 text-[10.5px] font-medium text-muted-foreground">
                            <span>实际模型</span>
                            <span>显示名称</span>
                            <span className="text-center">上下文</span>
                            <span className="text-center">输出上限</span>
                            <span aria-hidden="true" />
                          </div>
                          <div className="max-h-[280px] space-y-1 overflow-y-auto p-2 [scrollbar-gutter:stable]">
                            {providerModels.map((model, index) => {
                              const known = knownModelLimits[model.id];
                              const contextTooLarge = Boolean(known?.contextWindow && (model.contextWindow ?? 0) > known.contextWindow);
                              const outputTooLarge = Boolean(known?.maxOutputTokens && (model.maxOutputTokens ?? 0) > known.maxOutputTokens);
                              return (
                                <div key={`${model.id}-${index}`} className="grid min-w-0 grid-cols-[minmax(0,1.2fr)_minmax(0,1fr)_74px_74px_28px] items-center gap-1.5 rounded-lg px-1 py-1">
                                  <Input
                                    className="min-w-0 px-2"
                                    value={model.id}
                                    aria-label={`模型 ${index + 1} 实际模型`}
                                    onChange={(event) => {
                                      const id = event.target.value;
                                      const previousId = model.id;
                                      setProviderModels((models) => models.map((item, position) => position === index ? { ...item, id } : item));
                                      if (providerModel === previousId) setProviderModel(id);
                                      setKnownModelLimits((limits) => {
                                        if (!limits[previousId]) return limits;
                                        const { [previousId]: known, ...rest } = limits;
                                        return id.trim() ? { ...rest, [id.trim()]: known } : rest;
                                      });
                                    }}
                                  />
                                  <Input
                                    className="min-w-0 px-2"
                                    value={model.name}
                                    aria-label={`模型 ${index + 1} 显示名称`}
                                    onChange={(event) => setProviderModels((models) => models.map((item, position) => position === index ? { ...item, name: event.target.value } : item))}
                                  />
                                  <Input
                                    inputMode="numeric"
                                    className={cn("min-w-0 px-1.5 text-center", contextTooLarge && "border-destructive/60 text-destructive focus-visible:border-destructive focus-visible:ring-destructive/15")}
                                    aria-invalid={contextTooLarge}
                                    aria-label={`模型 ${index + 1} 上下文长度`}
                                    title={known?.contextWindow ? `已知上限 ${formatContextWindow(known.contextWindow)}` : "留空自动推断"}
                                    placeholder="自动"
                                    value={tokenCountInputValue(model.contextWindow)}
                                    onChange={(event) => {
                                      const contextWindow = parseTokenCount(event.target.value);
                                      setProviderModels((models) => models.map((item, position) => position === index ? { ...item, contextWindow } : item));
                                    }}
                                  />
                                  <Input
                                    inputMode="numeric"
                                    className={cn("min-w-0 px-1.5 text-center", outputTooLarge && "border-destructive/60 text-destructive focus-visible:border-destructive focus-visible:ring-destructive/15")}
                                    aria-invalid={outputTooLarge}
                                    aria-label={`模型 ${index + 1} 输出上限`}
                                    title={known?.maxOutputTokens ? `已知上限 ${formatContextWindow(known.maxOutputTokens)}` : "留空自动推断"}
                                    placeholder="自动"
                                    value={tokenCountInputValue(model.maxOutputTokens)}
                                    onChange={(event) => {
                                      const maxOutputTokens = parseTokenCount(event.target.value);
                                      setProviderModels((models) => models.map((item, position) => position === index ? { ...item, maxOutputTokens } : item));
                                    }}
                                  />
                                  <Button
                                    type="button"
                                    variant="ghost"
                                    size="icon-xs"
                                    className="justify-self-center"
                                    icon={<Trash2 size={13} />}
                                    aria-label={`移除模型 ${model.id}`}
                                    title="移除模型"
                                    onClick={() => {
                                      const remaining = providerModels.filter((_, position) => position !== index);
                                      setProviderModels(remaining);
                                      if (providerModel === model.id) setProviderModel(remaining[0]?.id || "");
                                    }}
                                  />
                                </div>
                              );
                            })}
                          </div>
                        </div>
                      </div>
                    ) : null}
            
                    <Accordion
                      type="single"
                      collapsible
                      value={advancedOpen ? "advanced" : ""}
                      onValueChange={(value) => setAdvancedOpen(value === "advanced")}
                    >
                      <AccordionItem value="advanced" className="border-b-0 border-t border-border">
                        <AccordionTrigger className="settings-row min-h-[62px] px-[17px] py-3 hover:no-underline">
                          <span className="flex min-w-0 flex-col gap-0.5 text-left">
                            <span className="text-[13px] font-medium leading-[1.42] text-foreground">高级配置</span>
                            <span className="text-[11.5px] font-normal leading-[1.45] text-muted-foreground">缓存、上下文、额度与费用查询</span>
                          </span>
                        </AccordionTrigger>
                        <AccordionContent className="border-t border-border bg-muted/10 p-0">
                          <AgentFormRow title="缓存保留" desc="按协议自动启用">
                            <Select
                              aria-label="缓存保留策略"
                              contentClassName="z-[13000]"
                              value={providerCacheRetention}
                              disabled={providerEditorBusy}
                              onChange={(value) => setProviderCacheRetention(value as "default" | "extended")}
                              options={[
                                { label: "自动（接口默认时长）", value: "default" },
                                { label: "延长保留时间", value: "extended" },
                              ]}
                            />
                          </AgentFormRow>
            
                          <AgentFormRow
                            title="上下文窗口"
                            desc={selectedModelWindow
                              ? `留空使用 ${formatContextWindow(selectedModelWindow)} tokens`
                              : "留空使用模型内置窗口"}
                          >
                            <Input
                              className={CONTROL_NUMERIC}
                              inputMode="numeric"
                              value={providerContextWindow}
                              onChange={(event) => setProviderContextWindow(event.target.value)}
                              placeholder="留空自动"
                            />
                          </AgentFormRow>
            
                          <AgentFormRow title="余额查询 URL" desc="留空自动查询">
                            <Input
                              className={CONTROL_URL}
                              aria-label="余额查询 URL"
                              value={providerBalanceUrl}
                              onChange={(event) => setProviderBalanceUrl(event.target.value)}
                              placeholder="/user/balance"
                            />
                          </AgentFormRow>
            
                          <AgentFormRow title="消费查询 URL" desc="累计消费金额接口">
                            <Input
                              className={CONTROL_URL}
                              aria-label="消费查询 URL"
                              value={providerSpendingUrl}
                              onChange={(event) => setProviderSpendingUrl(event.target.value)}
                              placeholder="https://example.com/api/usage"
                            />
                          </AgentFormRow>
            
                          <AgentFormRow title="查询访问令牌" desc="跨域查询时填写">
                            <Input
                              className={CONTROL_SECRET}
                              type="password"
                              autoComplete="new-password"
                              aria-label="查询访问令牌"
                              value={providerUsageToken}
                              onChange={(event) => setProviderUsageToken(event.target.value)}
                              placeholder="Bearer Token"
                            />
                          </AgentFormRow>
            
                          <AgentFormRow title="计价币种" desc="输入与输出价格单位">
                            <Input
                              className={CONTROL_NUMERIC}
                              aria-label="计价币种"
                              value={providerCurrency}
                              onChange={(event) => setProviderCurrency(event.target.value)}
                              placeholder="CNY / USD"
                            />
                          </AgentFormRow>
            
                          <AgentFormRow title="输入价格" desc="每 1M tokens">
                            <Input
                              className={CONTROL_NUMERIC}
                              inputMode="decimal"
                              value={providerInputPrice}
                              onChange={(event) => setProviderInputPrice(event.target.value)}
                              placeholder="1.00"
                            />
                          </AgentFormRow>
            
                          <AgentFormRow title="输出价格" desc="每 1M tokens">
                            <Input
                              className={CONTROL_NUMERIC}
                              inputMode="decimal"
                              value={providerOutputPrice}
                              onChange={(event) => setProviderOutputPrice(event.target.value)}
                              placeholder="2.00"
                            />
                          </AgentFormRow>
            
                          <AgentFormRow title="当前余额" desc="测试当前查询配置">
                            <div className="flex w-[440px] max-w-full items-center justify-end gap-3">
                              <div className="flex min-w-0 flex-1 flex-wrap items-center gap-x-4 gap-y-1 text-[12px] text-muted-foreground" aria-live="polite">
                                <span>余额 <b className="font-medium text-foreground">{queryingQuota ? "查询中" : quotaResult ? formatQuotaBalance(quotaResult.value || null) : "待查询"}</b></span>
                                <span>已消费 <b className="font-medium text-foreground">{quotaResult ? formatQuotaSpent(quotaResult.value || null) : "待查询"}</b></span>
                              </div>
                              <Button variant="outline" icon={<RefreshCw size={13} />} loading={queryingQuota} onClick={() => void queryDraftQuota()}>查询</Button>
                            </div>
                          </AgentFormRow>
                        </AccordionContent>
                      </AccordionItem>
                    </Accordion>
            
                    <SettingsActions>
                      {editingProviderId ? (
                        <Button type="button" variant="ghost" disabled={providerEditorBusy} onClick={closeProviderEditor}>取消</Button>
                      ) : null}
                      <Button
                        type="button"
                        variant="outline"
                        icon={<PlugZap size={13} />}
                        disabled={providerEditorBusy}
                        onClick={() => void openDraftTest()}
                      >
                        测试连接
                      </Button>
                      <Button type="button" icon={<Save size={13} />} loading={savingProvider} onClick={() => void saveProvider()}>
                        保存
                      </Button>
                    </SettingsActions>
        </SettingsGroup>
  );

  return (
    <SettingsPane title="Agent 与模型">
      {!editingProviderId ? renderProviderForm() : null}

      <SettingsGroup title="已保存的 Provider" className="mb-0 min-w-0">
            
                    {savedProvidersLoading ? (
                      <div className="settings-row flex min-h-[64px] items-center gap-3 px-[17px] py-3" aria-live="polite">
                        <span className="size-8 animate-pulse rounded-lg bg-muted" />
                        <div className="flex min-w-0 flex-1 flex-col gap-1.5">
                          <span className="h-3 w-28 animate-pulse rounded bg-muted" />
                          <span className="h-2.5 w-48 max-w-full animate-pulse rounded bg-muted/80" />
                        </div>
                      </div>
                    ) : savedProviderError ? (
                      <div className="settings-row flex min-h-[64px] items-center justify-between gap-4 px-[17px] py-3">
                        <p className="min-w-0 text-[12px] leading-relaxed text-muted-foreground">已保存的 Provider 读取失败：{savedProviderError}</p>
                        <Button variant="outline" size="sm" onClick={() => void refreshSavedProviders()}>重试</Button>
                      </div>
                    ) : savedProviders.length ? (
                      savedProviders.map((provider) => {
                        const protocol = PROTOCOL_OPTIONS.find((option) => option.value === provider.kind)?.label || provider.kind;
                        return (
                          <div key={provider.id} className="settings-row relative flex min-h-[64px] items-center gap-3 px-[17px] py-3">
                            <span className="flex size-8 shrink-0 items-center justify-center rounded-lg border border-border/70 bg-muted/30">
                              <ProviderIcon brand={provider.brand || "custom"} size={18} />
                            </span>
                            <div className="flex min-w-0 flex-1 flex-col gap-0.5">
                              <div className="flex min-w-0 items-center gap-2">
                                <span className="truncate text-[13px] font-medium leading-[1.42] text-foreground">{provider.name}</span>
                                <span className="shrink-0 rounded-md bg-muted/60 px-2 py-0.5 text-[10.5px] font-medium text-muted-foreground">{protocol}</span>
                              </div>
                              <div className="truncate text-[11.5px] leading-[1.45] text-muted-foreground">
                                {provider.model || "未设置默认模型"} · {provider.baseUrl}
                              </div>
                            </div>
                            <div className="flex shrink-0 items-center gap-1">
                              <Button
                                type="button"
                                size="icon-sm"
                                variant="ghost"
                                icon={<PlugZap size={14} />}
                                aria-label={`测试 ${provider.name} 的模型`}
                                title="测试模型"
                                onClick={() => openSavedProviderTest(provider)}
                              />
                              <Button
                                type="button"
                                size="icon-sm"
                                variant="ghost"
                                icon={<Pencil size={14} />}
                                aria-label={`编辑 ${provider.name}`}
                                title="编辑"
                                onClick={() => editProvider(provider)}
                              />
                              <Button
                                type="button"
                                size="icon-sm"
                                variant="ghost"
                                icon={<Trash2 size={14} />}
                                loading={deletingProviderId === provider.id}
                                aria-label={`删除 ${provider.name}`}
                                title="删除"
                                onClick={() => setPendingDeletion(provider)}
                              />
                            </div>
                          </div>
                        );
                      })
                    ) : (
                      <div className="settings-row flex min-h-[72px] items-center gap-3 px-[17px] py-3.5">
                        <span className="flex size-8 shrink-0 items-center justify-center rounded-lg border border-dashed border-border bg-muted/20 text-muted-foreground">
                          <Plug size={14} />
                        </span>
                        <div className="min-w-0">
                          <div className="text-[13px] font-medium text-foreground">还没有已保存的 Provider</div>
                          <p className="mt-0.5 text-[11.5px] leading-relaxed text-muted-foreground">
                            {isTauri() ? "在下方完成配置并保存后，会在这里显示。" : "桌面应用中保存 Provider 后，会在这里显示。"}
                          </p>
                        </div>
                      </div>
                    )}
      </SettingsGroup>
      {testTarget ? (
        <SettingsEditDialog
          title={`测试 ${testTarget.name}`}
          busy={testingProvider}
          onClose={closeConnectionTest}
          className="!max-w-[500px]"
          overlayClassName="z-[12500]"
        >
          <div className="flex flex-col gap-5 pt-2">
            <div className="flex items-center gap-3 rounded-xl border border-border/70 bg-muted/20 p-3">
              <span className="flex size-9 shrink-0 items-center justify-center rounded-lg border border-border/70 bg-background">
                <ProviderIcon brand={testTarget.brand || "custom"} size={19} />
              </span>
              <div className="min-w-0 flex-1">
                <div className="flex min-w-0 items-center gap-2">
                  <span className="truncate text-[13px] font-medium text-foreground">{testTarget.name}</span>
                  <span className="shrink-0 rounded-md bg-muted/70 px-2 py-0.5 text-[10.5px] font-medium text-muted-foreground">
                    {PROTOCOL_OPTIONS.find((option) => option.value === testTarget.kind)?.label || testTarget.kind}
                  </span>
                </div>
                <div className="truncate text-[11.5px] text-muted-foreground">{testTarget.baseUrl}</div>
              </div>
            </div>

            <div className="grid grid-cols-[96px_minmax(0,1fr)] items-center gap-3">
              <span className="text-[12px] font-medium text-foreground">测试协议</span>
              <Select
                aria-label="测试协议"
                contentClassName="z-[13000]"
                value={testProtocol}
                disabled={testingProvider || testDiscovering}
                onChange={setTestProtocol}
                options={PROTOCOL_OPTIONS}
              />
            </div>

            <div className="grid grid-cols-[96px_minmax(0,1fr)] items-center gap-3">
              <span className="text-[12px] font-medium text-foreground">测试模型</span>
              <Select
                aria-label="测试模型"
                contentClassName="z-[13000]"
                disabled={testingProvider || testDiscovering}
                value={testModel}
                searchable
                searchPlaceholder="搜索模型"
                placeholder={testDiscovering ? "正在发现模型…" : "选择模型"}
                onChange={setTestModel}
                options={mergeModels(testTarget.models, testTarget.model ? [{ id: testTarget.model, name: testTarget.model }] : []).map((model) => ({
                  value: model.id,
                  label: model.name,
                }))}
              />
            </div>

            <label className="flex flex-col gap-1.5 text-[12px] font-medium text-foreground">
              测试消息
              <textarea
                value={testMessage}
                onChange={(event) => setTestMessage(event.target.value)}
                rows={3}
                className="min-h-[76px] w-full resize-none rounded-lg border border-input bg-background px-3 py-2 text-[12px] font-normal leading-relaxed text-foreground outline-none transition-colors placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/20"
                placeholder={TEST_MESSAGE_DEFAULT}
              />
            </label>

            {testStatus ? (
              <div role="status" className={cn(
                "rounded-xl border px-3 py-2.5 text-[12px] leading-relaxed",
                testStatus.kind === "success"
                  ? "border-emerald-500/25 bg-emerald-500/[0.08] text-emerald-700 dark:text-emerald-300"
                  : testStatus.kind === "warning"
                    ? "border-amber-500/25 bg-amber-500/[0.08] text-amber-700 dark:text-amber-300"
                    : "border-destructive/25 bg-destructive/[0.08] text-destructive",
              )}>
                {testStatus.message}
              </div>
            ) : null}

            <SettingsActions className="!px-0 !pb-0 !pt-1">
              <Button type="button" variant="outline" disabled={testingProvider} onClick={closeConnectionTest}>取消</Button>
              <Button
                type="button"
                icon={<SendHorizontal size={13} />}
                loading={testingProvider}
                disabled={testDiscovering || !testModel.trim()}
                onClick={() => void runRealConnectionTest(testTarget, testModel, testProtocol)}
              >
                发送测试
              </Button>
            </SettingsActions>
          </div>
        </SettingsEditDialog>
      ) : null}


      {editingProviderId ? (
        <SettingsEditDialog
          title={`编辑 ${savedProviders.find((provider) => provider.id === editingProviderId)?.name || editingProviderId}`}
          busy={providerEditorBusy}
          onClose={() => { if (!providerEditorBusy) closeProviderEditor(); }}
          className="!max-w-[720px]"
        >
          {renderProviderForm()}
        </SettingsEditDialog>
      ) : null}

      <ConfirmDialog
        open={Boolean(pendingDeletion)}
        title="删除供应商"
        message={`确认删除供应商「${pendingDeletion?.name || ""}」及其已保存的密钥？历史对话不会删除。`}
        confirmLabel="确认删除"
        busy={Boolean(deletingProviderId)}
        onCancel={() => { if (!deletingProviderId) setPendingDeletion(null); }}
        onConfirm={() => void confirmProviderDeletion()}
      />

      {discoveredModels ? (
        <ModelDiscoveryDialog
          models={discoveredModels}
          savedIds={selectableModels.map((model) => model.id)}
          onClose={() => setDiscoveredModels(null)}
          onAdd={(selected) => {
            setProviderModels((models) => mergeModels(models, selected));
            if (!providerModel && selected[0]) setProviderModel(selected[0].id);
            setDiscoveredModels(null);
          }}
        />
      ) : null}
    </SettingsPane>
  );
};

export default AgentPanel;

