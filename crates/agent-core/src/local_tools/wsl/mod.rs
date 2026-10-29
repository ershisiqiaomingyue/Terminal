use super::context::LocalToolContext;
use crate::{
    CommandExecutionResult, CommandOutputSink, ToolDefinition, ToolHandler, WslRuntimeConfig,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;

#[derive(Clone)]
pub(in crate::local_tools) struct WslPathMapper {
    config: WslRuntimeConfig,
    unc_prefix: String,
}

impl WslPathMapper {
    pub(super) fn new(config: WslRuntimeConfig) -> Self {
        Self {
            unc_prefix: format!(r"\\wsl.localhost\{}", config.distribution),
            config,
        }
    }

    pub(super) fn to_unc_path(&self, raw: &str) -> String {
        let value = raw.trim();
        if value.starts_with(r"\\wsl.localhost\") || value.starts_with(r"\\wsl$\") {
            return value.to_string();
        }
        let linux = if value == "~" || value.is_empty() {
            self.config.root.clone()
        } else if value.starts_with('/') {
            crate::normalize_linux_path(value)
        } else {
            crate::normalize_linux_path(&format!("{}/{}", self.config.root, value))
        };
        if linux == "/" {
            self.unc_prefix.clone()
        } else {
            format!(
                "{}\\{}",
                self.unc_prefix,
                linux.trim_start_matches('/').replace('/', "\\")
            )
        }
    }

    pub(super) fn from_unc_path(&self, raw: &str) -> String {
        let value = raw.trim();
        let prefix = self.unc_prefix.as_str();
        if value.eq_ignore_ascii_case(prefix) {
            return "/".to_string();
        }
        let prefix_with_separator = format!("{}\\", prefix);
        if value.len() > prefix_with_separator.len()
            && value[..prefix_with_separator.len()].eq_ignore_ascii_case(&prefix_with_separator)
        {
            return format!(
                "/{}",
                value[prefix_with_separator.len()..]
                    .replace('\\', "/")
                    .trim_matches('/')
            );
        }
        value.to_string()
    }
}

pub(in crate::local_tools) struct WslToolAdapter {
    pub(in crate::local_tools) inner: Box<dyn ToolHandler>,
    pub(in crate::local_tools) mapper: WslPathMapper,
}

fn map_wsl_input(value: &mut Value, mapper: &WslPathMapper) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    for key in ["path", "source", "destination", "cwd"] {
        if let Some(Value::String(path)) = object.get_mut(key) {
            *path = mapper.to_unc_path(path);
        }
    }
}

fn map_wsl_output(value: Value, mapper: &WslPathMapper) -> Value {
    match value {
        Value::String(value) => Value::String(mapper.from_unc_path(&value)),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|value| map_wsl_output(value, mapper))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, map_wsl_output(value, mapper)))
                .collect(),
        ),
        value => value,
    }
}

#[async_trait]
impl ToolHandler for WslToolAdapter {
    fn definition(&self) -> ToolDefinition {
        self.inner.definition()
    }

    async fn execute(&self, mut input: Value) -> Result<Value, String> {
        map_wsl_input(&mut input, &self.mapper);
        self.inner
            .execute(input)
            .await
            .map(|value| map_wsl_output(value, &self.mapper))
    }
}

pub(super) fn wsl_mapper(context: &LocalToolContext) -> Option<WslPathMapper> {
    context.wsl.clone().map(WslPathMapper::new)
}

pub(super) fn wsl_linux_cwd(context: &LocalToolContext, cwd: &Path) -> String {
    wsl_mapper(context)
        .map(|mapper| mapper.from_unc_path(&cwd.to_string_lossy()))
        .filter(|path| path.starts_with('/'))
        .or_else(|| context.wsl.as_ref().map(|config| config.root.clone()))
        .unwrap_or_else(|| "/".to_string())
}

pub(super) async fn execute_wsl_process(
    context: &LocalToolContext,
    command: &str,
    args: &[String],
    cwd: &Path,
    timeout_ms: u64,
    output_sink: Option<&CommandOutputSink>,
) -> Result<CommandExecutionResult, String> {
    let config = context
        .wsl
        .as_ref()
        .ok_or_else(|| "当前工具上下文不是 WSL".to_string())?;
    #[cfg(target_os = "windows")]
    {
        return crate::wsl_process::execute(
            &config.distribution,
            &wsl_linux_cwd(context, cwd),
            command,
            args,
            timeout_ms,
            context.cancellation.clone(),
            output_sink,
        )
        .await;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (config, command, args, cwd, timeout_ms, output_sink);
        Err("WSL 执行目标只支持 Windows".to_string())
    }
}

pub(super) async fn execute_wsl_shell(
    context: &LocalToolContext,
    shell: &str,
    command: &str,
    cwd: &Path,
    timeout_ms: u64,
    output_sink: Option<&CommandOutputSink>,
) -> Result<CommandExecutionResult, String> {
    let shell = if shell == "wsl" { "bash" } else { shell };
    execute_wsl_process(
        context,
        shell,
        &["-lc".to_string(), command.to_string()],
        cwd,
        timeout_ms,
        output_sink,
    )
    .await
}

pub(in crate::local_tools) struct CurrentTimeTool(pub(in crate::local_tools) LocalToolContext);

#[async_trait]
impl ToolHandler for CurrentTimeTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "get_current_time",
            "Get the current local date, time, and timezone from the host computer. Use this for questions about now, today's date, or the current time; never guess the clock.",
            json!({ "type": "object", "properties": {} }),
        )
    }

    async fn execute(&self, _input: Value) -> Result<Value, String> {
        if self.0.wsl.is_some() {
            let command = "printf '__AT_LOCAL__%s\\n' \"$(date '+%Y-%m-%d %H:%M:%S' 2>/dev/null || true)\"; printf '__AT_ISO__%s\\n' \"$(date -Iseconds 2>/dev/null || date '+%Y-%m-%dT%H:%M:%S%z' 2>/dev/null || true)\"; printf '__AT_TZ__%s\\n' \"$(date '+%Z %z' 2>/dev/null || true)\"";
            let cwd = Path::new(".");
            let output = execute_wsl_shell(
                &self.0,
                "bash",
                command,
                cwd,
                30_000,
                self.0.output_sink.as_ref(),
            )
            .await?;
            if !output.status.success() {
                return Err(format!(
                    "读取 WSL 时间失败：{}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            let mut local = String::new();
            let mut iso = String::new();
            let mut timezone = String::new();
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                if let Some(value) = line.strip_prefix("__AT_LOCAL__") {
                    local = value.trim().to_string();
                } else if let Some(value) = line.strip_prefix("__AT_ISO__") {
                    iso = value.trim().to_string();
                } else if let Some(value) = line.strip_prefix("__AT_TZ__") {
                    timezone = value.trim().to_string();
                }
            }
            if local.is_empty() {
                return Err("WSL 未返回有效时间".to_string());
            }
            return Ok(json!({
                "local": local,
                "iso": iso,
                "timezone": timezone,
                "target": "wsl",
                "backend": "wsl.exe",
            }));
        }
        let now = chrono::Local::now();
        Ok(json!({
            "local": now.format("%Y-%m-%d %H:%M:%S").to_string(),
            "iso": now.to_rfc3339(),
            "timezone": now.offset().to_string(),
        }))
    }
}

pub(in crate::local_tools) struct WorkingDirectoryTool {
    pub(in crate::local_tools) roots: Vec<String>,
}

#[async_trait]
impl ToolHandler for WorkingDirectoryTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "get_working_directory",
            "Return the current user-selected Agent working directory. If the conversation has no selected workspace, return null instead of guessing a process directory.",
            json!({ "type": "object", "properties": {} }),
        )
    }

    async fn execute(&self, _input: Value) -> Result<Value, String> {
        Ok(json!({
            "path": crate::selected_working_directory(&self.roots).map(|path| path.to_string_lossy().to_string()),
            "selected": !self.roots.is_empty(),
        }))
    }
}
