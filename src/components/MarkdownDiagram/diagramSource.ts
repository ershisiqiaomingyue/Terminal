export function isMermaidLanguage(language: string) {
  return /^(?:mermaid|mermaidjs|mmd)$/i.test(language);
}

export function diagramSourceError(source: string): string | null {
  if (source.length > 50000 || source.split("\n").length > 2000) return "图表过大，请拆分后查看。";
  // 模型提供的配置不能覆盖应用的主题或安全设置。
  if (/%%\s*\{/.test(source) || /^\s*---(?:\r?\n)/.test(source)) return "图表配置由应用统一管理，请移除图源中的配置声明。";
  return null;
}

export function attachDiagramHost(pre: HTMLPreElement) {
  const code = pre.querySelector("code");
  const language = Array.from(code?.classList || []).find((value) => value.startsWith("language-"))?.slice(9) || "";
  if (!code || !isMermaidLanguage(language)) return false;
  const host = pre.ownerDocument.createElement("div");
  host.className = "markdown-diagram-mount";
  host.dataset.diagramSource = code.textContent || "";
  pre.replaceWith(host);
  return true;
}
