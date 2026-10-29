use super::super::*;

/// Summary output has no exact known total: report actual characters, and a
/// conservative estimated percentage (characters / summary budget, clamped
/// to 95%) so the frontend can drive a determinate track while running.
///
/// `compaction_progress` is omitted until there is something to measure: the
/// opening frame (no characters yet) plus a budget of zero both have no honest
/// percentage, and sending 0 would render a determinate 0% bar instead of the
/// frontend's indeterminate sweep.
#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_compaction_progress(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    compaction_id: &str,
    summary_characters: usize,
    processed_messages: usize,
    before_context_tokens: usize,
    summary_budget_characters: usize,
) {
    emit_agent_context_for_turn(
        app,
        compaction_progress_payload(
            session_id,
            compaction_id,
            summary_characters,
            processed_messages,
            before_context_tokens,
            summary_budget_characters,
        ),
        Some(turn_id),
    );
}

/// The running-compaction payload. Split out of the emit above so the rule
/// above (`compaction_progress` only once it can be measured) stays testable
/// without an `AppHandle`.
pub(crate) fn compaction_progress_payload(
    session_id: &str,
    compaction_id: &str,
    summary_characters: usize,
    processed_messages: usize,
    before_context_tokens: usize,
    summary_budget_characters: usize,
) -> Value {
    let mut payload = json!({
        "session_id": session_id, "compaction_id": compaction_id,
        "compaction_status": "running", "compaction_summary_chars": summary_characters,
        "compaction_processed_messages": processed_messages,
        "before_context_tokens": before_context_tokens,
    });
    if summary_characters > 0 && summary_budget_characters > 0 {
        let estimated = ((summary_characters as f64 / summary_budget_characters as f64) * 95.0)
            .round() as usize;
        if let Value::Object(object) = &mut payload {
            object.insert("compaction_progress".to_string(), json!(estimated.min(95)));
        }
    }
    payload
}
