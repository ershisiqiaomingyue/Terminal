export type DiagnosticEvent = { level: "info" | "warn" | "error"; event: string; timestamp: string; data: Record<string, unknown> };
export type DiagnosticContext = { view?: string; threadId?: string; turnId?: string; running?: boolean };

export function diagnosticError(error: unknown): string {
  let text: string;
  try { text = error instanceof Error ? `${error.name}: ${error.message}\n${error.stack || ""}` : typeof error === "string" ? error : JSON.stringify(error) || String(error); }
  catch { text = String(error); }
  return text.slice(0, 8_000)
    .replace(/(bearer\s+)[A-Za-z0-9._~+/=-]+/gi, "$1[REDACTED]")
    .replace(/((?:api[_-]?key|access[_-]?token|refresh[_-]?token|password|passwd|secret|authorization)\s*["']?\s*[:=]\s*["']?)[^\s,"'};]+/gi, "$1[REDACTED]")
    .replace(/\bsk-[A-Za-z0-9_-]{12,}\b/g, "[REDACTED]");
}

/** A bounded persistent queue: failures are retried, including after a renderer reload. */
export class DiagnosticEventBuffer {
  private pending: DiagnosticEvent[];
  private flushing = false;
  private persist: (events: DiagnosticEvent[]) => void;
  get size() { return this.pending.length; }
  constructor(initial: DiagnosticEvent[], persist: (events: DiagnosticEvent[]) => void) {
    this.pending = initial.filter(event => event && typeof event.event === "string" && typeof event.timestamp === "string" && typeof event.data === "object").slice(-100);
    this.persist = persist;
  }
  add(event: DiagnosticEvent) {
    this.pending.push(event);
    if (this.pending.length > 100) {
      this.pending.shift();
      const previous = this.pending.find(value => value.event === "buffer_overflow");
      if (previous) previous.data.dropped = Number(previous.data.dropped || 0) + 1;
      else this.pending.splice(0, 1, { level: "warn", event: "buffer_overflow", timestamp: event.timestamp, data: { dropped: 2 } });
    }
    this.persist([...this.pending]);
  }
  async flush(send: (events: DiagnosticEvent[]) => Promise<unknown>) {
    if (this.flushing || !this.pending.length) return;
    this.flushing = true;
    const batch = this.pending.slice(0, 50);
    try {
      await send(batch);
      const delivered = new Set(batch);
      this.pending = this.pending.filter(event => !delivered.has(event));
      this.persist([...this.pending]);
    } finally { this.flushing = false; }
  }
}
