import type {
  TerminalEngine,
  TerminalOptions,
  TerminalRenderer,
  TerminalTheme,
} from "./terminalAdapter.ts";
import {
  TERMINAL_THEME_AUTO,
  findTerminalColorScheme,
  schemeContrastRatio,
  schemeToTerminalTheme,
} from "./terminalThemes.ts";
import { buildExtendedAnsi, normalizeHexColor, readTerminalAppearance } from "./terminalTheme.ts";

/** 终端光标形状，与 xterm 的 `cursorStyle` 对齐。 */
export type TerminalCursorStyle = "block" | "bar" | "underline";

/** 配色来源：完全跟随应用主题，或在主题之上做逐色覆盖。 */
export type TerminalPaletteSource = "theme" | "custom";

/** 可覆盖的调色板键。前四个是对应主题的语义槽位，其余是 16 个 ANSI 色。 */
export const TERMINAL_PALETTE_KEYS = [
  "background",
  "foreground",
  "cursor",
  "selection",
  "black",
  "red",
  "green",
  "yellow",
  "blue",
  "magenta",
  "cyan",
  "white",
  "brightBlack",
  "brightRed",
  "brightGreen",
  "brightYellow",
  "brightBlue",
  "brightMagenta",
  "brightCyan",
  "brightWhite",
] as const;

export type TerminalPaletteKey = (typeof TERMINAL_PALETTE_KEYS)[number];
export type TerminalPalette = Partial<Record<TerminalPaletteKey, string>>;

export interface TerminalSettings {
  /** 内核：core（自研内核，默认）或 xterm.js（回退分支）。改动后重开终端生效。 */
  engine: TerminalEngine;
  /** 渲染后端，仅自研内核生效：webgl（默认）或 canvas。 */
  renderer: TerminalRenderer;
  /** 是否显示内核内建滚动条（仅自研内核生效）。 */
  scrollbar: boolean;
  /** 主体：保留多少行回滚缓冲。 */
  scrollback: number;
  cursorStyle: TerminalCursorStyle;
  cursorBlink: boolean;
  /** 平滑滚动：关闭时立即定位到目标行。 */
  smoothScroll: boolean;
  /** 滚轮灵敏度倍数。 */
  scrollSensitivity: number;
  /** 文字：字号（px）、行高倍数、字重、字符间距（px）。 */
  fontSize: number;
  lineHeight: number;
  fontWeight: number;
  fontWeightBold: number;
  letterSpacing: number;
  /** 样式：终端主题（auto = 跟随应用主题，否则为内置配色 id）。 */
  themeId: string;
  /** 配色来源（theme 之上可逐色覆盖）、逐色覆盖表、最小对比度（null = 跟随主题）。 */
  paletteSource: TerminalPaletteSource;
  palette: TerminalPalette;
  minimumContrastRatio: number | null;
  /**
   * 命令失败后是否在终端里亮一条"要诊断吗"的提示（F46）。
   *
   * 默认**开着**：失败诊断是这套 AI 能力最高频的入口，默认关掉等于没人会发现它。
   * 但它是全局开关——不喜欢被提示的用户可以一次关掉，且不会影响顶栏手动入口。
   */
  aiFailureHint: boolean;
}

/** 各项的合法区间，同时供设置面板生成控件。 */
export const TERMINAL_LIMITS = {
  scrollback: { min: 100, max: 100_000, step: 100 },
  fontSize: { min: 8, max: 32, step: 1 },
  lineHeight: { min: 1, max: 2, step: 0.05 },
  fontWeight: { min: 300, max: 900, step: 100 },
  letterSpacing: { min: -2, max: 4, step: 0.5 },
  scrollSensitivity: { min: 1, max: 10, step: 1 },
  minimumContrastRatio: { min: 1, max: 21, step: 0.5 },
} as const;

export const DEFAULT_TERMINAL_SETTINGS: TerminalSettings = {
  engine: "core",
  renderer: "webgl",
  scrollbar: true,
  scrollback: 5000,
  cursorStyle: "block",
  cursorBlink: true,
  smoothScroll: false,
  scrollSensitivity: 1,
  fontSize: 12,
  lineHeight: 1.45,
  fontWeight: 400,
  fontWeightBold: 700,
  letterSpacing: 0,
  themeId: TERMINAL_THEME_AUTO,
  paletteSource: "theme",
  palette: {},
  minimumContrastRatio: null,
  aiFailureHint: true,
};

const STORAGE_KEY = "android-tool.terminal-settings";
const listeners = new Set<(settings: TerminalSettings) => void>();
let memoryFallback: string | null = null;

function clampNumber(value: unknown, min: number, max: number, fallback: number, step = 0): number {
  const raw = typeof value === "number" ? value : Number.parseFloat(String(value ?? ""));
  if (!Number.isFinite(raw)) return fallback;
  const snapped = step > 0 ? Math.round(raw / step) * step : raw;
  const bounded = Math.min(max, Math.max(min, snapped));
  return step > 0 && step < 1 ? Math.round(bounded * 1000) / 1000 : Math.round(bounded);
}

function normalizePalette(value: unknown): TerminalPalette {
  if (!value || typeof value !== "object") return {};
  const source = value as Record<string, unknown>;
  const palette: TerminalPalette = {};
  for (const key of TERMINAL_PALETTE_KEYS) {
    const color = normalizeHexColor(typeof source[key] === "string" ? (source[key] as string) : undefined);
    if (color) palette[key] = color;
  }
  return palette;
}

/** 把任意来源（localStorage / 旧版本数据 / 用户输入）收敛成合法设置。 */
export function normalizeTerminalSettings(value: unknown): TerminalSettings {
  const source = (value && typeof value === "object" ? value : {}) as Partial<TerminalSettings>;
  const limits = TERMINAL_LIMITS;
  return {
    // 旧存档里的 "ghostty" 是同一个自研内核的旧名字，统一收敛到 "core"。
    engine: (source.engine as string) === "xterm" ? "xterm" : "core",
    renderer: source.renderer === "canvas" ? "canvas" : "webgl",
    scrollbar: typeof source.scrollbar === "boolean" ? source.scrollbar : DEFAULT_TERMINAL_SETTINGS.scrollbar,
    scrollback: clampNumber(source.scrollback, limits.scrollback.min, limits.scrollback.max, DEFAULT_TERMINAL_SETTINGS.scrollback, limits.scrollback.step),
    cursorStyle: source.cursorStyle === "bar" || source.cursorStyle === "underline" ? source.cursorStyle : "block",
    cursorBlink: typeof source.cursorBlink === "boolean" ? source.cursorBlink : DEFAULT_TERMINAL_SETTINGS.cursorBlink,
    smoothScroll: typeof source.smoothScroll === "boolean" ? source.smoothScroll : DEFAULT_TERMINAL_SETTINGS.smoothScroll,
    scrollSensitivity: clampNumber(source.scrollSensitivity, limits.scrollSensitivity.min, limits.scrollSensitivity.max, DEFAULT_TERMINAL_SETTINGS.scrollSensitivity, limits.scrollSensitivity.step),
    fontSize: clampNumber(source.fontSize, limits.fontSize.min, limits.fontSize.max, DEFAULT_TERMINAL_SETTINGS.fontSize, limits.fontSize.step),
    lineHeight: clampNumber(source.lineHeight, limits.lineHeight.min, limits.lineHeight.max, DEFAULT_TERMINAL_SETTINGS.lineHeight, limits.lineHeight.step),
    fontWeight: clampNumber(source.fontWeight, limits.fontWeight.min, limits.fontWeight.max, DEFAULT_TERMINAL_SETTINGS.fontWeight, limits.fontWeight.step),
    fontWeightBold: clampNumber(source.fontWeightBold, limits.fontWeight.min, limits.fontWeight.max, DEFAULT_TERMINAL_SETTINGS.fontWeightBold, limits.fontWeight.step),
    letterSpacing: clampNumber(source.letterSpacing, limits.letterSpacing.min, limits.letterSpacing.max, DEFAULT_TERMINAL_SETTINGS.letterSpacing, limits.letterSpacing.step),
    // 内置主题 id 必须命中目录；旧存档没有该字段 / id 已下架都回落「跟随应用主题」。
    themeId: findTerminalColorScheme(source.themeId) ? (source.themeId as string) : TERMINAL_THEME_AUTO,
    paletteSource: source.paletteSource === "custom" ? "custom" : "theme",
    palette: normalizePalette(source.palette),
    minimumContrastRatio: source.minimumContrastRatio === null || source.minimumContrastRatio === undefined
      ? null
      : clampNumber(source.minimumContrastRatio, limits.minimumContrastRatio.min, limits.minimumContrastRatio.max, limits.minimumContrastRatio.max, limits.minimumContrastRatio.step),
    // 旧版本存档里没有这个字段，缺省走默认值（开），不做"未设置即关闭"。
    aiFailureHint: typeof source.aiFailureHint === "boolean" ? source.aiFailureHint : DEFAULT_TERMINAL_SETTINGS.aiFailureHint,
  };
}

function readStoredValue(): string | null {
  try {
    const storage = globalThis.localStorage;
    if (storage) return storage.getItem(STORAGE_KEY);
  } catch {
    // 隐私模式或受限环境：退回进程内存储。
  }
  return memoryFallback;
}

function writeStoredValue(value: string | null) {
  try {
    const storage = globalThis.localStorage;
    if (storage) {
      if (value === null) storage.removeItem(STORAGE_KEY);
      else storage.setItem(STORAGE_KEY, value);
      return;
    }
  } catch {
    // 写入失败时仍保留进程内副本。
  }
  memoryFallback = value;
}

/** 读取终端设置；损坏或缺省的数据会回落到默认值。 */
export function readTerminalSettings(): TerminalSettings {
  const stored = readStoredValue();
  if (!stored) return { ...DEFAULT_TERMINAL_SETTINGS, palette: {} };
  try {
    return normalizeTerminalSettings(JSON.parse(stored));
  } catch {
    return { ...DEFAULT_TERMINAL_SETTINGS, palette: {} };
  }
}

function commit(settings: TerminalSettings): TerminalSettings {
  writeStoredValue(JSON.stringify(settings));
  listeners.forEach((listener) => listener(settings));
  return settings;
}

/** 局部更新终端设置，持久化后通知所有订阅者（终端实例据此实时应用）。 */
export function updateTerminalSettings(patch: Partial<TerminalSettings>): TerminalSettings {
  const next = normalizeTerminalSettings({ ...readTerminalSettings(), ...patch });
  return commit(next);
}

/** 恢复默认设置。 */
export function resetTerminalSettings(): TerminalSettings {
  return commit({ ...DEFAULT_TERMINAL_SETTINGS, palette: {} });
}

export function subscribeTerminalSettings(listener: (settings: TerminalSettings) => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** 设置中所有可直接写进终端构造参数/`options` 的项。 */
export function terminalOptions(settings: TerminalSettings, engine: TerminalEngine = "core"): TerminalOptions {
  const options: TerminalOptions = {
    cursorBlink: settings.cursorBlink,
    cursorStyle: settings.cursorStyle,
    fontSize: settings.fontSize,
    fontWeight: settings.fontWeight,
    fontWeightBold: settings.fontWeightBold,
    letterSpacing: settings.letterSpacing,
    lineHeight: settings.lineHeight,
    scrollback: settings.scrollback,
    scrollSensitivity: settings.scrollSensitivity,
    smoothScrollDuration: settings.smoothScroll ? 100 : 0,
  };
  if (engine === "core") {
    options.renderer = settings.renderer;
    // 内建滚动条一律关掉：它画在 canvas 自己的宽高里（`renderer.ts:1069`
    // `scrollbarX = canvasWidth - 8 - 4`），而 canvas 只有 `cols × 字宽` 宽、比宿主
    // 窄一截，于是那条滚动条缩在画布右缘、脱离面板右框，还在正文上压出一条轨道
    // 底色。改为 DOM 自绘（`views/Terminal/terminalScrollbar.ts`），位置由 CSS
    // 钉在面板右缘。`settings.scrollbar` 现在只决定要不要显示这条自绘滚动条。
    options.scrollbar = false;
  }
  return options;
}

/** 在运行中的终端实例上原地套用设置，无需重建实例。 */
export function applyTerminalSettings(
  target: { options: TerminalOptions },
  settings: TerminalSettings,
  engine: TerminalEngine = "core",
) {
  const options = target.options;
  options.cursorBlink = settings.cursorBlink;
  options.cursorStyle = settings.cursorStyle;
  options.fontSize = settings.fontSize;
  options.fontWeight = settings.fontWeight;
  options.fontWeightBold = settings.fontWeightBold;
  options.letterSpacing = settings.letterSpacing;
  options.lineHeight = settings.lineHeight;
  options.scrollback = settings.scrollback;
  options.scrollSensitivity = settings.scrollSensitivity;
  options.smoothScrollDuration = settings.smoothScroll ? 100 : 0;
  if (engine === "core") {
    options.renderer = settings.renderer;
    // 同 terminalOptions：内建滚动条恒关，滚动条由 terminalScrollbar.ts 自绘。
    options.scrollbar = false;
  }
}

/** 调色板键 → 终端主题字段。 */
export const PALETTE_THEME_KEYS: Record<TerminalPaletteKey, keyof TerminalTheme> = {
  background: "background",
  foreground: "foreground",
  cursor: "cursor",
  selection: "selectionBackground",
  black: "black",
  red: "red",
  green: "green",
  yellow: "yellow",
  blue: "blue",
  magenta: "magenta",
  cyan: "cyan",
  white: "white",
  brightBlack: "brightBlack",
  brightRed: "brightRed",
  brightGreen: "brightGreen",
  brightYellow: "brightYellow",
  brightBlue: "brightBlue",
  brightMagenta: "brightMagenta",
  brightCyan: "brightCyan",
  brightWhite: "brightWhite",
};

/** 在主题调色板上叠加设置里的逐色覆盖，并重算灰阶与光标底色。 */
export function resolveTerminalTheme(base: TerminalTheme, settings: TerminalSettings): TerminalTheme {
  if (settings.paletteSource !== "custom" || Object.keys(settings.palette).length === 0) return base;
  const theme: TerminalTheme = { ...base };
  for (const key of TERMINAL_PALETTE_KEYS) {
    const color = settings.palette[key];
    if (!color) continue;
    (theme as Record<string, unknown>)[PALETTE_THEME_KEYS[key]] = color;
  }
  theme.cursorAccent = theme.background;
  theme.extendedAnsi = buildExtendedAnsi(theme.foreground, theme.background);
  return theme;
}

/** 取出两处终端构造与实时更新都要用的外观选项。
 *  选中内置终端主题时以配色目录为基座（不读 CSS 变量，应用亮暗切换不再影响终端），
 *  「跟随应用主题」维持原路径；逐色覆盖（palette）叠在两种基座之上。 */
export function resolveTerminalAppearance(settings: TerminalSettings, element?: Element) {
  const scheme = findTerminalColorScheme(settings.themeId);
  if (scheme) {
    return {
      theme: resolveTerminalTheme(schemeToTerminalTheme(scheme), settings),
      minimumContrastRatio: settings.minimumContrastRatio ?? schemeContrastRatio(scheme),
    };
  }
  const base = readTerminalAppearance(element);
  return {
    theme: resolveTerminalTheme(base.theme, settings),
    minimumContrastRatio: settings.minimumContrastRatio ?? base.minimumContrastRatio,
  };
}
