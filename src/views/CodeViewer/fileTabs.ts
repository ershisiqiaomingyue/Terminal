// 文件标签的纯逻辑：路径裁剪、标签标题、关闭后激活谁。
// 抽成纯函数是为了能单测——标签条本身只是把这里的结果画出来。

export function isMarkdownFile(path: string) {
  return /\.(?:md|markdown)$/i.test(path);
}

export function fileName(path: string) {
  return path.split(/[\\/]/).filter(Boolean).pop() || path;
}

// 相对工程根目录的路径；不在根内（符号链接指到外部）时原样返回。
export function relativeToRoot(root: string, path: string) {
  if (!root || !path) return path;
  const base = root.replace(/[\\/]+$/, "");
  if (path === base) return "";
  const slash = `${base}/`, backslash = `${base}\\`;
  if (path.startsWith(slash) || path.startsWith(backslash)) return path.slice(slash.length);
  return path;
}

// 标签标题只显示文件名；重名文件（不同目录同名）补上父目录，避免看不出是哪一个。
export function tabTitles(paths: string[]) {
  const counts = new Map<string, number>();
  for (const path of paths) {
    const name = fileName(path);
    counts.set(name, (counts.get(name) || 0) + 1);
  }
  return paths.map((path) => {
    const name = fileName(path);
    if ((counts.get(name) || 0) < 2) return name;
    const parts = path.split(/[\\/]/).filter(Boolean);
    return parts.length > 1 ? `${parts[parts.length - 2]}/${name}` : name;
  });
}

// 关掉一个标签后该激活谁：优先右边一个，没有就左边一个，全关掉则为空。
export function nextActiveAfterClose(paths: string[], closing: string, active: string) {
  if (closing !== active) return active;
  const index = paths.indexOf(closing);
  if (index < 0) return active;
  const remaining = paths.filter((path) => path !== closing);
  return remaining[Math.min(index, remaining.length - 1)] || "";
}

// 标签条上「有未保存改动」的标记，与标题里的 * 保持同一份判定。
export function tabDirtyLabel(dirty: boolean) {
  return dirty ? "*" : "";
}

// 工具栏第一行的「先项目、后具体文件路径」：项目名取根目录名，
// 文件路径相对根目录；不在根内（符号链接指到外部）时退回绝对路径。
export function projectName(root: string) {
  return root ? fileName(root) : "";
}

export function projectFilePath(root: string, path: string) {
  if (!path) return "";
  const relative = relativeToRoot(root, path);
  return relative || fileName(path);
}

// 打开一个文件：新标签放在最左，原来的整体往右推；超过上限时从最右（最久打开）淘汰，
// 但有未保存改动的标签跳过，免得默默丢掉草稿。
export const MAX_OPEN_FILES = 20;

export function insertOpenFile(paths: readonly string[], path: string, isDirty: (path: string) => boolean, limit = MAX_OPEN_FILES) {
  if (paths.includes(path)) return { open: [...paths], evicted: [] as string[] };
  const open = [path, ...paths];
  const evicted: string[] = [];
  while (open.length > Math.max(1, limit)) {
    let index = -1;
    for (let i = open.length - 1; i >= 1; i -= 1) {
      if (!isDirty(open[i])) { index = i; break; }
    }
    if (index < 0) break;
    evicted.push(...open.splice(index, 1));
  }
  return { open, evicted };
}

interface PartitionFileTabsOptions {
  gap?: number;
  moreWidth?: number;
  fallbackWidth?: number;
  minTabWidth?: number;
}

export type FileTabPartition<T> = { visible: T[]; hidden: T[]; flexible: { index: number; width: number } | null };

// 按每个标签的实际渲染宽度算「左边能摆几个」，摆不下的进右侧 › 悬浮层。
// 最后那个放不下、但剩余空间还够一个最小标签时，就把它压窄塞进来（flexible），
// 这样右侧不会留一条既放不下标签又空着的缝。
// 当前激活的标签一定可见：放不下时从尾部挤掉别的标签，避免「切了却没显示」。
export function partitionFileTabs<T extends { path: string }>(tabs: readonly T[], activePath: string, width: number, measuredWidths?: ReadonlyMap<string, number>, options: PartitionFileTabsOptions = {}): FileTabPartition<T> {
  const gap = options.gap ?? 2;
  const moreWidth = options.moreWidth ?? 30;
  const fallbackWidth = options.fallbackWidth ?? 72;
  const minTabWidth = options.minTabWidth ?? 72;
  const available = Number.isFinite(width) ? Math.max(0, width) : 0;
  const tabWidth = (tab: T) => {
    const measured = measuredWidths?.get(tab.path);
    return measured && Number.isFinite(measured) && measured > 0 ? measured : fallbackWidth;
  };
  const total = tabs.reduce((sum, tab) => sum + tabWidth(tab), 0) + Math.max(0, tabs.length - 1) * gap;
  if (total <= available) return { visible: [...tabs], hidden: [] as T[], flexible: null };
  if (!tabs.length) return { visible: [] as T[], hidden: [] as T[], flexible: null };

  const budget = Math.max(0, available - moreWidth - gap);
  const visible: T[] = [];
  let used = 0;
  let flexible: { index: number; width: number } | null = null;
  for (const tab of tabs) {
    const gapCost = visible.length ? gap : 0;
    const next = used + gapCost + tabWidth(tab);
    if (next <= budget) { visible.push(tab); used = next; continue; }
    const room = budget - used - gapCost;
    if (room >= minTabWidth) { flexible = { index: visible.length, width: room }; visible.push(tab); }
    break;
  }
  const active = tabs.find((tab) => tab.path === activePath);
  if (active && !visible.some((tab) => tab.path === activePath)) {
    visible.push(active);
    flexible = null;
    const totalOf = () => visible.reduce((sum, tab, index) => sum + (index ? gap : 0) + tabWidth(tab), 0);
    while (visible.length > 1 && totalOf() > budget) visible.splice(visible.length - 2, 1);
    const sum = totalOf();
    // 仍差一点点时把激活标签自己压窄，保证它能完整显示。
    if (sum > budget) {
      const others = sum - tabWidth(visible[visible.length - 1]) - (visible.length > 1 ? gap : 0);
      flexible = { index: visible.length - 1, width: Math.max(minTabWidth, budget - others) };
    }
  }
  if (!visible.length) visible.push(tabs[0]);
  const visiblePaths = new Set(visible.map((tab) => tab.path));
  return { visible, hidden: tabs.filter((tab) => !visiblePaths.has(tab.path)), flexible };
}

// 标签右键菜单的批量关闭：关右侧 / 关其他 / 关全部。
// 顺序就是标签条上的顺序（最新打开的排在第一个），所以「右侧」= 该标签之后的所有标签。
export type TabCloseAction = "right" | "others" | "all";

export function pathsToClose(paths: readonly string[], target: string, action: TabCloseAction) {
  if (action === "all") return [...paths];
  if (action === "others") return paths.filter((path) => path !== target);
  const index = paths.indexOf(target);
  return index < 0 ? [] : paths.slice(index + 1);
}

// 关掉一个标签时要不要提示：只有「有未保存改动」的标签才需要。
export function dirtyPaths(files: Record<string, { content: string; draft: string }>, openFiles: readonly string[]) {
  return openFiles.filter((path) => {
    const file = files[path];
    return Boolean(file) && file.content !== file.draft;
  });
}
