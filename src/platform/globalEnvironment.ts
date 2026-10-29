import { useMemo, useSyncExternalStore } from "react";
import { listWslProfiles, type SshConnection, type WslProfile } from "./api";
import { getWslStatus } from "./chatApi";
import { connectGlobalSsh, getGlobalSshSnapshot, restoreGlobalSshSelection, refreshGlobalSshConnection, useGlobalSshConnection } from "./globalSsh";
import { resolveGlobalEnvironment, type EnvironmentSelection, type WslConnectionState } from "./executionEnvironment";
import { formatInvokeError, isTauri } from "./runtime";

export const WSL_PROFILES_CHANGED_EVENT = "android-tool-wsl-profiles-changed";
export const OPEN_GLOBAL_ENVIRONMENT_EVENT = "android-tool-open-global-environment";

export function openGlobalSshMenu() {
  window.dispatchEvent(new CustomEvent(OPEN_GLOBAL_ENVIRONMENT_EVENT, { detail: { tab: "ssh" } }));
}
type EnvironmentState = {
  selection: EnvironmentSelection;
  profiles: WslProfile[];
  profileId: string | null;
  status: WslConnectionState;
  remoteStatus: Record<string, unknown> | null;
  error: string;
  profilesError: string;
};
let state: EnvironmentState = { selection: "auto", profiles: [], profileId: null, status: "idle", remoteStatus: null, error: "", profilesError: "" };
let generation = 0;
let profilesPromise: Promise<void> | null = null;
let statusRequest: { generation: number; promise: Promise<boolean> } | null = null;
const listeners = new Set<() => void>();
const getSnapshot = () => state;
function publish(next: EnvironmentState) {
  state = next;
  for (const listener of listeners) listener();
}

export function selectGlobalLocal() {
  generation += 1;
  publish({ ...state, selection: "local", status: "idle", remoteStatus: null, error: "" });
}

export async function selectGlobalSsh(connection: SshConnection) {
  generation += 1;
  publish({ ...state, selection: "ssh", status: "idle", remoteStatus: null, error: "" });
  return connectGlobalSsh(connection);
}

export function refreshGlobalWslProfiles(): Promise<void> {
  if (profilesPromise) return profilesPromise;
  if (!isTauri()) return Promise.resolve();
  profilesPromise = listWslProfiles().then((profiles) => {
    const previous = state.profiles.find((profile) => profile.id === state.profileId);
    const current = profiles.find((profile) => profile.id === state.profileId);
    const changed = state.profileId !== null && JSON.stringify(previous) !== JSON.stringify(current);
    if (changed) generation += 1;
    publish({ ...state, profiles, profilesError: "", ...(changed ? { status: "idle" as const, remoteStatus: null, error: current ? "WSL 配置已变更，请重新连接。" : "所选 WSL 配置已删除，请重新选择。" } : {}) });
  }).catch((error) => publish({ ...state, profilesError: formatInvokeError(error) }))
    .finally(() => { profilesPromise = null; });
  return profilesPromise;
}

export function refreshGlobalWslStatus(): Promise<boolean> {
  if (state.selection !== "wsl" || !state.profileId) return Promise.resolve(false);
  const profile = state.profiles.find((item) => item.id === state.profileId);
  if (!profile) return Promise.resolve(false);
  const requestGeneration = generation;
  if (statusRequest?.generation === requestGeneration) return statusRequest.promise;
  const promise = getWslStatus(profile.id, profile.defaultRoot || undefined).then((remoteStatus) => {
    if (generation === requestGeneration) publish({ ...state, status: "connected", remoteStatus, error: "" });
    return generation === requestGeneration;
  }).catch((error) => {
    if (generation === requestGeneration) publish({ ...state, status: "failed", remoteStatus: null, error: formatInvokeError(error) });
    return false;
  }).finally(() => {
    if (statusRequest?.generation === requestGeneration) statusRequest = null;
  });
  statusRequest = { generation: requestGeneration, promise };
  return promise;
}

export function selectGlobalWsl(profile: WslProfile) {
  if (state.selection === "wsl" && state.profileId === profile.id
    && (state.status === "connected" || state.status === "connecting")) return Promise.resolve();
  generation += 1;
  publish({ ...state, selection: "wsl", profileId: profile.id, status: "connecting", remoteStatus: null, error: "" });
  return refreshGlobalWslStatus();
}

function refreshProfiles() { void refreshGlobalWslProfiles(); }
function subscribe(listener: () => void) {
  listeners.add(listener);
  if (listeners.size === 1) {
    refreshProfiles();
    window.addEventListener("focus", refreshProfiles);
    window.addEventListener(WSL_PROFILES_CHANGED_EVENT, refreshProfiles);
  }
  return () => {
    listeners.delete(listener);
    if (!listeners.size) {
      window.removeEventListener("focus", refreshProfiles);
      window.removeEventListener(WSL_PROFILES_CHANGED_EVENT, refreshProfiles);
    }
  };
}

export async function refreshGlobalEnvironment() {
  await Promise.all([refreshGlobalSshConnection(), refreshGlobalWslProfiles()]);
}

export function getGlobalEnvironmentTarget() {
  const ssh = getGlobalSshSnapshot();
  const profile = state.profiles.find((item) => item.id === state.profileId) || null;
  return resolveGlobalEnvironment(state.selection, ssh.connection, ssh.progress, profile, state.status, state.error);
}

export function useGlobalEnvironment() {
  const ssh = useGlobalSshConnection();
  const environment = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
  return useMemo(() => {
    const profile = environment.profiles.find((item) => item.id === environment.profileId) || null;
    return { ...environment, ssh, profile, ...resolveGlobalEnvironment(environment.selection, ssh.connection, ssh.progress, profile, environment.status, environment.error) };
  }, [ssh, environment]);
}

// Window handoff copies the selection; the existing Rust connection stays shared.
export function captureGlobalEnvironment() { return { ...state, sshId: getGlobalSshSnapshot().connection?.id || null }; }
export function restoreGlobalEnvironment(value: unknown) {
  if (!value || typeof value !== "object") return;
  const saved = value as EnvironmentState & { sshId?: string | null };
  if (!["auto", "local", "ssh", "wsl"].includes(saved.selection)) return;
  state = { ...state, ...saved };
  restoreGlobalSshSelection(saved.sshId || null);
}
