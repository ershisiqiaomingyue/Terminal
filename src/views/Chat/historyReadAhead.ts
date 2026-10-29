import type { HistoryRecord } from "../../platform/chatApi";

/** Small, short-lived read-ahead buffer; ordinary history reads stay authoritative. */
export class HistoryReadAhead<T> {
  private entries = new Map<string, { request: Promise<T | null>; value?: T; expires: number }>();
  private read: (id: string) => Promise<T | null>;
  private now: () => number;
  private ttl: number;
  private limit: number;

  constructor(read: (id: string) => Promise<T | null>, now = Date.now, ttl = 5_000, limit = 3) {
    this.read = read;
    this.now = now;
    this.ttl = ttl;
    this.limit = limit;
  }

  private entry(id: string) {
    const entry = this.entries.get(id);
    if (entry && entry.expires > this.now()) return entry;
    return undefined;
  }

  // Expired content can paint immediately, but take() never treats it as fresh.
  peek(id: string) { return this.entries.get(id)?.value; }
  ready() { return [...this.entries.keys()].flatMap(id => { const value = this.peek(id); return value === undefined ? [] : [value]; }); }

  prefetch(id: string): Promise<T | null> {
    const previous = this.entry(id);
    if (previous) return previous.request;
    const entry = { expires: this.now() + this.ttl, value: this.peek(id) } as { request: Promise<T | null>; value?: T; expires: number };
    entry.request = Promise.resolve().then(() => this.read(id)).then(value => {
      if (this.entries.get(id) === entry) {
        if (value !== null) { entry.value = value; entry.expires = this.now() + this.ttl; }
        else this.entries.delete(id);
      }
      return value;
    }, error => {
      if (this.entries.get(id) === entry) this.entries.delete(id);
      throw error;
    });
    this.entries.delete(id);
    while (this.entries.size >= this.limit) this.entries.delete(this.entries.keys().next().value!);
    this.entries.set(id, entry);
    return entry.request;
  }

  take(id: string): Promise<T | null> | undefined {
    const entry = this.entry(id);
    this.entries.delete(id);
    return entry?.request;
  }

  invalidate(id: string) { this.entries.delete(id); }
}

// Configure the reader separately to keep the navigation/restore modules lightweight.
let readHistory: (id: string) => Promise<HistoryRecord | null> = () => Promise.resolve(null);
export const conversationHistoryReadAhead = new HistoryReadAhead<HistoryRecord>(id => readHistory(id));
export function configureHistoryReadAhead(read: typeof readHistory) { readHistory = read; }
