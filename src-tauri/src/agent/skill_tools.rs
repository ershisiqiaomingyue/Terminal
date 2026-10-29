use super::*;

/// Skills are application resources, independent of the selected execution host.
pub(super) struct LoadSkillTool {
    pub app: AppHandle,
    pub cancellation: Arc<AtomicBool>,
}

#[async_trait]
impl ToolHandler for LoadSkillTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new("load_skill", "Read an installed instruction Skill by its exact catalog id before following its workflow. This only loads SKILL.md; it does not execute scripts, change the execution environment or grant additional permissions.", json!({
            "type": "object", "properties": {"id": {"type": "string", "minLength": 1}}, "required": ["id"], "additionalProperties": false
        })).read_only()
    }

    async fn execute(&self, input: Value) -> Result<Value, String> {
        if self.cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".into());
        }
        let id = skill_id(&input)?.to_owned();
        let app = self.app.clone();
        let instructions =
            tauri::async_runtime::spawn_blocking(move || load_skill_instructions(&app, &[id]))
                .await
                .map_err(|error| format!("Skill 加载任务失败：{error}"))??;
        if self.cancellation.load(Ordering::Relaxed) {
            return Err("请求已取消".into());
        }
        let (id, content) = instructions.into_iter().next().ok_or("未找到技能说明")?;
        Ok(json!({"id": id, "instructions": content, "executed": false,
            "scope": "按当前用户任务使用技能说明；执行环境、可访问路径和权限保持当前设置。"}))
    }
}

fn skill_id(input: &Value) -> Result<&str, String> {
    input
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty() && id.len() <= 256)
        .ok_or_else(|| "需要提供 Skills 清单中的有效 id".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_loader_requires_catalog_id_instead_of_arbitrary_paths() {
        assert_eq!(
            skill_id(&json!({"id": " android-build "})).unwrap(),
            "android-build"
        );
        assert!(skill_id(&json!({"path": "D:/file.md"})).is_err());
        assert!(skill_id(&json!({"id": " "})).is_err());
    }

    #[test]
    fn automatic_selection_uses_metadata_and_requires_loading_before_execution() {
        let catalog: Vec<crate::skill_runtime::SkillView> = serde_json::from_value(json!([
            {"id":"build", "name":"构建", "description":"编译 Android", "repository":"skills", "path":"build", "runtime":"instruction", "dependencies":[], "status":"ready", "statusMessage":"可加载", "builtin":false},
            {"id":"python", "name":"脚本", "description":"分析日志", "repository":"skills", "path":"python", "runtime":"python", "dependencies":["python"], "status":"not_ready", "statusMessage":"尚未接入", "builtin":false}
        ])).unwrap();
        let prompt = skill_catalog_prompt(&catalog);
        assert!(prompt.contains("不要求用户输入斜杠"));
        assert!(prompt.contains("先调用 load_skill"));
        assert!(prompt.contains("/build：构建；编译 Android"));
        assert!(prompt.contains("/python：脚本；分析日志；运行类型：python；状态：not_ready"));
        assert!(prompt.contains("未就绪技能不能执行"));
        assert!(prompt.contains("如果没有 load_skill 工具"));
        assert!(prompt.contains("不要仅因应用名、项目名、目录名或旧历史"));
        assert!(prompt.contains("历史加载过的技能不自动成为本轮全局规则"));
    }
}
