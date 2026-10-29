//! G5-C：轮次预算与压缩触发线的计算，从 `turn.rs` 里拆出来。
//!
//! 这一块原本夹在 `run_agent_turn` 的轮次循环中间，只依赖少数几个入参和一个
//! 入参式的「是否触发压缩」事实，属于最容易低风险外移的部分：搬走之后仍然
//! 只有一个调用点，行为与顺序完全不变，但 `turn.rs` 的循环体不再被三四十行
//! 纯算术占住。

use super::*;

/// 一轮请求的预算视图（都是 token 数）。
pub(super) struct RoundBudget {
    /// 消息本身可用的预算（已扣掉图片预留与 runtime context）。
    pub context_budget: usize,
    /// 输出预留。
    pub output_reserve: usize,
    /// 请求发出前的消息估计量。
    pub before_tokens: usize,
    /// 每轮都随请求发送的固定开销：system prompt + 工具声明 + runtime context。
    pub request_overhead: usize,
    /// 实际生效的压缩触发线（已按 P8-C 校准值与 P8-E 滞回调整）。
    pub compaction_trigger: usize,
}

/// 计算本轮预算与压缩触发线。
///
/// `calibrated_charge_percent` 是 P8-C 的校准值（供应商计费 / 本地估计 × 100），
/// `hysteresis` 是 P8-E 上一次实际触发的位置；两者都由调用方从运行时状态里取，
/// 这个函数本身不碰任何全局状态，便于单独测试。
pub(super) fn plan_round_budget(
    context_window: Option<u32>,
    system_prompt: &str,
    request_tools: &[agent_core::ToolDefinition],
    messages: &[agent_core::AgentMessage],
    final_response_requested: bool,
    runtime_context_tokens: usize,
    image_token_reserve: usize,
    calibrated_charge_percent: Option<usize>,
    hysteresis: Option<agent_core::CompactionHysteresis>,
) -> RoundBudget {
    let context_budget = agent_context_budget(
        context_window,
        system_prompt,
        request_tools,
        final_response_requested,
    )
    .saturating_sub(image_token_reserve)
    .saturating_sub(runtime_context_tokens)
    .max(1);
    let output_reserve = agent_output_reserve(context_window, final_response_requested);
    let before_tokens =
        estimate_agent_request_tokens(messages, system_prompt, request_tools) + image_token_reserve;
    // The trigger above is a message-level number, but the user pays for the whole
    // request: system prompt, tool definitions and runtime context ride along on
    // every call. Discount that fixed overhead so the ceiling bounds what the
    // provider actually bills.
    let request_overhead = estimate_agent_request_tokens(&[], system_prompt, request_tools)
        .saturating_add(runtime_context_tokens)
        .saturating_add(image_token_reserve);
    let scaled = agent_core::compaction_trigger_tokens_scaled_with_overhead(
        context_budget,
        request_overhead,
        calibrated_charge_percent,
    );
    // P8-E：滞回——上一次实际触发的位置会把本轮触发线下移一段安全带，
    // 否则压缩后刚降到触发线以内的请求，下一轮多出一点工具输出就会再压一次。
    let compaction_trigger = hysteresis
        .map(|hysteresis| hysteresis.effective_trigger(scaled))
        .unwrap_or(scaled);
    RoundBudget {
        context_budget,
        output_reserve,
        before_tokens,
        request_overhead,
        compaction_trigger,
    }
}
