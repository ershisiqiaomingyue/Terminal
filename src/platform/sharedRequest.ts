/** Share overlapping reads only. Completed values are not cached across mutations. */
export function shareInFlight<T, K>(read: (key: K) => Promise<T>) {
  const pending = new Map<K, Promise<T>>();
  return (key: K): Promise<T> => {
    const existing = pending.get(key);
    if (existing) return existing;
    const request = Promise.resolve().then(() => read(key)).finally(() => pending.delete(key));
    pending.set(key, request);
    return request;
  };
}
