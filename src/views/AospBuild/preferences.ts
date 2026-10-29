export type AospTargetKind = "wsl" | "ssh";
export interface SavedAospCommand { id: string; name: string; command: string }
export interface AospPreferences {
  kind: AospTargetKind;
  lastTargets: Partial<Record<AospTargetKind, string>>;
  roots: Record<string, string>;
  lunch: string;
  module: string;
  commands: SavedAospCommand[];
  commandDraft: string;
  commandName: string;
}

export const AOSP_PREFERENCES_KEY = "android-tool.aosp-preferences.v1";
export const workspaceKey = (kind: AospTargetKind, id: string) => `${kind}:${id}`;

export function parseAospPreferences(value: unknown): AospPreferences {
  const saved = value && typeof value === "object" ? value as Record<string, unknown> : {};
  const targets = saved.lastTargets && typeof saved.lastTargets === "object" ? saved.lastTargets as Record<string, unknown> : {};
  const roots = saved.roots && typeof saved.roots === "object" ? saved.roots : {};
  return {
    kind: saved.kind === "ssh" ? "ssh" : "wsl",
    lastTargets: Object.fromEntries(Object.entries(targets).filter(([key, value]) => (key === "wsl" || key === "ssh") && typeof value === "string")),
    roots: Object.fromEntries(Object.entries(roots).filter(([key, value]) => /^(wsl|ssh):/.test(key) && typeof value === "string")),
    lunch: typeof saved.lunch === "string" ? saved.lunch : "",
    module: typeof saved.module === "string" ? saved.module : "",
    commands: Array.isArray(saved.commands) ? saved.commands.filter((item): item is SavedAospCommand =>
      item && typeof item.id === "string" && typeof item.name === "string" && typeof item.command === "string" && !!item.command.trim()) : [],
    commandDraft: typeof saved.commandDraft === "string" ? saved.commandDraft : "",
    commandName: typeof saved.commandName === "string" ? saved.commandName : "",
  };
}

export function readAospPreferences(): AospPreferences {
  try {
    const saved = window.localStorage.getItem(AOSP_PREFERENCES_KEY);
    if (saved) return parseAospPreferences(JSON.parse(saved));
    const legacy = JSON.parse(window.localStorage.getItem("android-tool.aosp-workspace-selection.v1") || "null");
    if (legacy && (legacy.targetKind === "wsl" || legacy.targetKind === "ssh") && typeof legacy.targetId === "string") {
      return parseAospPreferences({ kind: legacy.targetKind, lastTargets: { [legacy.targetKind]: legacy.targetId }, roots: { [workspaceKey(legacy.targetKind, legacy.targetId)]: legacy.workspaceRoot || "" } });
    }
  } catch { /* Storage may be unavailable; keep the page usable. */ }
  return parseAospPreferences(null);
}

export function rememberAospWorkspace(preferences: AospPreferences, kind: AospTargetKind, id: string, root: string): AospPreferences {
  return { ...preferences, kind, lastTargets: { ...preferences.lastTargets, [kind]: id }, roots: { ...preferences.roots, [workspaceKey(kind, id)]: root } };
}

export function restoreAospWorkspace(preferences: AospPreferences, kind: AospTargetKind, targets: { id: string; defaultRoot?: string | null }[]) {
  const target = targets.find((item) => item.id === preferences.lastTargets[kind]);
  return { id: target?.id || "", root: target ? preferences.roots[workspaceKey(kind, target.id)] ?? target.defaultRoot ?? "" : "" };
}
