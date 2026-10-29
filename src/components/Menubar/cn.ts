import { clsx, type ClassValue } from "clsx";

/** 本项目不用 Tailwind，这里只做类名拼接（对应 rain-ui 的 cn，省掉 tailwind-merge）。 */
export function cn(...inputs: ClassValue[]) {
  return clsx(inputs);
}
