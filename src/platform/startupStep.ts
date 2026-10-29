/** A startup dependency must resolve before mounting views that write preferences. */
import { formatErrorText } from "./errorText.ts";

export function startupStep<T>(label: string, action: () => Promise<T>, timeoutMs = 10_000): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`${label}超时，请重新加载后重试。`)), timeoutMs);
    Promise.resolve().then(action).then(resolve, (error: unknown) => {
      reject(new Error(`${label}失败：${formatErrorText(error)}`));
    }).finally(() => clearTimeout(timer));
  });
}
