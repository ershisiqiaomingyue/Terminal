import React from "react";
import { Button, SegmentedControl, Select } from "rain-ui";
import { RotateCcw } from "lucide-react";
import {
  getAppMonoFontStack,
  readAppMonoFont,
  subscribeAppMonoFont,
  type AppMonoFont,
} from "@/platform/font";
import { subscribeAppTheme } from "@/platform/theme";
import {
  PALETTE_THEME_KEYS,
  TERMINAL_PALETTE_KEYS,
  readTerminalSettings,
  resolveTerminalAppearance,
  subscribeTerminalSettings,
  updateTerminalSettings,
  type TerminalPaletteKey,
  type TerminalSettings,
} from "@/platform/terminalSettings";
import { TERMINAL_COLOR_SCHEMES, TERMINAL_THEME_AUTO, type TerminalColorScheme } from "@/platform/terminalThemes";
import { normalizeHexColor } from "@/platform/terminalTheme";
import { SettingRow } from "./settingsUI";

const PREVIEW_WIDTH = "w-[420px]";

const PALETTE_LABELS: Record<TerminalPaletteKey, string> = {
  background: "背景",
  foreground: "前景",
  cursor: "光标",
  selection: "选区",
  black: "黑",
  red: "红",
  green: "绿",
  yellow: "黄",
  blue: "蓝",
  magenta: "品红",
  cyan: "青",
  white: "白",
  brightBlack: "亮黑",
  brightRed: "亮红",
  brightGreen: "亮绿",
  brightYellow: "亮黄",
  brightBlue: "亮蓝",
  brightMagenta: "亮品红",
  brightCyan: "亮青",
  brightWhite: "亮白",
};

const PALETTE_SOURCE_OPTIONS = [
  { value: "theme", label: "跟随应用主题" },
  { value: "custom", label: "自定义配色" },
] as const;

const CONTRAST_OPTIONS = [
  { label: "跟随主题", value: "theme" },
  { label: "1 · 不干预", value: "1" },
  { label: "3", value: "3" },
  { label: "4.5 · AA", value: "4.5" },
  { label: "7 · AAA", value: "7" },
  { label: "10", value: "10" },
];

/** 主题下拉选项前的一排色点：主题底色衬底 + 五个 ANSI 色。 */
const THEME_DOT_KEYS = ["red", "green", "yellow", "blue", "magenta"] as const;

const ThemeDots: React.FC<{ scheme: TerminalColorScheme | null }> = ({ scheme }) => (
  <span
    className="inline-flex h-4 items-center gap-[2px] rounded border border-border/70 px-1"
    style={scheme ? { background: scheme.background } : { background: "var(--terminal-canvas-bg)" }}
    aria-hidden
  >
    {THEME_DOT_KEYS.map((key) => (
      <i key={key} className="size-[5px] rounded-full" style={{ background: scheme ? scheme[key] : `var(--terminal-${key})` }} />
    ))}
  </span>
);

/** 终端配色：主题、实时预览、调色板与最小对比度。 */
export const TerminalThemeSettings: React.FC = () => {
  const [settings, setSettings] = React.useState<TerminalSettings>(readTerminalSettings);
  const [monoFont, setMonoFont] = React.useState<AppMonoFont>(readAppMonoFont);
  const [themeRevision, setThemeRevision] = React.useState(0);

  React.useEffect(() => subscribeTerminalSettings(setSettings), []);
  React.useEffect(() => subscribeAppMonoFont(setMonoFont), []);
  React.useEffect(() => subscribeAppTheme(() => setThemeRevision((value) => value + 1)), []);

  const update = (patch: Partial<TerminalSettings>) => setSettings(updateTerminalSettings(patch));
  const isCustomPalette = settings.paletteSource === "custom";

  const previewTheme = React.useMemo(
    () => resolveTerminalAppearance(settings).theme,
    // 跟随应用主题时 CSS 变量会随亮暗切换变化，需要重新读取。
    [settings, themeRevision],
  );

  const previewStyle: React.CSSProperties = {
    background: previewTheme.background,
    color: previewTheme.foreground,
    fontFamily: getAppMonoFontStack(monoFont),
    fontSize: `${settings.fontSize}px`,
    lineHeight: settings.lineHeight,
    fontWeight: settings.fontWeight,
    letterSpacing: `${settings.letterSpacing}px`,
  };

  return (
    <>
      <SettingRow title="终端主题" desc="跟随应用主题、内置配色或自定义配色。">
        <Select
          size="md"
          value={settings.themeId}
          onChange={(value) => update({ themeId: value })}
          searchable
          searchPlaceholder="搜索终端主题"
          aria-label="终端主题"
          options={[
            {
              value: TERMINAL_THEME_AUTO,
              label: "跟随应用主题",
              leading: <ThemeDots scheme={null} />,
            },
            ...TERMINAL_COLOR_SCHEMES.map((scheme) => ({
              value: scheme.id,
              label: scheme.name,
              leading: <ThemeDots scheme={scheme} />,
            })),
          ]}
        />
      </SettingRow>

      <SettingRow
        title="终端预览"
        desc="当前主题、字体与间距的实时效果。"
        controlClassName={`${PREVIEW_WIDTH} items-start justify-end py-0.5`}
        className="items-start"
      >
        <div
          className="w-full overflow-x-auto rounded-xl border border-border/70 p-3 text-left"
          style={previewStyle}
          aria-label="终端外观预览"
        >
          <div className="whitespace-pre">
            <span style={{ color: previewTheme.green }}>❯</span> cargo check
          </div>
          <div className="whitespace-pre" style={{ color: previewTheme.brightBlack }}>Finished `dev` profile in 2.3s</div>
          <div className="whitespace-pre">
            <span style={{ color: previewTheme.green }}>✓</span>{"    "}<span style={{ color: previewTheme.green }}>0 warnings</span>
            <span className="ml-1 inline-block h-[1em] w-[0.42em] align-text-bottom" style={{ background: previewTheme.cursor }} />
          </div>
          <div className="whitespace-pre">
            <span style={{ color: previewTheme.red }}>✗ build failed</span>{" "}
            <span style={{ color: previewTheme.yellow }}>exit 1</span>{" "}
            <span style={{ color: previewTheme.brightBlack }}>· 2.3s</span>
          </div>
          <div className="whitespace-pre">
            <span style={{ color: previewTheme.blue }}>ℹ</span> npm run dev{" "}
            <span style={{ color: previewTheme.magenta }}>--watch</span>{" "}
            <span style={{ color: previewTheme.cyan }}>ready in 412ms</span>
          </div>
        </div>
      </SettingRow>

      <SettingRow title="配色来源" desc="跟随主题或逐色覆盖。">
        <SegmentedControl
          value={settings.paletteSource}
          onChange={(value) => update({ paletteSource: value })}
          options={PALETTE_SOURCE_OPTIONS}
          label="配色来源"
        />
      </SettingRow>

      {isCustomPalette ? (
        <SettingRow
          title="调色板"
          desc="点击色块修改，未覆盖的颜色继续跟随主题。"
          controlClassName="w-[420px] items-start justify-end py-0.5"
          className="items-start"
        >
          <div className="flex w-full flex-col gap-2.5">
            <div className="grid w-full grid-cols-4 gap-1.5">
              {TERMINAL_PALETTE_KEYS.map((key) => {
                const effective = normalizeHexColor(previewTheme[PALETTE_THEME_KEYS[key]] as string | undefined) || "#000000";
                const overridden = Boolean(settings.palette[key]);
                return (
                  <label
                    key={key}
                    className={`flex h-8 min-w-0 items-center gap-1.5 rounded-lg border px-2 text-[11px] transition-colors ${overridden ? "border-primary/60 bg-primary/5" : "border-border/70 bg-muted/20 hover:bg-muted/40"}`}
                    title={`${PALETTE_LABELS[key]} ${effective}`}
                  >
                    <input
                      type="color"
                      className="size-5 shrink-0 cursor-pointer rounded border-0 bg-transparent p-0"
                      value={effective}
                      aria-label={`${PALETTE_LABELS[key]}颜色`}
                      onChange={(event) => update({ palette: { ...settings.palette, [key]: event.target.value } })}
                    />
                    <span className="truncate text-foreground">{PALETTE_LABELS[key]}</span>
                    <span className="ml-auto font-mono text-[9px] tabular-nums text-muted-foreground">{effective.slice(1)}</span>
                  </label>
                );
              })}
            </div>
            <Button
              variant="outline"
              size="sm"
              icon={<RotateCcw size={13} />}
              disabled={Object.keys(settings.palette).length === 0}
              onClick={() => update({ palette: {} })}
            >
              恢复主题配色
            </Button>
          </div>
        </SettingRow>
      ) : null}

      <SettingRow title="最小对比度" desc="低于该比值的前景色会自动调亮或调暗。">
        <Select
          size="md"
          value={settings.minimumContrastRatio === null ? "theme" : String(settings.minimumContrastRatio)}
          onChange={(value) => update({ minimumContrastRatio: value === "theme" ? null : Number.parseFloat(value) })}
          options={CONTRAST_OPTIONS}
        />
      </SettingRow>
    </>
  );
};

export { PALETTE_LABELS };

export default TerminalThemeSettings;
