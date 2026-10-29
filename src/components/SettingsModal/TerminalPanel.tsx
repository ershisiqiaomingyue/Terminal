import React from "react";
import { Button, SegmentedControl, Select, Switch } from "@/components/ui";
import {
  APP_MONO_FONT_OPTIONS,
  getAppMonoFontStack,
  readAppMonoFont,
  setAppMonoFont,
  subscribeAppMonoFont,
  type AppMonoFont,
} from "@/platform/font";
import { subscribeAppTheme } from "@/platform/theme";
import {
  DEFAULT_TERMINAL_SETTINGS,
  PALETTE_THEME_KEYS,
  TERMINAL_LIMITS,
  TERMINAL_PALETTE_KEYS,
  readTerminalSettings,
  resetTerminalSettings,
  resolveTerminalAppearance,
  subscribeTerminalSettings,
  updateTerminalSettings,
  type TerminalPaletteKey,
  type TerminalSettings,
} from "@/platform/terminalSettings";
import { TERMINAL_COLOR_SCHEMES, TERMINAL_THEME_AUTO, type TerminalColorScheme } from "@/platform/terminalThemes";
import { normalizeHexColor } from "@/platform/terminalTheme";
import { RotateCcw } from "lucide-react";
import { useSettingsNotification } from "./SettingsNotification";

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

const ANSI_KEYS = TERMINAL_PALETTE_KEYS.filter((key) => !["background", "foreground", "cursor", "selection"].includes(key));

const SCROLLBACK_OPTIONS = [1000, 2000, 4000, 5000, 10000, 20000, 50000, 100000];

const CURSOR_STYLE_OPTIONS = [
  { value: "block", label: "方块" },
  { value: "bar", label: "竖线" },
  { value: "underline", label: "下划线" },
] as const;

const PALETTE_SOURCE_OPTIONS = [
  { value: "theme", label: "跟随应用主题" },
  { value: "custom", label: "自定义配色" },
] as const;

const ENGINE_OPTIONS = [
  { value: "core", label: "自研内核" },
  { value: "xterm", label: "xterm.js" },
] as const;

const RENDERER_OPTIONS = [
  { value: "webgl", label: "WebGL2" },
  { value: "canvas", label: "Canvas" },
] as const;

const WEIGHT_OPTIONS = [
  { label: "细体 (300)", value: "300" },
  { label: "常规 (400)", value: "400" },
  { label: "中等 (500)", value: "500" },
  { label: "半粗 (600)", value: "600" },
  { label: "粗体 (700)", value: "700" },
  { label: "特粗 (800)", value: "800" },
];

const CONTRAST_OPTIONS = [
  { label: "跟随主题", value: "theme" },
  { label: "1 · 不干预", value: "1" },
  { label: "3", value: "3" },
  { label: "4.5 · AA", value: "4.5" },
  { label: "7 · AAA", value: "7" },
  { label: "10", value: "10" },
];

const DEFAULTS_SUMMARY = `${DEFAULT_TERMINAL_SETTINGS.scrollback.toLocaleString("zh-CN")} 行缓冲 · ${DEFAULT_TERMINAL_SETTINGS.fontSize}px 字号 · 行高 ${DEFAULT_TERMINAL_SETTINGS.lineHeight} · 箭头光标闪烁 · 配色跟随应用主题`;

/** 主题下拉选项前的一排色点：主题底色衬底 + 五个 ANSI 色，行高保持单行。 */
const THEME_DOT_KEYS = ["red", "green", "yellow", "blue", "magenta"] as const;

const ThemeDots: React.FC<{ scheme: TerminalColorScheme | null }> = ({ scheme }) => (
  <span
    className="terminal-theme-dots-mini"
    style={scheme ? { background: scheme.background } : { background: "var(--terminal-canvas-bg)" }}
  >
    {THEME_DOT_KEYS.map((key) => (
      <i key={key} style={{ background: scheme ? scheme[key] : `var(--terminal-${key})` }} />
    ))}
  </span>
);

interface RangeRowProps {
  title: string;
  desc: string;
  value: number;
  min: number;
  max: number;
  step: number;
  display: string;
  disabled?: boolean;
  onChange: (value: number) => void;
}

const RangeRow: React.FC<RangeRowProps> = ({ title, desc, value, min, max, step, display, disabled, onChange }) => (
  <div className={"setting-row-item" + (disabled ? " is-disabled" : "")}>
    <div className="setting-label-block">
      <span className="setting-title">{title}</span>
      <span className="setting-desc">{desc}</span>
    </div>
    <div className="setting-control-block terminal-range-control">
      <input
        type="range"
        className="terminal-range"
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        aria-label={title}
        onChange={(event) => onChange(Number.parseFloat(event.target.value))}
      />
      <span className="terminal-range-value">{display}</span>
    </div>
  </div>
);

export const TerminalPanel: React.FC = () => {
  const [settings, setSettings] = React.useState<TerminalSettings>(readTerminalSettings);
  const [monoFont, setMonoFont] = React.useState<AppMonoFont>(readAppMonoFont);
  const [themeRevision, setThemeRevision] = React.useState(0);
  const notify = useSettingsNotification();

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

  const scrollbackOptions = SCROLLBACK_OPTIONS.includes(settings.scrollback)
    ? SCROLLBACK_OPTIONS
    : [...SCROLLBACK_OPTIONS, settings.scrollback].sort((a, b) => a - b);

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
    <section className="settings-pane">
      <header className="pane-header">
        <h2 className="pane-title">终端</h2>
      </header>
      <p className="setting-desc terminal-pane-desc">
        终端主体、文字与样式设置。所有终端共用这套配置，修改后立即作用于已打开的终端窗口。
      </p>

      <div>
        <div className="setting-group-title">主体</div>
        <div className="setting-card-group">
          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">终端内核</span>
              <span className="setting-desc">
                自研内核为默认内核：基于开源 vt 解析核心深度定制的高性能 WASM 实现，解析吞吐实测约 4.7 倍于 xterm.js，并支持 WebGL2 渲染；xterm.js 为兼容回退分支。切换后立即重建终端实例。
              </span>
            </div>
            <div className="setting-control-block">
              <SegmentedControl
                value={settings.engine}
                onChange={(value) => update({ engine: value })}
                options={ENGINE_OPTIONS}
                label="终端内核"
              />
            </div>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">渲染后端</span>
              <span className="setting-desc">WebGL2 由 GPU 绘制字形，Canvas 作为兼容回退；仅自研内核生效，改动后立即重装渲染器（WebGL 不可用时自动回退 Canvas）。</span>
            </div>
            <div className="setting-control-block">
              <SegmentedControl
                value={settings.renderer}
                onChange={(value) => update({ renderer: value })}
                options={RENDERER_OPTIONS.map((option) => ({ ...option, disabled: settings.engine !== "core" }))}
                label="渲染后端"
              />
            </div>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">内建滚动条</span>
              <span className="setting-desc">使用内核自带的滚动条（仅自研内核生效）。</span>
            </div>
            <div className="setting-control-block">
              <Switch
                checked={settings.scrollbar}
                disabled={settings.engine !== "core"}
                onChange={(checked) => update({ scrollbar: checked })}
                aria-label="内建滚动条"
              />
            </div>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">回滚缓冲行数</span>
              <span className="setting-desc">每个终端会话保留的历史输出行数；行数越大占用内存越多。</span>
            </div>
            <div className="setting-control-block">
              <Select
                size="sm"
                value={String(settings.scrollback)}
                onChange={(value) => update({ scrollback: Number.parseInt(value, 10) })}
                options={scrollbackOptions.map((lines) => ({ label: `${lines.toLocaleString("zh-CN")} 行`, value: String(lines) }))}
                style={{ width: "100%" }}
              />
            </div>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">光标样式</span>
              <span className="setting-desc">终端输入光标的外观</span>
            </div>
            <div className="setting-control-block">
              <SegmentedControl
                value={settings.cursorStyle}
                onChange={(value) => update({ cursorStyle: value })}
                options={CURSOR_STYLE_OPTIONS}
                label="光标样式"
              />
            </div>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">光标闪烁</span>
              <span className="setting-desc">关闭后可减少远程会话的渲染唤醒，长时间运行更省电。</span>
            </div>
            <div className="setting-control-block">
              <Switch checked={settings.cursorBlink} onChange={(checked) => update({ cursorBlink: checked })} aria-label="光标闪烁" />
            </div>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">平滑滚动</span>
              <span className="setting-desc">滚轮与滚动条整屏滚动时带过渡动画。</span>
            </div>
            <div className="setting-control-block">
              <Switch checked={settings.smoothScroll} onChange={(checked) => update({ smoothScroll: checked })} aria-label="平滑滚动" />
            </div>
          </div>

          <RangeRow
            title="滚轮灵敏度"
            desc="每格滚轮滚动的行数倍率"
            value={settings.scrollSensitivity}
            min={TERMINAL_LIMITS.scrollSensitivity.min}
            max={TERMINAL_LIMITS.scrollSensitivity.max}
            step={TERMINAL_LIMITS.scrollSensitivity.step}
            display={`${settings.scrollSensitivity}×`}
            onChange={(value) => update({ scrollSensitivity: value })}
          />
        </div>
      </div>

      <div>
        <div className="setting-group-title">文字</div>
        <div className="setting-card-group">
          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">等宽字体</span>
              <span className="setting-desc">终端、Logcat 与代码共用同一套等宽字体。</span>
            </div>
            <div className="setting-control-block">
              <Select
                size="sm"
                value={monoFont}
                onChange={(value) => {
                  const font = value as AppMonoFont;
                  setAppMonoFont(font);
                  setMonoFont(font);
                }}
                options={[...APP_MONO_FONT_OPTIONS]}
                searchable
                searchPlaceholder="搜索等宽字体"
                style={{ width: "100%" }}
              />
            </div>
          </div>

          <RangeRow
            title="字号"
            desc="终端字符大小（像素）"
            value={settings.fontSize}
            min={TERMINAL_LIMITS.fontSize.min}
            max={TERMINAL_LIMITS.fontSize.max}
            step={TERMINAL_LIMITS.fontSize.step}
            display={`${settings.fontSize} px`}
            onChange={(value) => update({ fontSize: value })}
          />

          <RangeRow
            title="行高"
            desc="单元格高度相对字号的倍数"
            value={settings.lineHeight}
            min={TERMINAL_LIMITS.lineHeight.min}
            max={TERMINAL_LIMITS.lineHeight.max}
            step={TERMINAL_LIMITS.lineHeight.step}
            display={settings.lineHeight.toFixed(2)}
            onChange={(value) => update({ lineHeight: value })}
          />

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">常规字重</span>
              <span className="setting-desc">正文文本的字重；无对应字重的字体会自动回退。</span>
            </div>
            <div className="setting-control-block">
              <Select
                size="sm"
                value={String(settings.fontWeight)}
                onChange={(value) => update({ fontWeight: Number.parseInt(value, 10) })}
                options={WEIGHT_OPTIONS}
                style={{ width: "100%" }}
              />
            </div>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">粗体字重</span>
              <span className="setting-desc">SGR 1（粗体）与 `--` 高亮文本使用的字重。</span>
            </div>
            <div className="setting-control-block">
              <Select
                size="sm"
                value={String(settings.fontWeightBold)}
                onChange={(value) => update({ fontWeightBold: Number.parseInt(value, 10) })}
                options={WEIGHT_OPTIONS}
                style={{ width: "100%" }}
              />
            </div>
          </div>

          <RangeRow
            title="字符间距"
            desc={
              settings.engine === "core"
                ? "仅 xterm.js 内核支持；自研内核的单元格宽度由字体与字号决定，会忽略该值。"
                : "单元格宽度额外增加的像素数，负值更紧凑。"
            }
            value={settings.letterSpacing}
            min={TERMINAL_LIMITS.letterSpacing.min}
            max={TERMINAL_LIMITS.letterSpacing.max}
            step={TERMINAL_LIMITS.letterSpacing.step}
            display={`${settings.letterSpacing} px`}
            disabled={settings.engine === "core"}
            onChange={(value) => update({ letterSpacing: value })}
          />
        </div>
      </div>

      <div>
        <div className="setting-group-title">样式</div>
        <div className="setting-card-group">
          <div className="setting-row-item terminal-theme-row">
            <div className="setting-label-block">
              <span className="setting-title">终端主题</span>
              <span className="setting-desc">内置配色直接应用到所有终端；跟随应用主题则随亮暗自动切换。自定义配色在其上叠加。</span>
            </div>
            <div className="setting-control-block">
              <Select
                size="sm"
                value={settings.themeId}
                onChange={(value) => update({ themeId: value })}
                searchable
                searchPlaceholder="搜索终端主题"
                style={{ width: "100%" }}
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
            </div>
            {/* 实时预览（flex-basis 100% 换行铺满）：选哪个主题立刻看到粗略的
                文字显示效果——提示符、命令、输出、状态色各来一行。 */}
            <div className="terminal-theme-preview" style={previewStyle}>
              <div>
                <span style={{ color: previewTheme.green }}>❯</span> adb devices
              </div>
              <div style={{ color: previewTheme.brightBlack }}>List of devices attached</div>
              <div>
                R58M12ABCDE{"\u00a0\u00a0\u00a0\u00a0"}<span style={{ color: previewTheme.green }}>device</span>
              </div>
              <div>
                <span style={{ color: previewTheme.red }}>✗ build failed</span>{" "}
                <span style={{ color: previewTheme.yellow }}>exit 1</span>{" "}
                <span style={{ color: previewTheme.brightBlack }}>· 2.3s</span>
              </div>
              <div>
                <span style={{ color: previewTheme.blue }}>ℹ</span> npm run dev{" "}
                <span style={{ color: previewTheme.magenta }}>--watch</span>{" "}
                <span style={{ color: previewTheme.cyan }}>ready in 412ms</span>
              </div>
            </div>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">配色来源</span>
              <span className="setting-desc">跟随所选终端主题，或在其之上逐色覆盖调色板。</span>
            </div>
            <div className="setting-control-block">
              <SegmentedControl
                value={settings.paletteSource}
                onChange={(value) => update({ paletteSource: value })}
                options={PALETTE_SOURCE_OPTIONS}
                label="配色来源"
              />
            </div>
          </div>

          {isCustomPalette && (
            <div className="setting-row-item">
              <div className="setting-label-block">
                <span className="setting-title">调色板</span>
                <span className="setting-desc">点击色块修改；未修改的颜色继续跟随应用主题。</span>
              </div>
              <div className="setting-control-block terminal-palette-control">
                <div className="terminal-color-grid">
                  {TERMINAL_PALETTE_KEYS.map((key) => {
                    const effective = normalizeHexColor(previewTheme[PALETTE_THEME_KEYS[key]] as string | undefined) || "#000000";
                    const overridden = Boolean(settings.palette[key]);
                    return (
                      <label key={key} className={`terminal-color-field ${overridden ? "overridden" : ""}`}>
                        <input
                          type="color"
                          className="terminal-color-input"
                          value={effective}
                          aria-label={`${PALETTE_LABELS[key]}颜色`}
                          onChange={(event) => update({ palette: { ...settings.palette, [key]: event.target.value } })}
                        />
                        <span className="terminal-color-label">{PALETTE_LABELS[key]}</span>
                        <span className="terminal-color-value">{effective}</span>
                      </label>
                    );
                  })}
                </div>
                <Button
                  size="xs"
                  variant="secondary"
                  icon={<RotateCcw size={13} />}
                  disabled={Object.keys(settings.palette).length === 0}
                  onClick={() => update({ palette: {} })}
                >
                  恢复主题配色
                </Button>
              </div>
            </div>
          )}

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">最小对比度</span>
              <span className="setting-desc">低于该比值的前景色会被自动调亮/调暗，避免真彩色输出在浅色主题下看不清。</span>
            </div>
            <div className="setting-control-block">
              <Select
                size="sm"
                value={settings.minimumContrastRatio === null ? "theme" : String(settings.minimumContrastRatio)}
                onChange={(value) => update({ minimumContrastRatio: value === "theme" ? null : Number.parseFloat(value) })}
                options={CONTRAST_OPTIONS}
                style={{ width: "100%" }}
              />
            </div>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">命令失败时提示诊断</span>
              <span className="setting-desc">命令以非 0 退出或中断后，在终端下方显示一条"要诊断吗"的提示。关闭后顶栏的手动入口仍然可用。</span>
            </div>
            <div className="setting-control-block">
              <Switch
                checked={settings.aiFailureHint}
                onChange={(checked) => update({ aiFailureHint: checked })}
                aria-label="命令失败时提示诊断"
              />
            </div>
          </div>

          <div className="setting-row-item">
            <div className="setting-label-block">
              <span className="setting-title">预览</span>
              <span className="setting-desc">当前字体、字号、行高与调色板的实际效果。</span>
            </div>
            <div className="setting-control-block">
              <div className="terminal-settings-preview" style={previewStyle}>
                <div>~/workspace $ adb devices</div>
                <div style={{ color: previewTheme.brightBlack }}>List of devices attached</div>
                <div>
                  R58M12ABCDE{"\t"}<span style={{ color: previewTheme.green }}>device</span>
                  <span className="terminal-settings-cursor" style={{ background: previewTheme.cursor }} />
                </div>
                <div className="terminal-settings-swatches">
                  {ANSI_KEYS.map((key) => (
                    <span
                      key={key}
                      className="terminal-settings-swatch"
                      style={{ background: previewTheme[PALETTE_THEME_KEYS[key]] as string | undefined, color: previewTheme.background }}
                      title={PALETTE_LABELS[key]}
                    >
                      Aa
                    </span>
                  ))}
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>

      <div className="terminal-settings-actions">
        <Button
          size="sm"
          variant="secondary"
          icon={<RotateCcw size={14} />}
          onClick={() => {
            setSettings(resetTerminalSettings());
            notify("已恢复默认终端设置。", "success");
          }}
        >
          恢复默认设置
        </Button>
        <span className="setting-desc">
          默认值：{DEFAULTS_SUMMARY}
        </span>
      </div>
    </section>
  );
};
