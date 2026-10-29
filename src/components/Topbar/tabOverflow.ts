const TAB_GAP = 6;
const MIN_TAB_WIDTH = 108;
const MORE_WIDTH = 32;

export function promoteTab<T extends { id: string }>(tabs: readonly T[], id: string): T[] {
  const tab = tabs.find((item) => item.id === id);
  return tab ? [tab, ...tabs.filter((item) => item.id !== id)] : [...tabs];
}

interface PartitionOptions {
  maxVisible?: number;
  gap?: number;
  moreWidth?: number;
}

export function partitionTabs<T extends { id: string }>(tabs: readonly T[], activeId: string, width: number, measuredWidths?: ReadonlyMap<string, number>, options: PartitionOptions = {}) {
  const limit = Math.max(1, options.maxVisible ?? Infinity);
  const gap = options.gap ?? TAB_GAP;
  const moreWidth = options.moreWidth ?? MORE_WIDTH;
  const available = Number.isFinite(width) ? Math.max(0, width) : 0;
  const tabWidth = (tab: T) => {
    const measured = measuredWidths?.get(tab.id);
    return measured && Number.isFinite(measured) && measured > 0 ? measured : MIN_TAB_WIDTH;
  };
  const total = tabs.reduce((sum, tab) => sum + tabWidth(tab), 0) + Math.max(0, tabs.length - 1) * gap;
  if (total <= available && tabs.length <= limit) {
    return { visible: [...tabs], hidden: [] as T[] };
  }

  const active = tabs.find((tab) => tab.id === activeId);
  const visibleIds = new Set(active ? [active.id] : []);
  let used = active ? tabWidth(active) : 0;
  for (const tab of tabs) {
    if (visibleIds.has(tab.id)) continue;
    const next = used + (visibleIds.size ? gap : 0) + tabWidth(tab);
    if (visibleIds.size >= limit || next > available - moreWidth - gap) break;
    visibleIds.add(tab.id);
    used = next;
  }
  if (!visibleIds.size && tabs.length) visibleIds.add(tabs[0].id);
  return {
    visible: tabs.filter((tab) => visibleIds.has(tab.id)),
    hidden: tabs.filter((tab) => !visibleIds.has(tab.id)),
  };
}
