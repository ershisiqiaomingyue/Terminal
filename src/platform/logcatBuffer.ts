const STORAGE_KEY = "android-tool.logcat.buffer-bytes";

/** Android Studio 的屏幕缓冲默认约 1 MB。2 MB 大约能留住一两万行短日志，又不会把界面撑满。 */
export const DEFAULT_LOGCAT_BUFFER_BYTES = 2 * 1024 * 1024;

export const LOGCAT_BUFFER_OPTIONS = [
  { bytes: 1 * 1024 * 1024, label: "1 MB" },
  { bytes: DEFAULT_LOGCAT_BUFFER_BYTES, label: "2 MB" },
  { bytes: 4 * 1024 * 1024, label: "4 MB" },
  { bytes: 8 * 1024 * 1024, label: "8 MB" },
] as const;

const listeners = new Set<(bytes: number) => void>();

export function readLogcatBufferBytes(): number {
  if (typeof window === "undefined") return DEFAULT_LOGCAT_BUFFER_BYTES;
  try {
    const parsed = Number(window.localStorage.getItem(STORAGE_KEY));
    if (LOGCAT_BUFFER_OPTIONS.some((option) => option.bytes === parsed)) return parsed;
  } catch {
    // Keep the default when storage is unavailable.
  }
  return DEFAULT_LOGCAT_BUFFER_BYTES;
}

export function setLogcatBufferBytes(bytes: number) {
  const next = LOGCAT_BUFFER_OPTIONS.some((option) => option.bytes === bytes)
    ? bytes
    : DEFAULT_LOGCAT_BUFFER_BYTES;
  try {
    window.localStorage.setItem(STORAGE_KEY, String(next));
  } catch {
    // The current page still uses the selected size.
  }
  listeners.forEach((listener) => listener(next));
}

export function subscribeLogcatBuffer(listener: (bytes: number) => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export interface LogcatBuffer<T extends { raw: string }> {
  lines: T[];
  bytes: number;
  dropped: number;
  unseen: number;
  filtered: number[];
  offsets: number[];
}

export function createLogcatBuffer<T extends { raw: string }>(): LogcatBuffer<T> {
  return { lines: [], bytes: 0, dropped: 0, unseen: 0, filtered: [], offsets: [0] };
}

export function resetLogcatBuffer<T extends { raw: string }>(buffer: LogcatBuffer<T>) {
  buffer.lines = [];
  buffer.bytes = 0;
  buffer.dropped = 0;
  buffer.unseen = 0;
  buffer.filtered = [];
  buffer.offsets = [0];
}

function trimLogcatBuffer<T extends { raw: string }>(buffer: LogcatBuffer<T>, budget: number) {
  let removed = 0;
  while (removed < buffer.lines.length - 1 && buffer.bytes > budget) {
    buffer.bytes -= buffer.lines[removed].raw.length + 1;
    removed += 1;
  }
  if (buffer.bytes < 0) buffer.bytes = 0;
  if (!removed) return;
  buffer.lines.splice(0, removed);
  buffer.dropped += removed;
  const next: number[] = [];
  for (const index of buffer.filtered) {
    if (index >= removed) next.push(index - removed);
  }
  buffer.filtered = next;
}

export function replaceLogcatLines<T extends { raw: string }>(
  buffer: LogcatBuffer<T>,
  lines: T[],
  matches: (line: T) => boolean,
  budget: number,
) {
  resetLogcatBuffer(buffer);
  appendLogcatLines(buffer, lines, matches, budget, false);
}

const MAX_LOGCAT_LINES = 50_000;

export function appendLogcatLines<T extends { raw: string }>(
  buffer: LogcatBuffer<T>,
  lines: T[],
  matches: (line: T) => boolean,
  budget: number,
  countUnseen: boolean,
) {
  let visible = 0;
  for (const line of lines) {
    buffer.lines.push(line);
    buffer.bytes += line.raw.length + 1;
    if (matches(line)) {
      buffer.filtered.push(buffer.lines.length - 1);
      visible += 1;
    }
  }

  // Hard limit on line count to prevent memory issues
  if (buffer.lines.length > MAX_LOGCAT_LINES) {
    const excess = buffer.lines.length - MAX_LOGCAT_LINES;
    buffer.lines.splice(0, excess);
    buffer.dropped += excess;
    // Recalculate bytes
    buffer.bytes = buffer.lines.reduce((sum, line) => sum + line.raw.length + 1, 0);
    // Adjust filtered indices
    const next: number[] = [];
    for (const index of buffer.filtered) {
      if (index >= excess) next.push(index - excess);
    }
    buffer.filtered = next;
  }

  trimLogcatBuffer(buffer, budget);
  if (countUnseen) buffer.unseen += visible;
  return visible;
}

export function rebuildLogcatFilter<T extends { raw: string }>(buffer: LogcatBuffer<T>, matches: (line: T) => boolean) {
  const filtered: number[] = [];
  for (let index = 0; index < buffer.lines.length; index += 1) {
    if (matches(buffer.lines[index])) filtered.push(index);
  }
  buffer.filtered = filtered;
}

const layouts = new WeakMap<object, { key: string; filtered: number[]; offsets: number[] }>();

export function layoutLogcatBuffer<T extends { raw: string }>(buffer: LogcatBuffer<T>, rowHeight: (line: T) => number, layoutKey?: string) {
  const previous = layouts.get(buffer);
  // Appending leaves existing rows unchanged. Filtering, trimming and resizing invalidate their heights.
  const reuse = layoutKey !== undefined && previous?.key === layoutKey
    && previous.filtered === buffer.filtered && previous.offsets === buffer.offsets;
  const offsets = reuse ? buffer.offsets : [0];
  for (let index = offsets.length - 1; index < buffer.filtered.length; index += 1) {
    offsets[index + 1] = offsets[index] + rowHeight(buffer.lines[buffer.filtered[index]]);
  }
  buffer.offsets = offsets;
  if (layoutKey !== undefined) layouts.set(buffer, { key: layoutKey, filtered: buffer.filtered, offsets });
  else layouts.delete(buffer);
}

export function findLogcatRow(offsets: number[], top: number) {
  let low = 0;
  let high = Math.max(0, offsets.length - 2);
  while (low < high) {
    const mid = (low + high) >> 1;
    if (offsets[mid + 1] <= top) low = mid + 1;
    else high = mid;
  }
  return low;
}
