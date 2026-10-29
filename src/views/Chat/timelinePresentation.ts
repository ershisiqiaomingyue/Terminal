export type ToolGroupPresentationState = "running" | "pending" | "done" | "error" | "unknown" | "cancelled" | "skipped" | "interrupted";

export function toolGroupPresentationState(statuses: string[], running: boolean): ToolGroupPresentationState {
  if (running) return "running";
  for (const status of ["error", "unknown", "interrupted", "cancelled", "pending", "skipped"] as const) {
    if (statuses.includes(status)) return status;
  }
  // A settled turn with an unfinished call has no verified successful result.
  if (statuses.includes("running")) return "unknown";
  return "done";
}

export function disclosureIsOpen(explicit: boolean | undefined) {
  return explicit === true;
}

/** Keep the live footer and committed timeline from sharing disclosure state. */
export function timelineDisclosureKey(scope: "live" | "committed", id: string) {
  return `${scope}:${id}`;
}

function stringField(record: Record<string, unknown>, keys: string[]) {
  for (const key of keys) {
    const value = record[key];
    if (typeof value === "string" && value.trim()) return value.trim();
  }
  return "";
}

/**
 * Transfer tools name a source and a destination instead of a single path.
 * Rendering them as "source → destination" keeps the live heading readable;
 * without this the heading fell back to dumping the whole argument JSON.
 */
export function transferSummary(record: Record<string, unknown>) {
  const source = stringField(record, ["local_path", "source", "from", "src"]);
  const destination = stringField(record, ["remote_path", "destination", "to", "dst"]);
  if (!source || !destination) return "";
  return `${source} → ${destination}`;
}

export function commandGroupLabel(state: ToolGroupPresentationState, preview = "") {
  if (state === "running") return preview ? `正在运行 ${preview}` : "正在运行命令";
  if (state === "error") return "命令执行失败";
  if (state === "pending") return "指令等待执行";
  if (state === "cancelled") return "指令已取消";
  if (state === "skipped") return "指令未执行";
  if (state === "interrupted") return "指令执行中断";
  if (state === "unknown") return "指令结果未保存";
  return "运行了命令";
}

export function compactToolGroupLabel(names: string[], state: ToolGroupPresentationState) {
  const kinds = new Set(names.map(name => {
    const tool = name.replace(/^(windows|ssh|wsl)__/, "");
    if (tool === "ask_user") return "询问了用户";
    if (["run_local_command", "run_local_shell", "run_interactive_shell", "run_project_check"].includes(tool)) return "运行了命令";
    if (["write_local_file", "apply_patch", "edit_file"].includes(tool)) return "编辑了文件";
    if (["read_local_file", "read_local_log_range"].includes(tool)) return "读取了文件";
    if (tool.startsWith("search_")) return "进行了搜索";
    return "使用了工具";
  }));
  const label = [...kinds].join("、");
  if (state === "running") return kinds.size === 1 && kinds.has("询问了用户") ? "等待用户回答" : "正在执行操作";
  if (state === "error") return `${label} · 有操作失败`;
  if (state === "unknown") return `${label} · 结果未保存`;
  if (state === "interrupted") return `${label} · 已中断`;
  if (state === "cancelled") return "操作已取消";
  if (state === "pending") return "操作等待执行";
  if (state === "skipped") return "操作未执行";
  return label;
}
