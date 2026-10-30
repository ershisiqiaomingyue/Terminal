/**
 * 失败自动修复的提示词（设计文档 9.4 第 2 步 / T4-7）。
 *
 * 与 `prompts.ts` 的分工：那个负责"解释/诊断"（输出是自由文本），这个负责
 * "给一条能直接执行的修复命令"（输出必须是 JSON，且只一条命令）。
 *
 * 三条约束写进系统提示词，因为模型最容易在这里越界：
 * 1. **只给一条**命令——多条会让审批卡片变成"批一送 N"，违背 9.4 的逐条审批；
 * 2. **不自动执行**——本文件只产文本，执行一律走审批卡片；
 * 3. 能用只读手段确认的，先给只读命令；真要破坏性操作必须写明后果。
 *
 * 纯字符串组装、不 import 平台 API，可被 Node 单测直接加载。
 */
import type { CommandBlock } from "../blocks/types.ts";
import { snippetFromBlock } from "./prompts.ts";
import { utf8ByteLength } from "../blocks/ansiText.ts";

/** 修复命令的请求预算：比解释更紧，因为只产一条命令。 */
export const REPAIR_LIMITS = {
  maxBytes: 8 * 1024,
  maxLines: 80,
  headLines: 20,
} as const;

export interface RepairPromptInput {
  block: CommandBlock;
  /** 上一轮诊断文本（9.4 第 2 步的产物），作为修复命令的依据。 */
  diagnosis: string;
  /** 第几轮（1 起），写进提示词让模型知道前几轮已经失效。 */
  attempt: number;
  /** 已尝试过的命令 + 结果，避免模型重复同一条。 */
  previous?: { command: string; outcome: "failed" | "cancelled" }[];
  limits?: Partial<typeof REPAIR_LIMITS>;
}

export interface BuiltRepairPrompt {
  system: string;
  user: string;
  bytes: number;
  tightened: boolean;
}

const SYSTEM = [
  "你是本机（Windows）、WSL 与 SSH 远程终端里的命令修复助手。",
  "用户的一条命令执行失败了，你已给出诊断，现在要给出一条**可以直接执行**的修复命令。",
  "",
  "输出要求（必须严格遵守）：",
  "1. 只输出一个 JSON 对象：{\"commands\": [\"一条完整命令\"], \"explanation\": \"一句话说明这条命令做什么\"}。",
  "2. commands 数组里**只能有一条**命令；需要串联时用 `&&`，不要拆成多个元素。",
  "3. 不要输出 JSON 之外的任何文字、Markdown 围栏或注释。",
  "4. 优先给只读的确认命令（如 `ls -l`、`git status`）；确实要改状态时，在 explanation 里写明会改什么。",
  "5. 绝对不要给 `rm -rf /`、`mkfs` 这类不可恢复的命令，除非用户明确要求清空数据。",
  "6. 不要重复用户已经尝试过且失败的命令。",
].join("\n");

/** 出口码 → 一句话（与 prompts.ts 的 describeExitCode 同口径，但这里只要简短值）。 */
function exitSummary(block: CommandBlock): string {
  if (block.status === "interrupted") return "被用户中断";
  if (block.status === "partial") return "输出被裁剪，不完整";
  if (block.exitCode === undefined) return "未知（shell 未上报）";
  return String(block.exitCode);
}

export function buildRepairPrompt(input: RepairPromptInput): BuiltRepairPrompt {
  const limits = { ...REPAIR_LIMITS, ...input.limits };
  const snippet = snippetFromBlock(input.block, {
    limits: { maxBytes: limits.maxBytes, maxLines: limits.maxLines, headLines: limits.headLines },
  });
  const parts: string[] = [];

  parts.push("## 失败的命令");
  parts.push("```");
  parts.push(input.block.command || "(空)");
  parts.push("```");
  parts.push(`出口码：${exitSummary(input.block)}`);
  parts.push(`执行环境：${input.block.targetKind}${input.block.cwd ? `，工作目录 ${input.block.cwd}` : ""}`);

  parts.push("");
  parts.push("## 输出（尾部优先）");
  parts.push("```");
  parts.push(snippet.text || "(没有输出)");
  parts.push("```");
  if (snippet.truncated) {
    parts.push(`（输出已裁剪，原文约 ${Math.round(snippet.originalBytes / 1024)} KB）`);
  }

  if (input.diagnosis.trim()) {
    parts.push("");
    parts.push("## 你的诊断");
    parts.push(input.diagnosis.trim());
  }

  if (input.previous?.length) {
    parts.push("");
    parts.push("## 已尝试过的修复（都失败或未执行）");
    for (const item of input.previous) {
      parts.push(`- \`${item.command}\` → ${item.outcome === "failed" ? "执行失败" : "用户未执行"}`);
    }
  }

  parts.push("");
  parts.push(`这是第 ${Math.max(1, input.attempt)} 轮修复。请给出下一步唯一一条修复命令（JSON）。`);

  const user = parts.join("\n");
  const bytes = utf8ByteLength(SYSTEM) + utf8ByteLength(user);
  return { system: SYSTEM, user, bytes, tightened: snippet.truncated };
}
