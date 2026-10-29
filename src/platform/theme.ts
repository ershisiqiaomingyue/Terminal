import { emit } from "@tauri-apps/api/event";
import { importedThemeName, remoteThemeFamily, type ThemeFamily } from "./themeNames";
import { invoke, isTauri, listen, openFile } from "./runtime";
import { notify } from "./notifications";
import { completeThemeColors, convertTheme, createThemeTextLoader, parseThemeDocument, remoteThemeLocation, repositoryIdentity, resolveThemeDocument, sanitizeThemeColors, themeCandidates, type ThemeCandidate } from "./themeImport";

export type AppTheme = "light" | "dark";
export type ImportedTheme = { id: string; name: string; source: "remote" | "local"; mode: AppTheme; colors: Record<string, string>; sourceColors?: Record<string, string>; family?: ThemeFamily };
const MAX_IMPORTED_THEMES = 300;
const THEME_KEY = "android-tool.theme";
const IMPORTED_THEMES_KEY = "android-tool.imported-themes.v1";
const ACTIVE_THEME_KEY = "android-tool.active-theme.v1";
const THEME_CHANGED_EVENT = "android-tool:theme-changed";
type ThemeUpdate = { mode: AppTheme; importedTheme: ImportedTheme | null };
const listeners = new Set<(theme: AppTheme) => void>();
let appliedVariables: string[] = [];
let themeSync: Promise<void> | undefined;
let appliedThemeSignature = "";

type ThemePreferences = { mode: AppTheme; activeId: string | null; themes: ImportedTheme[] };
const themeValues = new Map<string, string | null>();
let persistenceReady = false;
let themeWrites: Promise<unknown> = Promise.resolve();
let themeRevision = 0;

function readThemeValue(key: string): string | null {
  if (themeValues.has(key)) return themeValues.get(key) ?? null;
  try { return window.localStorage.getItem(key); } catch { return null; }
}

function writeThemeValue(key: string, value: string | null) {
  themeValues.set(key, value);
  try {
    if (value === null) window.localStorage.removeItem(key);
    else window.localStorage.setItem(key, value);
  } catch { /* The native file and memory remain authoritative when browser storage is full. */ }
}

function currentPreferences(): ThemePreferences {
  const themes = readImportedThemes();
  const activeId = readThemeValue(ACTIVE_THEME_KEY);
  return { mode: readAppTheme(), activeId: themes.some(theme => theme.id === activeId) ? activeId : null, themes };
}

function hydratePreferences(preferences: ThemePreferences) {
  ++themeRevision;
  writeThemeValue(THEME_KEY, preferences.mode);
  writeThemeValue(IMPORTED_THEMES_KEY, JSON.stringify(preferences.themes));
  writeThemeValue(ACTIVE_THEME_KEY, preferences.activeId);
}

function persistPreferences() {
  if (!persistenceReady || !isTauri()) return;
  const preferences = currentPreferences();
  themeWrites = themeWrites.catch(() => undefined)
    .then(() => invoke("save_theme_preferences", { preferences }))
    .catch(error => notify(`主题保存失败：${String(error)}`, "error"));
}

export function readAppTheme(): AppTheme {
  if (typeof window === "undefined") return "dark";
  const stored = readThemeValue(THEME_KEY);
  if (stored === "light" || stored === "dark") return stored;
  const param = new URLSearchParams(window.location.search).get("theme");
  if (param === "light" || param === "dark") return param;
  return "dark";
}

export function readImportedThemes(): ImportedTheme[] {
  try {
    const value: unknown = JSON.parse(readThemeValue(IMPORTED_THEMES_KEY) || "[]");
    if (!Array.isArray(value)) return [];
    return value.slice(0, MAX_IMPORTED_THEMES).flatMap((item) => {
      if (!item || typeof item.id !== "string" || !item.id.startsWith("custom:") || typeof item.name !== "string" || !["light", "dark"].includes(item.mode) || !["remote", "local"].includes(item.source)) return [];
      let colors = sanitizeThemeColors(item.colors);
      if (item.sourceColors && typeof item.sourceColors === "object" && !Array.isArray(item.sourceColors)) {
        try {
          const mapped = convertTheme({ colors: item.sourceColors }, item.mode).colors;
          colors = { ...mapped, ...Object.fromEntries(Object.entries(colors).filter(([key]) => key.startsWith("--code-syntax-"))) };
        } catch { /* Older palettes remain usable if source metadata is unavailable. */ }
      }
      const family = item.family && typeof item.family.id === "string" && typeof item.family.name === "string" ? item.family : undefined;
      return colors["--bg-canvas"] && colors["--text-primary"] ? [{ id: item.id, name: item.name, mode: item.mode, source: item.source, colors: completeThemeColors(colors, item.mode), sourceColors: item.sourceColors, family } as ImportedTheme] : [];
    });
  } catch { return []; }
}

export function readThemeSelection(): string {
  try {
    const active = readThemeValue(ACTIVE_THEME_KEY);
    if (readImportedThemes().some((theme) => theme.id === active)) return active!;
  } catch { /* use base theme */ }
  return readAppTheme();
}

export function applyAppTheme(theme: AppTheme) {
  if (typeof document !== "undefined") document.documentElement.setAttribute("data-theme", theme);
}

function clearCustomColors() {
  if (typeof document !== "undefined") {
    appliedVariables.forEach((property) => document.documentElement.style.removeProperty(property));
    document.documentElement.removeAttribute("data-custom-theme");
  }
  appliedVariables = [];
}

function notifyTheme(theme: AppTheme) {
  listeners.forEach((listener) => listener(theme));
}

function applyThemeUpdate({ mode, importedTheme }: ThemeUpdate) {
  const colors = importedTheme ? completeThemeColors(sanitizeThemeColors(importedTheme.colors), mode) : {};
  const signature = JSON.stringify([mode, importedTheme?.id || "", Object.entries(colors).sort(([a], [b]) => a.localeCompare(b))]);
  if (signature === appliedThemeSignature) return;
  clearCustomColors();
  applyAppTheme(mode);
  if (importedTheme && typeof document !== "undefined") {
    document.documentElement.setAttribute("data-custom-theme", importedTheme.id);
    Object.entries(colors).forEach(([property, value]) => document.documentElement.style.setProperty(property, value));
    appliedVariables = Object.keys(colors);
  }
  appliedThemeSignature = signature;
  notifyTheme(mode);
}

function refreshStoredTheme() {
  const selection = readThemeSelection();
  const importedTheme = readImportedThemes().find((theme) => theme.id === selection) || null;
  applyThemeUpdate({ mode: importedTheme?.mode || readAppTheme(), importedTheme });
}

function broadcastTheme(update: ThemeUpdate) {
  if (isTauri()) void themeWrites.then(() => emit(THEME_CHANGED_EVENT, update)).catch((error) => console.warn("Theme synchronization failed", error));
}

export function initializeThemeSync(): Promise<void> {
  if (themeSync) return themeSync;
  window.addEventListener("storage", (event) => {
    if (event.storageArea === window.localStorage && (event.key === null || [THEME_KEY, ACTIVE_THEME_KEY, IMPORTED_THEMES_KEY].includes(event.key))) {
      themeValues.clear(); refreshStoredTheme();
    }
  });
  window.addEventListener("focus", () => {
    if (!isTauri()) { themeValues.clear(); refreshStoredTheme(); return; }
    const revision = themeRevision;
    void themeWrites.then(() => invoke<ThemePreferences | null>("get_theme_preferences"))
      .then(preferences => {
        // A focus read may finish after the user has already picked another
        // theme. Never replace that newer choice with an older disk snapshot.
        if (revision !== themeRevision) return;
        if (preferences) hydratePreferences(preferences);
        refreshStoredTheme();
      })
      .catch(error => console.warn("Theme reload failed", error));
  });
  themeSync = (async () => {
    if (isTauri()) {
      await listen<ThemeUpdate>(THEME_CHANGED_EVENT, (update) => {
        if (update && (update.mode === "light" || update.mode === "dark")) {
          ++themeRevision;
          if (update.importedTheme) writeThemeValue(IMPORTED_THEMES_KEY, JSON.stringify([update.importedTheme, ...readImportedThemes().filter(theme => theme.id !== update.importedTheme!.id)]));
          writeThemeValue(THEME_KEY, update.mode);
          writeThemeValue(ACTIVE_THEME_KEY, update.importedTheme?.id ?? null);
          applyThemeUpdate(update);
        }
      });
      const revision = themeRevision;
      const preferences = await invoke<ThemePreferences>("initialize_theme_preferences", { origin: window.location.origin, legacy: currentPreferences() });
      const changedDuringStartup = revision !== themeRevision;
      if (!changedDuringStartup) hydratePreferences(preferences);
      persistenceReady = true;
      if (changedDuringStartup) persistPreferences();
    }
    // Re-read after registering: another window may have switched during startup.
    refreshStoredTheme();
  })();
  return themeSync;
}

export function setAppTheme(theme: AppTheme) {
  ++themeRevision;
  writeThemeValue(THEME_KEY, theme);
  writeThemeValue(ACTIVE_THEME_KEY, null);
  applyThemeUpdate({ mode: theme, importedTheme: null });
  persistPreferences();
  broadcastTheme({ mode: theme, importedTheme: null });
}

export function selectImportedTheme(id: string) {
  const theme = readImportedThemes().find((item) => item.id === id);
  if (!theme) throw new Error("主题不存在，请重新导入。");
  ++themeRevision;
  writeThemeValue(THEME_KEY, theme.mode);
  writeThemeValue(ACTIVE_THEME_KEY, theme.id);
  applyThemeUpdate({ mode: theme.mode, importedTheme: theme });
  persistPreferences();
  broadcastTheme({ mode: theme.mode, importedTheme: theme });
}

export function removeImportedTheme(id: string) {
  const active = readThemeSelection() === id;
  writeThemeValue(IMPORTED_THEMES_KEY, JSON.stringify(readImportedThemes().filter((theme) => theme.id !== id)));
  if (active) setAppTheme(readAppTheme());
  else { persistPreferences(); notifyTheme(readAppTheme()); }
}

function createImportedTheme(document: Record<string, unknown>, source: "remote" | "local", candidate: ThemeCandidate): ImportedTheme {
  const converted = convertTheme(document, candidate.mode);
  return { id: `custom:${source}:${candidate.location}`, name: importedThemeName(document.name, source, candidate.name), source, ...converted, sourceColors: document.colors as Record<string, string>, family: candidate.family };
}

function saveImportedThemes(imported: ImportedTheme[], activeId: string) {
  const ids = new Set(imported.map(theme => theme.id));
  const themes = [...imported, ...readImportedThemes().filter(item => !ids.has(item.id))];
  if (themes.length > MAX_IMPORTED_THEMES) throw new Error("已达到 300 个子主题的保存上限，请删除不需要的主题后再添加。");
  writeThemeValue(IMPORTED_THEMES_KEY, JSON.stringify(themes));
  selectImportedTheme(activeId);
}

function saveImportedTheme(document: Record<string, unknown>, source: "remote" | "local", candidate: ThemeCandidate): ImportedTheme {
  const theme = createImportedTheme(document, source, candidate);
  saveImportedThemes([theme], theme.id);
  return theme;
}

const downloadTheme = createThemeTextLoader((url) => invoke<string>("fetch_theme_text", { url }));
const downloadPackage = createThemeTextLoader(async (url) => JSON.stringify(await invoke<Record<string, string>>("fetch_theme_package", { url })), 2);

export async function discoverRemoteThemes(input: string): Promise<ThemeCandidate[]> {
  const location = remoteThemeLocation(input);
  const document = parseThemeDocument(await downloadTheme(location));
  const family = remoteThemeFamily(location, document);
  return themeCandidates(document, location).map(candidate => ({ ...candidate, family }));
}

export async function importRemoteTheme(candidate: ThemeCandidate): Promise<ImportedTheme> {
  const theme = await loadRemoteTheme(candidate);
  saveImportedThemes([theme], theme.id);
  return theme;
}

export async function importRemoteThemeFamily(candidates: ThemeCandidate[], onProgress?: (completed: number, total: number) => void): Promise<ImportedTheme[]> {
  const unique = [...new Map(candidates.map(candidate => [candidate.location, candidate])).values()];
  if (!unique.length || unique.length > 100) throw new Error("请选择有效的主题仓库（最多 100 个子主题）。");
  const imported: ImportedTheme[] = [];
  for (let offset = 0; offset < unique.length; offset += 4) {
    const results = await Promise.allSettled(unique.slice(offset, offset + 4).map(loadRemoteTheme));
    const failed = results.find(result => result.status === "rejected");
    if (failed?.status === "rejected") throw failed.reason;
    for (const result of results) if (result.status === "fulfilled") imported.push(result.value);
    onProgress?.(imported.length, unique.length);
  }
  // Commit once: a failed child must not leave a partially installed family.
  const selected = imported.find(theme => theme.mode === readAppTheme()) || imported[0];
  saveImportedThemes(imported, selected.id);
  return imported;
}

async function loadRemoteTheme(candidate: ThemeCandidate): Promise<ImportedTheme> {
  try {
    let document: Record<string, unknown>;
    try {
      document = await resolveThemeDocument(candidate.location, downloadTheme, (path, base) => remoteThemeLocation(new URL(path, base).href));
    } catch (error) {
      if (!candidate.extension || !/\b404\b/.test(String(error))) throw error;
      document = await loadPublishedTheme(candidate);
    }
    return createImportedTheme(document, "remote", candidate);
  } catch (error) {
    throw new Error(`已识别主题“${candidate.name}”，但主题文件未能导入：${error instanceof Error ? error.message : String(error)}`);
  }
}

async function loadPublishedTheme(candidate: ThemeCandidate) {
  const extension = candidate.extension!;
  const registry = `https://open-vsx.org/api/${encodeURIComponent(extension.publisher)}/${encodeURIComponent(extension.name)}/latest`;
  const metadata = parseThemeDocument(await downloadTheme(registry));
  if (repositoryIdentity(metadata.repository) !== extension.repository
    || metadata.namespace !== extension.publisher || metadata.name !== extension.name) {
    throw new Error("仓库未提供生成的主题文件，且发布包来源无法匹配。请使用已发布的主题 JSON 文件。");
  }
  const files = metadata.files as Record<string, unknown> | undefined;
  if (typeof files?.download !== "string") throw new Error("未找到主题发布包。");
  const packageUrl = new URL(files.download);
  if (packageUrl.protocol !== "https:" || packageUrl.hostname !== "open-vsx.org") throw new Error("主题发布包地址无效。");
  const documents: Record<string, string> = JSON.parse(await downloadPackage(packageUrl.href));
  const base = "https://theme-package.invalid/extension/package.json";
  const manifestText = documents["extension/package.json"];
  if (!manifestText) throw new Error("主题发布包缺少清单。");
  const manifest = parseThemeDocument(manifestText);
  if (manifest.publisher !== extension.publisher || manifest.name !== extension.name) throw new Error("主题发布包标识不匹配。");
  // Use the released manifest, which may differ from the repository's HEAD.
  const published = themeCandidates(manifest, base);
  const path = new URL(extension.path, base).href;
  const matched = published.find((item) => item.location === path) || published.find((item) => item.name === candidate.name);
  if (!matched) throw new Error("此主题尚未包含在最新发布包中。");
  return resolveThemeDocument(matched.location, async (url) => {
    const entry = new URL(url);
    if (entry.origin !== new URL(base).origin || !entry.pathname.startsWith("/extension/")) throw new Error("主题继承路径超出发布包。");
    const text = documents[decodeURIComponent(entry.pathname.slice(1))];
    if (text === undefined) throw new Error(`发布包缺少主题文件：${entry.pathname}`);
    return text;
  }, (path, base) => new URL(path, base).href);
}

export async function importLocalTheme(): Promise<ImportedTheme | null> {
  const path = await openFile("导入本地主题", [{ name: "VS Code 主题", extensions: ["json", "jsonc"] }]);
  if (!path) return null;
  const load = (path: string) => invoke<string>("read_code_viewer_file", { path });
  const relative = (path: string, base: string) => {
    if (/^(?:[a-z]+:|[\\/])/i.test(path)) throw new Error("本地主题 include 必须是相对路径。");
    return base.slice(0, Math.max(base.lastIndexOf("/"), base.lastIndexOf("\\")) + 1) + path;
  };
  const document = await resolveThemeDocument(path, load, relative);
  return saveImportedTheme(document, "local", { name: path.split(/[\\/]/).pop() || "本地主题", location: path });
}

export function subscribeAppTheme(listener: (theme: AppTheme) => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

applyAppTheme(readAppTheme());
const initialSelection = readThemeSelection();
if (initialSelection.startsWith("custom:")) refreshStoredTheme();
