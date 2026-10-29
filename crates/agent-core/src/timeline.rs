use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineSegment {
    Commentary,
    Tools,
}

pub fn fallback_commentary_for_tool(
    tool_name: &str,
    input: &Value,
    requested_tool: Option<&str>,
) -> String {
    let tool_name = crate::canonical_tool_name(tool_name);
    let requested_tool = requested_tool.map(crate::canonical_tool_name);
    let path = input
        .get("path")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::trim);
    let action = match tool_name {
        "__agent_path_access_probe" => path
            .map(|value| format!("我先确认你指定的 {value} 是否允许访问。"))
            .unwrap_or_else(|| "我先确认指定路径是否允许访问。".to_string()),
        "list_local_files" => path
            .map(|value| format!("我先看一下 {value} 目录，确认有哪些文件。"))
            .unwrap_or_else(|| "我先看一下目录，确认有哪些文件。".to_string()),
        "get_file_info" => path
            .map(|value| format!("我先读取 {value} 的远程元信息，确认它是文件还是目录。"))
            .unwrap_or_else(|| "我先读取远程路径元信息，确认它是文件还是目录。".to_string()),
        "search_local_logs" => "我先搜索本地日志，定位相关记录。".to_string(),
        "read_local_file" => "我先读取相关文件内容，确认具体信息。".to_string(),
        "create_directory" => "我先创建指定目录。".to_string(),
        "copy_file" => "我先复制指定文件。".to_string(),
        "move_file" => "我先移动指定文件。".to_string(),
        "copy_directory" => "我先复制指定目录。".to_string(),
        "move_directory" => "我先移动指定目录。".to_string(),
        "delete_file" => "我先确认后删除指定文件。".to_string(),
        "upload_file" => "我先把选中的本地文件上传到远程工作区。".to_string(),
        "download_file" => "我先把远程文件下载到本地。".to_string(),
        "upload_paths" => "我先将选中的文件和目录打包，一次上传到远程工作区。".to_string(),
        "download_path" => "我先将远程文件或目录打包下载到本地。".to_string(),
        "read_local_log_range" => "我先读取相关日志范围，确认具体报错。".to_string(),
        "get_working_directory" => "我先确认当前工作目录。".to_string(),
        "get_current_time" => "我先核对当前时间。".to_string(),
        "update_task_plan" => "我先更新执行计划。".to_string(),
        "run_local_command" | "run_local_shell" => {
            "我先运行一个受控命令，获取需要的结果。".to_string()
        }
        _ => "我先执行一个工具，补充必要信息。".to_string(),
    };
    if requested_tool == Some(tool_name) {
        format!("按你选择的快捷指令，{action}")
    } else {
        action
    }
}

pub fn take_tool_group_fallback(
    segment: &mut Option<TimelineSegment>,
    tool_name: &str,
    input: &Value,
    requested_tool: Option<&str>,
) -> Option<String> {
    let fallback = if matches!(
        segment,
        Some(TimelineSegment::Tools | TimelineSegment::Commentary)
    ) {
        None
    } else {
        Some(fallback_commentary_for_tool(
            tool_name,
            input,
            requested_tool,
        ))
    };
    *segment = Some(TimelineSegment::Tools);
    fallback
}

#[cfg(test)]
mod tests {
    #[test]
    fn environment_qualified_tools_get_specific_runtime_commentary() {
        for prefix in ["windows__", "ssh__", "wsl__"] {
            let tool = format!("{prefix}run_local_command");
            let note =
                super::fallback_commentary_for_tool(&tool, &serde_json::json!({}), Some(&tool));
            assert!(note.contains("受控命令"));
            assert!(note.starts_with("按你选择的快捷指令"));
            assert!(!note.contains("补充必要信息"));
        }
    }
}
