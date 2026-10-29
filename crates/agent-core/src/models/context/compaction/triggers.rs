//! Compaction trigger thresholds and checkpoint budgets.

pub(super) const RETAIN_RATIO_PERCENT: usize = 16;
/// Ceiling on the verbatim tail: without it a long-window model would re-send
/// nearly the whole transcript and compaction would stop controlling cost.
///
/// 24K is the deliberate floor of every compacted request: a smaller tail makes
/// the checkpoint carry work the model used to read verbatim.
pub(crate) const RETAIN_TOKEN_CEILING: usize = 24_000;
/// Floor keeping one whole exchange verbatim even under a tiny budget.
pub(crate) const RETAIN_MIN_MESSAGES: usize = 2;

/// Tokens of the message budget kept verbatim: `RETAIN_RATIO_PERCENT` of what
/// remains after the checkpoint budget, capped by `RETAIN_TOKEN_CEILING`.
pub(crate) fn retained_recent_tokens(message_budget: usize, checkpoint_budget: usize) -> usize {
    message_budget
        .saturating_sub(checkpoint_budget)
        .saturating_mul(RETAIN_RATIO_PERCENT)
        .saturating_div(100)
        .min(RETAIN_TOKEN_CEILING)
}

/// Share of the message budget at which compaction starts.
///
/// Waiting until the prompt no longer fits means every round re-sends almost
/// the whole window; summarizing at a share of the budget keeps each request
/// small instead of proportional to the window.
const COMPACTION_TRIGGER_PERCENT: usize = 80;
/// The whole-request ceiling for the compaction trigger
/// (see [`compaction_trigger_ceiling`]).
///
/// This is the number the user actually sees and pays for, so the ceiling is
/// expressed on it and then discounted by the fixed overhead a caller reports
/// (see [`compaction_trigger_tokens_with_overhead`]). A million-token window
/// would otherwise let `80%` of the budget grow the request to 800K before
/// anything is summarized.
///
/// Measured against persisted sessions (`sessions.usage.contextTokens` versus
/// the provider's reported `latestRequestUsage.input`) the estimator tracks the
/// provider within 1-5%, so the ceiling *is* the real peak request rather than an
/// optimistic guess; the drift correction in
/// [`compaction_trigger_tokens_scaled`] barely moves it while the two agree.
const COMPACTION_TRIGGER_TOTAL_TOKENS: usize = 80_000;

/// 触发点的**整个请求**上限：恒定 80K，再被窗口的 80% 兜住。
///
/// P8-A 曾把它按窗口分档放宽（≤128K→100K、≤256K→160K、≤512K→320K、更大→400K），
/// 理由是「大窗口还空着一半就压缩，要重放 checkpoint 并让 prompt cache 失效」。
/// 实测下来这个收益抵不过代价：1M 窗口要涨到 377K 才压，上下文里的细节被留到
/// 很晚才丢，而单次请求的绝对开销随峰值线性上涨。故收回为恒定档。
///
/// 随后又把恒定档从 100K 收到 80K：100K 扣掉固定开销后实际在 77K 起压，
/// 「**不超过八万**」这条线要由整请求峰值来兑现，而不是让消息行先顶到 77K、
/// 再由开销把请求抬到 100K。现在峰值就是 80K，消息行约 57K（80K − 23K 工具与
/// runtime 开销），压缩比之前早一步发生，早一步省下的是每轮都要重发的历史。
///
/// 返回的是**整个请求**（消息 + system + 工具定义 + runtime）的峰值，因此上界
/// 也受窗口本身约束，不会超过 80%。P8-E 的滞回（`CompactionHysteresis`）仍
/// 叠加在这个结果之上。
fn compaction_trigger_ceiling(max_tokens: usize) -> usize {
    COMPACTION_TRIGGER_TOTAL_TOKENS
        // 永远不许超过窗口的 80%——上限只放宽绝对峰值，不放宽相对安全带。
        .min(
            max_tokens
                .saturating_mul(COMPACTION_TRIGGER_PERCENT)
                .saturating_div(100),
        )
        .max(1)
}
/// Floor on the trigger once the fixed request overhead is discounted.
///
/// Tool definitions alone can cost 20K+ tokens; without a floor a session with a
/// very large tool set would compact down to nothing and every round would pay
/// for a summary it immediately outgrows.
pub(crate) const COMPACTION_TRIGGER_MIN_TOKENS: usize = 16_000;

/// Ceiling on the checkpoint's replayed payload, which is re-sent verbatim on
/// every following round until the next compaction.
pub(crate) const CHECKPOINT_TOKEN_CEILING: usize = 16_384;

/// Tokens of *messages* at which compaction starts: `COMPACTION_TRIGGER_PERCENT`
/// of the budget, capped by [`compaction_trigger_ceiling`] so the whole request
/// stays inside the window's tiered peak.
///
/// Callers that know the fixed request overhead (system prompt, tool
/// definitions, runtime context) should pass it to
/// [`compaction_trigger_tokens_with_overhead`] instead, so the ceiling applies
/// to what the provider bills rather than to the messages alone.
pub fn compaction_trigger_tokens(max_tokens: usize) -> usize {
    max_tokens
        .saturating_mul(COMPACTION_TRIGGER_PERCENT)
        .saturating_div(100)
        .min(compaction_trigger_ceiling(max_tokens))
        .max(1)
}

/// `compaction_trigger_tokens` with the request's fixed overhead discounted, so
/// the ceiling bounds the whole billed request and not just its messages.
pub fn compaction_trigger_tokens_with_overhead(max_tokens: usize, overhead_tokens: usize) -> usize {
    let ceiling = compaction_trigger_ceiling(max_tokens)
        .saturating_sub(overhead_tokens)
        .max(COMPACTION_TRIGGER_MIN_TOKENS);
    max_tokens
        .saturating_mul(COMPACTION_TRIGGER_PERCENT)
        .saturating_div(100)
        .min(ceiling)
        .max(1)
}

/// The same trigger, corrected when providers charge more than the local
/// estimate. The correction is clamped so one reported outlier cannot shrink it
/// to nothing.
pub fn compaction_trigger_tokens_scaled(
    max_tokens: usize,
    reported_percent: Option<usize>,
) -> usize {
    compaction_trigger_tokens_scaled_with_overhead(max_tokens, 0, reported_percent)
}

pub fn compaction_trigger_tokens_scaled_with_overhead(
    max_tokens: usize,
    overhead_tokens: usize,
    reported_percent: Option<usize>,
) -> usize {
    let base = compaction_trigger_tokens_with_overhead(max_tokens, overhead_tokens);
    match reported_percent {
        Some(percent) if percent > 100 => {
            let percent = percent.min(400);
            base.saturating_mul(100).saturating_div(percent).max(1)
        }
        _ => base,
    }
}
