import { invoke, isTauri } from "./runtime";
import { notify } from "./notifications";
import { cacheBrowserValue } from "./browserStorage";

export const PROJECT_PREFERENCE_KEYS = [
  "android-tool.chat.workspaces.v1",
  "android-tool.project-automation.v1",
  "android-tool.project-automation.active-project.v1",
] as const;
let recoverLegacyProjects = false;
let initialized = false;
let pendingWrite: Promise<unknown> = Promise.resolve();

/** Hydrate before React mounts, so empty origin storage cannot overwrite desktop data. */
export async function initializeProjectPreferences() {
  if (!isTauri()) return;
  const values = await invoke<Record<string, string>>("get_project_preferences");
  const workspaceKey = PROJECT_PREFERENCE_KEYS[0];
  recoverLegacyProjects = values[workspaceKey] === undefined;
  for (const key of PROJECT_PREFERENCE_KEYS) {
    if (values[key] !== undefined) window.localStorage.setItem(key, values[key]);
    else {
      const legacy = window.localStorage.getItem(key);
      if (legacy !== null && key !== workspaceKey) await invoke("save_project_preference", { key, value: legacy });
    }
  }
  initialized = true;
}

export function takeLegacyProjectRecovery() {
  const recover = recoverLegacyProjects;
  recoverLegacyProjects = false;
  return recover;
}

export function writeProjectPreference(key: typeof PROJECT_PREFERENCE_KEYS[number], value: string) {
  cacheBrowserValue(key, value);
  if (!isTauri()) return;
  if (!initialized || (key === PROJECT_PREFERENCE_KEYS[0] && recoverLegacyProjects)) return;
  // Preserve write order when users rapidly edit or remove projects.
  pendingWrite = pendingWrite.catch(() => undefined)
    .then(() => invoke("save_project_preference", { key, value }))
    .catch((error) => notify(`项目配置保存失败：${String(error)}`, "error"));
}
