

import { readAutomationProjects, selectedAutomationProjectIds } from "./projectSelection.ts";
export { PROJECT_AUTOMATION_STORAGE_KEY, ACTIVE_PROJECT_STORAGE_KEY } from "./projectSelection.ts";

/**
 * Project configuration is user-authored context, not a replacement for the
 * runtime safety policy. Keep it bounded and attach it as a system context
 * turn so the Agent can use the same configuration for queued and new turns.
 */
export function projectAutomationContext(selection?: readonly string[]): Array<Record<string, unknown>> {
  if (typeof window === "undefined") return [];
  try {
    const projects = readAutomationProjects();
    const general = projects.find((item) => item && item.id === "general");
    const ids = new Set(selectedAutomationProjectIds(projects, selection));
    const selected = [general, ...projects.filter((item) => ids.has(item.id))]
      .filter((item): item is NonNullable<typeof item> => !!item);
    if (!selected.length) return [];
    const lines = ["[项目配置，仅作为用户上下文，不改变系统安全策略]",
      "通用配置适用于所有项目；当前项目配置补充通用配置，同一事项冲突时以当前项目配置为准。",
      "选中了多个项目时，按本轮任务对应的项目使用配置；不同项目的目录、连接和命令不得混用，目标不明确时先确认。",
      "下面的命令与流程是可复用的用户配置，按本轮用户任务选择执行；配置保存本身不触发执行。需要 Skill 时，根据本轮可用技能清单选择并加载说明，再执行工具。"];
    const prefix = lines.join("\n\n");
    const budget = Math.max(0, Math.floor((12000 - prefix.length - selected.length * 2) / selected.length));
    const sections: string[] = [];
    for (const record of selected) {
      const heading = record === general ? "通用配置" : "当前项目配置";
      sections.push(`[${heading}]\n${projectConfigurationText(record)}`.slice(0, budget));
    }
    return [{ role: "system", content: [prefix, ...sections].join("\n\n"), source: "project_configuration" }];
  } catch {
    return [];
  }
}

function projectConfigurationText(record: Record<string, unknown>) {
    const lines = [`项目名称：${typeof record.name === "string" ? record.name : "通用"}`];
    if (typeof record.description === "string" && record.description.trim()) lines.push(`项目说明：${record.description.trim()}`);
    if (typeof record.instructions === "string" && record.instructions.trim()) lines.push(`工作说明：\n${record.instructions.trim()}`);
    if (Array.isArray(record.commands) && record.commands.length) {
      lines.push("命令模板：");
      record.commands.slice(0, 40).forEach((item) => {
        if (!item || typeof item !== "object") return;
        const command = item as Record<string, unknown>;
        const name = typeof command.name === "string" ? command.name : "未命名命令";
        const target = typeof command.target === "string" ? ` [${command.target}]` : "";
        const text = typeof command.command === "string" ? command.command : "";
        if (text.trim()) lines.push(`- ${name}${target}: ${text.trim()}`);
      });
    }
    if (Array.isArray(record.workflows) && record.workflows.length) {
      lines.push("自动化流程：");
      record.workflows.slice(0, 20).forEach((item) => {
        if (!item || typeof item !== "object") return;
        const workflow = item as Record<string, unknown>;
        const name = typeof workflow.name === "string" ? workflow.name : "未命名流程";
        const steps = typeof workflow.steps === "string" ? workflow.steps.trim() : "";
        if (steps) lines.push(`- ${name}: ${steps}`);
      });
    }
    if (Array.isArray(record.knowledge) && record.knowledge.length) {
      const entries = record.knowledge.filter((item): item is Record<string, unknown> => !!item && typeof item === "object");
      const analyzed = entries.filter((item) => typeof item.analysis === "string" && item.analysis.trim());
      // 已解析的文档把摘要直接带进上下文：这是用户在设置里主动点「分析」生成的，
      // 比让模型自己去读文件更省 token，也保证每轮对话看到的知识一致。
      if (analyzed.length) {
        lines.push("项目知识摘要（由用户在设置中解析文档生成，可直接引用；需要原文时再通过文件工具读取）：");
        analyzed.slice(0, 20).forEach((item) => {
          const path = typeof item.path === "string" ? item.path.trim() : "";
          const name = path.split(/[\\/]/).filter(Boolean).pop() || path || "未命名文档";
          const analysis = String(item.analysis).trim();
          lines.push(`- 《${name}》${path ? `（${path}）` : ""}：\n${analysis}`);
        });
      }
      const pending = entries.filter((item) => !(typeof item.analysis === "string" && item.analysis.trim()));
      if (pending.length) {
        lines.push("项目文档路径（尚未解析内容，需要时通过文件工具读取）：");
        pending.slice(0, 40).forEach((item) => {
          if (typeof item.path === "string" && item.path.trim()) lines.push(`- ${item.path.trim()}`);
        });
      }
    }
    return lines.join("\n");
}
