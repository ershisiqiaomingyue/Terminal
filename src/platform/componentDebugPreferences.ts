export type ComponentDebugTargets = {
  chat: boolean;
  components: boolean;
};

const STORAGE_KEY = "android-tool.component-debug-targets.v1";
const CHANGE_EVENT = "android-tool-component-debug-targets-changed";
const DEFAULTS: ComponentDebugTargets = { chat: false, components: false };

export function readComponentDebugTargets(): ComponentDebugTargets {
  try {
    const saved: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) || "null");
    if (saved && typeof saved === "object") {
      const value = saved as Partial<ComponentDebugTargets>;
      return {
        chat: typeof value.chat === "boolean" ? value.chat : DEFAULTS.chat,
        components: typeof value.components === "boolean" ? value.components : DEFAULTS.components,
      };
    }
  } catch { /* Use defaults when storage is unavailable or malformed. */ }
  return { ...DEFAULTS };
}

export function setComponentDebugTarget(target: keyof ComponentDebugTargets, enabled: boolean) {
  const next = { ...readComponentDebugTargets(), [target]: enabled };
  try { localStorage.setItem(STORAGE_KEY, JSON.stringify(next)); } catch { /* Keep the in-memory change usable. */ }
  window.dispatchEvent(new CustomEvent(CHANGE_EVENT, { detail: next }));
}

export function subscribeComponentDebugTargets(listener: (targets: ComponentDebugTargets) => void) {
  const update = (event: Event) => listener((event as CustomEvent<ComponentDebugTargets>).detail || readComponentDebugTargets());
  const storage = (event: StorageEvent) => { if (event.key === STORAGE_KEY) listener(readComponentDebugTargets()); };
  window.addEventListener(CHANGE_EVENT, update);
  window.addEventListener("storage", storage);
  return () => {
    window.removeEventListener(CHANGE_EVENT, update);
    window.removeEventListener("storage", storage);
  };
}
