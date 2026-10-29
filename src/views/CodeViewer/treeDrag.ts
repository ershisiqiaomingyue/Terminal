// 工程树的移动判定：只允许移动到非自身、非当前父级、且不在自身子树内的目录。
// 拖拽本身由 useTreeRowDrag 的 pointer 事件实现（Windows 上 Tauri 接管原生拖放，
// 页面内 HTML5 拖放事件收不到，因此不再使用 dataTransfer）。
const normalizedPath = (path: string) => path.replace(/\\/g, "/").replace(/\/+$/, "");
const pathWithin = (path: string, base: string) => {
  const candidate = normalizedPath(path); const root = normalizedPath(base);
  return candidate === root || candidate.startsWith(`${root}/`);
};
export function canMoveEntry(entry: { path: string; isDirectory: boolean; symlinkTarget?: string | null }, destinationDirectory: string) {
  const source = normalizedPath(entry.path); const destination = normalizedPath(destinationDirectory);
  if (!destination || destination === source) return false;
  const parent = source.slice(0, source.lastIndexOf("/")) || "/";
  if (parent === destination) return false;
  return !entry.isDirectory || entry.symlinkTarget != null || !pathWithin(destination, source);
}
