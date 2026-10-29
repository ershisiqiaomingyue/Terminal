use super::super::budget::estimate_text_tokens;
use super::consts::{CONTEXT_SUMMARY_PREFIX, LEGACY_CONTEXT_SUMMARY_PREFIX};
use serde_json::Value;
use std::collections::HashSet;

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

pub(super) fn truncate_text_to_token_budget(
    value: &str,
    max_tokens: usize,
    marker: &str,
) -> String {
    if max_tokens == 0 {
        return String::new();
    }
    if estimate_text_tokens(value) <= max_tokens {
        return value.to_string();
    }
    let marker = if estimate_text_tokens(marker) <= max_tokens {
        marker
    } else {
        ""
    };
    let character_count = value.chars().count();
    let mut low = 0usize;
    let mut high = character_count;
    let mut best = marker.to_string();
    while low <= high {
        let middle = low + (high - low) / 2;
        let mut candidate = truncate_text(value, middle);
        candidate.push_str(marker);
        if estimate_text_tokens(&candidate) <= max_tokens {
            best = candidate;
            low = middle.saturating_add(1);
        } else if middle == 0 {
            break;
        } else {
            high = middle - 1;
        }
    }
    best
}

fn semantic_segments(value: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut seen = HashSet::new();
    for line in value.lines() {
        let line = line.trim();
        if line.is_empty()
            || line == CONTEXT_SUMMARY_PREFIX.trim()
            || line == LEGACY_CONTEXT_SUMMARY_PREFIX.trim()
        {
            continue;
        }
        let pieces = if line.chars().count() > 240 {
            line.split_inclusive(|character| {
                matches!(character, '。' | '！' | '？' | '；' | '.' | '!' | '?' | ';')
            })
            .collect::<Vec<_>>()
        } else {
            vec![line]
        };
        for piece in pieces {
            let normalized = piece.split_whitespace().collect::<Vec<_>>().join(" ");
            if !normalized.is_empty() && seen.insert(normalized.clone()) {
                segments.push(normalized);
            }
        }
    }
    segments
}

fn is_constraint_segment(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    [
        "必须",
        "不要",
        "不能",
        "禁止",
        "只允许",
        "需要",
        "应当",
        "务必",
        "待办",
        "未完成",
        "must",
        "must not",
        "never",
        "do not",
        "don't",
        "only",
        "required",
        "todo",
        "pending",
        "need to",
        "should",
    ]
    .iter()
    .any(|keyword| value.contains(keyword))
}

pub(super) fn semantic_excerpt(value: &str, max_tokens: usize, prefer_constraints: bool) -> String {
    let segments = semantic_segments(value);
    if segments.is_empty() {
        return truncate_text_to_token_budget(value.trim(), max_tokens, "…");
    }
    let mut selected = Vec::new();
    selected.push(0usize);
    if prefer_constraints {
        selected.extend(
            segments
                .iter()
                .enumerate()
                .filter_map(|(index, segment)| is_constraint_segment(segment).then_some(index)),
        );
    }
    if segments.len() > 1 {
        selected.push(segments.len() - 1);
    }
    selected.sort_unstable();
    selected.dedup();
    let excerpt = selected
        .into_iter()
        .filter_map(|index| segments.get(index))
        .cloned()
        .collect::<Vec<_>>()
        .join(" / ");
    truncate_text_to_token_budget(&excerpt, max_tokens, "…")
}

fn collect_json_evidence(value: &Value, prefix: &str, output: &mut Vec<String>, depth: usize) {
    if depth > 3 || output.len() >= 12 {
        return;
    }
    match value {
        Value::Object(object) => {
            const EVIDENCE_KEYS: &[&str] = &[
                "source",
                "path",
                "file",
                "url",
                "command",
                "cwd",
                "query",
                "status",
                "success",
                "exitCode",
                "exit_code",
                "error",
                "message",
                "target",
                "backend",
            ];
            for key in EVIDENCE_KEYS {
                let Some(child) = object.get(*key) else {
                    continue;
                };
                let path = if prefix.is_empty() {
                    (*key).to_string()
                } else {
                    format!("{prefix}.{key}")
                };
                if child.is_object() || child.is_array() {
                    collect_json_evidence(child, &path, output, depth + 1);
                } else {
                    output.push(format!("{path}={child}"));
                }
                if output.len() >= 12 {
                    return;
                }
            }
            for (key, child) in object {
                if EVIDENCE_KEYS.contains(&key.as_str()) {
                    continue;
                }
                let path = if prefix.is_empty() {
                    key.to_string()
                } else {
                    format!("{prefix}.{key}")
                };
                collect_json_evidence(child, &path, output, depth + 1);
                if output.len() >= 12 {
                    return;
                }
            }
        }
        Value::Array(values) => {
            for (index, child) in values.iter().take(3).enumerate() {
                collect_json_evidence(child, &format!("{prefix}[{index}]"), output, depth + 1);
            }
        }
        _ if !prefix.is_empty() => output.push(format!("{prefix}={value}")),
        _ => {}
    }
}

pub(super) fn structured_evidence_excerpt(value: &str, max_tokens: usize) -> String {
    if let Ok(json) = serde_json::from_str::<Value>(value) {
        let mut fields = Vec::new();
        collect_json_evidence(&json, "", &mut fields, 0);
        if !fields.is_empty() {
            return truncate_text_to_token_budget(&fields.join("; "), max_tokens, "…");
        }
    }
    semantic_excerpt(value, max_tokens, false)
}
