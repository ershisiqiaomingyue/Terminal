/** Optional browser caches must never take down a view when the quota is exhausted. */
export function cacheBrowserValue(key: string, value: string): boolean {
  try {
    window.localStorage.setItem(key, value);
    return true;
  } catch { return false; }
}
