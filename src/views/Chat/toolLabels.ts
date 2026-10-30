import { canonicalToolName } from "./toolIdentity.ts";

/**
 * Single source of truth for tool display names.
 *
 * Two tables used to exist: `TOOL_LABELS` in AgentActivityTimeline and
 * `TOOL_STEP_TITLES` in chatMessagePresentation. They overlapped without being
 * identical, and both stopped at the local file/command tools, so log tools,
 * directory operations and task tools all rendered their raw snake_case id
 * (e.g. `query_android_device`) in the timeline, the background-task rows and
 * the permission prompt. Keeping one table here means a new tool is named once
 * and shows up everywhere.
 *
 * Names are written as verb phrases so they read naturally after 正在…,
 * e.g. 正在查询设备 / 正在执行设备操作.
 */
export const TOOL_LABELS: Record<string, string> = {
  // Interaction and planning
  ask_user: "询问用户",
  request_ssh_connection: "连接 SSH",
  update_task_plan: "更新任务计划",
  create_collaboration_job: "创建协作任务",
  load_skill: "加载技能",
  open_application_tool: "调度应用工具",
  goal: "推进长期目标",
  run_workflow: "运行工作流",
  echo_text: "回显文本",

  // Environment and workspace
  get_current_time: "读取当前时间",
  get_working_directory: "确认工作目录",
  get_file_info: "读取路径信息",
  get_directory_tree: "读取目录树",
  list_local_files: "读取目录",

  // Search and read
  search_local_files: "搜索文件",
  search_local_code: "搜索代码",
  read_local_file: "读取文件",
  read_local_log_range: "读取日志范围",
  search_local_logs: "搜索日志",
  rebuild_log_timeline: "重建日志时间线",

  // Write and file operations
  write_local_file: "写入文件",
  apply_patch: "修改文件",
  edit_file: "修改文件",
  create_directory: "创建目录",
  copy_file: "复制文件",
  move_file: "移动文件",
  delete_file: "删除文件",
  copy_directory: "复制目录",
  move_directory: "移动目录",
  set_file_permissions: "设置文件权限",

  // Transfer
  upload_file: "上传文件",
  download_file: "下载文件",
  upload_paths: "批量上传",
  download_path: "归档下载",
  upload_directory: "上传目录",
  download_directory: "下载目录",

  // Command and project
  run_local_command: "执行命令",
  run_local_shell: "执行命令",
  run_interactive_shell: "执行交互命令",
  run_project_check: "运行项目检查",

  // Language servers. These are registered from a name list
  // (agent/tool_registry.rs) instead of a `ToolDefinition::new("…")` literal, so
  // they are the tools the generic declaration scan cannot discover.
  lsp_diagnostics: "读取语言诊断",
  lsp_symbols: "读取符号列表",
  lsp_definition: "定位符号定义",
  lsp_references: "查找符号引用",
  lsp_hover: "读取悬浮提示",
  lsp_completion: "读取补全建议",
  lsp_rename: "生成重命名编辑",
  lsp_format: "生成格式化编辑",
  lsp_document_symbols: "读取文件符号树",
  lsp_workspace_symbols: "搜索工作区符号",

  // Git
  get_git_status: "读取 Git 状态",
  get_git_diff: "读取 Git 差异",
};

export function toolLabel(name: string) {
  const canonical = canonicalToolName(name);
  return TOOL_LABELS[canonical] || canonical.replace(/_/g, " ");
}

/** Replace every tool id that appears inside a sentence, leaving other words alone. */
export function localizeToolIds(text: string) {
  return text.replace(/\b[a-z][a-z0-9_]{2,}\b/g, (token) => {
    const canonical = canonicalToolName(token);
    return TOOL_LABELS[canonical] || token;
  });
}
