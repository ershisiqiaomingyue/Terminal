import type { ExecutionTarget } from "./types.ts";

/**
 * 只接受可寻址的远程环境（SSH / WSL）记录；本机工作区属于项目本身，不入绑定清单。
 * 历史记录可能来自旧版本或被手工编辑，这里做一次防御性过滤。
 */
export function readBoundEnvironments(value: unknown): ExecutionTarget[] {
  if (!Array.isArray(value)) return [];
  return value
    .filter((item): item is ExecutionTarget => {
      if (!item || typeof item !== "object") return false;
      const target = item as ExecutionTarget;
      if (target.kind !== "ssh" && target.kind !== "wsl") return false;
      if (typeof target.id !== "string" || !target.id.trim()) return false;
      return typeof target.label === "string";
    })
    .map((item) => ({ ...item }));
}
