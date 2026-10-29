/**
 * 项目知识文档的 AI 解析。
 *
 * 设置 →「项目与自动化」→「知识文档」里，文档导入后是「待分析」。
 * 点行上的「分析」按钮走这里：
 *  1. 先看有没有配置模型供应商（`listAgentProviders`），没有就直接返回 no_model，
 *     让 UI 提示"未配置 AI"，而不是发一次注定失败的请求；
 *  2. 读取文档内容——pdf / docx 这类二进制文档读不了（代码查看器只支持 UTF-8 文本、
 *     且单个文件 ≤ 1 MB），这时退回"只登记路径"的结果并如实说明；
 *  3. 目录会递归展开，收集其中的文本文件作为输入；
 *  4. 用 `agent_one_shot` 让模型输出结构化摘要，交给调用方写回项目库。
 *
 * 复用 `terminal/ai/terminalAiService` 的 `resolveModelChoice` / `buildBlockAiRequest`：
 *  `toolPolicy: "disabled"` 保证解析过程不会触发工具调用，也不会写入任何东西。
 */
import { listAgentProviders } from "@/platform/api";
import { isTauri, invoke } from "@/platform/runtime";
import { listLocalDirectory } from "@/platform/terminalWorkspace";
import {
  buildBlockAiRequest,
  resolveModelChoice,
  type TerminalProviderLike,
} from "@/platform/terminal/ai/terminalAiService";

/** 单份文档最多喂给模型的字符数：控制 token，也避免超长文档把请求撑爆。 */
const MAX_DOCUMENT_CHARS = 120_000;
/** 目录解析时最多收集的文件数。 */
const MAX_DIRECTORY_FILES = 12;
/** 单个文件读入上限（后端代码查看器自身限制 1 MB，这里再收一档）。 */
const MAX_FILE_BYTES = 512 * 1024;

/** 二进制文档：读不出文本，只能提示用户转成 md/txt。 */
const BINARY_DOCUMENT_EXTENSIONS = new Set(["pdf", "docx", "doc", "xlsx", "xls", "pptx", "ppt"]);

/** 目录解析时只挑这些可读文本，避免把图片、二进制包喂进去。 */
const READABLE_TEXT_EXTENSIONS = new Set([
  "md", "markdown", "txt", "text", "json", "yaml", "yml", "toml", "ini", "csv",
  "xml", "html", "htm", "log", "kts", "gradle", "properties", "cfg", "conf",
  "sh", "bat", "ps1", "java", "kt", "c", "h", "cpp", "py", "rs", "ts", "tsx", "js", "jsx",
]);

export interface KnowledgeAnalysisSuccess {
  ok: true;
  /** 模型给出的结构化摘要（Markdown）。 */
  summary: string;
  /** 实际使用的模型，`providerId:modelId`。 */
  modelId: string;
  /** 参与解析的文件数（单文件为 1）。 */
  documents: number;
  /** 内容超长被截断时为 true。 */
  truncated: boolean;
}

export interface KnowledgeAnalysisFailure {
  ok: false;
  /** no_model = 未配置供应商；unreadable = 文档读不出文本；invoke_failed = 请求失败。 */
  code: "no_model" | "unreadable" | "invoke_failed";
  message: string;
}

export type KnowledgeAnalysisResult = KnowledgeAnalysisSuccess | KnowledgeAnalysisFailure;

const fileNameOf = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() || path;

const extensionOf = (path: string) => {
  const name = fileNameOf(path);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
};

/** 把后端错误转成一句用户能懂的话；与终端 AI 的措辞保持一致。 */
function describeInvokeError(error: unknown): string {
  const raw = error instanceof Error ? error.message : typeof error === "string" ? error : "";
  const detail = raw.trim();
  if (!detail) return "AI 请求失败，请稍后重试。";
  if (/timeout|timed out/i.test(detail)) return `AI 请求超时，请重试。（${detail}）`;
  if (/401|403|unauthor|forbidden|api key/i.test(detail)) return `模型鉴权失败，请检查供应商配置。（${detail}）`;
  if (/429|rate limit|quota/i.test(detail)) return `模型限流或额度不足，请稍后重试。（${detail}）`;
  return `AI 请求失败：${detail}`;
}

function newRequestId(prefix: string): string {
  const random = Math.random().toString(36).slice(2, 8);
  return `${prefix}-${Date.now()}-${random}`;
}

interface CollectedDocument {
  name: string;
  text: string;
  /** 读不出内容的原因，用于在输出里如实标注。 */
  note?: string;
}

/** 读一个文件；失败时返回带说明的空文档，而不是让整次解析失败。 */
async function readFileDocument(path: string): Promise<CollectedDocument> {
  const name = fileNameOf(path);
  const extension = extensionOf(path);
  if (BINARY_DOCUMENT_EXTENSIONS.has(extension)) {
    return {
      name,
      text: "",
      note: `${extension.toUpperCase()} 是二进制文档，无法直接读取内容；如需解析请先转成 Markdown 或纯文本。`,
    };
  }
  try {
    const text = await invoke<string>("read_code_viewer_file", { path });
    const trimmed = text ?? "";
    if (trimmed.length > MAX_FILE_BYTES) {
      return { name, text: trimmed.slice(0, MAX_FILE_BYTES), note: "文件较大，只读取了前一部分。" };
    }
    return { name, text: trimmed };
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error ?? "");
    return { name, text: "", note: detail.trim() || "读取失败。" };
  }
}

/** 递归收集目录下的可读文本文件（限层数与数量，避免卡住或喂太多）。 */
async function collectDirectory(path: string, depth = 0): Promise<CollectedDocument[]> {
  if (depth > 2) return [];
  let entries;
  try {
    entries = await listLocalDirectory(path);
  } catch {
    return [];
  }
  const documents: CollectedDocument[] = [];
  for (const entry of entries) {
    if (documents.length >= MAX_DIRECTORY_FILES) break;
    if (entry.isDirectory) {
      documents.push(...await collectDirectory(entry.path, depth + 1));
      continue;
    }
    const extension = extensionOf(entry.path);
    if (!READABLE_TEXT_EXTENSIONS.has(extension)) continue;
    if (typeof entry.size === "number" && entry.size > MAX_FILE_BYTES) continue;
    documents.push(await readFileDocument(entry.path));
  }
  return documents;
}

/**
 * 收集知识文档的内容。
 * 判断是文件还是目录：目录读得到条目、文件读不到（用扩展名先挡一层）。
 */
async function collectDocuments(path: string): Promise<CollectedDocument[]> {
  if (!extensionOf(path)) {
    const fromDirectory = await collectDirectory(path);
    if (fromDirectory.length) return fromDirectory;
  }
  return [await readFileDocument(path)];
}

function buildSummaryPrompt(documents: CollectedDocument[], truncated: boolean): { system: string; user: string } {
  const system = [
    "你是 Android 项目知识库的整理助手。",
    "请阅读用户提供的项目文档，提炼成可供后续对话复用的项目知识，用简体中文、Markdown 输出。",
    "输出结构固定为四节，不要加额外的开场白或结尾寒暄：",
    "## 项目概况：这是什么设备/机型、基于什么芯片平台、面向什么场景。",
    "## 关键信息：版本号、分区、烧录/升级步骤、依赖工具、参数、注意事项。",
    "## 术语与约定：文档里出现的专有名词、代号、路径约定。",
    "## 后续可问的问题：3-5 条基于本文档最值得继续追问的点。",
    "只写文档里确实有的内容；文档没写的部分写「文档未提及」，不要编造。",
  ].join("\n");
  const body = documents
    .map((document) => {
      const header = `### 文件：${document.name}`;
      if (!document.text.trim()) {
        return `${header}\n（无内容：${document.note ?? "空文件"}）`;
      }
      return `${header}\n\`\`\`\n${document.text.slice(0, MAX_DOCUMENT_CHARS)}\n\`\`\``;
    })
    .join("\n\n");
  const user = [
    truncated
      ? "以下是项目文档内容（篇幅较长，只提供了前一部分）："
      : "以下是项目文档内容：",
    "",
    body,
  ].join("\n");
  return { system, user };
}

/**
 * 解析一份项目知识文档。
 * 调用方负责把结果写回 `AutomationProject.knowledge` 并更新状态。
 */
export async function analyzeProjectKnowledge(path: string): Promise<KnowledgeAnalysisResult> {
  if (!isTauri()) {
    return { ok: false, code: "invoke_failed", message: "文档解析仅在桌面应用中可用。" };
  }

  // 1. 有没有可用的模型。
  let providers: TerminalProviderLike[] = [];
  try {
    providers = await listAgentProviders();
  } catch {
    providers = [];
  }
  const choice = resolveModelChoice(providers as TerminalProviderLike[]);
  if (!choice) {
    return { ok: false, code: "no_model", message: "未配置 AI：请先在「Agent 接入」里添加模型供应商。" };
  }

  // 2. 读文档。
  const documents = await collectDocuments(path);
  const readable = documents.filter((document) => document.text.trim().length > 0);
  if (!readable.length) {
    const reason = documents.find((document) => document.note)?.note;
    return {
      ok: false,
      code: "unreadable",
      message: reason
        ? `无法读取文档内容：${reason}`
        : "文档里没有可读取的文本内容。",
    };
  }

  // 3. 拼请求：总量超限时截断并标记。
  let totalChars = 0;
  const budgeted: CollectedDocument[] = [];
  let truncated = false;
  for (const document of readable) {
    const remaining = MAX_DOCUMENT_CHARS - totalChars;
    if (remaining <= 0) {
      truncated = true;
      break;
    }
    if (document.text.length > remaining) {
      budgeted.push({ ...document, text: document.text.slice(0, remaining) });
      totalChars = MAX_DOCUMENT_CHARS;
      truncated = true;
      continue;
    }
    budgeted.push(document);
    totalChars += document.text.length;
  }
  if (documents.some((document) => document.note && document.text.trim().length > 0)) truncated = true;

  const prompt = buildSummaryPrompt(budgeted, truncated);
  const request = buildBlockAiRequest({
    modelId: choice.modelId,
    supportsReasoning: choice.supportsReasoning,
    prompt,
    sessionId: newRequestId("project-knowledge"),
    turnId: newRequestId("turn"),
  });

  // 4. 调模型。
  try {
    const response = await invoke<{ text?: string }>("agent_one_shot", { request });
    const summary = (response?.text ?? "").trim();
    if (!summary) {
      return { ok: false, code: "invoke_failed", message: "模型没有返回内容，请重试。" };
    }
    return {
      ok: true,
      summary,
      modelId: choice.modelId,
      documents: readable.length,
      truncated,
    };
  } catch (error) {
    return { ok: false, code: "invoke_failed", message: describeInvokeError(error) };
  }
}
