import { useEffect, useRef, useState } from "react";

type Capture = () => unknown;
let initial: Record<string, unknown> = {};
const captures = new Map<string, Capture>();

export function restoreToolWindowState(state: Record<string, unknown>) { initial = state; }
export function readToolWindowState<T>(key: string): T | undefined { return initial[key] as T | undefined; }
export function captureToolWindowState(toolKey: string): Record<string, unknown> {
  return Object.fromEntries([...captures].filter(([key]) => key === toolKey || key.startsWith(`${toolKey}-`)).map(([key, capture]) => [key, capture()]));
}
export function useToolWindowState(key: string, capture: Capture, enabled = true) {
  const ref = useRef(capture);
  ref.current = capture;
  useEffect(() => {
    if (!enabled) return;
    const get = () => ref.current();
    captures.set(key, get);
    return () => { if (captures.get(key) === get) captures.delete(key); };
  }, [key, enabled]);
}

export function useToolWindowValue<T>(tool: string, field: string, fallback: T | (() => T)) {
  const key = `${tool}-${field}`;
  const [value, setValue] = useState<T>(() => {
    const saved = readToolWindowState<T>(key);
    return saved === undefined ? typeof fallback === "function" ? (fallback as () => T)() : fallback : saved;
  });
  useToolWindowState(key, () => value);
  return [value, setValue] as const;
}

export function useToolDetachGuard(tool: string, busy: boolean) {
  useToolWindowState(`${tool}-guard`, () => {
    if (busy) throw new Error("当前操作正在执行，请完成后再拖出。");
    return null;
  });
}
