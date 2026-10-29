import { useEffect, useMemo, useSyncExternalStore } from "react";
import { getWslStatus } from "@/platform/chatApi";
import { getGlobalEnvironmentTarget, openGlobalSshMenu, useGlobalEnvironment } from "@/platform/globalEnvironment";
import { getGlobalSshSnapshot, refreshGlobalSshConnection } from "@/platform/globalSsh";
import { remoteAutoConnectPaused, REMOTE_CONNECTION_POLICY_EVENT, REMOTE_CONNECTION_POLICY_KEY, setRemoteAutoConnectPaused } from "@/platform/remoteConnectionPolicy";
import { formatInvokeError } from "@/platform/runtime";
import { aospGlobalSshState, AospConnectionController } from "./connectionState";

export function useAospConnection(kind: "ssh" | "wsl", id: string, enabled: boolean) {
  const environment = useGlobalEnvironment();
  const controller = useMemo(() => new AospConnectionController({
    check: async ({ id }) => { await getWslStatus(id); },
    paused: ({ kind, id }) => remoteAutoConnectPaused(kind, id), errorText: formatInvokeError,
  }), []);
  const state = useSyncExternalStore(controller.subscribe, controller.getSnapshot);
  useEffect(() => {
    if (!enabled || kind === "ssh") return;
    void controller.check({ kind, id }, true);
    const policyChanged = (event?: Event) => {
      const target = (event as CustomEvent<{ kind: string; id: string }> | undefined)?.detail;
      if (target && (target.kind !== kind || target.id !== id)) return;
      if (remoteAutoConnectPaused(kind, id)) controller.pause({ kind, id });
      else void controller.check({ kind, id }, true);
    };
    const storageChanged = (event: StorageEvent) => { if (event.key === REMOTE_CONNECTION_POLICY_KEY) policyChanged(); };
    window.addEventListener(REMOTE_CONNECTION_POLICY_EVENT, policyChanged);
    window.addEventListener("storage", storageChanged);
    return () => { controller.invalidate(); window.removeEventListener(REMOTE_CONNECTION_POLICY_EVENT, policyChanged); window.removeEventListener("storage", storageChanged); };
  }, [controller, enabled, kind, id]);
  if (kind === "ssh") {
    const selectedId = environment.target.kind === "ssh" ? environment.target.id : undefined;
    return {
      ...aospGlobalSshState(id, selectedId, environment.ssh.progress?.status, environment.ssh.error || environment.ssh.progress?.error || ""),
      ensure: async (target: { kind: "ssh" | "wsl"; id: string } = { kind, id }) => {
        const current = getGlobalEnvironmentTarget().target;
        const ssh = getGlobalSshSnapshot();
        return target.kind === "ssh" && current.kind === "ssh" && current.id === target.id
          && ssh.connection?.id === target.id && ssh.progress?.status === "connected";
      },
      retry: async () => {
        if (getGlobalEnvironmentTarget().target.kind === "ssh" && getGlobalSshSnapshot().progress?.status === "connected") {
          await refreshGlobalSshConnection();
          return getGlobalSshSnapshot().progress?.status === "connected";
        }
        openGlobalSshMenu();
        return false;
      },
      disconnect: async () => { openGlobalSshMenu(); },
    };
  }
  const key = `${kind}:${id}`;
  const snapshot = enabled && state.key === key ? state : { key, phase: "idle" as const, error: "" };
  return {
    ...snapshot,
    ensure: (target: { kind: "ssh" | "wsl"; id: string } = { kind, id }) => target.kind === "wsl" ? controller.check(target) : Promise.resolve(false),
    retry: () => { setRemoteAutoConnectPaused(kind, id, false); return controller.check({ kind, id }, true); },
    disconnect: async () => { setRemoteAutoConnectPaused(kind, id, true); controller.pause({ kind, id }); },
  };
}
