export type AppFont =
  | "noto"
  | "macos"
  | "ubuntu"
  | "system"
  | "noto-sans-sc"
  | "noto-serif-sc"
  | "wenkai"
  | "inter"
  | "lato"
  | "segoe-ui"
  | "dengxian"
  | "yahei"
  | "simsun"
  | "harmonyos"
  | "misans";
export type AppMonoFont =
  | "jetbrains"
  | "system"
  | "fira-code"
  | "source-code-pro"
  | "ibm-plex-mono"
  | "inconsolata"
  | "cascadia"
  | "consolas"
  | "sarasa"
  | "maple";

/* 随包分发的字体（src/assets/fonts）永远排在最前面，后面的系统字体只做兜底：
   缺那个字体时最多退化为系统默认，不会掉到 Times New Roman 那种怪字体。 */
export const APP_FONT_OPTIONS: ReadonlyArray<{ label: string; value: AppFont }> = [
  { label: "系统原生 (推荐)", value: "system" },
  { label: "思源黑体 (Noto Sans SC)", value: "noto-sans-sc" },
  { label: "思源宋体 (Noto Serif SC)", value: "noto-serif-sc" },
  { label: "霞鹜文楷 (LXGW WenKai)", value: "wenkai" },
  { label: "Inter (现代简洁)", value: "inter" },
  { label: "Lato (柔和圆润)", value: "lato" },
  { label: "现代英文 (Noto Sans)", value: "noto" },
  { label: "macOS 风格 (SF Pro)", value: "macos" },
  { label: "Ubuntu (开源)", value: "ubuntu" },
  { label: "Segoe UI (Windows 原生)", value: "segoe-ui" },
  { label: "等线 DengXian (Windows)", value: "dengxian" },
  { label: "微软雅黑 (Windows)", value: "yahei" },
  { label: "宋体 (衬线 · 长文阅读)", value: "simsun" },
  { label: "鸿蒙 HarmonyOS Sans (需已安装)", value: "harmonyos" },
  { label: "小米 MiSans (需已安装)", value: "misans" },
];

export const APP_MONO_FONT_OPTIONS: ReadonlyArray<{ label: string; value: AppMonoFont }> = [
  { label: "JetBrains Mono (推荐)", value: "jetbrains" },
  { label: "Fira Code", value: "fira-code" },
  { label: "Source Code Pro", value: "source-code-pro" },
  { label: "IBM Plex Mono", value: "ibm-plex-mono" },
  { label: "Inconsolata", value: "inconsolata" },
  { label: "系统等宽 (Cascadia / Consolas)", value: "system" },
  { label: "Cascadia Code (连字)", value: "cascadia" },
  { label: "Consolas (Windows 原生)", value: "consolas" },
  { label: "更纱黑体 Sarasa Mono (需已安装)", value: "sarasa" },
  { label: "Maple Mono (需已安装)", value: "maple" },
];

const FONT_KEY = "android-tool.font";
const MONO_FONT_KEY = "android-tool.mono-font";
const DEFAULT_APP_FONT: AppFont = "system";
const DEFAULT_APP_MONO_FONT: AppMonoFont = "jetbrains";

const monoFontListeners = new Set<(font: AppMonoFont) => void>();
const fontListeners = new Set<(font: AppFont) => void>();

/* 等宽字体普遍没有中文字形，不显式给中文回退时终端 / Logcat / 代码块里的中文
   会落到浏览器随手挑的字体上，宽度与正文对不齐。Sarasa 是 1:2 中文等宽，
   优先用；没装则退到雅黑（中文字形是方的，两格宽仍能对齐）。 */
const MONO_CJK_FALLBACK = '"Sarasa Mono SC", "Noto Sans Mono CJK SC", "Microsoft YaHei UI"';
const MONO_TAIL = `${MONO_CJK_FALLBACK}, monospace`;

const FONT_STACKS: Record<AppFont, string> = {
  "noto-sans-sc": '"Noto Sans SC", "Microsoft YaHei", sans-serif',
  "noto-serif-sc": '"Noto Serif SC", "SimSun", serif',
  wenkai: '"LXGW WenKai", "Noto Sans SC", "Microsoft YaHei", sans-serif',
  inter: '"Inter", "Noto Sans SC", "Microsoft YaHei", sans-serif',
  lato: '"Lato", "Noto Sans SC", "Microsoft YaHei", sans-serif',
  noto: '"Noto Sans UI", "Noto Sans SC", "Segoe UI Variable", "Segoe UI", "Microsoft YaHei", sans-serif',
  macos: '-apple-system, BlinkMacSystemFont, "SF Pro Display", "SF Pro Text", "PingFang SC", "Hiragino Sans GB", "Segoe UI Variable", "Segoe UI", "Microsoft YaHei", sans-serif',
  ubuntu: '"Ubuntu", "Noto Sans SC", "Segoe UI Variable", "Microsoft YaHei", sans-serif',
  "segoe-ui": '"Segoe UI Variable Display", "Segoe UI Variable", "Segoe UI", "Microsoft YaHei UI", "Microsoft YaHei", sans-serif',
  dengxian: '"DengXian", "等线", "Microsoft YaHei UI", "Microsoft YaHei", sans-serif',
  yahei: '"Microsoft YaHei UI", "Microsoft YaHei", "微软雅黑", "Segoe UI", sans-serif',
  simsun: '"SimSun", "宋体", "Noto Serif SC", serif',
  harmonyos: '"HarmonyOS Sans SC", "HarmonyOS Sans", "Noto Sans SC", "Microsoft YaHei UI", sans-serif',
  misans: '"MiSans", "MiSans VF", "Noto Sans SC", "Microsoft YaHei UI", sans-serif',
  system: '-apple-system, BlinkMacSystemFont, "Segoe UI Variable", "Segoe UI", "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei UI", "Microsoft YaHei", sans-serif',
};

const MONO_FONT_STACKS: Record<AppMonoFont, string> = {
  "fira-code": `"Fira Code", "JetBrains Mono", Consolas, ${MONO_TAIL}`,
  "source-code-pro": `"Source Code Pro", "JetBrains Mono", Consolas, ${MONO_TAIL}`,
  "ibm-plex-mono": `"IBM Plex Mono", "JetBrains Mono", Consolas, ${MONO_TAIL}`,
  inconsolata: `"Inconsolata", "JetBrains Mono", Consolas, ${MONO_TAIL}`,
  jetbrains: `"JetBrains Mono", "Cascadia Mono", "Cascadia Code", Consolas, ${MONO_TAIL}`,
  system: `"Cascadia Mono", "Cascadia Code", Consolas, "Courier New", ${MONO_TAIL}`,
  cascadia: `"Cascadia Code", "Cascadia Mono", "JetBrains Mono", Consolas, ${MONO_TAIL}`,
  consolas: `Consolas, "Cascadia Mono", "Courier New", ${MONO_TAIL}`,
  sarasa: `"Sarasa Mono SC", "Sarasa Fixed SC", "Maple Mono", "JetBrains Mono", ${MONO_CJK_FALLBACK}, monospace`,
  maple: `"Maple Mono", "Maple Mono NF", "JetBrains Mono", Consolas, ${MONO_TAIL}`,
};

function isAppFont(value: string | null): value is AppFont {
  return APP_FONT_OPTIONS.some((option) => option.value === value);
}

function isAppMonoFont(value: string | null): value is AppMonoFont {
  return APP_MONO_FONT_OPTIONS.some((option) => option.value === value);
}

export function readAppFont(): AppFont {
  if (typeof window === "undefined") return DEFAULT_APP_FONT;

  try {
    const stored = window.localStorage.getItem(FONT_KEY);
    if (isAppFont(stored)) return stored;
  } catch {
    // Keep the default when storage is unavailable.
  }

  return DEFAULT_APP_FONT;
}

/* 字体换上去到真正可用之间有一小段空档：mermaid 量文字、标签页量宽度这类
   一次性测量如果抢在这段时间里做，拿到的是回退字体的宽度。先 load 一次再通知。 */
function preloadStack(stack: string) {
  const loader = (document as Document & { fonts?: FontFaceSet }).fonts;
  if (!loader || typeof loader.load !== "function") return;
  const family = stack.split(",")[0].trim();
  if (!family) return;
  try {
    void loader.load(`16px ${family}`).catch(() => undefined);
  } catch {
    // A stack the engine cannot parse simply keeps the CSS fallback.
  }
}

export function applyAppFont(font: AppFont) {
  if (typeof document === "undefined") return;

  const root = document.documentElement;
  root.dataset.font = font;
  root.style.setProperty("--font-display", FONT_STACKS[font]);
  root.style.setProperty("--font-sans", FONT_STACKS[font]);
  preloadStack(FONT_STACKS[font]);
}

export function getAppFontStack(font = readAppFont()) {
  return FONT_STACKS[font];
}

export function setAppFont(font: AppFont) {
  applyAppFont(font);
  try {
    window.localStorage.setItem(FONT_KEY, font);
  } catch {
    // The active page still uses the selected font.
  }
  fontListeners.forEach((listener) => listener(font));
}

export function subscribeAppFont(listener: (font: AppFont) => void) {
  fontListeners.add(listener);
  return () => {
    fontListeners.delete(listener);
  };
}

export function readAppMonoFont(): AppMonoFont {
  if (typeof window === "undefined") return DEFAULT_APP_MONO_FONT;

  try {
    const stored = window.localStorage.getItem(MONO_FONT_KEY);
    if (isAppMonoFont(stored)) return stored;
  } catch {
    // Keep the default when storage is unavailable.
  }

  return DEFAULT_APP_MONO_FONT;
}

export function getAppMonoFontStack(font = readAppMonoFont()) {
  return MONO_FONT_STACKS[font];
}

export function applyAppMonoFont(font: AppMonoFont) {
  if (typeof document === "undefined") return;

  const root = document.documentElement;
  root.dataset.monoFont = font;
  root.style.setProperty("--font-mono", MONO_FONT_STACKS[font]);
  preloadStack(MONO_FONT_STACKS[font]);
}

export function setAppMonoFont(font: AppMonoFont) {
  applyAppMonoFont(font);
  try {
    window.localStorage.setItem(MONO_FONT_KEY, font);
  } catch {
    // The active page still uses the selected font.
  }
  monoFontListeners.forEach((listener) => listener(font));
}

export function subscribeAppMonoFont(listener: (font: AppMonoFont) => void) {
  monoFontListeners.add(listener);
  return () => {
    monoFontListeners.delete(listener);
  };
}

applyAppFont(readAppFont());
applyAppMonoFont(readAppMonoFont());
