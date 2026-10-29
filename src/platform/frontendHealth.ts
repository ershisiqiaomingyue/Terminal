import { invoke as nativeInvoke } from "@tauri-apps/api/core";
import { DiagnosticEventBuffer, diagnosticError, type DiagnosticContext, type DiagnosticEvent } from "./diagnosticEvents.ts";

const CACHE_KEY = "android-tool-pending-diagnostics-v1";
let buffer: DiagnosticEventBuffer | undefined;
let initialized = false;
let timer: ReturnType<typeof setTimeout> | undefined;
let context: DiagnosticContext = {};
let heartbeatBusy = false;
let pendingPersistence: DiagnosticEvent[] = [];
let persistenceTimer: ReturnType<typeof setTimeout> | undefined;
let lastPersisted = 0;
let lastResizeObserverWarning = 0;
const desktop = () => typeof window !== "undefined" && ("__TAURI_INTERNALS__" in window || "__TAURI__" in window);

function isResizeObserverWarning(error: unknown): string | null {
  const message = error instanceof Error ? error.message : String(error ?? "");
  return /ResizeObserver loop (completed with undelivered notifications|limit exceeded)/i.test(message)
    ? message
    : null;
}

function persistQueue(events: DiagnosticEvent[]) {
  pendingPersistence = events;
  const save = () => {
    persistenceTimer = undefined;
    lastPersisted = Date.now();
    try { sessionStorage.setItem(CACHE_KEY, JSON.stringify(pendingPersistence)); } catch {}
  };
  // Coalesce an exception storm so logging itself cannot stall the UI.
  if (Date.now() - lastPersisted >= 250) { clearTimeout(persistenceTimer); save(); }
  else if (persistenceTimer === undefined) persistenceTimer = setTimeout(save, 250);
}

function queue() {
  if (buffer) return buffer;
  let initial: DiagnosticEvent[] = [];
  try { const stored = JSON.parse(sessionStorage.getItem(CACHE_KEY) || "[]"); if (Array.isArray(stored)) initial = stored; } catch {}
  buffer = new DiagnosticEventBuffer(initial, persistQueue);
  return buffer;
}

async function boundedInvoke(command: string, args: Record<string, unknown>) {
  let timeout: ReturnType<typeof setTimeout> | undefined;
  try { return await Promise.race([nativeInvoke(command, args), new Promise((_, reject) => { timeout = setTimeout(() => reject(new Error("diagnostic IPC timeout")), 5_000); })]); }
  finally { clearTimeout(timeout); }
}

function flushSoon(delay = 100) {
  if (timer !== undefined || !desktop()) return;
  timer = setTimeout(() => {
    timer = undefined;
    void queue().flush((events) => boundedInvoke("report_diagnostic_events", { events })).then(() => {
      if (queue().size) flushSoon();
    }).catch(() => flushSoon(5_000));
  }, delay);
}

export function reportDiagnostic(event: string, data: Record<string, unknown> = {}, level: DiagnosticEvent["level"] = "info") {
  if (!desktop()) return;
  queue().add({ event, level, timestamp: new Date().toISOString(), data: { ...context, ...data } });
  flushSoon();
}

export function setDiagnosticContext(value: DiagnosticContext) {
  const next = { ...context, ...value };
  if (JSON.stringify(next) === JSON.stringify(context)) return;
  context = next;
  reportDiagnostic("context", value);
}

export function reportFrontendError(category: string, error: unknown, extra: Record<string, unknown> = {}) {
  reportDiagnostic(category, { ...extra, message: diagnosticError(error) }, "error");
}

async function heartbeat() {
  if (!desktop() || heartbeatBusy) return;
  heartbeatBusy = true;
  const memory = (performance as Performance & { memory?: { usedJSHeapSize: number } }).memory;
  const root = document.getElementById("root");
  const state = { ...context, visible: document.visibilityState === "visible", domNodes: document.querySelectorAll("*").length, heapBytes: memory?.usedJSHeapSize ?? null, blank: document.readyState === "complete" && Boolean(root && !root.firstElementChild) };
  try { await boundedInvoke("report_frontend_heartbeat", { state }); }
  catch {} finally { heartbeatBusy = false; }
}

export function initializeFrontendHealth() {
  if (initialized) return;
  initialized = true;
  window.addEventListener("error", (event) => {
    const resizeWarning = isResizeObserverWarning(event.error || event.message);
    if (resizeWarning) {
      // Chromium emits this after a layout callback writes another size. It is
      // recoverable and can otherwise obscure the native failure that matters.
      if (Date.now() - lastResizeObserverWarning > 30_000) {
        lastResizeObserverWarning = Date.now();
        reportDiagnostic("resize_observer_warning", { message: resizeWarning }, "warn");
      }
    } else if (event.error || event.message) reportFrontendError("uncaught", event.error || event.message);
    else if (event.target instanceof HTMLScriptElement || event.target instanceof HTMLLinkElement) {
      const src = event.target instanceof HTMLScriptElement ? event.target.src : event.target.href;
      try { reportFrontendError("resource-error", new URL(src, location.href).pathname); } catch {}
    }
  }, true);
  window.addEventListener("unhandledrejection", (event) => reportFrontendError("unhandled-rejection", event.reason));
  window.addEventListener("pagehide", () => {
    reportDiagnostic("pagehide");
    clearTimeout(persistenceTimer);
    persistenceTimer = undefined;
    try { sessionStorage.setItem(CACHE_KEY, JSON.stringify(pendingPersistence)); } catch {}
  });
  window.addEventListener("focus", () => { reportDiagnostic("focus"); void heartbeat(); });
  document.addEventListener("visibilitychange", () => { reportDiagnostic("visibility", { visible: document.visibilityState === "visible" }); void heartbeat(); });
  window.addEventListener("online", () => flushSoon());
  setInterval(() => { flushSoon(); void heartbeat(); }, 10_000);
  let longTaskTotal = 0;
  let longTaskCount = 0;
  let longTaskReported = 0;
  if (typeof PerformanceObserver !== "undefined" && PerformanceObserver.supportedEntryTypes.includes("longtask")) {
    const observer = new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) { longTaskTotal += entry.duration; longTaskCount++; }
      if (performance.now() - longTaskReported > 30_000 && longTaskTotal >= 1_000) {
        reportDiagnostic("long_tasks", { count: longTaskCount, totalDurationMs: Math.round(longTaskTotal) }, "warn");
        longTaskCount = 0; longTaskTotal = 0; longTaskReported = performance.now();
      }
    });
    observer.observe({ entryTypes: ["longtask"] });
  }
  reportDiagnostic("page_start", { theme: document.documentElement.dataset.theme, devicePixelRatio: window.devicePixelRatio });
  // Give React its initial mount before checking for an empty page.
  setTimeout(() => void heartbeat(), 3_000);
}
