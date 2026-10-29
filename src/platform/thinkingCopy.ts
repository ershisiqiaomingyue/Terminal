import { localPreferences } from "./localPreferences.ts";

export const DEFAULT_THINKING_COPY = {
  reasoning: ["思考中"],
  writing: ["回复中"],
  waiting: ["等待响应"],
  toolFinished: ["工具已完成"],
} as const;

export const THINKING_COPY_PRESETS = {
  default: DEFAULT_THINKING_COPY,
  elegant: {
  reasoning: ["凝神细思，寻其要义…", "心游万仞，细理千端…", "循迹求真，参详其间…", "抽丝寻绪，渐见端倪…", "沉吟片刻，推敲未定…"],
  writing: ["思绪渐明，提笔成章…", "正将所得，徐徐写来…", "斟酌字句，铺陈始末…", "落笔有序，细述其详…"],
  waiting: ["静候回音，思绪待续…", "片刻留白，待续下文…", "候其音信，再续此篇…", "且待回音，续理前绪…"],
  toolFinished: ["回音已至，正细辨其中…", "所得已呈，正参详始末…", "循新得之线索，再理其绪…", "前事已毕，正推求其意…"],
  },
  relaxed: {
    reasoning: ["让我捋一捋…", "先把线索排个队…", "这个问题值得认真想想…", "我来找找关键点…", "稍等，我组织一下思路…"],
    writing: ["答案正在成形…", "我把重点整理一下…", "马上把结论写清楚…"],
    waiting: ["正在等模型响应…", "稍等片刻，马上继续…", "先喝口电子咖啡，等个回应…"],
    toolFinished: ["工具跑完了，接着处理…", "线索到手，继续往下看…", "这一步办妥了…"],
  },
  technical: {
    reasoning: ["正在核对问题与约束…", "正在梳理相关信息…", "正在检查可行方案…"],
    writing: ["正在整理结果…", "正在组织答复…", "正在汇总关键结论…"],
    waiting: ["正在等待模型响应…", "请求已发送，等待返回…"],
    toolFinished: ["工具执行完成，继续处理…", "已收到工具结果，正在分析…"],
  },
} as const;

export type ThinkingCopyPreset = keyof typeof THINKING_COPY_PRESETS | "custom";
export const THINKING_COPY_PRESET_OPTIONS = [
  { value: "default", label: "简洁（默认）" },
  { value: "elegant", label: "雅致" },
  { value: "relaxed", label: "轻松" },
  { value: "technical", label: "技术向" },
] as const;

export type ThinkingCopyPhase = keyof typeof DEFAULT_THINKING_COPY;
export type ThinkingCopy = Readonly<Record<ThinkingCopyPhase, readonly string[]>>;
export const THINKING_COPY_PHASES = Object.keys(DEFAULT_THINKING_COPY) as ThinkingCopyPhase[];
export const THINKING_COPY_LABELS: Record<ThinkingCopyPhase, string> = {
  reasoning: "思考中", writing: "回复中", waiting: "等待响应", toolFinished: "工具执行后",
};

export function thinkingCopyForPreset(preset: Exclude<ThinkingCopyPreset, "custom">): ThinkingCopy {
  return normalizeThinkingCopy(THINKING_COPY_PRESETS[preset]);
}

export function thinkingCopyPresetFor(value: unknown): ThinkingCopyPreset {
  const copy = normalizeThinkingCopy(value);
  for (const [preset, candidate] of Object.entries(THINKING_COPY_PRESETS)) {
    if (THINKING_COPY_PHASES.every((phase) => JSON.stringify(copy[phase]) === JSON.stringify(candidate[phase]))) {
      return preset as keyof typeof THINKING_COPY_PRESETS;
    }
  }
  return "custom";
}

export function normalizeThinkingCopy(value: unknown): ThinkingCopy {
  const source = value && typeof value === "object" ? value as Record<string, unknown> : {};
  const result = {} as Record<ThinkingCopyPhase, readonly string[]>;
  for (const phase of THINKING_COPY_PHASES) {
    const phrases = Array.isArray(source[phase]) ? source[phase] as unknown[] : [];
    const valid = [...new Set(phrases.filter((phrase): phrase is string => typeof phrase === "string")
      .map((phrase) => phrase.trim()).filter((phrase) => phrase.length > 0 && phrase.length <= 160 && !/[\r\n]/.test(phrase)))].slice(0, 64);
    result[phase] = valid.length ? valid : DEFAULT_THINKING_COPY[phase];
  }
  return result;
}

export function thinkingCopyFromDraft(draft: Record<ThinkingCopyPhase, string>): ThinkingCopy {
  const result = {} as Record<ThinkingCopyPhase, string[]>;
  for (const phase of THINKING_COPY_PHASES) {
    const phrases = draft[phase].split(/\r?\n/).map((phrase) => phrase.trim()).filter(Boolean);
    if (phrases.length > 64 || phrases.some((phrase) => phrase.length > 160)) {
      throw new Error(`${THINKING_COPY_LABELS[phase]}：最多 64 条，每条不超过 160 字。`);
    }
    result[phase] = phrases;
  }
  return normalizeThinkingCopy(result);
}

const preferences = localPreferences<ThinkingCopy>("android-tool.agent.thinking-copy.v1", DEFAULT_THINKING_COPY, normalizeThinkingCopy);
export const readThinkingCopy = preferences.read;
export const setThinkingCopy = preferences.write;
export const subscribeThinkingCopy = preferences.subscribe;
