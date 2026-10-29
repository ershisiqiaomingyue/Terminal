    let mut tool_loop_progress = agent_core::tool_ledger::ToolLoopProgress::default();
    let mut termination_reason = "model_finished";
    let mut provider_context_retry_count = 0usize;
    let mut pending_provider_startup_error = None;
    let mut malformed_tool_signatures = HashSet::new();
    let mut tool_call_ledger = HashMap::<String, ToolCallLedgerEntry>::new();
    let mut dsml_stream = agent_core::ToolStreamState::default();
    let mut timeline_segment: Option<TimelineSegment>;
    let mut pending_ssh_connection_id: Option<String> = None;
    // Provider-reported input+output charge for the most recent request of this turn.
    // The local estimator undercounts real usage, so a reported over-window value is
    // authoritative for the window guard below; it is cleared as soon as a
    // reported-driven hard compaction has consumed it.
    let mut reported_request_tokens: Option<usize> = None;
    // At most one reported-usage hard compaction per turn, never a compaction loop.
    let mut reported_usage_compacted = false;
    // Provider count over the local estimate of the same request, times 100. The
    // estimator runs under the real charge on tool-heavy transcripts, so the
    // compaction trigger is scaled by this measured drift; `None` until a round
    // has reported usage.
    let mut context_charge_percent: Option<usize> = None;
    // P9-E：本轮之前压缩改写过请求前缀，缓存读数要从头起算（只置一次）。
