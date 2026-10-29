export type AospBusyAction = "probe" | "lunch" | "build" | "artifacts" | "install";

export interface AospRunTask {
  id: string;
  status: "idle" | "running" | "done" | "error" | "cancelled";
  progress: number;
  message: string;
  elapsedSeconds: number;
  error?: string;
}

export interface AospLunchTarget {
  name: string;
  outputs: string[];
}

export interface AospRunState {
  busyAction: AospBusyAction | null;
  task: AospRunTask;
  lunchTargets: AospLunchTarget[];
  logs: string[];
  logOffset: number;
}

const listeners = new Set<() => void>();

let state: AospRunState = {
  busyAction: null,
  task: {
    id: "task-init",
    status: "idle",
    progress: 0,
    message: "就绪，等待编译指令",
    elapsedSeconds: 0,
  },
  lunchTargets: [],
  logs: [],
  logOffset: 0,
};

export function getAospRun() {
  return state;
}

export function patchAospRun(partial: Partial<AospRunState>) {
  state = { ...state, ...partial };
  listeners.forEach((listener) => listener());
}

export function subscribeAospRun(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

// Join partial reads before splitting lines. Keep a bounded viewport while
// the backend retains the complete log on disk.
export function appendAospOutput(logs: string[], text: string) {
  if (!text) return logs;
  const prefix = logs.slice(0, -1);
  const tail = (logs[logs.length - 1] || "") + text.replace(/\r\n/g, "\n").replace(/\r/g, "\n").replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "");
  return [...prefix, ...tail.split("\n")].slice(-3000).map((line) => line.slice(-16000));
}

export function parseLunchMenu(text: string) {
  const targets = new Set<string>();
  for (const line of text.split(/\r?\n/)) {
    if (line.startsWith("__AT_")) continue;
    const numbered = line.match(/^\s*\d+\.\s+(\S+)/);
    const plain = line.match(/^\s*([A-Za-z0-9._-]+-(?:user|userdebug|eng))\s*$/);
    const target = numbered?.[1] || plain?.[1];
    if (target) targets.add(target);
  }
  return [...targets];
}

export function lunchProduct(target: string) {
  return target.replace(/-(?:userdebug|user|eng)$/, "");
}

export function lunchWasPackaged(target: string, builtProduct: string) {
  if (!builtProduct) return false;
  if (target === builtProduct) return true;
  const product = lunchProduct(target);
  return product === builtProduct || product === lunchProduct(builtProduct) || builtProduct.startsWith(`${product}-`);
}
