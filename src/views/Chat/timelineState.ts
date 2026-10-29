export function readTimelineExpanded(value: unknown): Record<string, boolean> | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const expanded = Object.fromEntries(Object.entries(value)
    .filter((entry): entry is [string, boolean] => typeof entry[1] === "boolean"));
  return Object.keys(expanded).length ? expanded : undefined;
}

export function updateTimelineExpanded(current: Record<string, boolean> | undefined, key: string, expanded: boolean) {
  const next = { ...(current || {}) };
  next[key] = expanded;
  return next;
}

export function timelineStateKey(message: { id: string; turnId?: string }) {
  return message.turnId ? `turn:${message.turnId}` : `message:${message.id}`;
}

/**
 * AI 回复结束后该收起的过程折叠区。
 *
 * 一轮在跑的时候思考、指令、工具输出是自动展开的；落定之后折回「已处理 X 秒」
 * 那一行，等价于用户手动点了一次折叠头。返回需要收起的 key，
 * 调用方把结果记进 `alreadySettled`，于是每个轮次只会被自动收起一次，
 * 事后用户主动点开不会被下一帧又按回去。
 */
export function collapseSettledTimelines(
  current: Record<string, boolean> | undefined,
  keys: readonly string[],
  pendingKeys: ReadonlySet<string>,
  alreadySettled: ReadonlySet<string>,
): string[] {
  const settled: string[] = [];
  for (const key of keys) {
    if (pendingKeys.has(key) || alreadySettled.has(key)) continue;
    // 只为「这一刻确实展开着」的轮次留痕：历史消息本来就没展开，
    // 若也记进 settled，之后懒加载恢复的展开态就再也收不回去了。
    if (current?.[key] !== true) continue;
    settled.push(key);
  }
  return settled;
}

/** 把一批轮次的过程区折叠起来；没有要收的就原样返回 `current`。 */
export function collapseTimelines(current: Record<string, boolean> | undefined, keys: readonly string[]) {
  if (!keys.length) return current;
  const next = { ...(current || {}) };
  for (const key of keys) next[key] = false;
  return next;
}

export function retainLiveTimelines(current: Record<string, boolean> | undefined, activeKeys: ReadonlySet<string>) {
  let next = current;
  for (const key of activeKeys) {
    // Remember the automatically visible process across completion/history
    // hydration, while respecting a user's explicit collapse.
    if (current?.[key] !== undefined) continue;
    if (next === current) next = { ...current };
    next![key] = true;
  }
  return next;
}
