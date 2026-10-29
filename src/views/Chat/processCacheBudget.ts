/** Conservative retained-size estimate without allocating another JSON copy.
 * Shared raw/projected event objects count once; cycles are safe. */
export function estimateRetainedBytes(value: unknown): number {
  const seen = new WeakSet<object>();
  const pending: unknown[] = [value];
  let bytes = 0;
  while (pending.length) {
    const item = pending.pop();
    if (typeof item === "string") bytes += item.length * 2;
    else if (item && typeof item === "object" && !seen.has(item)) {
      seen.add(item);
      bytes += 64;
      if (Array.isArray(item)) {
        bytes += item.length * 8;
        for (const child of item) pending.push(child);
      } else {
        for (const [key, child] of Object.entries(item)) { bytes += key.length * 2 + 16; pending.push(child); }
      }
    } else bytes += 8;
  }
  return bytes;
}

export function pruneIdleProcessEntries<T extends { bytes: number; touchedAt: number; inFlight: unknown; listeners: Set<unknown> }>(
  entries: Map<string, T>, maxEntries = 64, maxBytes = 16 * 1024 * 1024,
) {
  let totalBytes = 0;
  const idle: Array<[string, T]> = [];
  for (const pair of entries) {
    totalBytes += pair[1].bytes;
    if (!pair[1].inFlight && pair[1].listeners.size === 0) idle.push(pair);
  }
  idle.sort((a, b) => a[1].touchedAt - b[1].touchedAt);
  for (const [key, value] of idle) {
    if (entries.size <= maxEntries && totalBytes <= maxBytes) break;
    entries.delete(key);
    totalBytes -= value.bytes;
  }
}
