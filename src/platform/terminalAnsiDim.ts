/**
 * SGR 2（dim / faint）在亮色主题下的可读性补偿（开发方案 6.6 / T1-9）。
 *
 * 背景（阶段 0 实测，见开发方案附录 B / V5）：
 * - 内核把 SGR 2 固定按 50% 与背景混色，且 `minimumContrastRatio` 不作用于 dim
 *   （实测亮色主题 minimumContrastRatio = 4.5 时，dim 正文对比度只有 2.98、dim 红 2.39）。
 * - 50% 的 sRGB 混色在浅底上**不可能**达到 4.5:1（即使前景是纯黑也只有约 3.7:1），
 *   所以只能不让内核做这次混色：写入前把 dim 属性换成显式前景色。
 * - 暗色主题（--terminal-dim-strength = 50%）下 50% 混色已经够读，返回 null 表示不改写。
 */
import { readTerminalDimForeground } from "./terminalTheme.ts";

/** CSI SGR 序列：ESC [ 参数 m，参数只含数字与分号。 */
const SGR_PATTERN = /\u001b\[([0-9;]*)m/g;

/** dim 前景色的缓存时长：写入是热路径，不能每次都读计算样式。 */
export const DIM_FOREGROUND_CACHE_MS = 1000;

let cached: { at: number; color: string | null } | null = null;

/** 当前主题下补偿 dim 用的前景色；null 表示交给内核默认行为（暗色主题）。 */
export function currentDimForeground(now = Date.now()): string | null {
  if (cached && now - cached.at < DIM_FOREGROUND_CACHE_MS) return cached.color;
  // 没有 DOM 时（单测 / SSR）没有主题可读，直接按不改写处理。
  const color = typeof document === "undefined" ? null : readTerminalDimForeground();
  cached = { at: now, color };
  return color;
}

/** 主题切换后想立刻生效时调用（测试与显式刷新用）。 */
export function resetDimForegroundCache(): void {
  cached = null;
}

function hexToRgb(hex: string): [number, number, number] | null {
  const match = /^#([0-9a-f]{6})$/i.exec(hex.trim());
  if (!match) return null;
  const value = Number.parseInt(match[1], 16);
  return [(value >> 16) & 0xff, (value >> 8) & 0xff, value & 0xff];
}

/**
 * 处理一条 SGR 序列的参数表：
 * - 丢掉独立的 dim 属性（参数 2），但保留 38;2 与 48;2 里表示"真彩色"的那个 2；
 * - 记录这条序列是否自己设置了前景色（30-37 / 38 / 39 / 90-97）。
 */
function rewriteSgrParams(params: string): { body: string; dim: boolean; hasForeground: boolean } {
  const list = params.length ? params.split(";") : ["0"];
  const kept: string[] = [];
  let dim = false;
  let hasForeground = false;
  for (let index = 0; index < list.length; index += 1) {
    const value = list[index];
    if (value === "38" || value === "48") {
      // 扩展色：38;5;n 或 38;2;r;g;b，后面的参数属于这条指令，原样保留。
      const mode = list[index + 1];
      const take = mode === "5" ? 2 : mode === "2" ? 4 : 0;
      kept.push(value);
      for (let step = 0; step < take && index + 1 < list.length; step += 1) {
        index += 1;
        kept.push(list[index]);
      }
      if (value === "38") hasForeground = true;
      continue;
    }
    if (value === "2") {
      dim = true;
      continue;
    }
    if (value === "39" || (Number(value) >= 30 && Number(value) <= 37) || (Number(value) >= 90 && Number(value) <= 97)) {
      hasForeground = true;
    }
    kept.push(value);
  }
  return { body: kept.length ? kept.join(";") : "0", dim, hasForeground };
}

/**
 * 把 data 里"带 dim 的 SGR 序列"改写成显式颜色的序列：
 * - 序列自己设了前景色（如 `ESC[2;31m`）→ 只去掉 dim，用原色全强度渲染；
 * - 序列没设前景色（如 `ESC[2m`、`ESC[0;2m`）→ 追加 `38;2;r;g;b`，用与 CSS 相同的
 *   `--terminal-dim-strength` 混色结果。
 * dimForeground 为 null 时原样返回（暗色主题）。函数是幂等的：改写后的输出里
 * 真彩色的 2 会被 38/48 的参数解析吃掉，不会被当成 dim 再处理一次。
 */
export function rewriteDimSequences(data: string, dimForeground: string | null): string {
  if (!dimForeground || data.indexOf("\u001b[") < 0) return data;
  const rgb = hexToRgb(dimForeground);
  if (!rgb) return data;
  return data.replace(SGR_PATTERN, (match, params: string) => {
    const { body, dim, hasForeground } = rewriteSgrParams(params);
    if (!dim) return match;
    if (hasForeground) return `\u001b[${body}m`;
    return `\u001b[${body};38;2;${rgb[0]};${rgb[1]};${rgb[2]}m`;
  });
}
