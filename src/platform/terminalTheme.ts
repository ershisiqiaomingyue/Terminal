import type { TerminalTheme } from "./terminalAdapter.ts";

type Rgb = { r: number; g: number; b: number };

function parseColor(value?: string): Rgb | undefined {
  if (!value) return undefined;
  const text = value.trim();
  const hex = /^#([\da-f]{3}|[\da-f]{6})$/i.exec(text);
  if (hex) {
    const digits = hex[1].length === 3 ? hex[1].split("").map((part) => part + part).join("") : hex[1];
    return {
      r: Number.parseInt(digits.slice(0, 2), 16),
      g: Number.parseInt(digits.slice(2, 4), 16),
      b: Number.parseInt(digits.slice(4, 6), 16),
    };
  }
  const rgb = /^rgba?\(([^)]+)\)$/i.exec(text);
  if (!rgb) return undefined;
  const parts = rgb[1].split(/[,/\s]+/).filter(Boolean).map((part) => Number.parseFloat(part));
  if (parts.length < 3 || parts.some((part) => !Number.isFinite(part))) return undefined;
  return { r: parts[0], g: parts[1], b: parts[2] };
}

function toHex({ r, g, b }: Rgb) {
  const channel = (value: number) => Math.max(0, Math.min(255, Math.round(value))).toString(16).padStart(2, "0");
  return `#${channel(r)}${channel(g)}${channel(b)}`;
}

function mixColor(from: Rgb, to: Rgb, weight: number): Rgb {
  return {
    r: from.r + (to.r - from.r) * weight,
    g: from.g + (to.g - from.g) * weight,
    b: from.b + (to.b - from.b) * weight,
  };
}

function relativeLuminance({ r, g, b }: Rgb) {
  const channel = (value: number) => {
    const scaled = value / 255;
    return scaled <= 0.04045 ? scaled / 12.92 : ((scaled + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

function contrastRatio(first: Rgb, second: Rgb) {
  const a = relativeLuminance(first);
  const b = relativeLuminance(second);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

/** xterm 的 24 级灰阶（索引 232–255）在浅底上会淡到几乎看不见。
 *  浅底主题下按前景/背景重新铺一条「越靠近背景越浅、但都保住 AA」的灰阶，
 *  深底主题返回 undefined，保留 xterm 自带灰阶（那边本来就够用）。 */
export function buildExtendedAnsi(foreground?: string, background?: string): string[] | undefined {
  const ink = parseColor(foreground);
  const paper = parseColor(background);
  if (!ink || !paper) return undefined;
  if (relativeLuminance(paper) <= 0.5) return undefined;
  const floor = 4.6;
  if (contrastRatio(ink, paper) <= floor) return undefined;
  let low = 0;
  let high = 1;
  for (let step = 0; step < 24; step += 1) {
    const middle = (low + high) / 2;
    if (contrastRatio(mixColor(ink, paper, middle), paper) >= floor) low = middle;
    else high = middle;
  }
  return Array.from({ length: 24 }, (_, index) => toHex(mixColor(ink, paper, (index / 23) * low)));
}

/** 把任意 CSS 颜色写法收敛成 `#rrggbb`，供设置面板的颜色选择器使用。 */
export function normalizeHexColor(value?: string): string | undefined {
  const rgb = parseColor(value);
  return rgb ? toHex(rgb) : undefined;
}

/** Read the active CSS palette, including when returning to a built-in theme. */
export function readTerminalTheme(element: Element = document.documentElement): TerminalTheme {
  const style = getComputedStyle(element);
  const color = (token: string) => style.getPropertyValue(token).trim() || undefined;
  const background = color("--terminal-canvas-bg");
  const foreground = color("--terminal-foreground") || color("--text-primary");
  return {
    background,
    foreground,
    cursor: color("--terminal-cursor"),
    cursorAccent: background,
    selectionBackground: color("--terminal-selection"),
    black: color("--terminal-black"),
    red: color("--terminal-red"),
    green: color("--terminal-green"),
    yellow: color("--terminal-yellow"),
    blue: color("--terminal-blue"),
    magenta: color("--terminal-magenta"),
    cyan: color("--terminal-cyan"),
    white: color("--terminal-white"),
    brightBlack: color("--terminal-bright-black"),
    brightRed: color("--terminal-bright-red"),
    brightGreen: color("--terminal-bright-green"),
    brightYellow: color("--terminal-bright-yellow"),
    brightBlue: color("--terminal-bright-blue"),
    brightMagenta: color("--terminal-bright-magenta"),
    brightCyan: color("--terminal-bright-cyan"),
    brightWhite: color("--terminal-bright-white"),
    extendedAnsi: buildExtendedAnsi(foreground, background),
  };
}

/**
 * xterm 的 `minimumContrastRatio`：低于该比值的前景色会被自动调深/调亮。
 * 亮色主题把 truecolor 与 256 色输出拉回可读范围；暗色保持 1（不干预）。
 */
export function readTerminalContrastRatio(element: Element = document.documentElement): number {
  const raw = getComputedStyle(element).getPropertyValue("--terminal-contrast-ratio").trim();
  const ratio = Number.parseFloat(raw);
  return Number.isFinite(ratio) && ratio >= 1 ? ratio : 1;
}

/** 一次性取出 xterm 需要随主题一起更新的全部外观选项。 */
export function readTerminalAppearance(element: Element = document.documentElement) {
  return { theme: readTerminalTheme(element), minimumContrastRatio: readTerminalContrastRatio(element) };
}

/** 解析 `--terminal-dim-strength`（CSS color-mix 的百分比），非法值返回 undefined。 */
function readDimStrength(style: CSSStyleDeclaration): number | undefined {
  const raw = style.getPropertyValue("--terminal-dim-strength").trim();
  const match = /^([0-9.]+)%$/.exec(raw);
  if (!match) return undefined;
  const value = Number.parseFloat(match[1]) / 100;
  return Number.isFinite(value) ? value : undefined;
}

/**
 * 亮色主题下 dim（SGR 2）文字的补偿前景色，与 CSS 里 xterm DOM 分支使用的
 * `color-mix(in srgb, var(--terminal-foreground) var(--terminal-dim-strength), var(--terminal-canvas-bg))`
 * 完全一致；想让内核默认行为生效时返回 null。
 *
 * 暗色主题的 dim 强度就是 50%（内核固定值），不需要改写数据流，因此返回 null；
 * 亮色主题（实测 72%）才需要（开发方案 6.6 / V5）。
 */
export function readTerminalDimForeground(element: Element = document.documentElement): string | null {
  const style = getComputedStyle(element);
  const strength = readDimStrength(style);
  if (strength === undefined || strength <= 0.5) return null;
  const foreground = parseColor(style.getPropertyValue("--terminal-foreground").trim() || style.getPropertyValue("--text-primary").trim());
  const background = parseColor(style.getPropertyValue("--terminal-canvas-bg").trim());
  if (!foreground || !background) return null;
  // mixColor(from, to, weight) 里 weight 是 to 的权重，CSS color-mix 则给第一个颜色
  // 强度权重，所以这里传 1 - strength。
  return toHex(mixColor(foreground, background, 1 - strength));
}

export const __test__ = { parseColor, contrastRatio, mixColor, relativeLuminance };
