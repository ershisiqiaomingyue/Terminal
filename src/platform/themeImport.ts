import { parse, type ParseError } from "jsonc-parser";
import type { ThemeFamily } from "./themeNames";

export type ThemeMode = "light" | "dark";
export type ThemeCandidate = {
  name: string; location: string; mode?: ThemeMode;
  family?: ThemeFamily;
  extension?: { publisher: string; name: string; repository: string; path: string };
};

export function repositoryIdentity(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  try {
    const url = new URL(value.replace(/^git\+/, ""));
    if (url.hostname !== "github.com") return undefined;
    const parts = url.pathname.replace(/\.git\/?$/, "").split("/").filter(Boolean);
    return parts.length === 2 ? parts.join("/").toLowerCase() : undefined;
  } catch { return undefined; }
}

/** Cache successful reads and share concurrent requests; failed reads remain retryable. */
export function createThemeTextLoader(load: (url: string) => Promise<string>, maxEntries = 48) {
  const cache = new Map<string, { text: string; expires: number }>();
  const pending = new Map<string, Promise<string>>();
  return (url: string): Promise<string> => {
    const cached = cache.get(url);
    if (cached && cached.expires > Date.now()) return Promise.resolve(cached.text);
    const running = pending.get(url);
    if (running) return running;
    const request = Promise.resolve().then(() => load(url)).then((text) => {
      if (cache.size >= maxEntries) cache.delete(cache.keys().next().value!);
      cache.set(url, { text, expires: Date.now() + 5 * 60_000 });
      return text;
    }).finally(() => pending.delete(url));
    pending.set(url, request);
    return request;
  };
}

const TERMINAL_ANSI_NAMES = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"] as const;
const TERMINAL_ANSI_DEFAULTS = {
  light: ["#24292f", "#cf222e", "#116329", "#7d4e00", "#0969da", "#8250df", "#1b7c83", "#525252",
    "#6e7781", "#a40e26", "#1a7f37", "#633c01", "#0550ae", "#6639ba", "#096b72", "#1d1d1f"],
  dark: ["#8b949e", "#ff7b72", "#7ee787", "#d29922", "#79c0ff", "#d2a8ff", "#76e3ea", "#c9d1d9",
    "#a0a0a0", "#ffa198", "#aff5b4", "#e3b341", "#a5d6ff", "#e2c5ff", "#b3f0ff", "#ffffff"],
};

const COLOR_MAPPING: Record<string, string[]> = {
  ...Object.fromEntries([
    "table-bg", "table-text", "table-header-bg", "table-header-text", "table-stripe-bg", "table-hover-bg", "table-divider",
    "input-muted", "input-action-bg", "input-action-hover", "input-action-text",
    "input-surface-bg", "input-surface-hover-bg", "input-surface-focus-bg",
    "composer-bg", "composer-text", "composer-muted", "composer-control-bg", "composer-control-hover", "composer-control-text",
    "composer-send-bg", "composer-send-hover", "composer-send-text", "composer-disabled-bg", "composer-disabled-text",
    "composer-ring-track", "composer-divider", "composer-warning", "composer-danger", "composer-permission-bg", "composer-permission-hover",
    // Derived popover states are persisted with imported palettes so menus do
    // not fall back to the built-in indigo selection colors.
    "popover-item-hover", "popover-item-active-bg", "popover-item-active-text",
  ].map(name => [`--${name}`, []])),
  ...Object.fromEntries(TERMINAL_ANSI_NAMES.flatMap((name) => [
    [`--terminal-${name}`, [`terminal.ansi${name[0].toUpperCase()}${name.slice(1)}`]],
    [`--terminal-bright-${name}`, [`terminal.ansiBright${name[0].toUpperCase()}${name.slice(1)}`]],
  ])),
  // The title/menu bar and global tool rail form one continuous app frame.
  "--bg-app": ["titleBar.activeBackground", "activityBar.background", "sideBar.background"],
  "--context-navigation-bg": [],
  "--bg-canvas": ["editor.background"],
  "--bg-panel": ["panel.background", "editor.background"],
  // Cards are app surfaces, not VS Code floating editor widgets.
  "--bg-card": [],
  "--bg-surface": [],
  "--bg-card-hover": ["list.hoverBackground"],
  "--bg-hover": ["list.hoverBackground"],
  "--row-hover-bg": [],
  "--bg-selected": ["list.activeSelectionBackground", "list.inactiveSelectionBackground"],
  "--topbar-bg": ["titleBar.activeBackground", "sideBar.background", "editor.background"],
  "--popover-bg": ["quickInput.background", "editorWidget.background", "editor.background"],
  "--input-bg": ["input.background", "editorWidget.background", "editor.background"],
  "--input-bg-sunken": ["input.background", "editorWidget.background", "editor.background"],
  "--composer-bg": ["input.background", "editorWidget.background", "editor.background"],
  "--composer-text": ["input.foreground", "foreground", "editor.foreground"],
  "--input-bg-hover": [],
  "--input-bg-focus": [],
  "--input-text": ["input.foreground", "foreground", "editor.foreground"],
  "--terminal-canvas-bg": ["terminal.background", "editor.background"],
  "--terminal-foreground": ["terminal.foreground", "editor.foreground"],
  "--code-background": ["editor.background"],
  "--code-foreground": ["editor.foreground"],
  "--code-line-number": ["editorLineNumber.foreground", "descriptionForeground"],
  "--code-selection": ["editor.selectionBackground"],
  "--code-find-match": ["editor.findMatchHighlightBackground"],
  "--code-find-current": ["editor.findMatchBackground"],
  "--code-diff-added-bg": ["diffEditor.insertedLineBackground", "diffEditor.insertedTextBackground"],
  "--code-diff-removed-bg": ["diffEditor.removedLineBackground", "diffEditor.removedTextBackground"],
  "--code-diff-added-text": ["gitDecoration.addedResourceForeground"],
  "--code-diff-removed-text": ["gitDecoration.deletedResourceForeground"],
  "--text-primary": ["foreground", "editor.foreground"],
  "--text-secondary": ["descriptionForeground", "sideBar.foreground"],
  "--text-tertiary": ["disabledForeground", "descriptionForeground"],
  "--text-muted": ["descriptionForeground"],
  "--text-quaternary": ["disabledForeground", "descriptionForeground"],
  "--input-placeholder": ["input.placeholderForeground", "descriptionForeground", "editor.foreground"],
  "--terminal-cursor": ["terminalCursor.foreground", "editor.foreground"],
  "--terminal-selection": ["terminal.selectionBackground", "editor.selectionBackground"],
  "--chrome-ink": ["icon.foreground", "foreground", "editor.foreground"],
  "--chrome-muted": ["descriptionForeground", "sideBar.foreground"],
  "--chrome-hover": ["toolbar.hoverBackground", "list.hoverBackground", "editorWidget.background"],
  "--chrome-selected": ["list.activeSelectionBackground", "list.inactiveSelectionBackground"],
  "--chrome-hover-text": ["list.hoverForeground", "icon.foreground", "foreground", "editor.foreground"],
  "--selection-text": ["list.activeSelectionForeground", "list.inactiveSelectionForeground", "foreground", "editor.foreground"],
  "--button-hover-bg": [],
  "--button-hover-text": [],
  "--input-hover-text": [],
  "--accent-primary": ["textLink.foreground", "focusBorder", "button.background"],
  "--accent-hover": ["textLink.activeForeground", "textLink.foreground", "focusBorder"],
  // panel.border can be a saturated accent; ordinary separators are neutral.
  "--divider-color": [],
  "--border-subtle": [],
  // App primary buttons use the semantic accent palette completed below.
  "--btn-primary-bg": [],
  "--btn-primary-hover": [],
  "--btn-primary-text": [],
  "--btn-confirm-bg": [],
  "--btn-confirm-hover": [],
  "--btn-confirm-text": [],
  "--btn-secondary-bg": ["button.secondaryBackground", "input.background", "editorWidget.background"],
  "--btn-secondary-hover": ["button.secondaryHoverBackground", "list.hoverBackground"],
  "--btn-secondary-text": ["button.secondaryForeground", "foreground", "editor.foreground"],
  "--btn-subtle-text": ["descriptionForeground", "editor.foreground"],
  "--btn-subtle-bg": ["toolbar.background"],
  "--btn-subtle-hover": ["toolbar.hoverBackground", "list.hoverBackground"],
  "--control-unchecked-bg": ["checkbox.background"],
  "--control-checked-bg": ["checkbox.selectBackground", "textLink.foreground", "focusBorder"],
  "--control-checked-mark": ["checkbox.foreground"],
  "--scrollbar-thumb": ["scrollbarSlider.background"],
  "--scrollbar-thumb-hover": ["scrollbarSlider.hoverBackground"],
  "--success-text": ["gitDecoration.addedResourceForeground"],
  "--warning-text": ["editorWarning.foreground"],
  "--danger-text": ["errorForeground", "editorError.foreground"],
};

// Representative TextMate scopes for the categories emitted by highlight.js.
const SYNTAX_SCOPES: Record<string, string[]> = {
  "--code-syntax-comment": ["comment"],
  "--code-syntax-keyword": ["keyword", "storage.type", "storage.modifier"],
  "--code-syntax-literal": ["constant.language"],
  "--code-syntax-type": ["entity.name.type", "support.type", "support.class"],
  "--code-syntax-string": ["string"],
  "--code-syntax-attribute": ["entity.other.attribute-name"],
  "--code-syntax-number": ["constant.numeric"],
  "--code-syntax-function": ["entity.name.function", "support.function"],
  "--code-syntax-builtin": ["support", "variable.language"],
  "--code-syntax-meta": ["meta.preprocessor", "entity.name.tag"],
};

function syntaxColors(value: unknown): Record<string, string> {
  if (!Array.isArray(value)) return {};
  const colors: Record<string, string> = {};
  for (const [property, scopes] of Object.entries(SYNTAX_SCOPES)) {
    let bestScore = 0;
    for (const rule of value) {
      if (!rule || typeof rule !== "object" || !isThemeColor(rule.settings?.foreground)) continue;
      const selectors = (Array.isArray(rule.scope) ? rule.scope : [rule.scope])
        .filter((scope: unknown): scope is string => typeof scope === "string")
        .flatMap((scope: string) => scope.split(",").map((item) => item.trim()));
      for (const selector of selectors) {
        // Context-dependent selectors cannot describe an entire hljs category.
        if (!/^[\w.-]+$/.test(selector)) continue;
        for (const scope of scopes) {
          const score = scope === selector ? 1000 + selector.length : scope.startsWith(selector + ".") ? selector.length : 0;
          if (score && score >= bestScore) {
            colors[property] = rule.settings.foreground;
            bestScore = score;
          }
        }
      }
    }
  }
  return colors;
}

export function isThemeColor(value: unknown): value is string {
  return typeof value === "string" && /^#(?:[\da-f]{3}|[\da-f]{4}|[\da-f]{6}|[\da-f]{8})$/i.test(value);
}

export function sanitizeThemeColors(value: unknown): Record<string, string> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  return Object.fromEntries(Object.entries(value).filter(([key, color]) => (Object.prototype.hasOwnProperty.call(COLOR_MAPPING, key) || Object.prototype.hasOwnProperty.call(SYNTAX_SCOPES, key)) && isThemeColor(color)));
}

function channels(color: string) {
  const hex = color.slice(1);
  const expanded = hex.length <= 4 ? [...hex].map((digit) => digit + digit).join("") : hex;
  return [0, 2, 4, 6].map((offset) => offset === 6 && expanded.length < 8 ? 255 : parseInt(expanded.slice(offset, offset + 2), 16));
}

function mix(background: string, foreground: string, amount: number) {
  const bg = channels(background);
  const fg = channels(foreground);
  const weight = amount * fg[3] / 255;
  return "#" + bg.slice(0, 3).map((value, index) => Math.round(value * (1 - weight) + fg[index] * weight).toString(16).padStart(2, "0")).join("");
}

function contrastRatio(first: string, second: string) {
  const luminance = (color: string) => channels(color).slice(0, 3).reduce((total, channel, index) => {
    const value = channel / 255;
    return total + (value <= .04045 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4) * [.2126, .7152, .0722][index];
  }, 0);
  const a = luminance(first), b = luminance(second);
  return (Math.max(a, b) + .05) / (Math.min(a, b) + .05);
}

function readableColor(color: string, background: string, ink: string, target = 4.8) {
  const solid = mix(background, color, 1);
  if (contrastRatio(solid, background) >= target) return color;
  const end = contrastRatio(ink, background) >= target ? ink
    : contrastRatio("#fff", background) > contrastRatio("#000", background) ? "#fff" : "#000";
  for (let amount = .02; amount <= 1; amount += .02) {
    const adjusted = mix(solid, end, amount);
    if (contrastRatio(adjusted, background) >= target) return adjusted;
  }
  return end;
}

/** Complete older saved palettes too, so existing imports gain new surface tokens. */
export function completeThemeColors(input: Record<string, string>, mode: ThemeMode): Record<string, string> {
  const canvas = input["--bg-canvas"];
  const requestedInk = input["--text-primary"];
  if (!canvas || !requestedInk) return input;
  const ink = readableColor(requestedInk, canvas, requestedInk);
  const surface = mix(canvas, ink, mode === "dark" ? .09 : .045);
  const controlSurface = mix(canvas, ink, mode === "dark" ? .12 : .075);
  const hover = mix(canvas, ink, mode === "dark" ? .15 : .09);
  const selected = mix(canvas, ink, mode === "dark" ? .22 : .14);
  const secondary = mix(canvas, ink, .72);
  const muted = mix(canvas, ink, .56);
  const accent = input["--accent-primary"] || (mode === "light" ? "#31699d" : "#a9cef2");
  const codeInk = input["--code-foreground"] || ink;
  const codeCanvas = input["--code-background"] || canvas;
  const added = input["--success-text"] || (mode === "light" ? "#16834e" : "#7dd3a4");
  const removed = input["--danger-text"] || (mode === "light" ? "#c23d48" : "#f5a1a8");
  const colors: Record<string, string> = {
    "--bg-app": mix(canvas, mode === "light" ? "#000" : "#fff", mode === "light" ? .055 : .025),
    "--context-navigation-bg": surface,
    "--bg-card-hover": hover, "--bg-panel": canvas,
    "--page-header-bg": canvas, "--page-pane-bg": canvas,
    "--bg-hover": hover, "--bg-selected": selected,
    "--context-navigation-divider": mix(canvas, ink, .14),
    "--popover-item-hover": hover,
    "--popover-item-active-bg": selected,
    "--popover-item-active-text": readableColor(ink, selected, ink),
    "--popover-bg": surface,
    "--input-bg": mix(canvas, ink, .07), "--input-bg-sunken": mix(canvas, ink, .07),
    "--input-surface-bg": mix(canvas, ink, .11),
    "--input-surface-hover-bg": mix(canvas, ink, .15),
    "--input-surface-focus-bg": mix(canvas, ink, .125),
    "--input-bg-hover": hover, "--input-bg-focus": selected,
    "--control-unchecked-bg": selected,
    "--text-secondary": secondary, "--text-tertiary": muted,
    "--text-muted": muted, "--text-quaternary": muted, "--input-placeholder": muted,
    "--chrome-ink": ink, "--chrome-muted": secondary, "--chrome-hover": hover, "--chrome-selected": selected,
    "--accent-primary": accent, "--accent-hover": mix(accent, ink, .15),
    "--btn-secondary-bg": hover, "--btn-secondary-hover": selected, "--btn-secondary-text": ink,
    "--btn-subtle-bg": mix(canvas, ink, .07), "--btn-subtle-hover": selected, "--btn-subtle-text": secondary,
    "--scrollbar-thumb": mix(canvas, ink, .25), "--scrollbar-thumb-hover": mix(canvas, ink, .4),
    "--terminal-canvas-bg": canvas, "--terminal-foreground": ink, "--terminal-cursor": ink, "--terminal-selection": selected,
    "--code-background": codeCanvas, "--code-foreground": codeInk,
    "--code-line-number": mix(codeCanvas, codeInk, .56),
    "--code-selection": mix(codeCanvas, accent, .3),
    "--code-find-match": mix(codeCanvas, accent, .2), "--code-find-current": mix(codeCanvas, accent, .4),
    "--code-diff-added-bg": mix(codeCanvas, added, .14), "--code-diff-removed-bg": mix(codeCanvas, removed, .14),
    "--code-diff-added-text": added, "--code-diff-removed-text": removed,
    "--code-syntax-comment": mix(codeCanvas, codeInk, .56),
    "--code-syntax-keyword": removed, "--code-syntax-literal": accent, "--code-syntax-type": accent,
    "--code-syntax-string": added, "--code-syntax-attribute": accent,
    "--code-syntax-number": input["--warning-text"] || (mode === "light" ? "#bd6b23" : "#efaa68"),
    "--code-syntax-function": accent, "--code-syntax-builtin": accent, "--code-syntax-meta": removed,
    ...input,
    "--text-primary": ink,
    // Recompute semantic surfaces for older saved palettes as well.
    "--bg-card": surface, "--bg-surface": controlSurface,
    "--row-hover-bg": mix(surface, ink, .025),
    "--divider-color": mix(canvas, ink, .14), "--border-subtle": mix(canvas, ink, .14),
    // Imported themes may carry subdued editor widget colors for buttons.
    // Primary actions use the theme accent directly, with a contrast-safe ink.
    "--btn-primary-bg": accent,
    "--btn-primary-hover": mix(accent, ink, .15),
    "--btn-primary-text": readableColor(ink, accent, ink),
  };
  // A theme may explicitly make widgets equal to the editor. Keep our controls
  // distinguishable even in those palettes and in imports saved by older builds.
  for (const key of ["--bg-card", "--bg-surface", "--input-bg", "--input-bg-sunken", "--btn-subtle-bg", "--btn-secondary-bg"]) {
    if (mix(canvas, colors[key], 1) === mix(canvas, canvas, 1)) colors[key] = key.startsWith("--bg-") ? surface : hover;
  }
  colors["--topbar-bg"] ||= colors["--bg-app"];
  // Keep shared interactive surfaces safe even when a theme uses unrelated
  // selection/toolbar colors. Do this centrally for all existing consumers.
  for (const key of ["--bg-app", "--context-navigation-bg", "--topbar-bg", "--popover-bg", "--bg-panel", "--bg-card-hover", "--bg-hover", "--bg-selected", "--chrome-hover", "--chrome-selected", "--btn-secondary-bg", "--btn-secondary-hover", "--btn-subtle-bg", "--btn-subtle-hover", "--input-bg", "--input-bg-sunken"]) {
    const background = mix(canvas, colors[key], 1);
    colors[key] = contrastRatio(mix(background, ink, 1), background) >= 4.5 ? background : selected;
  }
  // Menu hover/selection must remain visible even when a VS Code theme maps
  // list.hoverBackground to the same color as its widget background. Prefer
  // the imported value, then move it toward the theme ink/accent until the
  // state is visibly distinct from the popover surface.
  const popoverSurface = colors["--popover-bg"];
  const distinctPopoverFill = (requested: string | undefined, candidates: string[]) =>
    [requested, ...candidates].filter((value): value is string => Boolean(value))
      .find(value => contrastRatio(value, popoverSurface) >= 1.12) || candidates[candidates.length - 1];
  colors["--popover-item-hover"] = distinctPopoverFill(colors["--popover-item-hover"], [
    mix(popoverSurface, ink, .12),
    mix(popoverSurface, accent, .16),
    mix(popoverSurface, ink, .2),
  ]);
  colors["--popover-item-active-bg"] = distinctPopoverFill(colors["--popover-item-active-bg"], [
    colors["--bg-selected"],
    mix(popoverSurface, accent, .22),
    mix(popoverSurface, ink, .24),
  ]);
  colors["--popover-item-active-text"] = readableColor(colors["--popover-item-active-text"] || ink, colors["--popover-item-active-bg"], ink);
  // The app frame must be consistent and visibly separate from tool content.
  const frame = colors["--bg-app"];
  const frameContrast = mode === "dark" ? 1.15 : 1.08;
  const frameCandidates = mode === "dark"
    ? [frame, mix(canvas, ink, .14), mix(canvas, ink, .2), mix(canvas, ink, .26), mix(canvas, ink, .32)]
    : [frame, mix(canvas, "#000", .12), mix(canvas, ink, .12), mix(canvas, ink, .18)];
  colors["--bg-app"] = frameCandidates
    .find(fill => contrastRatio(fill, canvas) >= frameContrast && contrastRatio(fill, surface) >= frameContrast && contrastRatio(fill, ink) >= 4.5) || hover;
  colors["--topbar-bg"] = colors["--bg-app"];
  // The floating rail is a separate surface inside the shared window frame.
  // Also repair older imports that flattened its fill into the frame color.
  colors["--context-navigation-bg"] = [colors["--context-navigation-bg"], surface, canvas]
    .find(fill => contrastRatio(fill, colors["--bg-app"]) >= 1.08 && contrastRatio(fill, ink) >= 4.5) || surface;
  // Text contrast alone cannot detect a button that disappears into its row.
  // Check the control fill against both the normal card and its hovered row.
  const controlParents = [surface, colors["--row-hover-bg"]];
  const distinctControlFill = (requested: string, extraParents: string[] = []) => {
    const backgrounds = [...controlParents, ...extraParents];
    return [requested, ...[.12, .16, .2, .24, .28, .32].map(amount => mix(canvas, ink, amount))]
      .find(fill => contrastRatio(fill, ink) >= 4.5 && backgrounds.every(parent => contrastRatio(fill, parent) >= 1.12)) || selected;
  };
  colors["--btn-secondary-bg"] = distinctControlFill(colors["--btn-secondary-bg"]);
  colors["--btn-secondary-hover"] = distinctControlFill(colors["--btn-secondary-hover"], [colors["--btn-secondary-bg"]]);
  for (const key of ["--input-bg", "--input-bg-sunken"]) {
    colors[key] = [colors[key], ...[.09, .12, .16, .2].map(amount => mix(canvas, ink, amount))]
      .find(fill => contrastRatio(fill, ink) >= 4.5 && [canvas, surface].every(parent => contrastRatio(fill, parent) >= 1.08)) || hover;
  }
  const inputBackground = colors["--input-bg-sunken"];
  const inputSurfaceParents = [canvas, surface, controlSurface];
  colors["--input-surface-bg"] = [colors["--input-surface-bg"], ...[.11, .14, .18, .22].map(amount => mix(canvas, ink, amount))]
    .find(fill => contrastRatio(fill, ink) >= 4.5 && inputSurfaceParents.every(parent => contrastRatio(fill, parent) >= 1.04)) || mix(canvas, ink, .14);
  colors["--input-surface-hover-bg"] = [colors["--input-surface-hover-bg"], mix(colors["--input-surface-bg"], ink, .06), mix(canvas, ink, .18)]
    .find(fill => contrastRatio(fill, ink) >= 4.5 && contrastRatio(fill, colors["--input-surface-bg"]) >= 1.04) || mix(colors["--input-surface-bg"], ink, .08);
  colors["--input-surface-focus-bg"] = [colors["--input-surface-focus-bg"], mix(colors["--input-surface-bg"], ink, .03), mix(canvas, ink, .13)]
    .find(fill => contrastRatio(fill, ink) >= 4.5 && contrastRatio(fill, colors["--input-surface-bg"]) >= 1.02) || colors["--input-surface-bg"];
  colors["--input-bg-hover"] = mix(inputBackground, ink, .04);
  colors["--input-bg-focus"] = mix(inputBackground, ink, .07);
  for (const key of ["--input-bg-hover", "--input-bg-focus"]) {
    if (contrastRatio(ink, colors[key]) < 4.5) colors[key] = inputBackground;
  }
  const inputSurfaces = [inputBackground, colors["--input-bg"], colors["--input-bg-hover"], colors["--input-bg-focus"]];
  const readableOnSurfaces = (requested: string, backgrounds: string[]) =>
    backgrounds.reduce((color, background) => readableColor(color, background, ink), requested);
  colors["--input-text"] = readableOnSurfaces(colors["--input-text"] || ink, inputSurfaces);
  colors["--input-hover-text"] = colors["--input-text"];
  colors["--button-hover-bg"] = colors["--chrome-hover"];
  colors["--button-hover-text"] = readableColor(ink, colors["--button-hover-bg"], ink);
  colors["--chrome-hover-text"] = readableColor(colors["--chrome-hover-text"] || ink, colors["--chrome-hover"], ink);
  colors["--selection-text"] = readableColor(colors["--selection-text"] || ink, colors["--bg-selected"], ink);
  const interactionSurfaces = [canvas, surface, colors["--bg-app"], colors["--topbar-bg"], colors["--context-navigation-bg"], colors["--popover-bg"], colors["--bg-hover"], colors["--bg-selected"], colors["--chrome-hover"], colors["--chrome-selected"], ...inputSurfaces];
  for (const key of ["--text-secondary", "--chrome-muted", "--text-tertiary", "--input-placeholder"]) {
    if (key !== "--input-placeholder" && mix(canvas, colors[key], 1) === mix(canvas, ink, 1)) {
      colors[key] = key === "--text-tertiary" ? muted : secondary;
    }
    colors[key] = readableOnSurfaces(colors[key], interactionSurfaces);
  }
  colors["--chrome-ink"] = readableOnSurfaces(colors["--chrome-ink"], [colors["--bg-app"], colors["--topbar-bg"], colors["--chrome-selected"], colors["--chrome-hover"]]);
  colors["--btn-secondary-text"] = readableOnSurfaces(colors["--btn-secondary-text"], [colors["--btn-secondary-bg"], colors["--btn-secondary-hover"]]);
  // Semantic labels and action text must remain readable on tinted controls,
  // including imported themes designed for different VS Code surfaces.
  colors["--btn-primary-text"] = readableOnSurfaces(ink, [colors["--btn-primary-bg"], colors["--btn-primary-hover"]]);
  // Confirmation actions use a solid accent, distinct from softly tinted actions.
  colors["--btn-confirm-bg"] = accent;
  colors["--btn-confirm-hover"] = mix(accent, ink, .12);
  colors["--btn-confirm-text"] = readableOnSurfaces(ink, [colors["--btn-confirm-bg"], colors["--btn-confirm-hover"]]);
  for (const key of ["--text-secondary", "--btn-subtle-text", "--input-placeholder"]) {
    colors[key] = readableColor(colors[key], mix(canvas, colors["--bg-selected"], 1), ink);
  }
  for (const key of ["--success-text", "--warning-text", "--danger-text"]) {
    const fallback = key === "--success-text" ? added : key === "--danger-text" ? removed
      : mode === "light" ? "#925b10" : "#e6bc66";
    colors[key] = readableColor(colors[key] || fallback, surface, ink);
  }
  // Compound inputs need foreground/background pairs calculated on their own
  // surface. Page-level button and muted colors cannot describe nested controls.
  const embeddedFill = (backgrounds: string[], minimum: number, base = inputBackground) =>
    [.1, .14, .18, .22, .26, .3, .34, .38].map(amount => mix(base, ink, amount))
      .find(fill => contrastRatio(fill, ink) >= 4.5 && backgrounds.every(bg => contrastRatio(fill, bg) >= minimum)) || selected;
  colors["--input-muted"] = readableOnSurfaces(mix(inputBackground, ink, .65), inputSurfaces);
  colors["--input-action-bg"] = embeddedFill(inputSurfaces, 1.12);
  colors["--input-action-hover"] = embeddedFill([...inputSurfaces, colors["--input-action-bg"]], 1.12);
  colors["--input-action-text"] = readableOnSurfaces(ink, [colors["--input-action-bg"], colors["--input-action-hover"]]);
  // The chat composer keeps the source input fill, even when compact form
  // controls need a stronger fill to stand apart from their surrounding card.
  const requestedComposerBackground = mix(canvas, colors["--composer-bg"] || inputBackground, 1);
  const composerBackground = contrastRatio(requestedComposerBackground, ink) >= 4.5 ? requestedComposerBackground : inputBackground;
  colors["--composer-bg"] = composerBackground;
  colors["--composer-control-bg"] = embeddedFill([composerBackground], 1.12, composerBackground);
  colors["--composer-control-hover"] = colors["--button-hover-bg"];
  const composerSurfaces = [composerBackground, colors["--composer-control-bg"], colors["--composer-control-hover"]];
  colors["--composer-text"] = readableOnSurfaces(colors["--composer-text"] || colors["--input-text"], composerSurfaces);
  colors["--composer-control-text"] = readableOnSurfaces(ink, composerSurfaces);
  colors["--composer-muted"] = readableOnSurfaces(mix(composerBackground, ink, .65), composerSurfaces);
  colors["--composer-send-bg"] = ink;
  colors["--composer-send-hover"] = mix(ink, composerBackground, .12);
  colors["--composer-send-text"] = readableOnSurfaces(composerBackground, [ink, colors["--composer-send-hover"]]);
  colors["--composer-disabled-bg"] = colors["--composer-control-bg"];
  colors["--composer-disabled-text"] = readableColor(mix(composerBackground, ink, .5), colors["--composer-disabled-bg"], ink, 3);
  colors["--composer-ring-track"] = embeddedFill([composerBackground], 1.35, composerBackground);
  colors["--composer-divider"] = mix(composerBackground, ink, .12);
  for (const status of ["warning", "danger"]) colors[`--composer-${status}`] = readableOnSurfaces(colors[`--${status}-text`], composerSurfaces);
  colors["--composer-permission-bg"] = mix(composerBackground, colors["--composer-danger"], .08);
  colors["--composer-permission-hover"] = mix(composerBackground, colors["--composer-danger"], .14);
  colors["--composer-danger"] = readableOnSurfaces(colors["--composer-danger"], [colors["--composer-permission-bg"], colors["--composer-permission-hover"]]);
  // Data and Markdown tables share neutral surfaces, independent of selection
  // and code-block colors from the imported editor theme.
  colors["--table-bg"] = canvas;
  colors["--table-header-bg"] = mix(canvas, ink, .1);
  colors["--table-stripe-bg"] = mix(canvas, ink, .035);
  colors["--table-hover-bg"] = mix(canvas, ink, .07);
  colors["--table-text"] = readableOnSurfaces(ink, [canvas, colors["--table-stripe-bg"], colors["--table-hover-bg"]]);
  colors["--table-header-text"] = readableColor(ink, colors["--table-header-bg"], ink);
  colors["--table-divider"] = mix(canvas, ink, .12);
  const terminalCanvas = mix(canvas, colors["--terminal-canvas-bg"], 1);
  colors["--terminal-foreground"] = readableColor(colors["--terminal-foreground"], terminalCanvas, ink, 4.5);
  // Older saved imports also gain ANSI colors; faint theme colors are adjusted
  // against the terminal's own canvas rather than the surrounding page.
  TERMINAL_ANSI_NAMES.forEach((name, index) => {
    for (const [prefix, offset] of [["", 0], ["bright-", 8]] as const) {
      const key = `--terminal-${prefix}${name}`;
      colors[key] = readableColor(colors[key] || TERMINAL_ANSI_DEFAULTS[mode][index + offset], terminalCanvas, colors["--terminal-foreground"], 4.5);
    }
  });
  const checkedBackground = mix(canvas, colors["--control-checked-bg"] || accent, 1);
  const requestedMark = colors["--control-checked-mark"];
  colors["--control-checked-bg"] = checkedBackground;
  colors["--control-checked-mark"] = requestedMark && contrastRatio(checkedBackground, mix(checkedBackground, requestedMark, 1)) >= 4.5
    ? requestedMark : contrastRatio(checkedBackground, "#ffffff") >= contrastRatio(checkedBackground, "#000000") ? "#ffffff" : "#000000";
  return colors;
}

export function parseThemeDocument(text: string): Record<string, unknown> {
  const errors: ParseError[] = [];
  const document: unknown = parse(text.replace(/^\uFEFF/, ""), errors, { allowTrailingComma: true });
  if (errors.length || !document || typeof document !== "object" || Array.isArray(document)) {
    throw new Error("主题文件不是有效的 JSON / JSONC 对象。");
  }
  return document as Record<string, unknown>;
}

export function convertTheme(document: Record<string, unknown>, mode?: ThemeMode) {
  const source = document.colors;
  if (!source || typeof source !== "object" || Array.isArray(source)) throw new Error("文件中没有 VS Code 主题 colors 配置。");
  const raw = source as Record<string, unknown>;
  if (!isThemeColor(raw["editor.background"]) || !isThemeColor(raw["editor.foreground"])) {
    throw new Error("主题需要有效的 editor.background 和 editor.foreground 颜色。");
  }
  const colors: Record<string, string> = {};
  Object.entries(COLOR_MAPPING).forEach(([property, keys]) => {
    const color = keys.map((key) => raw[key]).find(isThemeColor);
    if (color) colors[property] = color;
  });
  Object.assign(colors, syntaxColors(document.tokenColors));
  const hex = raw["editor.background"].slice(1);
  const rgb = hex.length <= 4 ? hex.slice(0, 3).split("").map((n) => parseInt(n + n, 16)) : [0, 2, 4].map((i) => parseInt(hex.slice(i, i + 2), 16));
  const inferredMode: ThemeMode = rgb[0] * 0.299 + rgb[1] * 0.587 + rgb[2] * 0.114 > 150 ? "light" : "dark";
  const resolvedMode = mode ?? (document.type === "light" ? "light" : document.type === "dark" ? "dark" : inferredMode);
  return { colors: completeThemeColors(colors, resolvedMode), mode: resolvedMode };
}

export function remoteThemeLocation(input: string): string {
  let url: URL;
  try { url = new URL(input.trim()); } catch { throw new Error("请输入有效的 GitHub 仓库或主题文件地址。"); }
  if (url.protocol !== "https:" || url.username || url.password) throw new Error("主题地址必须使用 HTTPS。");
  url.hash = "";
  if (url.hostname === "github.com") {
    const parts = url.pathname.split("/").filter(Boolean);
    if (parts.length < 2) throw new Error("请输入完整的 GitHub 仓库地址。");
    const repo = parts[1].replace(/\.git$/, "");
    if (parts.length === 2) return `https://raw.githubusercontent.com/${parts[0]}/${repo}/HEAD/package.json`;
    if (parts[2] === "blob" && parts.length >= 5) return `https://raw.githubusercontent.com/${parts[0]}/${repo}/${parts.slice(3).join("/")}`;
    if (parts[2] === "tree" && parts.length >= 4) return `https://raw.githubusercontent.com/${parts[0]}/${repo}/${parts.slice(3).join("/")}/package.json`;
    throw new Error("请使用 GitHub 仓库、主题文件或 raw 地址。");
  }
  return url.href;
}

export function themeCandidates(document: Record<string, unknown>, location: string): ThemeCandidate[] {
  if (document.colors || typeof document.include === "string") return [{ name: typeof document.name === "string" ? document.name : "远程主题", location }];
  const contributes = document.contributes as Record<string, unknown> | undefined;
  if (!Array.isArray(contributes?.themes)) throw new Error("仓库中没有找到 VS Code 主题，请使用主题 JSON 文件地址。");
  const result: ThemeCandidate[] = [];
  const manifestUrl = new URL(location);
  const repo = manifestUrl.hostname === "raw.githubusercontent.com" ? manifestUrl.pathname.split("/").filter(Boolean).slice(0, 2).join("/").toLowerCase() : undefined;
  const declaredRepo = repositoryIdentity(typeof document.repository === "string" ? document.repository : (document.repository as { url?: unknown } | undefined)?.url);
  const extension = repo && repo === declaredRepo && typeof document.publisher === "string" && typeof document.name === "string"
    && /^[\w-]+$/.test(document.publisher) && /^[\w-]+$/.test(document.name)
    ? { publisher: document.publisher, name: document.name, repository: repo } : undefined;
  for (const item of contributes.themes.slice(0, 100)) {
    if (!item || typeof item !== "object" || typeof item.path !== "string") continue;
    const label = typeof item.label === "string" && !/^%.*%$/.test(item.label) ? item.label : typeof item.id === "string" ? item.id : item.path.split("/").pop() || item.path;
    result.push({
      name: label,
      location: new URL(item.path, location).href,
      mode: item.uiTheme === "vs" || item.uiTheme === "hc-light" ? "light" : item.uiTheme === "vs-dark" || item.uiTheme === "hc-black" ? "dark" : undefined,
      ...(extension ? { extension: { ...extension, path: item.path } } : {}),
    });
  }
  if (!result.length) throw new Error("仓库没有可导入的主题文件。");
  return result;
}

export async function resolveThemeDocument(location: string, load: (path: string) => Promise<string>, relative: (path: string, base: string) => string, seen = new Set<string>()): Promise<Record<string, unknown>> {
  if (seen.size >= 6 || seen.has(location)) throw new Error("主题 include 继承循环或层数过多。");
  seen.add(location);
  const document = parseThemeDocument(await load(location));
  if (document.include === undefined) return document;
  if (typeof document.include !== "string" || !document.include.trim()) throw new Error("主题 include 路径无效。");
  const parent = await resolveThemeDocument(relative(document.include, location), load, relative, seen);
  return {
    ...parent, ...document,
    colors: { ...(parent.colors as object), ...(document.colors as object) },
    tokenColors: [...(Array.isArray(parent.tokenColors) ? parent.tokenColors : []), ...(Array.isArray(document.tokenColors) ? document.tokenColors : [])],
  };
}
