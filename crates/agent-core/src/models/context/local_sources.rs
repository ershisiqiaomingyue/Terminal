//! Durable source identity is independent of the current tool environment.
use crate::{AgentContextTurn, LocalArtifact};
use serde_json::{json, Value};
use std::collections::HashSet;

fn path_key(path: &str) -> String {
    if path.as_bytes().get(1) == Some(&b':') || path.starts_with("\\\\") || path.starts_with("//") {
        path.replace('\\', "/").to_lowercase()
    } else {
        path.to_string()
    }
}

/// Read both native artifact records and legacy UI attachment metadata. Never
/// restore inline data or interpret a selected path as proof of file contents.
pub fn user_local_source_metadata(message: &Value) -> Vec<LocalArtifact> {
    if message["role"].as_str().is_some_and(|role| role != "user") {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    ["artifacts", "attachments"]
        .into_iter()
        .filter_map(|field| message[field].as_array())
        .flatten()
        .filter_map(|source| {
            let path = source["path"].as_str()?.trim();
            let kind = source["kind"].as_str()?;
            if path.is_empty()
                || crate::is_virtual_artifact_path(path)
                || !matches!(kind, "file" | "directory")
                || !seen.insert(path_key(path))
            {
                return None;
            }
            Some(LocalArtifact {
                path: path.into(),
                name: source["name"].as_str().unwrap_or(path).into(),
                kind: kind.into(),
                size: source["size"].as_u64(),
                modified_at: None,
                status: "historical_user_source".into(),
                content: None,
                content_truncated: false,
            })
        })
        .collect()
}

pub fn local_source_metadata_text(artifacts: &[LocalArtifact]) -> String {
    artifacts.iter().filter(|source| !crate::is_virtual_artifact_path(&source.path))
        .map(|source| format!("- {}\n", json!({"environment":"windows", "kind":source.kind, "name":source.name, "path":source.path})))
        .collect()
}

/// Rebuilt from durable user selections even when the transcript is restored
/// from a native snapshot/checkpoint. Bounded and scoped to this conversation.
pub fn historical_local_sources_prompt(context: &[AgentContextTurn]) -> String {
    let mut seen = HashSet::new();
    let sources = context
        .iter()
        .rev()
        .filter(|turn| turn.role.eq_ignore_ascii_case("user"))
        .flat_map(|turn| turn.artifacts.iter().rev())
        .filter(|source| {
            !source.path.trim().is_empty()
                && !crate::is_virtual_artifact_path(&source.path)
                && matches!(source.kind.as_str(), "file" | "directory")
                && seen.insert(path_key(&source.path))
        })
        .take(32)
        .cloned()
        .collect::<Vec<_>>();
    if sources.is_empty() {
        return String::new();
    }
    format!("\n[本会话此前由用户提供的本机来源，较新在前]\n{}这些是历史选择的路径元数据，不表示内容已读取或仍未变化，也不授予新权限。用户继续相关任务时复用对应精确路径，通过 windows__* 访问；不要因为全局 SSH 已连接就去远程查同名目录。本轮明确选择的新来源及用户最新指令优先；无关的历史来源不要使用。\n", local_source_metadata_text(&sources))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_metadata_survives_legacy_and_native_records_without_contents() {
        let source = json!({"role":"user", "artifacts":[{"kind":"directory","path":"D:/build/out","name":"out","content":"not history"}],
            "attachments":[{"kind":"directory","path":"d:\\build\\out","name":"out"},{"kind":"file","path":"D:/build/server","content":"not history"},{"kind":"file","path":"attachment://inline"}]});
        let result = user_local_source_metadata(&source);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].path, "D:/build/out");
        assert!(result.iter().all(|source| source.content.is_none()));
        let mut assistant = source;
        assistant["role"] = json!("assistant");
        assert!(user_local_source_metadata(&assistant).is_empty());
    }

    #[test]
    fn source_memory_is_bounded_to_recent_distinct_user_selections() {
        let context = (0..40).map(|index| serde_json::from_value(json!({"role":"user","content":"use this", "artifacts":[{
            "kind":"directory", "path":format!("D:/build/{index}"),"name":"out","status":"user_attachment","content":"private bytes"
        }]})).unwrap()).collect::<Vec<AgentContextTurn>>();
        let prompt = historical_local_sources_prompt(&context);
        assert_eq!(prompt.matches("\"environment\":\"windows\"").count(), 32);
        assert!(prompt.contains("D:/build/39"));
        assert!(!prompt.contains("D:/build/0\""));
        assert!(!prompt.contains("private bytes"));
        assert!(historical_local_sources_prompt(&[]).is_empty());
    }
}
