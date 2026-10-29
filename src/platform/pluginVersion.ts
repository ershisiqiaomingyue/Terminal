/** SemVer precedence: build metadata is ignored and stable beats prerelease. */
export function hasPluginUpdate(installed: string | null, available?: string | null): boolean {
  const parse = (version?: string | null) => /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/.exec(version || '');
  const current = parse(installed), next = parse(available);
  if (!current || !next) return false;
  for (let i = 1; i <= 3; i++) {
    const a = BigInt(current[i]), b = BigInt(next[i]);
    if (a !== b) return b > a;
  }
  if (!current[4] || !next[4]) return Boolean(current[4] && !next[4]);
  const a = current[4].split('.'), b = next[4].split('.');
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    if (a[i] === b[i]) continue;
    if (a[i] === undefined || b[i] === undefined) return a[i] === undefined;
    const aNumber = /^\d+$/.test(a[i]), bNumber = /^\d+$/.test(b[i]);
    if (aNumber && bNumber) return BigInt(b[i]) > BigInt(a[i]);
    if (aNumber !== bNumber) return aNumber;
    return b[i] > a[i];
  }
  return false;
}