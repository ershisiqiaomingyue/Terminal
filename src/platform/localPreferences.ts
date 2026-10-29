export function localPreferences<T>(key: string, fallback: T, parse: (value: unknown) => T) {
  const listeners = new Set<(value: T) => void>();
  const read = (): T => {
    try {
      if (typeof window !== "undefined") return parse(JSON.parse(window.localStorage.getItem(key) || "null"));
    } catch { /* Corrupt or unavailable storage must not prevent startup. */ }
    return fallback;
  };
  return {
    read,
    write(value: T) {
      const next = parse(value);
      window.localStorage.setItem(key, JSON.stringify(next));
      listeners.forEach((listener) => listener(next));
    },
    subscribe(listener: (value: T) => void) {
      listeners.add(listener);
      const onStorage = (event: StorageEvent) => {
        if (event.storageArea === window.localStorage && (event.key === key || event.key === null)) listener(read());
      };
      if (typeof window !== "undefined") window.addEventListener("storage", onStorage);
      return () => {
        listeners.delete(listener);
        if (typeof window !== "undefined") window.removeEventListener("storage", onStorage);
      };
    },
  };
}
