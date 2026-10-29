export function pathInMutation(path: string, base: string, windows = false) {
  const normalize = (value: string) => {
    const path = value.replace(/\\/g, "/").replace(/\/+$/, "");
    return windows ? path.toLowerCase() : path;
  };
  const candidate = normalize(path); const root = normalize(base);
  return candidate === root || candidate.startsWith(`${root}/`);
}

export function mutatedPath(path: string, base: string, destination?: string | null, windows = false) {
  if (!pathInMutation(path, base, windows)) return path;
  return destination ? destination + path.slice(base.length) : "";
}
