import { useCallback, useEffect, useMemo, useSyncExternalStore } from "react";
import { getHistoryTurnEvents, type AgentEventPayload, type HistoryTurnEventPage } from "@/platform/chatApi";
import { historyProcessEvents, mergeHistoryProcessPage } from "./historyProcess";
import { estimateRetainedBytes, pruneIdleProcessEntries } from "./processCacheBudget";

const EMPTY_EVENTS: AgentEventPayload[] = [];
type ProcessSnapshot = {
  page: HistoryTurnEventPage | null;
  events: AgentEventPayload[];
  projected: AgentEventPayload[] | null;
  loading: boolean;
  error: string;
};
type ProcessEntry = {
  snapshot: ProcessSnapshot;
  listeners: Set<() => void>;
  inFlight: Promise<void> | null;
  touchedAt: number;
  bytes: number;
};

const EMPTY_SNAPSHOT: ProcessSnapshot = { page: null, events: EMPTY_EVENTS, projected: null, loading: false, error: "" };
const processCache = new Map<string, ProcessEntry>();

function pruneProcessCache() {
  pruneIdleProcessEntries(processCache);
}

function cacheEntry(key: string) {
  let entry = processCache.get(key);
  if (!entry) {
    entry = { snapshot: EMPTY_SNAPSHOT, listeners: new Set(), inFlight: null, touchedAt: Date.now(), bytes: 0 };
    processCache.set(key, entry);
  }
  entry.touchedAt = Date.now();
  return entry;
}

function updateEntry(entry: ProcessEntry, snapshot: ProcessSnapshot) {
  entry.snapshot = snapshot;
  entry.touchedAt = Date.now();
  entry.listeners.forEach((listener) => listener());
}

function loadProcessPage(entry: ProcessEntry, sessionId: string, turnId: string): Promise<void> {
  if (entry.inFlight) return entry.inFlight;
  if (entry.snapshot.page?.nextCursor === null) return Promise.resolve();
  const current = entry.snapshot;
  // Startup, navigation hover and mounted rows share this exact request/cache.
  entry.inFlight = Promise.resolve().then(async () => {
    try {
      const next = await getHistoryTurnEvents(sessionId, turnId, current.page?.nextCursor ?? undefined, current.page?.throughRowId);
      const events = mergeHistoryProcessPage(current.events, next.events);
      const snapshot = { page: next, events, projected: historyProcessEvents(events), loading: false, error: "" };
      entry.bytes = estimateRetainedBytes(snapshot);
      updateEntry(entry, snapshot);
    } catch (failure) {
      updateEntry(entry, { ...entry.snapshot, loading: false, error: String(failure) });
    } finally {
      entry.inFlight = null;
      entry.touchedAt = Date.now();
      pruneProcessCache();
    }
  });
  updateEntry(entry, { ...current, loading: true, error: "" });
  return entry.inFlight;
}

/** Warm the first page only; later pages remain an explicit user action. */
export function prefetchHistoryProcess(sessionId: string, turnId: string) {
  const entry = cacheEntry(JSON.stringify([sessionId, turnId]));
  return entry.snapshot.page ? Promise.resolve() : loadProcessPage(entry, sessionId, turnId);
}

export function useHistoryProcess(
  sessionId: string,
  turnId: string | undefined,
  expanded: boolean,
  enabled: boolean,
  autoLoad = enabled,
) {
  const key = enabled && sessionId && turnId ? JSON.stringify([sessionId, turnId]) : "";
  const entry = useMemo(() => key ? cacheEntry(key) : null, [key]);
  const subscribe = useCallback((listener: () => void) => {
    if (!entry) return () => undefined;
    entry.listeners.add(listener);
    entry.touchedAt = Date.now();
    return () => {
      entry.listeners.delete(listener);
      entry.touchedAt = Date.now();
      pruneProcessCache();
    };
  }, [entry]);
  const getSnapshot = useCallback(() => entry?.snapshot || EMPTY_SNAPSHOT, [entry]);
  const state = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
  const load = useCallback(async () => {
    if (!enabled || !turnId || !entry) return;
    await loadProcessPage(entry, sessionId, turnId);
  }, [enabled, entry, sessionId, turnId]);
  useEffect(() => {
    if (expanded && autoLoad && !state.page && !state.error) void load();
  }, [autoLoad, expanded, load, state.error, state.page]);
  return {
    events: state.projected,
    loading: state.loading,
    error: state.error,
    hasMore: Boolean(state.page?.nextCursor),
    load,
  };
}
