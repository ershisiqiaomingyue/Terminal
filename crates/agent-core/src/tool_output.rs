//! Model-facing pruning of oversized tool results.
//!
//! Mirrors the tool-result pruner: one tool result larger than
//! `TOOL_RESULT_INLINE_LIMIT_CHARS` is collapsed to "head + marker + tail" as
//! soon as it becomes an `AgentMessage` for the model. Only the model-facing
//! copy is bounded: the complete output keeps flowing through the tool events
//! that the UI renders and that the session record persists, so the user never
//! loses the evidence the model is told was elided.

/// Tool results at or below this many characters reach the model verbatim.
pub const TOOL_RESULT_INLINE_LIMIT_CHARS: usize = 8 * 1024;

/// Characters kept from each end of an over-limit tool result.
pub const TOOL_RESULT_EDGE_CHARS: usize = 4 * 1024;

/// Inserted between the kept head and tail. Deliberately self-describing: the
/// model must know the middle is gone and where the complete text still lives,
/// otherwise it repeats the same oversized read instead of narrowing it.
pub const TOOL_RESULT_ELISION_MARKER: &str = "\n\n……[工具结果过长，中间内容已裁剪：这里只保留开头与结尾各 4096 字符；完整内容仍保留在会话记录中，可在界面查看。如果缺少的是关键部分，请用更小的范围（read_file 的 start_line/end_line 或 max_bytes、命令输出过滤）重新读取，不要重复同样的调用。]……\n\n";

/// Collapse an oversized tool result to its head and tail before it becomes a
/// model-facing message. Counts characters (not bytes) and never slices inside
/// a UTF-8 character. Idempotent: a result that already carries the marker is
/// returned unchanged, so replaying or re-pruning a transcript is a no-op.
pub fn prune_tool_result_for_model(output: &str) -> String {
    if output.contains(TOOL_RESULT_ELISION_MARKER) {
        return output.to_string();
    }
    let char_count = output.chars().count();
    if char_count <= TOOL_RESULT_INLINE_LIMIT_CHARS {
        return output.to_string();
    }
    let head_end = char_boundary(output, TOOL_RESULT_EDGE_CHARS).unwrap_or(output.len());
    let tail_start =
        char_boundary(output, char_count - TOOL_RESULT_EDGE_CHARS).unwrap_or(output.len());
    let mut pruned = String::with_capacity(
        head_end + TOOL_RESULT_ELISION_MARKER.len() + output.len().saturating_sub(tail_start),
    );
    pruned.push_str(&output[..head_end]);
    pruned.push_str(TOOL_RESULT_ELISION_MARKER);
    pruned.push_str(&output[tail_start..]);
    pruned
}

/// Byte index where character `char_offset` starts. `None` when the string is
/// shorter, which keeps every slice on a UTF-8 character boundary.
fn char_boundary(text: &str, char_offset: usize) -> Option<usize> {
    text.char_indices().nth(char_offset).map(|(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pruned_len() -> usize {
        TOOL_RESULT_EDGE_CHARS * 2 + TOOL_RESULT_ELISION_MARKER.chars().count()
    }

    #[test]
    fn short_output_is_returned_verbatim() {
        for short in ["", "ok", "3 files changed", "错误：路径不在允许范围内"] {
            assert_eq!(prune_tool_result_for_model(short), short);
        }
    }

    #[test]
    fn exactly_the_inline_limit_is_returned_verbatim() {
        let text = "a".repeat(TOOL_RESULT_INLINE_LIMIT_CHARS);
        let pruned = prune_tool_result_for_model(&text);
        assert_eq!(pruned, text);
        assert!(!pruned.contains(TOOL_RESULT_ELISION_MARKER));
    }

    #[test]
    fn one_character_over_the_limit_keeps_head_marker_and_tail() {
        let text = format!(
            "{}{}{}",
            "H".repeat(TOOL_RESULT_EDGE_CHARS),
            "M",
            "T".repeat(TOOL_RESULT_EDGE_CHARS)
        );
        assert_eq!(text.chars().count(), TOOL_RESULT_INLINE_LIMIT_CHARS + 1);
        let pruned = prune_tool_result_for_model(&text);
        assert_eq!(pruned.chars().count(), pruned_len());
        assert!(pruned.starts_with(&"H".repeat(TOOL_RESULT_EDGE_CHARS)));
        assert!(pruned.ends_with(&"T".repeat(TOOL_RESULT_EDGE_CHARS)));
        assert!(pruned.contains(TOOL_RESULT_ELISION_MARKER));
        // The dropped middle must not survive anywhere in the pruned result.
        assert!(!pruned.replace(TOOL_RESULT_ELISION_MARKER, "").contains('M'));
    }

    #[test]
    fn marker_tells_the_model_the_complete_result_still_exists() {
        let pruned = prune_tool_result_for_model(&"x".repeat(TOOL_RESULT_INLINE_LIMIT_CHARS + 1));
        assert!(pruned.contains("完整内容仍保留在会话记录中"));
        assert!(pruned.contains("已裁剪"));
    }

    #[test]
    fn pruning_is_idempotent() {
        let text = "x".repeat(TOOL_RESULT_INLINE_LIMIT_CHARS * 4);
        let once = prune_tool_result_for_model(&text);
        let twice = prune_tool_result_for_model(&once);
        assert_eq!(once, twice);
        assert_eq!(once.chars().count(), pruned_len());
    }

    #[test]
    fn multibyte_boundaries_never_panic_and_stay_on_character_edges() {
        let exact = "中".repeat(TOOL_RESULT_INLINE_LIMIT_CHARS);
        assert_eq!(prune_tool_result_for_model(&exact), exact);

        let over = "中".repeat(TOOL_RESULT_INLINE_LIMIT_CHARS + 1);
        let pruned = prune_tool_result_for_model(&over);
        assert_eq!(pruned.chars().count(), pruned_len());
        assert!(pruned.starts_with(&"中".repeat(TOOL_RESULT_EDGE_CHARS)));
        assert!(pruned.ends_with(&"中".repeat(TOOL_RESULT_EDGE_CHARS)));

        // Mixed 3-byte and 4-byte characters, with a one-byte prefix so the
        // 4096-character edges do not land on the string's own byte boundaries.
        let mixed = format!("a{}{}", "中".repeat(5_000), "😀".repeat(5_000));
        assert_eq!(mixed.chars().count(), 10_001);
        let pruned = prune_tool_result_for_model(&mixed);
        assert_eq!(pruned.chars().count(), pruned_len());
        assert!(pruned.starts_with(&format!("a{}", "中".repeat(TOOL_RESULT_EDGE_CHARS - 1))));
        assert!(pruned.ends_with(&"😀".repeat(TOOL_RESULT_EDGE_CHARS)));
        assert_eq!(prune_tool_result_for_model(&pruned), pruned);
    }
}
