use serde_json::Value;

use crate::credentials::{inline_binary_data_url, MAX_INLINE_BINARY_DATA_URL_CHARS};
use crate::{
    effective_tool_policy, is_virtual_artifact_path, AgentChatRequest, AgentLogEvidence,
    AgentToolPolicy, AnalysisReport,
};

const MAX_INLINE_ARTIFACT_CHARS: usize = 64 * 1024;

fn truncate_text(value: &str, max_chars: usize) -> String {
    let length = value.chars().count();
    if length <= max_chars {
        return value.to_string();
    }
    if max_chars <= 1 {
        return "…".to_string();
    }
    let available = max_chars - 1;
    let head_length = (available * 3 / 5).max(1);
    let tail_length = available.saturating_sub(head_length);
    let head = value.chars().take(head_length).collect::<String>();
    let tail = value
        .chars()
        .skip(length.saturating_sub(tail_length))
        .collect::<String>();
    format!("{}…{}", head, tail)
}

fn truncate_data_url(value: &str, max_chars: usize) -> (String, bool) {
    if value.chars().count() <= max_chars {
        return (value.to_string(), false);
    }
    let Some((header, payload)) = value.split_once(',') else {
        return (truncate_text(value, max_chars), true);
    };
    let header_chars = header.chars().count();
    let payload_budget = max_chars.saturating_sub(header_chars + 1).saturating_sub(3);
    let payload_chars = payload_budget / 4 * 4;
    let prefix = payload.chars().take(payload_chars).collect::<String>();
    (format!("{header},{prefix}"), true)
}

pub fn compose_agent_prompt(request: &AgentChatRequest, evidence: &AgentLogEvidence) -> String {
    let mut prompt = request.message.trim().to_string();
    if let Some(jira) = request.jira_context.as_ref() {
        prompt.push_str("\n\n[Jira 背景]\n");
        prompt.push_str(&format!("{} {}\n", jira.key, jira.summary));
        if !jira.description.is_empty() {
            prompt.push_str(&jira.description);
            prompt.push('\n');
        }
        if let Some(status) = jira.status.as_deref() {
            prompt.push_str(&format!("状态: {status}\n"));
        }
    }
    let has_inline_browser_attachment = request.artifacts.iter().any(|artifact| {
        is_virtual_artifact_path(&artifact.path)
            && artifact
                .content
                .as_deref()
                .is_some_and(|content| !content.trim().is_empty())
    });
    if !request.artifacts.is_empty()
        && (effective_tool_policy(request) == AgentToolPolicy::Auto
            || has_inline_browser_attachment)
    {
        let has_workspace_source = request
            .artifacts
            .iter()
            .any(|artifact| artifact.kind == "directory");
        let has_log_source = request.artifacts.iter().any(|artifact| {
            artifact.kind != "directory"
                && (artifact.name.to_ascii_lowercase().contains("log")
                    || artifact.name.to_ascii_lowercase().ends_with(".txt")
                    || artifact.name.to_ascii_lowercase().ends_with(".trace")
                    || artifact.name.to_ascii_lowercase().ends_with(".jsonl"))
        });
        prompt.push_str("\n[已选择的本地来源元数据]\n");
        for artifact in &request.artifacts {
            let label = if artifact.kind == "directory" {
                "工作目录"
            } else if is_virtual_artifact_path(&artifact.path) {
                "浏览器附件"
            } else if has_log_source {
                "文件来源"
            } else {
                "本地文件"
            };
            prompt.push_str(&format!(
                "- {label}: {}\n  精确路径: {}\n  类型: {}，大小: {:?}\n",
                artifact.name, artifact.path, artifact.kind, artifact.size
            ));
            if let Some(data_url) = inline_binary_data_url(artifact) {
                let (bounded_data_url, data_url_truncated) =
                    truncate_data_url(data_url, MAX_INLINE_BINARY_DATA_URL_CHARS);
                prompt.push_str("  内嵌二进制内容（data URL，仅本轮可用，不是本地路径）:\n");
                prompt.push_str(&bounded_data_url);
                prompt.push('\n');
                if artifact.content_truncated || data_url_truncated {
                    prompt.push_str("  [二进制附件内容已截断，以上是可用前缀]\n");
                }
            } else if let Some(content) = artifact
                .content
                .as_deref()
                .filter(|content| !content.is_empty())
            {
                prompt.push_str("  内嵌内容（仅本轮可用，不是本地路径）:\n");
                prompt.push_str(&truncate_text(content, MAX_INLINE_ARTIFACT_CHARS));
                prompt.push('\n');
                if artifact.content_truncated {
                    prompt.push_str("  [文件内容已截断]\n");
                }
            } else if artifact.status == "read_failed" && is_virtual_artifact_path(&artifact.path) {
                prompt.push_str("  [浏览器附件读取失败，未能取得文件内容]\n");
            } else if is_virtual_artifact_path(&artifact.path) {
                prompt.push_str("  [浏览器附件没有可内嵌的文本内容，只能使用文件元数据]\n");
            }
        }
        let selected_source_paths = request
            .artifacts
            .iter()
            .filter(|artifact| artifact.kind != "directory" || artifact.status == "user_attachment")
            .map(|artifact| artifact.path.as_str())
            .collect::<Vec<_>>();
        if !selected_source_paths.is_empty() {
            prompt.push_str(
                "本轮用户明确选择的文件或目录优先于默认项目根和历史候选。涉及这些来源时，使用上面的精确本机路径和 windows__* 工具（若可用）；不要改去 SSH 或其他目录搜索同名来源。\n",
            );
        }
        if has_log_source {
            prompt.push_str(
                "日志或文件内容由 Rust 在本地按范围读取，未将完整文件或压缩包上传模型。\n",
            );
        }
        if has_workspace_source {
            prompt.push_str("当前工作区已作为 Agent 的受控访问根目录提供。用户明确要求检查工作区中的目录、文件、代码或权限问题时，应直接调用本地工具读取，不要回答‘用户没有提供文件’；工作区路径本身不代表已经读取全部内容。\n");
        } else if !has_log_source && !has_inline_browser_attachment {
            prompt.push_str("这里只提供用户明确选择的路径元数据；不要因为存在工作目录就主动扫描日志或目录，只有问题确实需要本地证据时才调用对应工具。\n");
        }
        if request
            .artifacts
            .iter()
            .any(|artifact| is_virtual_artifact_path(&artifact.path))
        {
            prompt.push_str("浏览器附件使用 attachment:// 虚拟引用，不要把它传给本地文件工具；如果存在内嵌内容，直接基于内嵌内容回答。\n");
        }
        prompt.push_str(
            "除非用户明确要求继续或比较历史问题，本轮不得把历史会话中与当前选择来源不一致的文件名、日志、工具结果或结论当作当前文件的证据。\n",
        );
        if !evidence.ranges.is_empty()
            || !evidence.search_hits.is_empty()
            || !evidence.timeline.is_empty()
        {
            prompt.push_str("\n[受控本地证据]\n");
        }
        for range in &evidence.ranges {
            prompt.push_str(&format!(
                "范围 {}:{}-{}\n{}\n",
                range.name, range.line_start, range.line_end, range.content
            ));
        }
        if !evidence.search_hits.is_empty() {
            prompt.push_str("关键词命中:\n");
            for hit in evidence.search_hits.iter().take(24) {
                prompt.push_str(&format!(
                    "- {}:{} [{}-{}] {}\n",
                    hit.name, hit.line_start, hit.byte_start, hit.byte_end, hit.excerpt
                ));
            }
        }
        if !evidence.timeline.is_empty() {
            prompt.push_str("时间线候选:\n");
            for event in evidence.timeline.iter().take(48) {
                prompt.push_str(&format!(
                    "- {}:{} {} {}\n",
                    event.name,
                    event.line,
                    event.timestamp.as_deref().unwrap_or("未知时间"),
                    event.summary
                ));
            }
        }
    }
    if request.analysis_mode {
        prompt.push_str(
            "\n[输出协议]\n只输出一个 JSON 对象，不要 Markdown 包裹，字段必须为：conclusion(string), suspectedCauses(string[]), evidence(string[]), timeline(string[]), nextSteps(string[])。每个数组最多 8 项。每个结论都要区分‘已验证’、‘在当前搜索范围未发现’和‘待验证’；负面检索不能写成全局不存在。如果关键证据不足，明确写出搜索范围、缺口和具体下一步，不要用泛化建议冒充结论。",
        );
    }
    prompt
}

pub fn parse_analysis_report(text: &str) -> Result<AnalysisReport, String> {
    let trimmed = text.trim();
    let candidate = if let Some(start) = trimmed.find("```") {
        let after_start = &trimmed[start + 3..];
        let after_language = after_start
            .strip_prefix("json")
            .or_else(|| after_start.strip_prefix("JSON"))
            .unwrap_or(after_start);
        let end = after_language
            .rfind("```")
            .ok_or_else(|| "模型报告包含未闭合的代码块".to_string())?;
        after_language[..end].trim()
    } else {
        trimmed
    };
    let mut report = candidate
        .match_indices('{')
        .rev()
        .find_map(|(index, _)| serde_json::from_str::<Value>(&candidate[index..]).ok())
        .and_then(normalize_analysis_report)
        .ok_or_else(|| "结构化报告解析失败：模型未返回可解析的 JSON 报告".to_string())?;
    report.conclusion = report.conclusion.trim().to_string();
    if report.conclusion.is_empty() {
        return Err("结构化报告缺少 conclusion".to_string());
    }
    report.suspected_causes.truncate(8);
    report.evidence.truncate(8);
    report.timeline.truncate(8);
    report.next_steps.truncate(8);
    Ok(report)
}

fn json_value_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_string())
        }
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Null => None,
        _ => serde_json::to_string(value).ok(),
    }
}

fn json_field_text(object: &serde_json::Map<String, Value>, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| object.get(*name).and_then(json_value_text))
}

fn json_field_list(object: &serde_json::Map<String, Value>, names: &[&str]) -> Vec<String> {
    names
        .iter()
        .find_map(|name| object.get(*name))
        .map(|value| match value {
            Value::Array(items) => items.iter().filter_map(json_value_text).collect(),
            Value::String(text) => text
                .lines()
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect(),
            _ => json_value_text(value).into_iter().collect(),
        })
        .unwrap_or_default()
}

fn normalize_analysis_report(value: Value) -> Option<AnalysisReport> {
    let object = value.as_object()?;
    let mut conclusion = json_field_text(
        object,
        &["conclusion", "summary", "answer", "result", "analysis"],
    )?;
    let suspected_causes = json_field_list(
        object,
        &[
            "suspectedCauses",
            "suspected_causes",
            "causes",
            "root_causes",
            "possible_causes",
        ],
    );
    let mut evidence = json_field_list(object, &["evidence", "key_evidence", "findings"]);
    let timeline = json_field_list(object, &["timeline", "events"]);
    let mut next_steps = json_field_list(
        object,
        &[
            "nextSteps",
            "next_steps",
            "recommendation",
            "recommendations",
            "actions",
        ],
    );

    if let Some(status) = object.get("current_status").and_then(Value::as_object) {
        let fields = ["file", "rules_count", "covered_denied", "coverage"];
        let status_text = fields
            .iter()
            .filter_map(|name| {
                status
                    .get(*name)
                    .and_then(json_value_text)
                    .map(|text| format!("{name}={text}"))
            })
            .collect::<Vec<_>>();
        if !status_text.is_empty() {
            evidence.push(format!("当前状态：{}", status_text.join("；")));
        }
    }
    if let Some(rules) = object.get("matched_rules").and_then(Value::as_array) {
        for rule in rules {
            if let Some(rule_object) = rule.as_object() {
                let target = json_field_text(rule_object, &["denied_target", "target", "name"]);
                let line = json_field_text(rule_object, &["rule_line", "line", "source"]);
                let text = match (target, line) {
                    (Some(target), Some(line)) => format!("{target}：{line}"),
                    (Some(target), None) => target,
                    (None, Some(line)) => line,
                    (None, None) => json_value_text(rule).unwrap_or_default(),
                };
                if !text.is_empty() {
                    evidence.push(text);
                }
            }
        }
    }
    if let Some(assessment) = object.get("severity_assessment").and_then(Value::as_object) {
        if let Some(risk) = json_field_text(assessment, &["current_risk", "risk", "severity"]) {
            conclusion = format!("{conclusion}（当前风险：{risk}）");
        }
        if let Some(reason) = json_field_text(assessment, &["reason", "basis"]) {
            evidence.push(format!("风险判断依据：{reason}"));
        }
        if next_steps.is_empty() {
            next_steps = json_field_list(
                assessment,
                &["recommendation", "recommendations", "next_steps"],
            );
        }
    }

    Some(AnalysisReport {
        conclusion,
        suspected_causes,
        evidence,
        timeline,
        next_steps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentToolPolicy, LocalArtifact};

    fn request_with_artifact(artifact: LocalArtifact) -> AgentChatRequest {
        AgentChatRequest {
            model_id: "test-model".to_string(),
            message: "分析这个附件".to_string(),
            jira_context: None,
            artifacts: vec![artifact],
            workspace_path: None,
            session_id: None,
            attachments: Vec::new(),
            turn_id: None,
            permission_mode: None,
            analysis_mode: false,
            context: Vec::new(),
            tool_policy: AgentToolPolicy::Auto,
            skill_ids: Vec::new(),
            requested_tool: None,
            reasoning_effort: None,
            task_plan_mode: None,
            task_plan_min_steps: None,
            execution_target: None,
            execution_targets: Vec::new(),
            collaboration_depth: None,
        }
    }

    #[test]
    fn embeds_browser_binary_content_in_prompt() {
        let request = request_with_artifact(LocalArtifact {
            path: "attachment://sample.bin:1:3".to_string(),
            name: "sample.bin".to_string(),
            kind: "file".to_string(),
            size: Some(3),
            modified_at: Some("1".to_string()),
            status: "inline_binary".to_string(),
            content: Some("data:application/octet-stream;base64,AQID".to_string()),
            content_truncated: false,
        });
        let prompt = compose_agent_prompt(&request, &AgentLogEvidence::default());
        assert!(prompt.contains("内嵌二进制内容"));
        assert!(prompt.contains("data:application/octet-stream;base64,AQID"));
        assert!(!prompt.contains("只能使用文件元数据"));
    }

    #[test]
    fn keeps_inline_browser_content_when_tools_are_disabled() {
        let mut request = request_with_artifact(LocalArtifact {
            path: "attachment://sample.bin:1:3".to_string(),
            name: "sample.bin".to_string(),
            kind: "file".to_string(),
            size: Some(3),
            modified_at: Some("1".to_string()),
            status: "inline_binary".to_string(),
            content: Some("data:application/octet-stream;base64,AQID".to_string()),
            content_truncated: false,
        });
        request.tool_policy = AgentToolPolicy::Disabled;
        let prompt = compose_agent_prompt(&request, &AgentLogEvidence::default());
        assert!(prompt.contains("data:application/octet-stream;base64,AQID"));
    }

    #[test]
    fn truncates_binary_data_url_on_base64_boundary() {
        let payload = "A".repeat(MAX_INLINE_BINARY_DATA_URL_CHARS);
        let data_url = format!("data:application/octet-stream;base64,{payload}");
        let request = request_with_artifact(LocalArtifact {
            path: "attachment://large.bin:1:100000".to_string(),
            name: "large.bin".to_string(),
            kind: "file".to_string(),
            size: Some(100_000),
            modified_at: Some("1".to_string()),
            status: "inline_binary".to_string(),
            content: Some(data_url),
            content_truncated: true,
        });
        let prompt = compose_agent_prompt(&request, &AgentLogEvidence::default());
        assert!(prompt.contains("二进制附件内容已截断"));
        let encoded = prompt
            .split("内嵌二进制内容（data URL，仅本轮可用，不是本地路径）:\n")
            .nth(1)
            .and_then(|value| value.lines().next())
            .expect("binary data URL should be present");
        let payload = encoded.split_once(',').expect("data URL header").1;
        assert_eq!(payload.len() % 4, 0);
    }
}
