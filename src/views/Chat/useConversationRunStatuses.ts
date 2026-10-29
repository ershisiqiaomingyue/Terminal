import { useEffect, useRef, useState } from "react";
import { getAgentThreadSnapshot, listAgentRuns, type AgentThreadSnapshot } from "@/platform/chatApi";
import { isTauri, listen } from "@/platform/runtime";
import type { AgentThreadUpdate } from "./threadUpdates";
import { acceptConversationRunStatus, type ConversationRunStatus } from "./conversationRunStatus";
import type { RunStatusView } from "./sessionRowStatus";

/**
 * 把内部运行状态翻成侧栏要消费的视图：phase 之外再带上 pending 类型，
 * 侧栏才能区分「待回答」和「待审批」这两种完全不同的黄点。
 */
function toView(status: ConversationRunStatus): RunStatusView | null {
  if (!status.phase) return null;
  return status.phase === "waiting"
    ? { phase: "waiting", pending: status.pending ?? undefined }
    : { phase: "running" };
}

export function useConversationRunStatuses() {
  const [phases, setPhases] = useState<Record<string, RunStatusView>>({});
  const statuses = useRef(new Map<string, ConversationRunStatus>());
  useEffect(() => {
    if (!isTauri()) return;
    let active = true;
    let refreshing = false;
    let ready = false;
    let disposers: Array<() => void> = [];
    const receive = (snapshot: AgentThreadSnapshot) => {
      if (!active || !snapshot?.id) return;
      const previous = statuses.current.get(snapshot.id);
      const next = acceptConversationRunStatus(previous, snapshot);
      if (!next || next === previous) return;
      statuses.current.set(snapshot.id, next);
      // Token deltas advance revisions but do not repaint the navigation; a change in
      // the pending kind (question <-> approval) must repaint even though phase stays.
      if (previous?.phase === next.phase && (previous?.pending ?? null) === (next.pending ?? null)) return;
      setPhases((current) => {
        const updated = { ...current };
        const view = toView(next);
        if (view) updated[snapshot.id] = view;
        else delete updated[snapshot.id];
        return updated;
      });
    };
    const refresh = async () => {
      if (!active || refreshing || !ready || document.visibilityState === "hidden") return;
      refreshing = true;
      try {
        const runs = await listAgentRuns();
        if (!active) return;
        const ids = new Set([...statuses.current].filter(([, status]) => status.phase).map(([id]) => id));
        const liveIds = new Set(runs.filter((run) => run.status === "preparing" || run.status === "running").map((run) => run.sessionId));
        for (const id of liveIds) ids.add(id);
        const pending = [...ids];
        const worker = async () => {
          while (active && pending.length) {
            const id = pending.shift()!;
            try {
              const known = statuses.current.get(id);
              let snapshot = await getAgentThreadSnapshot(id, known?.revision);
              // A versioned null can mean unchanged or removed. Verify before clearing.
              if (!snapshot && known?.phase && !liveIds.has(id)) {
                snapshot = await getAgentThreadSnapshot(id);
                if (active && !snapshot && statuses.current.get(id) === known) {
                  statuses.current.set(id, { revision: known.revision, phase: null, pending: null });
                  setPhases((current) => { const next = { ...current }; delete next[id]; return next; });
                }
              }
              if (snapshot) receive(snapshot);
            } catch { /* A later event or reconciliation can recover this row. */ }
          }
        };
        await Promise.all(Array.from({ length: Math.min(4, pending.length) }, worker));
      } catch { /* Keep confirmed live state during a transient IPC failure. */ }
      finally { refreshing = false; }
    };
    let subscribing = false;
    const subscribe = async () => {
      if (!active || ready || subscribing) return;
      subscribing = true;
      const acquired: Array<() => void> = [];
      try {
        acquired.push(await listen<AgentThreadUpdate>("agent:thread_update", (update) => receive(update.kind === "snapshot" ? update.snapshot : update.metadata)));
        acquired.push(await listen<AgentThreadSnapshot>("agent:thread_snapshot", receive));
        if (!active) { acquired.forEach((dispose) => dispose()); return; }
        disposers = acquired;
        ready = true;
        void refresh();
      } catch { acquired.forEach((dispose) => dispose()); }
      finally { subscribing = false; }
    };
    const reconcile = () => { if (ready) void refresh(); else void subscribe(); };
    const interval = window.setInterval(() => {
      if (!ready || [...statuses.current.values()].some((status) => status.phase)) reconcile();
    }, 10_000);
    window.addEventListener("focus", reconcile);
    document.addEventListener("visibilitychange", reconcile);
    void subscribe();
    return () => {
      active = false;
      disposers.forEach((dispose) => dispose());
      window.clearInterval(interval);
      window.removeEventListener("focus", reconcile);
      document.removeEventListener("visibilitychange", reconcile);
    };
  }, []);
  return phases;
}
