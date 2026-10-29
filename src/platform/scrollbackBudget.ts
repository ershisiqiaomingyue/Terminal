/**
 * 终端滚动缓冲的字节预算（开发方案 6.7）。
 *
 * 自研内核（WASM）的每个 cell 约 8 字节，行数上限本身不控制内存，
 * 因此必须按字节换算行数，并在多终端之间做公平分配。
 */
export const APPROX_BYTES_PER_CELL = 8;
/** 单个 WebView 内所有终端共享的默认总预算。 */
export const DEFAULT_BUDGET_BYTES = 192 * 1024 * 1024;
/** 每个终端至少保留的字节数。 */
export const FLOOR_BYTES = 2 * 1024 * 1024;
export const MIN_BYTES = 1 * 1024 * 1024;
export const MAX_BYTES = 64 * 1024 * 1024;
export const MAX_LINES = 100_000;
/** 兜底列数：拿不到真实列宽时按 120 列估算。 */
export const FALLBACK_COLS = 120;

/**
 * 同时打开的终端页签上限（开发方案 6.7.2：WASM 内存只增不减，必须限制并发数）。
 * ADB Shell 视图与对话终端视图分别计数。
 */
export const MAX_ADB_TERMINALS = 8;
export const MAX_CHAT_TERMINALS = 4;

function clampNumber(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) return min;
  return Math.min(max, Math.max(min, value));
}

/** 把字节预算换算成该终端允许的 scrollback 行数。 */
export function linesForBudget(bytes: number, cols: number): number {
  const safeBytes = clampNumber(bytes, 0, Number.MAX_SAFE_INTEGER);
  const cells = Math.floor(safeBytes / APPROX_BYTES_PER_CELL);
  const safeCols = Math.max(1, Math.floor(Number.isFinite(cols) ? cols : FALLBACK_COLS));
  return Math.max(0, Math.min(MAX_LINES, Math.floor(cells / safeCols)));
}

/** 多终端公平分配（输入输出都是字节数）：先给每个终端 floor，剩余按 max-min 比例分配。 */
export function allocate(requests: number[], budget: number = DEFAULT_BUDGET_BYTES): number[] {
  const safeBudget = clampNumber(budget, 0, Number.MAX_SAFE_INTEGER);
  const count = Math.max(1, requests.length);
  if (!requests.length) return [];
  const floorEach = Math.min(FLOOR_BYTES, safeBudget);
  // 预算连 floor 都不够时平均分配，避免某个终端独吞。
  if (floorEach * count >= safeBudget) return requests.map(() => Math.floor(safeBudget / count));
  const remaining = safeBudget - floorEach * count;
  const wants = requests.map((request) => Math.max(0, clampNumber(request, 0, MAX_BYTES) - floorEach));
  const wantSum = wants.reduce((total, value) => total + value, 0);
  if (wantSum <= 0) return requests.map(() => floorEach);
  return requests.map((_, index) => floorEach + Math.floor(remaining * (wants[index] / wantSum)));
}

export interface ScrollbackLinesInput {
  /** 用户在设置里选的行数上限。 */
  requested: number;
  /** 当前终端的列数；小于 20 时按 80 估算。 */
  cols?: number;
  /** 同一 WebView 内同时打开的终端数量，用于把总预算摊薄。 */
  terminals?: number;
  /** 覆盖默认总预算（测试用）。 */
  budget?: number;
}

/**
 * 计算新终端实际使用的 scrollback 行数：取"用户设置"与"字节预算"的较小值。
 * WASM 内存不可收缩，因此这个值只在创建时确定，运行期不缩容（见 6.7.2）。
 */
export function scrollbackLinesForSession(input: ScrollbackLinesInput): number {
  const requested = Math.max(0, Math.floor(clampNumber(input.requested, 0, MAX_LINES)));
  const cols = Math.max(20, Math.floor(input.cols ?? FALLBACK_COLS));
  const terminals = Math.max(1, Math.floor(input.terminals ?? 1));
  const budget = clampNumber(input.budget ?? DEFAULT_BUDGET_BYTES, 0, Number.MAX_SAFE_INTEGER);
  const share = allocate(new Array<number>(terminals).fill(MAX_BYTES), budget)[0] ?? MIN_BYTES;
  const budgetLines = linesForBudget(Math.max(MIN_BYTES, Math.min(share, MAX_BYTES)), cols);
  return Math.max(0, Math.min(requested, budgetLines));
}
