import { localPreferences } from "./localPreferences.ts";

export const TOOL_PARAMETER_RULES = {
  simpleperfDuration: { label: "Simpleperf 默认采样时长", min: 1, max: 120, unit: "秒", initial: 10 },
  methodDuration: { label: "Java 方法默认采样时长", min: 1, max: 120, unit: "秒", initial: 5 },
  methodInterval: { label: "Java 方法默认采样间隔", min: 100, max: 100_000, unit: "微秒", initial: 1000 },
} as const;
export type ToolParameter = keyof typeof TOOL_PARAMETER_RULES;
export type ToolPreferences = Record<ToolParameter, number>;
export const TOOL_PARAMETERS = Object.keys(TOOL_PARAMETER_RULES) as ToolParameter[];
export const DEFAULT_TOOL_PREFERENCES: ToolPreferences = { simpleperfDuration: 10, methodDuration: 5, methodInterval: 1000 };

export function normalizeToolPreferences(value: unknown): ToolPreferences {
  const source = value && typeof value === "object" ? value as Record<string, unknown> : {};
  const result = { ...DEFAULT_TOOL_PREFERENCES };
  for (const key of TOOL_PARAMETERS) {
    const number = source[key];
    const rule = TOOL_PARAMETER_RULES[key];
    if (typeof number === "number" && Number.isInteger(number) && number >= rule.min && number <= rule.max) result[key] = number;
  }
  return result;
}

export function toolPreferencesFromDraft(draft: Record<ToolParameter, string>): ToolPreferences {
  const result = { ...DEFAULT_TOOL_PREFERENCES };
  for (const key of TOOL_PARAMETERS) {
    const number = Number(draft[key]);
    const rule = TOOL_PARAMETER_RULES[key];
    if (!draft[key].trim() || !Number.isInteger(number) || number < rule.min || number > rule.max) {
      throw new Error(`${rule.label}须为 ${rule.min}–${rule.max} ${rule.unit}的整数。`);
    }
    result[key] = number;
  }
  return result;
}

const preferences = localPreferences("android-tool.tools.preferences.v1", DEFAULT_TOOL_PREFERENCES, normalizeToolPreferences);
export const readToolPreferences = preferences.read;
export const setToolPreferences = preferences.write;
export const subscribeToolPreferences = preferences.subscribe;
