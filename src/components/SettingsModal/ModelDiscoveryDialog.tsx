import { useMemo, useState } from "react";
import { Search } from "lucide-react";
import { Button, Checkbox, Input } from "rain-ui";
import type { AgentModelSettings } from "@/platform/api";
import { SettingsEditDialog } from "./SettingsEditDialog";

import { parseTokenCount, tokenCountInputValue } from "./modelWindowFormat";

/** 发现之后就地补写的上限；键是模型 id，未出现的键表示沿用接口返回值。 */
type ModelLimits = { contextWindow?: number | null; maxOutputTokens?: number | null };

export function ModelDiscoveryDialog({ models, savedIds, onClose, onAdd }: {
  models: AgentModelSettings[];
  savedIds: string[];
  onClose: () => void;
  onAdd: (models: AgentModelSettings[]) => void;
}) {
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(() => new Set(savedIds));
  // 上游 /models 大多不返回窗口，所以发现之后就允许在这里补：上下文长度（输入）
  // 与单轮输出上限（输出）。留空 = 自动：上下文按已知窗口或 64K 估算，输出上限
  // 按上下文窗口推算。
  const [limits, setLimits] = useState<Record<string, ModelLimits>>({});
  const visible = useMemo(() => {
    const search = query.trim().toLowerCase();
    return models.filter((model) => `${model.id} ${model.name}`.toLowerCase().includes(search));
  }, [models, query]);
  const count = models.filter((model) => selected.has(model.id)).length;
  const selectedVisible = visible.filter((model) => selected.has(model.id)).length;
  // 用户改过就以改过的为准（包括清空成「自动」，用 null 记住），没改过才看接口返回值。
  const limitValue = (model: AgentModelSettings, key: keyof ModelLimits) => (limits[model.id] && key in limits[model.id] ? limits[model.id][key] : model[key]);
  // 缺窗口的模型运行时只会拿到 64K 保守预算，在这里数出来提醒用户补填。
  const missingWindow = useMemo(
    () => models.filter((model) => !limitValue(model, "contextWindow")).length,
    // limitValue 只读 limits 与 models，两者都在依赖里。
    [models, limits],
  );
  const updateLimit = (id: string, patch: ModelLimits) => setLimits((previous) => ({ ...previous, [id]: { ...previous[id], ...patch } }));
  const toggle = (ids: string[], checked: boolean) => setSelected((previous) => {
    const next = new Set(previous);
    ids.forEach((id) => checked ? next.add(id) : next.delete(id));
    return next;
  });

  return <SettingsEditDialog title="选择要添加的模型" onClose={onClose} className="!max-w-[660px]">
    <div className="mb-3 flex items-center gap-3.5">
      <Input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜索模型名称或 ID" aria-label="搜索发现的模型" prefixIcon={<Search size={14} />} className="min-w-0 flex-1" />
      <Checkbox label={query ? "全选结果" : "全选"} checked={visible.length > 0 && selectedVisible === visible.length} className="shrink-0 whitespace-nowrap"
        indeterminate={selectedVisible > 0 && selectedVisible < visible.length} disabled={!visible.length}
        onChange={(checked) => toggle(visible.map((model) => model.id), checked)} />
    </div>
    <div className="max-h-[min(380px,50dvh)] overflow-y-auto [overscroll-behavior:contain] [scrollbar-gutter:stable]" role="group" aria-label="发现的模型">
      {visible.map((model) => <div key={model.id} className="flex min-h-[40px] items-center gap-2 rounded-md p-2 hover:bg-muted/60" title={savedIds.includes(model.id) ? "已在模型目录中" : undefined}>
        <label className="flex min-w-0 flex-1 cursor-pointer items-center gap-2.5">
          <Checkbox checked={selected.has(model.id)} aria-label={model.id} onChange={(checked) => toggle([model.id], checked)} />
          <span className="flex min-w-0 flex-1 flex-col gap-[3px] [overflow-wrap:anywhere]"><b className="text-xs font-medium">{model.name || model.id}</b>{model.name && model.name !== model.id && <small className="text-[11px] text-muted-foreground">{model.id}</small>}</span>
        </label>
        <Input inputMode="numeric" className="w-[88px] shrink-0 text-center text-[11px]" aria-label={`${model.id} 上下文长度`}
          title="上下文长度（输入 tokens）：留空按已知窗口或 64K 估算" placeholder="上下文"
          value={tokenCountInputValue(limitValue(model, "contextWindow"))}
          onChange={(event) => updateLimit(model.id, { contextWindow: parseTokenCount(event.target.value) })} />
        <Input inputMode="numeric" className="w-[88px] shrink-0 text-center text-[11px]" aria-label={`${model.id} 输出上限`}
          title="单轮输出上限（tokens）：留空按上下文窗口推算" placeholder="输出上限"
          value={tokenCountInputValue(limitValue(model, "maxOutputTokens"))}
          onChange={(event) => updateLimit(model.id, { maxOutputTokens: parseTokenCount(event.target.value) })} />
      </div>)}
      {!visible.length && <div className="px-2 py-6 text-center text-[11px] text-muted-foreground">没有匹配的模型</div>}
    </div>
    <footer className="mt-4 flex items-center justify-end gap-2"><span className="mr-auto text-[11px] text-muted-foreground">已选 {count} / {models.length}{missingWindow > 0 && ` · ${missingWindow} 个未返回上下文，可直接填写`}</span><Button variant="outline" onClick={onClose}>取消</Button>
      <Button variant="default" disabled={!count} onClick={() => onAdd(models.filter((model) => selected.has(model.id)).map((model) => ({ ...model, ...limits[model.id] })))}>添加所选</Button>
    </footer>
  </SettingsEditDialog>;
}
