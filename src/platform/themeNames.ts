export type ThemeFamily = { id: string; name: string };
type NamedTheme = { id: string; name: string; mode: "light" | "dark"; family?: ThemeFamily };

export function remoteThemeFamily(location: string, document?: Record<string, unknown>): ThemeFamily {
  const url = new URL(location);
  const parts = url.pathname.split("/").filter(Boolean);
  const repository = url.hostname === "raw.githubusercontent.com" && parts.length >= 2 ? parts.slice(0, 2) : null;
  const declaredName = [document?.displayName, document?.name].find(value => typeof value === "string" && value.trim() && !/^%.*%$/.test(value));
  return {
    id: repository ? `github:${repository.join("/").toLowerCase()}` : location,
    name: typeof declaredName === "string" ? declaredName.trim() : repository?.[1] || themeFileName(location),
  };
}

export function groupThemeOptions<T extends NamedTheme>(themes: T[]): (T & { family: ThemeFamily })[] {
  const families = new Map<string, { family: ThemeFamily; themes: T[] }>();
  // Prefer stored manifest names even when older siblings have no family metadata.
  const known = new Map(themes.filter(theme => theme.family).map(theme => [theme.family!.id, theme.family!]));
  for (const theme of distinguishThemeNames(themes)) {
    let family = theme.family;
    if (!family) {
      if (theme.id.startsWith("custom:local:")) family = { id: "local", name: "本地主题" };
      else {
        try { family = remoteThemeFamily(theme.id.replace(/^custom:remote:/, "")); }
        catch { family = { id: "remote", name: "远程主题" }; }
      }
    }
    family = known.get(family.id) || family;
    const group = families.get(family.id) || { family, themes: [] };
    group.themes.push(theme);
    families.set(family.id, group);
  }
  return [...families.values()].flatMap(group => group.themes.map(theme => ({ ...theme, family: group.family })));
}

export function importedThemeName(documentName: unknown, source: "remote" | "local", candidateName: string): string {
  const name = typeof documentName === "string" ? documentName.trim() : "";
  // Repository manifests identify variants; JSON names may identify only the family.
  return source === "remote" ? candidateName.trim() || name || "远程主题" : name || candidateName;
}

function themeFileName(id: string): string {
  const location = id.replace(/^custom:(?:remote|local):/, "");
  let path = location;
  try { path = new URL(location).pathname; } catch { /* Local paths are not URLs. */ }
  const file = path.split(/[\\/]/).pop() || location;
  try { return decodeURIComponent(file).replace(/\.jsonc?$/i, ""); }
  catch { return file.replace(/\.jsonc?$/i, ""); }
}

/** Disambiguate older saved names for display without rewriting user data or IDs. */
export function distinguishThemeNames<T extends NamedTheme>(themes: T[]): T[] {
  const counts = new Map<string, number>();
  themes.forEach(({ name }) => counts.set(name, (counts.get(name) || 0) + 1));
  const labels = themes.map((theme) => counts.get(theme.name)! > 1
    ? `${theme.name} · ${theme.mode === "light" ? "亮色" : "暗色"}` : theme.name);
  const modeCounts = new Map<string, number>();
  labels.forEach((name) => modeCounts.set(name, (modeCounts.get(name) || 0) + 1));
  const fileLabels = labels.map((name, index) => modeCounts.get(name)! > 1
    ? `${name} · ${themeFileName(themes[index].id)}` : name);
  const fileCounts = new Map<string, number>();
  fileLabels.forEach((name) => fileCounts.set(name, (fileCounts.get(name) || 0) + 1));
  return themes.map((theme, index) => ({ ...theme, name: fileCounts.get(fileLabels[index])! > 1
    ? `${fileLabels[index]} · ${theme.id.replace(/^custom:(?:remote|local):/, "")}` : fileLabels[index] }));
}
