use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use thiserror::Error;

use crate::{PermissionDecision, PermissionEvaluator};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub requires_permission: bool,
    #[serde(default, skip_serializing)]
    pub read_only: bool,
    #[serde(default, skip_serializing)]
    pub internal: bool,
}

impl ToolDefinition {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema,
            requires_permission: false,
            read_only: false,
            internal: false,
        }
    }

    pub fn requiring_permission(mut self) -> Self {
        self.requires_permission = true;
        self
    }

    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    pub fn internal(mut self) -> Self {
        self.internal = true;
        self
    }
}

#[async_trait]
pub trait ToolHandler: Send + Sync {
    fn definition(&self) -> ToolDefinition;

    async fn execute(&self, input: Value) -> Result<Value, String>;
}

struct AliasedToolHandler {
    definition: ToolDefinition,
    inner: Arc<dyn ToolHandler>,
}

#[async_trait]
impl ToolHandler for AliasedToolHandler {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        self.inner.execute(input).await
    }
}

/// 远程工具用来点名「绑定到该对话的某一台主机」的参数名。
pub const ROUTING_TARGET_ARGUMENT: &str = "target";

/// 归一化 `target` 取值，使连接名与连接 ID 都能匹配（忽略大小写与首尾空白）。
pub fn normalize_routing_target(value: &str) -> String {
    value.trim().to_lowercase()
}

/// 把同一个远程工具暴露给多台已绑定主机：不带 `target` 时按首选主机执行，带
/// `target` 时路由到对应主机的同名工具。
///
/// `target` 只参与路由，委托前会从输入里移除，避免污染内层工具的入参校验；
/// 事件与审计仍使用调用方记录的原始输入（含 `target`），便于前端展示。
struct RoutingAliasHandler {
    definition: ToolDefinition,
    primary: Arc<dyn ToolHandler>,
    targets: BTreeMap<String, Arc<dyn ToolHandler>>,
    target_hint: String,
}

impl RoutingAliasHandler {
    /// 取出并剥离 `target`，返回（归一化后的主机键, 剥离后的输入）。
    fn take_target(&self, input: Value) -> (Option<String>, Value) {
        let mut input = input;
        let requested = input
            .as_object_mut()
            .and_then(|map| map.remove(ROUTING_TARGET_ARGUMENT))
            .and_then(|value| value.as_str().map(normalize_routing_target))
            .filter(|value| !value.is_empty());
        (requested, input)
    }
}

#[async_trait]
impl ToolHandler for RoutingAliasHandler {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        let (requested, input) = self.take_target(input);
        let Some(requested) = requested else {
            return self.primary.execute(input).await;
        };
        let handler = self.targets.get(&requested).ok_or_else(|| {
            format!(
                "未绑定名为 {} 的远程主机；可用的主机有：{}。",
                requested, self.target_hint
            )
        })?;
        handler.execute(input).await
    }
}

/// 给 `additionalProperties: false` 的工具 schema 补一个可选 `target` 参数。
fn insert_routing_target_schema(schema: &mut Value, targets: &[String], hint: &str) {
    let Some(object) = schema.as_object_mut() else {
        return;
    };
    if object.get("type").and_then(Value::as_str) != Some("object") {
        return;
    }
    let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) else {
        return;
    };
    properties.insert(
        ROUTING_TARGET_ARGUMENT.to_string(),
        serde_json::json!({
            "type": "string",
            "enum": targets,
            "description": hint,
        }),
    );
}
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ToolRegistryError {
    #[error("tool name must not be empty")]
    EmptyName,
    #[error("tool is already registered: {0}")]
    Duplicate(String),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ToolExecutionError {
    #[error("tool is not registered: {0}")]
    NotFound(String),
    #[error("tool has no executable handler: {0}")]
    NoHandler(String),
    #[error("tool permission denied: {reason}")]
    PermissionDenied { reason: String },
    #[error("tool permission requires user confirmation: {request_id}")]
    PermissionRequired { request_id: String },
    #[error("tool execution failed: {0}")]
    Handler(String),
}

#[derive(Default)]
pub struct ToolRegistry {
    definitions: BTreeMap<String, ToolDefinition>,
    handlers: BTreeMap<String, Arc<dyn ToolHandler>>,
}

impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        Self {
            definitions: self.definitions.clone(),
            handlers: self.handlers.clone(),
        }
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, definition: ToolDefinition) -> Result<(), ToolRegistryError> {
        if definition.name.trim().is_empty() {
            return Err(ToolRegistryError::EmptyName);
        }
        if self.definitions.contains_key(&definition.name) {
            return Err(ToolRegistryError::Duplicate(definition.name));
        }
        self.definitions.insert(definition.name.clone(), definition);
        Ok(())
    }

    pub fn register_handler<H>(&mut self, handler: H) -> Result<(), ToolRegistryError>
    where
        H: ToolHandler + 'static,
    {
        let definition = handler.definition();
        let name = definition.name.clone();
        self.register(definition)?;
        self.handlers.insert(name, Arc::new(handler));
        Ok(())
    }

    pub fn register_handler_boxed(
        &mut self,
        handler: Box<dyn ToolHandler>,
    ) -> Result<(), ToolRegistryError> {
        let definition = handler.definition();
        let name = definition.name.clone();
        self.register(definition)?;
        self.handlers.insert(name, handler.into());
        Ok(())
    }

    /// Expose an existing handler under an environment-qualified name.
    ///
    /// The alias keeps the original schema and permission metadata while the
    /// runtime can route an explicit `windows__*`, `ssh__*`, or `wsl__*`
    /// request without replacing the turn's default environment.
    pub fn register_alias_from(
        &mut self,
        alias: impl Into<String>,
        source: &ToolRegistry,
        source_name: &str,
        description_prefix: &str,
    ) -> Result<(), ToolRegistryError> {
        let alias = alias.into();
        if alias.trim().is_empty() {
            return Err(ToolRegistryError::EmptyName);
        }
        if self.definitions.contains_key(&alias) {
            return Err(ToolRegistryError::Duplicate(alias));
        }
        let Some(source_definition) = source.definitions.get(source_name) else {
            return Ok(());
        };
        let Some(source_handler) = source.handlers.get(source_name) else {
            return Ok(());
        };
        let mut definition = source_definition.clone();
        definition.name = alias.clone();
        if !description_prefix.trim().is_empty() {
            definition.description =
                format!("{} {}", description_prefix.trim(), definition.description);
        }
        let handler: Arc<dyn ToolHandler> = Arc::new(AliasedToolHandler {
            definition: definition.clone(),
            inner: source_handler.clone(),
        });
        self.definitions.insert(alias.clone(), definition);
        self.handlers.insert(alias, handler);
        Ok(())
    }
    /// 暴露一个可路由到多台已绑定主机的环境别名。
    ///
    /// `primary` 决定别名继承的定义、权限元数据与缺省目标主机；`targets` 是
    /// `(主机名或连接 ID, 该主机工具所在的注册表)` 列表。只要至少有一台主机可
    /// 路由，就把可选 `target` 参数注入 schema（枚举列出全部主机名）；没有可用
    /// 主机时退化为普通别名，不带 `target` 的调用始终按 `primary` 执行，因此旧
    /// 会话与提示词里的 `ssh__*` 用法保持不变。
    pub fn register_routed_alias(
        &mut self,
        alias: impl Into<String>,
        primary: (&ToolRegistry, &str),
        targets: &[(String, &ToolRegistry)],
        description_prefix: &str,
        target_hint: &str,
    ) -> Result<(), ToolRegistryError> {
        let alias = alias.into();
        if alias.trim().is_empty() {
            return Err(ToolRegistryError::EmptyName);
        }
        if self.definitions.contains_key(&alias) {
            return Err(ToolRegistryError::Duplicate(alias));
        }
        let (primary_registry, source_name) = primary;
        let Some(primary_definition) = primary_registry.definitions.get(source_name) else {
            return Ok(());
        };
        let Some(primary_handler) = primary_registry.handlers.get(source_name) else {
            return Ok(());
        };
        let mut routed: BTreeMap<String, Arc<dyn ToolHandler>> = BTreeMap::new();
        let mut names: Vec<String> = Vec::new();
        for (target, registry) in targets {
            let key = normalize_routing_target(target);
            if key.is_empty() || routed.contains_key(&key) {
                continue;
            }
            let Some(handler) = registry.handlers.get(source_name) else {
                continue;
            };
            names.push(target.trim().to_string());
            routed.insert(key, handler.clone());
        }
        let mut definition = primary_definition.clone();
        definition.name = alias.clone();
        if !description_prefix.trim().is_empty() {
            definition.description =
                format!("{} {}", description_prefix.trim(), definition.description);
        }
        if !names.is_empty() {
            definition.description = format!("{} {}", definition.description, target_hint.trim());
            insert_routing_target_schema(&mut definition.input_schema, &names, target_hint.trim());
        }
        let handler: Arc<dyn ToolHandler> = Arc::new(RoutingAliasHandler {
            definition: definition.clone(),
            primary: primary_handler.clone(),
            targets: routed,
            target_hint: names.join("、"),
        });
        self.definitions.insert(alias.clone(), definition);
        self.handlers.insert(alias, handler);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&ToolDefinition> {
        self.definitions.get(name)
    }

    /// Decorate an existing tool without changing its public name or permissions.
    pub fn replace_handler<H: ToolHandler + 'static>(
        &mut self,
        handler: H,
    ) -> Result<(), ToolRegistryError> {
        let definition = handler.definition();
        let name = definition.name.clone();
        if name.trim().is_empty() {
            return Err(ToolRegistryError::EmptyName);
        }
        self.definitions.insert(name.clone(), definition);
        self.handlers.insert(name, Arc::new(handler));
        Ok(())
    }

    pub fn list(&self) -> Vec<&ToolDefinition> {
        self.definitions.values().collect()
    }

    pub fn list_public(&self) -> Vec<&ToolDefinition> {
        self.definitions
            .values()
            .filter(|definition| !definition.internal)
            .collect()
    }

    pub async fn execute<E>(
        &self,
        name: &str,
        input: Value,
        permissions: &E,
    ) -> Result<Value, ToolExecutionError>
    where
        E: PermissionEvaluator + ?Sized,
    {
        self.execute_with_start(name, input, permissions, || {})
            .await
    }

    pub async fn execute_with_start<E, F>(
        &self,
        name: &str,
        input: Value,
        permissions: &E,
        on_execution_start: F,
    ) -> Result<Value, ToolExecutionError>
    where
        E: PermissionEvaluator + ?Sized,
        F: FnOnce(),
    {
        let definition = self
            .definitions
            .get(name)
            .ok_or_else(|| ToolExecutionError::NotFound(name.to_string()))?;

        match permissions.evaluate(definition, &input) {
            PermissionDecision::Allow => {}
            PermissionDecision::Deny { reason } => {
                return Err(ToolExecutionError::PermissionDenied { reason });
            }
            PermissionDecision::Ask { request_id } => {
                return Err(ToolExecutionError::PermissionRequired { request_id });
            }
        }

        let handler = self
            .handlers
            .get(name)
            .ok_or_else(|| ToolExecutionError::NoHandler(name.to_string()))?;
        on_execution_start();
        handler
            .execute(input)
            .await
            .map_err(ToolExecutionError::Handler)
    }
}

#[cfg(test)]
mod tests {
    use super::{ToolDefinition, ToolHandler, ToolRegistry, ROUTING_TARGET_ARGUMENT};
    use crate::AllowPermission;
    use async_trait::async_trait;
    use serde_json::{json, Value};

    struct Echo;

    #[async_trait]
    impl ToolHandler for Echo {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("echo", "echo input", json!({ "type": "object" })).read_only()
        }

        async fn execute(&self, input: Value) -> Result<Value, String> {
            Ok(input)
        }
    }

    #[tokio::test]
    async fn aliases_preserve_metadata_and_delegate_execution() {
        let mut source = ToolRegistry::new();
        source.register_handler(Echo).expect("source tool");
        let mut combined = source.clone();
        combined
            .register_alias_from("windows__echo", &source, "echo", "[Windows]")
            .expect("alias");

        let alias = combined.get("windows__echo").expect("alias definition");
        assert!(alias.read_only);
        assert!(alias.description.starts_with("[Windows]"));
        assert_eq!(
            combined
                .execute("windows__echo", json!({ "value": 7 }), &AllowPermission)
                .await
                .expect("alias execution"),
            json!({ "value": 7 })
        );
    }

    struct Tagged {
        tag: &'static str,
    }

    #[async_trait]
    impl ToolHandler for Tagged {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new(
                "echo",
                "echo input",
                json!({ "type": "object", "properties": {}, "additionalProperties": false }),
            )
            .read_only()
        }

        async fn execute(&self, input: Value) -> Result<Value, String> {
            Ok(json!({ "tag": self.tag, "input": input }))
        }
    }

    fn tagged_registry(tag: &'static str) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry
            .register_handler(Tagged { tag })
            .expect("tagged tool");
        registry
    }

    #[tokio::test]
    async fn routed_aliases_dispatch_by_target_and_strip_the_argument() {
        let primary = tagged_registry("primary");
        let host_a = tagged_registry("host-a");
        let host_b = tagged_registry("ssh-b");
        let mut combined = primary.clone();
        combined
            .register_routed_alias(
                "ssh__echo",
                (&primary, "echo"),
                &[
                    ("host-a".to_string(), &host_a),
                    ("ssh-b".to_string(), &host_b),
                ],
                "[SSH]",
                "可选 target 参数用于点名本对话绑定的其他 SSH 主机。",
            )
            .expect("routed alias");

        let definition = combined.get("ssh__echo").expect("alias definition");
        assert!(definition.read_only);
        assert!(definition.description.starts_with("[SSH]"));
        assert!(definition.description.contains("target"));
        assert_eq!(
            definition.input_schema["properties"][ROUTING_TARGET_ARGUMENT]["enum"],
            json!(["host-a", "ssh-b"])
        );
        assert_eq!(
            definition.input_schema["additionalProperties"],
            json!(false)
        );

        assert_eq!(
            combined
                .execute("ssh__echo", json!({ "value": 1 }), &AllowPermission)
                .await
                .expect("primary execution"),
            json!({ "tag": "primary", "input": { "value": 1 } })
        );
        assert_eq!(
            combined
                .execute(
                    "ssh__echo",
                    json!({ "target": "HOST-A", "value": 2 }),
                    &AllowPermission,
                )
                .await
                .expect("routed execution"),
            json!({ "tag": "host-a", "input": { "value": 2 } })
        );

        let error = combined
            .execute(
                "ssh__echo",
                json!({ "target": "missing" }),
                &AllowPermission,
            )
            .await
            .expect_err("unknown target must fail");
        assert!(format!("{error}").contains("未绑定"), "{error}");
    }

    #[tokio::test]
    async fn routed_aliases_fall_back_to_plain_aliases_without_ready_hosts() {
        let primary = tagged_registry("primary");
        let mut combined = primary.clone();
        combined
            .register_routed_alias(
                "ssh__echo",
                (&primary, "echo"),
                &[],
                "[SSH]",
                "可选 target 参数用于点名本对话绑定的其他 SSH 主机。",
            )
            .expect("routed alias");

        let definition = combined.get("ssh__echo").expect("alias definition");
        assert!(definition.input_schema["properties"]
            .get(ROUTING_TARGET_ARGUMENT)
            .is_none());
        assert_eq!(
            combined
                .execute("ssh__echo", json!({ "value": 3 }), &AllowPermission)
                .await
                .expect("execution"),
            json!({ "tag": "primary", "input": { "value": 3 } })
        );
    }
}
