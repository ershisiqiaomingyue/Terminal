use super::super::context::{LocalToolContext, MAX_SEARCH_RESULTS};
use super::super::utilities::{is_allowed_tool_path, selected_tool_path};
use globset::GlobBuilder;
use regex::RegexBuilder;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

/// Shared argument parsing so both backends agree on semantics.
pub(in crate::local_tools::search) struct SearchSpec {
    pub(super) query: String,
    pub(super) root: PathBuf,
    pub(super) boundary: PathBuf,
    pub(super) pattern: String,
    pub(super) case_insensitive: bool,
    pub(super) include: Vec<String>,
    pub(super) offset: usize,
    pub(super) limit: usize,
    pub(super) code: bool,
    /// Collapse matches to one entry per file instead of one per line.
    pub(super) files_only: bool,
}

pub(super) fn parse_spec(
    context: &LocalToolContext,
    input: &Value,
    code: bool,
) -> Result<SearchSpec, String> {
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or("检索需要非空 query")?
        .to_string();
    let root = selected_tool_path(&input, &context.roots, context.allow_external_paths)?;
    if !is_allowed_tool_path(
        &root,
        &context.roots,
        &context.excluded_paths,
        context.allow_external_paths,
    ) {
        return Err("只能搜索用户明确选择的目录".into());
    }
    let boundary = fs::canonicalize(&root).map_err(|error| format!("无法读取检索路径：{error}"))?;
    let pattern = if input["regex"].as_bool().unwrap_or(false) {
        query.clone()
    } else {
        regex::escape(&query)
    };
    let case_insensitive = !input["case_sensitive"].as_bool().unwrap_or(false);
    // Validate the expression here so both backends fail identically and the
    // message never depends on ripgrep's wording.
    RegexBuilder::new(&pattern)
        .case_insensitive(case_insensitive)
        .size_limit(2 * 1024 * 1024)
        .build()
        .map_err(|error| format!("检索表达式无效：{error}"))?;
    let include = match input.get("include") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(patterns)) => {
            let mut values = Vec::new();
            for pattern in patterns {
                let pattern = pattern.as_str().ok_or("include 必须是 glob 字符串数组")?;
                GlobBuilder::new(pattern)
                    .literal_separator(true)
                    .build()
                    .map_err(|e| format!("include glob 无效：{e}"))?;
                values.push(pattern.to_string());
            }
            values
        }
        _ => return Err("include 必须是 glob 字符串数组".into()),
    };
    Ok(SearchSpec {
        query,
        root,
        boundary,
        pattern,
        case_insensitive,
        include,
        offset: input["offset"].as_u64().unwrap_or(0).min(usize::MAX as u64) as usize,
        limit: input["max_results"]
            .as_u64()
            .unwrap_or(64)
            .clamp(1, MAX_SEARCH_RESULTS as u64) as usize,
        code,
        files_only: input["files_only"].as_bool().unwrap_or(false),
    })
}
