export class BoundedTextCache<T> {
  private readonly entries = new Map<string, { value: T; size: number }>();
  private size = 0;
  private readonly maxEntries: number;
  private readonly maxCharacters: number;

  constructor(maxEntries: number, maxCharacters: number) {
    this.maxEntries = maxEntries;
    this.maxCharacters = maxCharacters;
  }

  get(key: string): T | undefined {
    const entry = this.entries.get(key);
    if (!entry) return undefined;
    this.entries.delete(key);
    this.entries.set(key, entry);
    return entry.value;
  }

  set(key: string, value: T, valueCharacters: number) {
    const previous = this.entries.get(key);
    if (previous) { this.size -= previous.size; this.entries.delete(key); }
    const size = key.length + valueCharacters;
    if (size > this.maxCharacters) return;
    this.entries.set(key, { value, size });
    this.size += size;
    while (this.entries.size > this.maxEntries || this.size > this.maxCharacters) {
      const oldest = this.entries.entries().next().value!;
      this.size -= oldest[1].size;
      this.entries.delete(oldest[0]);
    }
  }
}
