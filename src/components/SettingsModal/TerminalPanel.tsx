import React from "react";
import { Button, SegmentedControl, Select, Slider, Switch } from "rain-ui";
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
import { RotateCcw } from "lucide-react";
import { PALETTE_LABELS } from "./TerminalThemeSettings";
import { usePortal } from "rain-ui";
import { SettingRow, SettingsGroup, SettingsPane } from "./settingsUI";

const SLIDER_CONTROL_WIDTH = "w-[220px]";

const ANSI_KEYS = TERMINAL_PALETTE_KEYS.filter((key) => !["background", "foreground", "cursor", "selection"].includes(key));

const SCROLLBACK_OPTIONS = [1000, 2000, 4000, 5000, 10000, 20000, 50000, 100000];

const CURSOR_STYLE_OPTIONS = [
  { value: "block", label: "方块" },
  { value: "bar", label: "竖线" },
  { value: "underline", label: "下划线" },
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
  <SettingRow
    title={title}
    desc={desc}
    controlClassName={SLIDER_CONTROL_WIDTH}
    className={disabled ? "opacity-50" : undefined}
  >
    <div className="flex w-full items-center gap-3">
      <Slider
        value={value}
        min={min}
        max={max}
        step={step}
        disabled={disabled}
        aria-label={title}
        className="min-w-0 flex-1"
        onValueChange={onChange}
      />
      <span className="w-12 shrink-0 text-right font-mono text-xs tabular-nums text-muted-foreground">{display}</span>
    </div>
  </SettingRow>
);

export const TerminalPanel: React.FC = () => {
  const [settings, setSettings] = React.useState<TerminalSettings>(readTerminalSettings);
  const [monoFont, setMonoFont] = React.useState<AppMonoFont>(readAppMonoFont);
  const [themeRevision, setThemeRevision] = React.useState(0);
  const portal = usePortal();

  React.useEffect(() => subscribeTerminalSettings(setSettings), []);
  React.useEffect(() => subscribeAppMonoFont(setMonoFont), []);
  React.useEffect(() => subscribeAppTheme(() => setThemeRevision((value) => value + 1)), []);

  const update = (patch: Partial<TerminalSettings>) => setSettings(updateTerminalSettings(patch));
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
    <SettingsPane
      title="终端"
      action={
        <Button
          variant="outline"
          icon={<RotateCcw size={14} />}
          onClick={() => {
            setSettings(resetTerminalSettings());
            portal("已恢复默认终端设置。", "success");
          }}
        >
          恢复默认设置
        </Button>
      }
    >
      <SettingsGroup title="主体">
        <SettingRow title="终端内核" desc="切换后立即重建终端实例。">
          <SegmentedControl
            value={settings.engine}
            onChange={(value) => update({ engine: value })}
            options={ENGINE_OPTIONS}
            label="终端内核"
          />
        </SettingRow>

        <SettingRow title="渲染后端" desc="WebGL 不可用时自动回退 Canvas。">
          <SegmentedControl
            value={settings.renderer}
            onChange={(value) => update({ renderer: value })}
            options={RENDERER_OPTIONS.map((option) => ({ ...option, disabled: settings.engine !== "core" }))}
            label="渲染后端"
          />
        </SettingRow>

        <SettingRow title="内建滚动条" desc="使用内核自带的滚动条（仅自研内核生效）。">
          <Switch
            checked={settings.scrollbar}
            disabled={settings.engine !== "core"}
            onCheckedChange={(checked) => update({ scrollbar: checked })}
            aria-label="内建滚动条"
          />
        </SettingRow>

        <SettingRow title="回滚缓冲行数" desc="行数越大占用内存越多。">
          <Select
            size="md"
            value={String(settings.scrollback)}
            onChange={(value) => update({ scrollback: Number.parseInt(value, 10) })}
            options={scrollbackOptions.map((lines) => ({ label: `${lines.toLocaleString("zh-CN")} 行`, value: String(lines) }))}
          />
        </SettingRow>

        <SettingRow title="光标样式" desc="终端输入光标的外观。">
          <SegmentedControl
            value={settings.cursorStyle}
            onChange={(value) => update({ cursorStyle: value })}
            options={CURSOR_STYLE_OPTIONS}
            label="光标样式"
          />
        </SettingRow>

        <SettingRow title="光标闪烁" desc="关闭后减少渲染开销。">
          <Switch checked={settings.cursorBlink} onCheckedChange={(checked) => update({ cursorBlink: checked })} aria-label="光标闪烁" />
        </SettingRow>

        <SettingRow title="平滑滚动" desc="滚轮与滚动条整屏滚动时带过渡动画。">
          <Switch checked={settings.smoothScroll} onCheckedChange={(checked) => update({ smoothScroll: checked })} aria-label="平滑滚动" />
        </SettingRow>

        <RangeRow
          title="滚轮灵敏度"
          desc="每格滚轮滚动的行数倍率。"
          value={settings.scrollSensitivity}
          min={TERMINAL_LIMITS.scrollSensitivity.min}
          max={TERMINAL_LIMITS.scrollSensitivity.max}
          step={TERMINAL_LIMITS.scrollSensitivity.step}
          display={`${settings.scrollSensitivity}×`}
          onChange={(value) => update({ scrollSensitivity: value })}
        />
      </SettingsGroup>

      <SettingsGroup title="字体与渲染">
        <SettingRow title="等宽字体" desc="终端与代码共用同一套等宽字体。">
          <Select
            size="md"
            value={monoFont}
            onChange={(value) => {
              const font = value as AppMonoFont;
              setAppMonoFont(font);
              setMonoFont(font);
            }}
            options={[...APP_MONO_FONT_OPTIONS]}
            searchable
            searchPlaceholder="搜索等宽字体"
          />
        </SettingRow>

        <RangeRow
          title="字号"
          desc="终端字符大小（像素）。"
          value={settings.fontSize}
          min={TERMINAL_LIMITS.fontSize.min}
          max={TERMINAL_LIMITS.fontSize.max}
          step={TERMINAL_LIMITS.fontSize.step}
          display={`${settings.fontSize}px`}
          onChange={(value) => update({ fontSize: value })}
        />

        <RangeRow
          title="行高"
          desc="单元格高度相对字号的倍数。"
          value={settings.lineHeight}
          min={TERMINAL_LIMITS.lineHeight.min}
          max={TERMINAL_LIMITS.lineHeight.max}
          step={TERMINAL_LIMITS.lineHeight.step}
          display={settings.lineHeight.toFixed(2)}
          onChange={(value) => update({ lineHeight: value })}
        />

        <SettingRow title="常规字重" desc="无对应字重时自动回退。">
          <Select
            size="md"
            value={String(settings.fontWeight)}
            onChange={(value) => update({ fontWeight: Number.parseInt(value, 10) })}
            options={WEIGHT_OPTIONS}
          />
        </SettingRow>

        <SettingRow title="粗体字重" desc="粗体与高亮文本使用的字重。">
          <Select
            size="md"
            value={String(settings.fontWeightBold)}
            onChange={(value) => update({ fontWeightBold: Number.parseInt(value, 10) })}
            options={WEIGHT_OPTIONS}
          />
        </SettingRow>

        <RangeRow
          title="字符间距"
          desc={settings.engine === "core" ? "仅 xterm.js 支持。" : "额外增加的像素间距，负值更紧凑。"}
          value={settings.letterSpacing}
          min={TERMINAL_LIMITS.letterSpacing.min}
          max={TERMINAL_LIMITS.letterSpacing.max}
          step={TERMINAL_LIMITS.letterSpacing.step}
          display={`${settings.letterSpacing}px`}
          disabled={settings.engine === "core"}
          onChange={(value) => update({ letterSpacing: value })}
        />
      </SettingsGroup>

      <SettingsGroup title="预览与诊断">
        <SettingRow title="命令失败时提示诊断" desc="命令失败后在终端下方显示诊断提示。">
          <Switch
            checked={settings.aiFailureHint}
            onCheckedChange={(checked) => update({ aiFailureHint: checked })}
            aria-label="命令失败时提示诊断"
          />
        </SettingRow>

        <SettingRow
          title="预览"
          desc="当前字体、字号、行高与调色板的实际效果。"
          controlClassName="w-[420px] items-start justify-end py-0.5"
          className="items-start"
        >
          <div
            className="w-full overflow-hidden rounded-xl border border-border/70 p-3 text-left"
            style={previewStyle}
            aria-label="终端外观预览"
          >
            <div className="whitespace-pre">~/workspace $ cargo check</div>
            <div className="whitespace-pre" style={{ color: previewTheme.brightBlack }}>Finished `dev` profile in 2.3s</div>
            <div className="whitespace-pre">
              <span style={{ color: previewTheme.green }}>✓</span>{"\t"}<span style={{ color: previewTheme.green }}>0 warnings</span>
              <span className="ml-1 inline-block h-[1em] w-[0.42em] align-text-bottom" style={{ background: previewTheme.cursor }} />
            </div>
            <div className="mt-2 flex flex-wrap gap-1">
              {ANSI_KEYS.map((key) => (
                <span
                  key={key}
                  className="rounded px-1 py-px text-[9px] leading-relaxed"
                  style={{ background: previewTheme[PALETTE_THEME_KEYS[key]] as string | undefined, color: previewTheme.background }}
                  title={PALETTE_LABELS[key]}
                >
                  Aa
                </span>
              ))}
            </div>
          </div>
        </SettingRow>
      </SettingsGroup>

    </SettingsPane>
  );
};

