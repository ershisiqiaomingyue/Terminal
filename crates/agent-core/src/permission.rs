use crate::{
    canonical_tool_name, controlled_tool_for_command, external_path_scope_key_for_environment,
    is_allowed_command, is_allowed_windows_shell, is_forbidden_destructive_command,
    is_forbidden_destructive_command_line, is_read_only_tool, is_windows_shell_command,
    path_access_requires_confirmation_for_target,
    path_access_requires_external_execution_for_target, permission_requires_confirmation,
    permission_scope_key_for_environment, permission_scope_key_for_target, ToolDefinition,
    WorkspacePermissionConfig,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow,
    Deny { reason: String },
    Ask { request_id: String },
}

pub trait PermissionEvaluator: Send + Sync {
    fn evaluate(&self, definition: &ToolDefinition, input: &Value) -> PermissionDecision;
}

pub struct PermissionMode<'a> {
    session_id: &'a str,
    mode: &'a str,
    roots: &'a [String],
    is_remote_target: bool,
    scope_namespace: &'a str,
    session_approvals: &'a Mutex<HashSet<(String, String)>>,
    one_time_approvals: &'a Mutex<HashSet<(String, String)>>,
    workspace_permissions: Option<&'a WorkspacePermissionConfig>,
}

impl<'a> PermissionMode<'a> {
    pub fn new(
        session_id: &'a str,
        mode: &'a str,
        roots: &'a [String],
        is_remote_target: bool,
        scope_namespace: &'a str,
        session_approvals: &'a Mutex<HashSet<(String, String)>>,
        one_time_approvals: &'a Mutex<HashSet<(String, String)>>,
        workspace_permissions: Option<&'a WorkspacePermissionConfig>,
    ) -> Self {
        Self {
            session_id,
            mode,
            roots,
            is_remote_target,
            scope_namespace,
            session_approvals,
            one_time_approvals,
            workspace_permissions,
        }
    }
}

impl PermissionEvaluator for PermissionMode<'_> {
    fn evaluate(&self, definition: &ToolDefinition, input: &Value) -> PermissionDecision {
        let tool_name = canonical_tool_name(&definition.name);
        if tool_name == "run_local_command" && is_forbidden_destructive_command(input) {
            return PermissionDecision::Deny {
                reason: crate::FORBIDDEN_DESTRUCTIVE_COMMAND_MESSAGE.to_string(),
            };
        }
        if tool_name == "run_local_shell"
            && input
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(is_forbidden_destructive_command_line)
        {
            return PermissionDecision::Deny {
                reason: crate::FORBIDDEN_DESTRUCTIVE_COMMAND_MESSAGE.to_string(),
            };
        }
        if tool_name == "run_local_command" && !self.is_remote_target {
            if let Some(command) = input.get("command").and_then(Value::as_str) {
                if let Some(tool) = controlled_tool_for_command(command) {
                    return PermissionDecision::Deny {
                        reason: format!("请改用受控工具 {tool}，不会启动 shell 别名"),
                    };
                }
                if is_windows_shell_command(command) {
                    return PermissionDecision::Deny {
                        reason: format!(
                            "不能通过 run_local_command 启动 shell `{command}`，请改用 run_local_shell"
                        ),
                    };
                }
                if !is_allowed_command(command) {
                    return PermissionDecision::Deny {
                        reason: "命令不在 Agent 允许列表中".to_string(),
                    };
                }
            }
        }
        if tool_name == "run_local_shell" {
            let shell = input
                .get("shell")
                .and_then(Value::as_str)
                .map(|value| value.trim().to_ascii_lowercase())
                .unwrap_or_default();
            let shell_allowed = if self.is_remote_target {
                matches!(shell.as_str(), "bash" | "sh" | "zsh")
            } else {
                is_allowed_windows_shell(&shell)
            };
            if !shell_allowed {
                return PermissionDecision::Deny {
                    reason: if self.is_remote_target {
                        "SSH 远程 shell 只支持 bash、sh 或 zsh".to_string()
                    } else {
                        "Windows shell 只支持 powershell、pwsh、cmd、bash 和 wsl".to_string()
                    },
                };
            }
        }
        let external_path = path_access_requires_external_execution_for_target(
            tool_name,
            input,
            self.roots,
            self.is_remote_target,
        );
        let read_only = definition.read_only || is_read_only_tool(tool_name, input);
        let risk_auto_allowed = self.mode == "risk" && read_only && !external_path;
        let requires_permission = definition.requires_permission && !risk_auto_allowed;
        let path_permission = path_access_requires_confirmation_for_target(
            self.mode,
            tool_name,
            input,
            self.roots,
            read_only,
            self.is_remote_target,
        );
        let workspace_approval_key =
            permission_scope_key_for_target(tool_name, input, self.roots, self.is_remote_target);
        let workspace_external_scope = crate::external_path_scope_key_for_target(
            tool_name,
            input,
            self.roots,
            self.is_remote_target,
        );
        let scope_key = workspace_approval_key.clone();
        let approval_key = permission_scope_key_for_environment(
            tool_name,
            input,
            self.roots,
            self.is_remote_target,
            self.scope_namespace,
        );
        let external_scope = external_path_scope_key_for_environment(
            tool_name,
            input,
            self.roots,
            self.is_remote_target,
            self.scope_namespace,
        );
        let mut approval_keys = vec![approval_key.clone()];
        // P7-B：精确 key 之后补一层环境种类 key（`ssh:<连接 id>` → `ssh::{scope}`），
        // 换连接 / 换 profile 时同一环境的授权仍然命中；跨环境仍靠种类前缀隔离。
        // 注意必须传**未加 namespace 的 scope**，否则会拼成 `ssh::ssh:<id>::...`。
        if let Some(kind_key) = crate::permission_scope_kind_key(self.scope_namespace, &scope_key) {
            approval_keys.push(kind_key);
        }
        if let Some(scope) = external_scope {
            if let Some(kind_key) = crate::permission_scope_kind_key(self.scope_namespace, &scope) {
                if !approval_keys.contains(&kind_key) {
                    approval_keys.push(kind_key);
                }
            }
            if scope != approval_key {
                approval_keys.push(scope);
            }
        }
        let mut workspace_approval_keys = vec![workspace_approval_key.clone()];
        if let Some(scope) = workspace_external_scope {
            if scope != workspace_approval_key {
                workspace_approval_keys.push(scope);
            }
        }
        if self.workspace_permissions.is_some_and(|permissions| {
            workspace_approval_keys
                .iter()
                .any(|key| permissions.denied_scopes.iter().any(|scope| scope == key))
        }) && self.mode != "full"
        {
            return PermissionDecision::Deny {
                reason: "工作区权限策略拒绝了当前操作".to_string(),
            };
        }
        if (requires_permission || path_permission)
            && self.workspace_permissions.is_some_and(|permissions| {
                workspace_approval_keys
                    .iter()
                    .any(|key| permissions.allowed_scopes.iter().any(|scope| scope == key))
            })
        {
            return PermissionDecision::Allow;
        }
        if (requires_permission || path_permission)
            && self
                .session_approvals
                .lock()
                .map(|values| {
                    approval_keys
                        .iter()
                        .any(|key| values.contains(&(self.session_id.to_string(), key.clone())))
                })
                .unwrap_or(false)
        {
            return PermissionDecision::Allow;
        }
        if (requires_permission || path_permission)
            && self
                .one_time_approvals
                .lock()
                .map(|values| {
                    approval_keys
                        .iter()
                        .any(|key| values.contains(&(self.session_id.to_string(), key.clone())))
                })
                .unwrap_or(false)
        {
            return PermissionDecision::Allow;
        }
        if path_permission {
            return PermissionDecision::Ask {
                request_id: permission_request_id(self.session_id, &definition.name),
            };
        }
        if self.mode == "full" {
            return PermissionDecision::Allow;
        }
        if !permission_requires_confirmation(self.mode, requires_permission) {
            return PermissionDecision::Allow;
        }
        PermissionDecision::Ask {
            request_id: permission_request_id(self.session_id, &definition.name),
        }
    }
}

fn permission_request_id(session_id: &str, tool_name: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    format!("{session_id}:{tool_name}:{suffix:x}")
}

pub fn parse_tool_call_input(
    initial_input: Value,
    arguments: &str,
    tool_name: &str,
) -> Result<Value, Value> {
    if arguments.trim().is_empty() {
        return Ok(initial_input);
    }
    serde_json::from_str(arguments).map_err(|error| {
        let message = error.to_string();
        let code = if message.contains("EOF") || message.contains("unterminated") {
            "tool_arguments_incomplete"
        } else {
            "tool_arguments_invalid"
        };
        serde_json::json!({
            "error": format!("工具参数不完整，无法执行 {tool_name}，模型需要重新发送完整参数。解析信息：{message}"),
            "code": code,
            "retryable": true,
            "tool": tool_name,
        })
    })
}

pub struct AllowPermission;

impl PermissionEvaluator for AllowPermission {
    fn evaluate(&self, _definition: &ToolDefinition, _input: &Value) -> PermissionDecision {
        PermissionDecision::Allow
    }
}

pub struct DenyPermission {
    pub reason: String,
}

impl PermissionEvaluator for DenyPermission {
    fn evaluate(&self, _definition: &ToolDefinition, _input: &Value) -> PermissionDecision {
        PermissionDecision::Deny {
            reason: self.reason.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolDefinition;
    use serde_json::json;

    fn permission_mode(is_remote_target: bool) -> PermissionMode<'static> {
        let session_id: &'static str = Box::leak("test-session".to_string().into_boxed_str());
        let mode: &'static str = Box::leak("ask".to_string().into_boxed_str());
        let roots: &'static [String] =
            Box::leak(vec!["/remote/aosp".to_string()].into_boxed_slice());
        let approvals = Box::leak(Box::new(Mutex::new(HashSet::new())));
        let one_time_approvals = Box::leak(Box::new(Mutex::new(HashSet::new())));
        PermissionMode::new(
            session_id,
            mode,
            roots,
            is_remote_target,
            if is_remote_target {
                "ssh:test-connection"
            } else {
                "windows"
            },
            approvals,
            one_time_approvals,
            None,
        )
    }

    #[test]
    fn remote_commands_are_permission_gated_without_local_allowlist() {
        let definition = ToolDefinition::new("run_local_command", "remote command", json!({}))
            .requiring_permission();
        let input = json!({
            "command": "make",
            "args": ["-j8"],
            "cwd": "/remote/aosp"
        });

        assert!(matches!(
            permission_mode(true).evaluate(&definition, &input),
            PermissionDecision::Ask { .. }
        ));
        assert!(matches!(
            permission_mode(false).evaluate(&definition, &input),
            PermissionDecision::Deny { .. }
        ));
    }

    #[test]
    fn destructive_remote_commands_remain_hard_blocked() {
        let definition = ToolDefinition::new("run_local_command", "remote command", json!({}))
            .requiring_permission();
        let input = json!({
            "command": "rm",
            "args": ["-rf", "*"],
            "cwd": "/remote/aosp"
        });

        assert!(matches!(
            permission_mode(true).evaluate(&definition, &input),
            PermissionDecision::Deny { .. }
        ));
    }

    #[test]
    fn full_access_skips_approvals_for_local_and_remote_paths_but_keeps_hard_blocks() {
        let roots = vec!["/remote/aosp".to_string()];
        let approvals = Mutex::new(HashSet::new());
        let once = Mutex::new(HashSet::new());
        for remote in [false, true] {
            let policy =
                PermissionMode::new("s", "full", &roots, remote, "test", &approvals, &once, None);
            for (tool, input) in [
                ("list_local_files", json!({"path": "/outside/project"})),
                (
                    "write_local_file",
                    json!({"path": "/outside/project/a", "content": "value"}),
                ),
            ] {
                let definition =
                    ToolDefinition::new(tool, "test", json!({})).requiring_permission();
                assert_eq!(
                    policy.evaluate(&definition, &input),
                    PermissionDecision::Allow
                );
            }
            let definition =
                ToolDefinition::new("run_local_command", "test", json!({})).requiring_permission();
            assert!(matches!(
                policy.evaluate(&definition, &json!({"command": "rm", "args": ["-rf", "*"]})),
                PermissionDecision::Deny { .. }
            ));
        }
    }

    #[test]
    fn remote_shell_accepts_common_posix_shells() {
        let definition = ToolDefinition::new("run_local_shell", "remote shell", json!({}))
            .requiring_permission();
        for shell in ["bash", "sh", "zsh"] {
            assert!(matches!(
                permission_mode(true).evaluate(
                    &definition,
                    &json!({ "shell": shell, "command": "printf ok" })
                ),
                PermissionDecision::Ask { .. }
            ));
        }
    }

    #[test]
    fn session_approval_is_isolated_by_environment_identity() {
        let session_id = "test-session";
        let mode = "ask";
        let roots = vec!["/remote/aosp".to_string()];
        let approvals = Mutex::new(HashSet::new());
        let one_time_approvals = Mutex::new(HashSet::new());
        let definition =
            ToolDefinition::new("run_local_shell", "write", json!({})).requiring_permission();
        let input = json!({ "shell": "bash", "command": "make", "cwd": "/remote/aosp" });
        let ssh_scope = permission_scope_key_for_environment(
            &definition.name,
            &input,
            &roots,
            true,
            "ssh:connection-a",
        );
        approvals
            .lock()
            .expect("approvals")
            .insert((session_id.to_string(), ssh_scope));

        let ssh = PermissionMode::new(
            session_id,
            mode,
            &roots,
            true,
            "ssh:connection-a",
            &approvals,
            &one_time_approvals,
            None,
        );
        let wsl = PermissionMode::new(
            session_id,
            mode,
            &roots,
            true,
            "wsl:profile-a",
            &approvals,
            &one_time_approvals,
            None,
        );

        assert_eq!(ssh.evaluate(&definition, &input), PermissionDecision::Allow);
        assert!(matches!(
            wsl.evaluate(&definition, &input),
            PermissionDecision::Ask { .. }
        ));
    }

    /// P7-B：换连接（`ssh:<id>` 变了）之后，同一环境的「本次会话允许」仍然命中；
    /// 跨环境（ssh ↔ wsl ↔ 本地）绝不串权。
    #[test]
    fn session_approval_survives_a_connection_switch_but_not_a_environment_switch() {
        let session_id = "test-session";
        let roots = vec!["/remote/aosp".to_string()];
        let approvals = Mutex::new(HashSet::new());
        let one_time_approvals = Mutex::new(HashSet::new());
        let definition =
            ToolDefinition::new("run_local_shell", "write", json!({})).requiring_permission();
        let input = json!({ "shell": "bash", "command": "make", "cwd": "/remote/aosp" });

        // 用户点「本次会话允许」：精确 key + 环境种类 key 一起写入。
        let exact = permission_scope_key_for_environment(
            &definition.name,
            &input,
            &roots,
            true,
            "ssh:connection-a",
        );
        // 生产路径里种类 key 由命名空间 + 同一 scope 拼成；这里直接按同一规则构造，
        // 避免重复调用 scope 函数掩盖真实差异。
        let scope = permission_scope_key_for_target(&definition.name, &input, &roots, true);
        let kind = crate::permission_scope_kind_key("ssh:connection-a", &scope).expect("kind key");
        assert_eq!(exact, format!("ssh:connection-a::{scope}"));
        assert_eq!(kind, format!("ssh::{scope}"));
        {
            let mut approvals = approvals.lock().expect("approvals");
            approvals.insert((session_id.to_string(), exact.clone()));
            approvals.insert((session_id.to_string(), kind.clone()));
        }

        // 换到同一台机器的另一条连接：种类 key 命中。
        let other_connection = PermissionMode::new(
            session_id,
            "ask",
            &roots,
            true,
            "ssh:connection-b",
            &approvals,
            &one_time_approvals,
            None,
        );
        assert_eq!(
            other_connection.evaluate(&definition, &input),
            PermissionDecision::Allow
        );

        // 换环境（WSL）或换会话：必须重新询问。
        for namespace in ["wsl:profile-a", "ssh:"] {
            let other_environment = PermissionMode::new(
                session_id,
                "ask",
                &roots,
                true,
                namespace,
                &approvals,
                &one_time_approvals,
                None,
            );
            let decision = other_environment.evaluate(&definition, &input);
            if namespace == "ssh:" {
                // 空目标 id 仍是 ssh 种类：按设计命中（同一环境）。
                assert_eq!(decision, PermissionDecision::Allow);
            } else {
                assert!(matches!(decision, PermissionDecision::Ask { .. }));
            }
        }
        let other_session = PermissionMode::new(
            "other-session",
            "ask",
            &roots,
            true,
            "ssh:connection-b",
            &approvals,
            &one_time_approvals,
            None,
        );
        assert!(matches!(
            other_session.evaluate(&definition, &input),
            PermissionDecision::Ask { .. }
        ));
    }
}
