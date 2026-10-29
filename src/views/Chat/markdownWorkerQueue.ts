import type { LongMarkdownRequest, LongMarkdownResult } from "./longMarkdownProjection.ts";

type WorkerLike = Pick<Worker, "postMessage" | "terminate" | "onmessage" | "onerror">;
type Client = { result: (value: LongMarkdownResult) => void; fail: () => void };

/** One parser for all mounted messages; each client retains only its newest request. */
export class MarkdownWorkerQueue {
  private worker: WorkerLike | null = null;
  private clients = new Map<symbol, Client>();
  private pending = new Map<symbol, LongMarkdownRequest>();
  private active: { id: symbol; request: LongMarkdownRequest } | null = null;
  private timeout: ReturnType<typeof setTimeout> | undefined;
  private idle: ReturnType<typeof setTimeout> | undefined;

  private createWorker: () => WorkerLike;
  constructor(createWorker: () => WorkerLike) { this.createWorker = createWorker; }

  subscribe(result: Client["result"], fail: Client["fail"]) {
    const id = Symbol();
    this.clients.set(id, { result, fail });
    return {
      submit: (request: LongMarkdownRequest) => {
        if (!this.clients.has(id)) return;
        const active = this.active;
        if (active?.id === id && active.request.source === request.source && active.request.streaming === request.streaming) return;
        this.pending.set(id, request);
        this.pump();
      },
      dispose: () => {
        this.clients.delete(id);
        this.pending.delete(id);
        if (!this.clients.size) this.stop();
      },
    };
  }

  private stop() {
    clearTimeout(this.timeout);
    clearTimeout(this.idle);
    this.worker?.terminate();
    this.worker = null;
    this.active = null;
  }

  private finish(value?: LongMarkdownResult) {
    const job = this.active;
    if (!job) return;
    clearTimeout(this.timeout);
    this.active = null;
    const client = this.clients.get(job.id);
    if (value) client?.result(value);
    else {
      this.stop();
      this.pending.delete(job.id);
      client?.fail();
    }
    this.pump();
  }

  private pump() {
    if (this.active) return;
    clearTimeout(this.idle);
    const next = this.pending.entries().next().value;
    if (!next) {
      // Keep the parser between adjacent streaming deltas, then release it.
      if (this.worker) this.idle = setTimeout(() => this.stop(), 5_000);
      return;
    }
    const [id, request] = next;
    this.pending.delete(id);
    this.active = { id, request };
    try {
      if (!this.worker) {
        const worker = this.createWorker();
        this.worker = worker;
        worker.onmessage = (event) => {
          if (this.worker !== worker) return;
          const value = event.data as LongMarkdownResult & { error?: boolean };
          this.finish(value.error ? undefined : value);
        };
        worker.onerror = () => { if (this.worker === worker) this.finish(); };
      }
      // A pathological Markdown document must not block every later message.
      this.timeout = setTimeout(() => this.finish(), 15_000);
      this.worker.postMessage(request);
    } catch { this.finish(); }
  }
}
