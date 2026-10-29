import React, { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Check, ChevronDown, Plus, RotateCcw, Search, Settings2 } from "lucide-react";
import type { AgentModelSettings, AgentProvider } from "@/platform/api";
import { effectiveReasoningEffort, type ReasoningEffort } from "@/platform/modelCapabilities";
import { Input } from "@/components/ui/Input";
import { Button } from "@/components/ui/Button";
import { ProviderIcon } from "@/components/ProviderIcon";
import "./agent-model-picker.css";

export type AgentModelSelection = {
  providerId: string;
  modelId: string;
};

/**
 * Hard ceiling for the provider / model option panels.
 *
 * Placement already clamps a panel to the space between its anchor and the
 * window edge. On a tall window that budget is large, so a provider with
 * dozens of models grew until the list nearly filled the screen. A fixed cap
 * keeps the popover a predictable size and makes the list scroll internally.
 */
const OPTIONS_MAX_HEIGHT = 300;

const INFERENCE_LEVELS = [
  { value: "low", label: "低" },
  { value: "medium", label: "中" },
  { value: "high", label: "高" },
  { value: "xhigh", label: "极高" },
  { value: "max", label: "最高" },
] as const;

interface AgentModelPickerProps {
  providers: AgentProvider[];
  selection: AgentModelSelection;
  disabled?: boolean;
  inferenceLevel?: ReasoningEffort;
  onChange: (selection: AgentModelSelection) => void;
  onInferenceLevelChange?: (level: ReasoningEffort) => void;
  onOpenSettings: () => void;
  className?: string;
}

function formatContextWindow(value?: number | null) {
  if (!value || value <= 0) return "";
  if (value >= 1_000_000) {
    const mega = value / 1_000_000;
    return `${Number.isInteger(mega) ? mega : Number(mega.toFixed(2))}M`;
  }
  if (value >= 1_000) {
    const kilo = value / 1_000;
    return `${Number.isInteger(kilo) ? kilo : kilo.toFixed(1)}K`;
  }
  return String(value);
}

function providerModels(provider?: AgentProvider): AgentModelSettings[] {
  if (!provider) return [];
  return provider.models.length
    ? provider.models
    : provider.model
      ? [{ id: provider.model, name: provider.model, contextWindow: provider.contextWindow }]
      : [];
}

export const AgentModelPicker: React.FC<AgentModelPickerProps> = ({
  providers,
  selection,
  disabled = false,
  inferenceLevel,
  onChange,
  onInferenceLevelChange,
  onOpenSettings,
  className = "",
}) => {
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const optionsRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [menuPlacement, setMenuPlacement] = useState<{ top: number; right: number; maxHeight: number; side: "above" | "below" }>({ top: 0, right: 16, maxHeight: 320, side: "above" });
  const [optionsPlacement, setOptionsPlacement] = useState({ top: 8, right: 16, maxHeight: 240 });
  const [providerMenuOpen, setProviderMenuOpen] = useState(false);
  const [modelMenuOpen, setModelMenuOpen] = useState(false);
  const [inferenceMenuOpen, setInferenceMenuOpen] = useState(false);
  const [query, setQuery] = useState("");
  const provider = providers.find((item) => item.id === selection.providerId) || providers[0];
  const models = providerModels(provider);
  const model = models.find((item) => item.id === selection.modelId) || models[0];
  const inferenceLevels = model?.reasoningEfforts
    ? INFERENCE_LEVELS.filter((level) => model.reasoningEfforts!.includes(level.value))
    : INFERENCE_LEVELS.filter((level) => level.value !== "xhigh");
  const currentEffort = inferenceLevel ? effectiveReasoningEffort(model, inferenceLevel) : null;
  const inferenceEnabled = Boolean(model?.supportsReasoning && currentEffort && onInferenceLevelChange && inferenceLevels.length > 1);
  const inferenceIndex = Math.max(0, inferenceLevels.findIndex((level) => level.value === currentEffort));
  const inferenceLabel = inferenceLevels[inferenceIndex]?.label || "";
  const visibleModels = useMemo(() => {
    const normalized = query.trim().toLowerCase();
    return normalized ? models.filter((item) => item.name.toLowerCase().includes(normalized) || item.id.toLowerCase().includes(normalized)) : models;
  }, [models, query]);

  const close = () => {
    setOpen(false);
    setProviderMenuOpen(false);
    setModelMenuOpen(false);
    setInferenceMenuOpen(false);
    setQuery("");
  };

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node) && !menuRef.current?.contains(event.target as Node)) close();
    };
    const onEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopImmediatePropagation();
      close();
      triggerRef.current?.focus({ preventScroll: true });
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    document.addEventListener("keydown", onEscape, true);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("keydown", onEscape, true);
    };
  }, [open]);

  useLayoutEffect(() => {
    if (!open || !triggerRef.current || !menuRef.current) return;
    const place = () => {
      const trigger = triggerRef.current?.getBoundingClientRect();
      const menu = menuRef.current;
      if (!trigger || !menu) return;
      const above = Math.max(0, trigger.top - 15);
      const below = Math.max(0, window.innerHeight - trigger.bottom - 15);
      // scrollHeight excludes the border; maxHeight uses the border box.
      const height = menu.scrollHeight + menu.offsetHeight - menu.clientHeight;
      const side = above >= height || above >= below ? "above" : "below";
      const maxHeight = Math.max(1, Math.min(height, side === "above" ? above : below));
      const top = side === "above"
        ? Math.max(8, trigger.top - Math.min(height, maxHeight) - 7)
        : trigger.bottom + 7;
      setMenuPlacement({ top, right: Math.max(8, window.innerWidth - trigger.right), maxHeight, side });
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [open, providerMenuOpen, modelMenuOpen, inferenceMenuOpen, providers.length, models.length]);

  useLayoutEffect(() => {
    if (!open || !(providerMenuOpen || modelMenuOpen || inferenceMenuOpen)) return;
    const place = () => {
      const options = optionsRef.current;
      const anchor = options?.closest<HTMLElement>(".agent-model-menu-row-wrap")?.getBoundingClientRect();
      if (!options || !anchor) return;
      const above = Math.max(0, anchor.top - 14);
      const below = Math.max(0, window.innerHeight - anchor.bottom - 14);
      const naturalHeight = options.scrollHeight + options.offsetHeight - options.clientHeight;
      const side = above >= naturalHeight || above >= below ? "above" : "below";
      const available = side === "above" ? above : below;
      const maxHeight = Math.max(1, Math.min(naturalHeight, available, OPTIONS_MAX_HEIGHT));
      const menuRight = menuRef.current?.getBoundingClientRect().right ?? anchor.right;
      const top = side === "above" ? Math.max(8, anchor.top - maxHeight - 6) : anchor.bottom + 6;
      setOptionsPlacement({ top, right: Math.max(8, window.innerWidth - menuRight), maxHeight });
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [open, providerMenuOpen, modelMenuOpen, inferenceMenuOpen, query, visibleModels.length, providers.length]);

  useEffect(() => {
    if (!inferenceEnabled) setInferenceMenuOpen(false);
  }, [inferenceEnabled]);

  useEffect(() => {
    if (!providers.length) setProviderMenuOpen(false);
    if (!models.length) setModelMenuOpen(false);
  }, [providers.length, models.length]);

  const chooseProvider = (nextProvider: AgentProvider) => {
    const choices = providerModels(nextProvider);
    const fallback = choices.find((item) => item.id === nextProvider.model) || choices[0];
    onChange({ providerId: nextProvider.id, modelId: fallback?.id || "" });
    setProviderMenuOpen(false);
    setModelMenuOpen(false);
    setInferenceMenuOpen(false);
    setQuery("");
  };

  const chooseModel = (modelId: string) => {
    onChange({ providerId: provider?.id || "", modelId });
    setModelMenuOpen(false);
    setInferenceMenuOpen(false);
    setQuery("");
  };

  const openSettings = () => {
    close();
    onOpenSettings();
  };

  return <div ref={rootRef} className={`agent-model-picker ${className}`.trim()} onKeyDown={(event) => {
    if (event.key === "Escape" && open) {
      event.stopPropagation();
      close();
      rootRef.current?.querySelector<HTMLButtonElement>(".agent-model-trigger")?.focus();
    }
  }}>
    <button
      ref={triggerRef}
      type="button"
      className={`agent-model-trigger${open ? " open" : ""}`}
      title="选择供应商和模型"
      aria-expanded={open}
      disabled={disabled}
      onClick={() => {
        if (disabled) return;
        setOpen((value) => !value);
        setProviderMenuOpen(false);
        setModelMenuOpen(false);
        setInferenceMenuOpen(false);
        setQuery("");
      }}
    >
      {provider && <ProviderIcon brand={model?.iconBrand || provider.brand || "custom"} size={16} />}
      <span className="agent-model-trigger-label">{model?.name || "未配置模型"}</span>
      {model?.contextWindow ? <em className="agent-model-context-badge" title={`上下文 ${model.contextWindow.toLocaleString()} tokens`}>{formatContextWindow(model.contextWindow)}</em> : null}
      {inferenceEnabled && <span className="agent-model-trigger-effort" title={`推理强度：${inferenceLabel}`}>{inferenceLabel}</span>}
      <ChevronDown size={14} className={open ? "rotated" : ""} />
    </button>
    {open && createPortal(<div className="agent-model-picker agent-model-portal"><div
      ref={menuRef}
      className={`agent-model-menu is-${menuPlacement.side}`}
      style={{ position: "fixed", top: `${menuPlacement.top}px`, right: `${menuPlacement.right}px`, bottom: "auto", maxHeight: `${menuPlacement.maxHeight}px`, overflow: "visible" }}
    >
      <div className="agent-model-menu-row-wrap">
        <button type="button" className="agent-model-menu-row" disabled={!providers.length} onClick={() => { setProviderMenuOpen((value) => !value); setModelMenuOpen(false); setInferenceMenuOpen(false); }}>
          <span>供应商</span>
          <span className="agent-model-menu-value">{provider?.name || "未配置 Provider"}</span>
          <ChevronDown size={14} className={providerMenuOpen ? "rotated" : ""} />
        </button>
        {providerMenuOpen && providers.length > 0 && <div ref={optionsRef} className="agent-model-options agent-provider-options" style={{ position: "fixed", top: `${optionsPlacement.top}px`, right: `${optionsPlacement.right}px`, bottom: "auto", maxHeight: `${optionsPlacement.maxHeight}px` }}>
          {providers.map((item) => <button key={item.id} type="button" className={`agent-model-option agent-provider-option${item.id === provider?.id ? " selected" : ""}`} onClick={() => chooseProvider(item)}>
            <ProviderIcon brand={item.brand || "custom"} size={20} />
            <span className="agent-provider-label"><b>{item.name}</b><small>{item.kind.replace(/_/g, " ")} · {providerModels(item).length} 个模型</small></span>
            {item.id === provider?.id && <Check size={14} />}
          </button>)}
          {!providers.length && <span className="agent-model-option-empty">未配置 Provider</span>}
          <div className="agent-model-menu-divider" />
          <button type="button" className="agent-model-option agent-add-provider-option" onClick={openSettings}><Settings2 size={13} /><span>添加供应商</span><Plus size={13} /></button>
        </div>}
      </div>
      <div className="agent-model-menu-row-wrap">
        <button type="button" className="agent-model-menu-row" disabled={!models.length} onClick={() => { setModelMenuOpen((value) => !value); setProviderMenuOpen(false); setInferenceMenuOpen(false); setQuery(""); }}>
          <span>模型</span>
          <span className="agent-model-menu-value">{model?.name || "未配置模型"}</span>
          <ChevronDown size={14} className={modelMenuOpen ? "rotated" : ""} />
        </button>
        {modelMenuOpen && models.length > 0 && <div ref={optionsRef} className="agent-model-options agent-model-list-options" style={{ position: "fixed", top: `${optionsPlacement.top}px`, right: `${optionsPlacement.right}px`, bottom: "auto", maxHeight: `${optionsPlacement.maxHeight}px` }}>
          <div className="agent-model-search">
            <Input
              size="sm"
              prefixIcon={<Search size={13} />}
              value={query}
              placeholder="搜索模型"
              onChange={(event) => setQuery(event.target.value)}
            />
          </div>
          {visibleModels.map((item) => <button key={item.id} type="button" className={`agent-model-option agent-model-option-with-meta${item.id === model?.id ? " selected" : ""}`} onClick={() => chooseModel(item.id)}>
            <ProviderIcon brand={item.iconBrand || provider?.brand || "custom"} size={16} />
            <span className="agent-model-option-label"><b title={item.name}>{item.name}</b>
              {item.billing?.badges.map((badge) => <span key={badge} className={`agent-model-promotion${item.billing?.free ? " is-free" : ""}`}>{badge}</span>)}
            </span>
            {item.contextWindow ? <em className="agent-model-context-badge" title={`上下文 ${item.contextWindow.toLocaleString()} tokens`}>{formatContextWindow(item.contextWindow)}</em> : null}
            {item.billing && <span className="agent-model-multiplier" title={item.billing.rateUnknown ? "优惠已结束，等待刷新积分倍率" : "积分倍率"}>
              {!item.billing.rateUnknown && item.billing.multiplier != null && Number.isFinite(item.billing.multiplier) ? `${item.billing.multiplier.toFixed(2)}x` : "—"}
            </span>}
            <span className="agent-model-selection-mark">{item.id === model?.id && <Check size={14} />}</span>
          </button>)}
          {!visibleModels.length && <span className="agent-model-option-empty">{models.length ? "没有匹配的模型" : "该 Provider 未保存模型"}</span>}
          <div className="agent-model-menu-divider" />
          <button type="button" className="agent-model-option agent-add-provider-option" onClick={openSettings}><Settings2 size={13} /><span>添加供应商</span><Plus size={13} /></button>
        </div>}
      </div>
      {inferenceLevel && onInferenceLevelChange && <div className="agent-model-menu-row-wrap">
        <button type="button" className="agent-model-menu-row" disabled={!inferenceEnabled} aria-expanded={inferenceMenuOpen} onClick={() => {
          setInferenceMenuOpen((value) => !value);
          setProviderMenuOpen(false);
          setModelMenuOpen(false);
        }}>
          <span>推理强度</span>
          <span className="agent-model-menu-value">{inferenceEnabled ? inferenceLabel : model?.supportsReasoning ? "固定思考模式" : "不可用"}</span>
          <ChevronDown size={14} className={inferenceMenuOpen ? "rotated" : ""} />
        </button>
        {inferenceMenuOpen && inferenceEnabled && <div ref={optionsRef} className="agent-model-options agent-inference-options" style={{ position: "fixed", top: `${optionsPlacement.top}px`, right: `${optionsPlacement.right}px`, bottom: "auto", maxHeight: `${optionsPlacement.maxHeight}px` }}>
          <div className="agent-inference-heading">
            <span>推理强度</span>
            <Button type="button" variant="subtle" size="xs" className="agent-inference-reset" title="恢复默认推理强度" aria-label="恢复默认推理强度" icon={<RotateCcw size={14} />} onClick={() => onInferenceLevelChange(inferenceLevels.find((level) => level.value === "medium")?.value || inferenceLevels[0].value)} />
          </div>
          <div className="agent-inference-model" title={model?.name}>{model?.name}</div>
          <div className={`agent-inference-track${inferenceIndex === 0 ? " at-minimum" : ""}`} style={{ "--inference-progress": inferenceIndex / Math.max(1, inferenceLevels.length - 1) } as React.CSSProperties}>
            <span className="agent-inference-track-base" aria-hidden="true" />
            <span className="agent-inference-track-fill" aria-hidden="true" />
            <div className="agent-inference-ticks" aria-hidden="true">{inferenceLevels.map((level) => <i key={level.value} />)}</div>
            <span className="agent-inference-thumb" aria-hidden="true" />
            <input type="range" min={0} max={inferenceLevels.length - 1} step={1} value={inferenceIndex} aria-label="推理强度" aria-valuetext={inferenceLabel} onChange={(event) => onInferenceLevelChange(inferenceLevels[Number(event.target.value)].value)} />
          </div>
          <div className="agent-inference-labels" role="group" aria-label="推理强度档位">{inferenceLevels.map((level) => <button type="button" key={level.value} aria-pressed={level.value === currentEffort} onClick={() => onInferenceLevelChange(level.value)}>{level.label}</button>)}</div>
        </div>}
      </div>}
      <div className="agent-model-menu-divider" />
      <button type="button" className="agent-model-option agent-add-provider-option" onClick={openSettings}><Settings2 size={13} /><span>添加供应商</span><Plus size={13} /></button>
    </div></div>, document.body)}
  </div>;
};
