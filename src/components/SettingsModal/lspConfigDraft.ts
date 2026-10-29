/**
 * LSP 面板的纯逻辑：把配置里的一个服务器展开成可编辑草稿，再把草稿折叠回
 * `save_lsp_server` 需要的那三个字段。
 *
 * 单独放在这里（而不是留在 LspPanel.tsx 里）是为了能用 node:test 直接覆盖：
 * 写回时丢字段、丢条目、丢映射都会静默地让 LSP 工具失效，必须有测试守着。
 * 本文件不依赖 React，也不做任何 I/O。
 */

export interface LanguageRow {
  /** 扩展名，不带点（如 rs）。 */
  extension: string;
  /** LSP 语言 ID（如 rust）。 */
  language: string;
}

export interface ServerDraft {
  command: string;
  args: string[];
  languages: LanguageRow[];
}

/** 草稿来源只需这三个字段，因此可以直接接住后端返回的 LspServerView。 */
export interface LspServerDraftSource {
  command: string;
  args: string[];
  languages: Record<string, string>;
}

/** 后端视图 → 草稿。数组与映射都复制一份，编辑草稿不会改动已加载的视图。 */
export function draftFrom(server: LspServerDraftSource): ServerDraft {
  return {
    command: server.command,
    args: [...server.args],
    languages: Object.entries(server.languages).map(([extension, language]) => ({ extension, language })),
  };
}

/**
 * 参数草稿 → 提交给后端的数组：去首尾空白、丢掉空行（用户点了「添加参数」
 * 又没填的情况），其余顺序不变。后端拒绝空参数，所以空行必须在提交前清掉。
 */
export function normalizeArgs(args: string[]): string[] {
  return args.map((argument) => argument.trim()).filter((argument) => argument.length > 0);
}

/**
 * 语言映射草稿 → 提交给后端的完整映射。
 *
 * - 整行留空 = 用户放弃这一行，忽略；
 * - 半填（只有扩展名或只有语言 ID）报错，而不是丢弃：静默丢一条映射会让该
 *   扩展名的 LSP 工具失效，用户在界面上看不出任何区别；
 * - 容忍写成 ".rs" 的扩展名，提交前去掉前导点（后端只接受扩展名本身）；
 * - 全部为空时报错，避免保存出一个永远不会被选中的服务器。
 */
export function collectLanguages(rows: LanguageRow[]): Record<string, string> {
  const languages: Record<string, string> = {};
  for (const row of rows) {
    const extension = row.extension.trim().replace(/^\.+/, "");
    const language = row.language.trim();
    if (!extension && !language) continue;
    if (!extension || !language) throw new Error("每条语言映射都要同时填写扩展名和语言 ID。");
    languages[extension] = language;
  }
  if (Object.keys(languages).length === 0) throw new Error("至少需要保留一条「扩展名 → 语言 ID」映射。");
  return languages;
}

/**
 * 探测状态的徽标文案。后端可能返回未知状态（将来新增），一律显示成「未找到」，
 * 但 `unlaunchable`（文件找到了、本机没有能启动它的宿主）必须单独说明：
 * 把它显示成「未找到」会让用户去装一个其实已经装好的服务器。
 */
export function serverStatusLabel(status: string): string {
  if (status === "ready") return "可用";
  if (status === "disabled") return "已禁用";
  if (status === "invalid") return "条目不合法";
  if (status === "unlaunchable") return "找到但无法启动";
  return "未找到";
}

/** 徽标色调，对应 Badge 的 variant。 */
export function serverStatusTone(status: string): "success" | "warning" | "danger" | "neutral" {
  if (status === "ready") return "success";
  if (status === "disabled") return "neutral";
  if (status === "invalid" || status === "unlaunchable") return "danger";
  return "warning";
}
