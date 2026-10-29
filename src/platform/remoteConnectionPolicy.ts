export const REMOTE_CONNECTION_POLICY_KEY = "android-tool.remote-auto-connect-paused.v1";
export const REMOTE_CONNECTION_POLICY_EVENT = "android-tool-remote-connection-policy-changed";

export function parsePausedRemoteTargets(value: unknown): string[] {
  return Array.isArray(value) ? [...new Set(value.filter((key): key is string => typeof key === "string" && /^(ssh|wsl):.+/.test(key)))] : [];
}

let paused = new Set<string>();
let storedValue: string | null | undefined;
function read() {
  try {
    const value = window.localStorage.getItem(REMOTE_CONNECTION_POLICY_KEY);
    if (value !== storedValue) { paused = new Set(parsePausedRemoteTargets(JSON.parse(value || "[]"))); storedValue = value; }
  }
  catch { /* Keep the in-memory choice when storage is unavailable. */ }
  return paused;
}

export function remoteAutoConnectPaused(kind: "ssh" | "wsl", id: string) {
  return read().has(`${kind}:${id}`);
}

export function setRemoteAutoConnectPaused(kind: "ssh" | "wsl", id: string, pause: boolean) {
  const targets = read();
  const key = `${kind}:${id}`;
  if (pause) targets.add(key); else targets.delete(key);
  try { const value = JSON.stringify([...targets]); window.localStorage.setItem(REMOTE_CONNECTION_POLICY_KEY, value); storedValue = value; } catch { /* Retain in memory. */ }
  window.dispatchEvent(new CustomEvent(REMOTE_CONNECTION_POLICY_EVENT, { detail: { kind, id, paused: pause } }));
}
