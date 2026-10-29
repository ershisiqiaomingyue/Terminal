import { useEffect, useState } from "react";
import { getAgentThreadSnapshot, type AgentThreadSnapshot } from "@/platform/chatApi";
import { isTauri, listen } from "@/platform/runtime";
import { shouldApplyThreadSnapshot } from "./threadSnapshotState";
import {
  ThreadUpdateReconciler,
  type AgentThreadUpdate,
  type ThreadUpdateReconcileResult,
} from "./threadUpdates";

export function useAgentThreadSnapshot(sessionId: string) {
  const [snapshot, setSnapshot] = useState<AgentThreadSnapshot | null>(null);

  useEffect(() => {
    let active = true;
    let frame: number | null = null;
    let buffered: AgentThreadSnapshot | null = null;
    let unlistenSnapshots: (() => void) | undefined;
    let unlistenUpdates: (() => void) | undefined;
    let retry: ReturnType<typeof setTimeout> | undefined;
    let failures = 0;
    let listenersReady = false;
    let refreshing = false;
    let refreshRequested = false;
    let subscribing = false;
    const reconciler = new ThreadUpdateReconciler(sessionId);

    const publish = () => {
      frame = null;
      const incoming = buffered;
      buffered = null;
      if (active && incoming) {
        setSnapshot((current) => shouldApplyThreadSnapshot(current, incoming)
          || (current?.id === incoming.id && current.revision === incoming.revision) ? incoming : current);
      }
    };

    const stage = ({ snapshot: incoming, changed }: ThreadUpdateReconcileResult) => {
      if (!active || !changed || !incoming) return;
      buffered = incoming;
      if (incoming.activeTurn?.status !== "running" || incoming.status === "waiting_for_permission") {
        if (frame !== null) window.cancelAnimationFrame(frame);
        publish();
      } else if (frame === null) {
        frame = window.requestAnimationFrame(publish);
      }
    };

    const scheduleRetry = (action: () => Promise<void>) => {
      if (!active || retry !== undefined) return;
      const delay = Math.min(30_000, 500 * 2 ** Math.min(failures++, 6));
      retry = setTimeout(() => {
        retry = undefined;
        if (active) void action();
      }, delay);
    };

    const refresh = async () => {
      if (!active || !listenersReady) {
        refreshRequested = true;
        return;
      }
      if (refreshing) {
        refreshRequested = true;
        return;
      }
      refreshing = true;
      refreshRequested = false;
      let failed = false;
      try {
        const value = await getAgentThreadSnapshot(sessionId, reconciler.refreshAfterRevision);
        if (active && value) stage(reconciler.acceptSnapshot(value));
        if (!reconciler.needsRefresh) failures = 0;
      } catch {
        failed = true;
      } finally {
        refreshing = false;
        if (!active) return;
        if (failed) {
          scheduleRetry(refresh);
        } else if (refreshRequested) {
          void refresh();
        } else if (reconciler.needsRefresh) {
          scheduleRetry(refresh);
        }
      }
    };

    const requestRefresh = () => {
      refreshRequested = true;
      if (active && listenersReady && !refreshing && retry === undefined) void refresh();
    };

    const receiveSnapshot = (incoming: AgentThreadSnapshot) => {
      if (!active) return;
      const result = reconciler.acceptSnapshot(incoming);
      stage(result);
      if (result.needsRefresh) requestRefresh();
    };

    const receiveUpdate = (update: AgentThreadUpdate) => {
      if (!active) return;
      const result = reconciler.acceptUpdate(update);
      stage(result);
      if (result.needsRefresh) requestRefresh();
    };

    const subscribe = async () => {
      if (!active || subscribing || listenersReady) return;
      subscribing = true;
      let disposeUpdates: (() => void) | undefined;
      let disposeSnapshots: (() => void) | undefined;
      try {
        // Incremental updates are primary. A baseline read after both listeners
        // are installed closes either listener's handshake window.
        disposeUpdates = await listen<AgentThreadUpdate>("agent:thread_update", receiveUpdate);
        if (!active) { disposeUpdates(); return; }
        disposeSnapshots = await listen<AgentThreadSnapshot>("agent:thread_snapshot", receiveSnapshot);
        if (!active) { disposeUpdates(); disposeSnapshots(); return; }
        unlistenUpdates = disposeUpdates;
        unlistenSnapshots = disposeSnapshots;
        listenersReady = true;
        failures = 0;
        requestRefresh();
      } catch {
        disposeUpdates?.();
        disposeSnapshots?.();
        listenersReady = false;
        scheduleRetry(subscribe);
      } finally {
        subscribing = false;
      }
    };

    const reconcile = () => {
      if (document.visibilityState === "hidden") return;
      if (retry !== undefined) {
        clearTimeout(retry);
        retry = undefined;
      }
      if (listenersReady) requestRefresh(); else void subscribe();
    };

    setSnapshot(null);
    if (!sessionId || !isTauri()) return;
    window.addEventListener("focus", reconcile);
    document.addEventListener("visibilitychange", reconcile);
    const interval = setInterval(() => {
      if (document.visibilityState !== "hidden" && reconciler.snapshot?.activeTurn?.status === "running") requestRefresh();
    }, 5_000);
    void subscribe();

    return () => {
      active = false;
      if (frame !== null) window.cancelAnimationFrame(frame);
      if (retry !== undefined) clearTimeout(retry);
      clearInterval(interval);
      window.removeEventListener("focus", reconcile);
      document.removeEventListener("visibilitychange", reconcile);
      unlistenSnapshots?.();
      unlistenUpdates?.();
    };
  }, [sessionId]);

  return snapshot?.id === sessionId ? snapshot : null;
}
