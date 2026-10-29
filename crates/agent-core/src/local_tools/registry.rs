use super::commands::{
    GetGitDiffTool, GetGitStatusTool, RunLocalCommandTool, RunLocalShellTool, RunProjectCheckTool,
};
use super::context::{LocalToolContext, PATH_ACCESS_PROBE_TOOL};
use super::files::read::ReadLocalFileTool;
use super::files::write::WriteLocalFileTool;
use super::files::{CopyFileTool, CreateDirectoryTool, DeleteFileTool, MoveFileTool};
use super::patches::ApplyPatchTool;
use super::search::{
    DirectoryTreeTool, ListLocalFilesTool, SearchLocalCodeTool, SearchLocalFilesTool,
};
use super::wsl::{CurrentTimeTool, WorkingDirectoryTool, WslPathMapper, WslToolAdapter};
use crate::{ToolDefinition, ToolHandler, ToolRegistry};
use async_trait::async_trait;
use serde_json::{json, Value};

struct PathAccessProbeTool;

#[async_trait]
impl ToolHandler for PathAccessProbeTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            PATH_ACCESS_PROBE_TOOL,
            "Internal runtime permission preflight. Never expose this tool to the model.",
            json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "action": { "type": "string" }
                },
                "required": ["path"]
            }),
        )
        .requiring_permission()
        .internal()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        Ok(json!({
            "preflight": true,
            "path": input.get("path").and_then(Value::as_str).unwrap_or_default(),
            "action": input.get("action").and_then(Value::as_str).unwrap_or_default(),
        }))
    }
}

pub fn register_local_tools(
    registry: &mut ToolRegistry,
    context: LocalToolContext,
) -> Result<(), String> {
    let registrations: Vec<Box<dyn ToolHandler>> = vec![
        Box::new(CurrentTimeTool(context.for_tool("get_current_time"))),
        Box::new(WorkingDirectoryTool {
            roots: context.roots.clone(),
        }),
        Box::new(ListLocalFilesTool(context.for_tool("list_local_files"))),
        Box::new(SearchLocalFilesTool(context.for_tool("search_local_files"))),
        Box::new(SearchLocalCodeTool(context.for_tool("search_local_code"))),
        Box::new(DirectoryTreeTool(context.for_tool("get_directory_tree"))),
        Box::new(ApplyPatchTool(context.for_tool("apply_patch"))),
        Box::new(ReadLocalFileTool(context.for_tool("read_local_file"))),
        Box::new(WriteLocalFileTool(context.for_tool("write_local_file"))),
        Box::new(CreateDirectoryTool(context.for_tool("create_directory"))),
        Box::new(CopyFileTool(context.for_tool("copy_file"))),
        Box::new(MoveFileTool(context.for_tool("move_file"))),
        Box::new(DeleteFileTool(context.for_tool("delete_file"))),
        Box::new(RunLocalCommandTool(context.for_tool("run_local_command"))),
        Box::new(GetGitStatusTool(context.for_tool("get_git_status"))),
        Box::new(GetGitDiffTool(context.for_tool("get_git_diff"))),
        Box::new(RunProjectCheckTool(context.for_tool("run_project_check"))),
        Box::new(RunLocalShellTool(context.for_tool("run_local_shell"))),
        Box::new(PathAccessProbeTool),
    ];
    let wsl_mapper = context.wsl.clone().map(WslPathMapper::new);
    for handler in registrations {
        let handler: Box<dyn ToolHandler> = if let Some(mapper) = wsl_mapper.clone() {
            Box::new(WslToolAdapter {
                inner: handler,
                mapper,
            })
        } else {
            handler
        };
        registry
            .register_handler_boxed(handler)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}
