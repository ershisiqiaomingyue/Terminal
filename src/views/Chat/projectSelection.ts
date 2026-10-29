export const PROJECT_AUTOMATION_STORAGE_KEY = "android-tool.project-automation.v1";
export const ACTIVE_PROJECT_STORAGE_KEY = "android-tool.project-automation.active-project.v1";

export type AutomationProjectConfiguration = Record<string, unknown> & { id: string; name: string };

export function readAutomationProjects(): AutomationProjectConfiguration[] {
  if (typeof window === "undefined") return [];
  try {
    const raw: unknown = JSON.parse(window.localStorage.getItem(PROJECT_AUTOMATION_STORAGE_KEY) || "[]");
    if (!Array.isArray(raw)) return [];
    const seen = new Set<string>();
    return raw.filter((item): item is AutomationProjectConfiguration => {
      if (!item || typeof item.id !== "string" || !item.id || typeof item.name !== "string" || seen.has(item.id)) return false;
      seen.add(item.id);
      return true;
    });
  } catch { return []; }
}

/** Undefined preserves the legacy setting; an explicit empty list means general only. */
export function selectedAutomationProjectIds(projects: AutomationProjectConfiguration[], selection?: readonly string[]) {
  const requested = new Set(selection ?? [typeof window === "undefined" ? "general" : window.localStorage.getItem(ACTIVE_PROJECT_STORAGE_KEY) || "general"]);
  return projects.filter((project) => project.id !== "general" && requested.has(project.id)).map((project) => project.id);
}

export function readProjectSelection(value: unknown): string[] | undefined {
  return Array.isArray(value) ? [...new Set(value.filter((id): id is string => typeof id === "string" && !!id && id !== "general"))] : undefined;
}
