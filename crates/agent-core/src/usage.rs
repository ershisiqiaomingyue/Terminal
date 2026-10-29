use crate::{ProviderCapabilities, ProviderKind};
use serde::{Deserialize, Serialize};

/// Provider counters for one model request, separate from turn totals.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestCacheUsage {
    pub provider_kind: Option<ProviderKind>,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
}

impl RequestCacheUsage {
    pub fn from_usage(kind: ProviderKind, usage: Option<&AgentUsage>) -> Self {
        Self {
            provider_kind: Some(kind),
            input_tokens: usage.and_then(|usage| usage.input_tokens),
            cached_input_tokens: usage.and_then(|usage| usage.cached_input_tokens),
            cache_creation_input_tokens: usage.and_then(|usage| usage.cache_creation_input_tokens),
        }
    }
}

/// Average output rate pairs reported tokens with full model request time.
/// Include waiting/buffered reasoning before visible output, but exclude tools.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GenerationMetrics {
    pub tokens: u64,
    pub duration_ms: u64,
}

impl GenerationMetrics {
    pub fn with_request(mut self, reported_tokens: Option<u64>, duration_ms: u64) -> Self {
        // An unsampled request must not skew either side of the ratio.
        if let Some(tokens) = reported_tokens.filter(|_| duration_ms > 0) {
            self.tokens = self.tokens.saturating_add(tokens);
            self.duration_ms = self.duration_ms.saturating_add(duration_ms);
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentUsage {
    #[serde(rename = "inputTokens")]
    pub input_tokens: Option<u64>,
    #[serde(rename = "outputTokens")]
    pub output_tokens: Option<u64>,
    #[serde(
        rename = "generationTokens",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub generation_tokens: Option<u64>,
    #[serde(
        rename = "generationDurationMs",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub generation_duration_ms: Option<u64>,
    #[serde(
        rename = "generationTimingSource",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub generation_timing_source: Option<String>,
    #[serde(rename = "cachedInputTokens")]
    pub cached_input_tokens: Option<u64>,
    #[serde(rename = "cacheCreationInputTokens")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(rename = "cacheStatsSource", skip_serializing_if = "Option::is_none")]
    pub cache_stats_source: Option<String>,
    #[serde(rename = "cacheStats", skip_serializing_if = "Option::is_none")]
    pub cache_stats: Option<AgentCacheStats>,
    #[serde(
        rename = "latestRequestUsage",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub latest_request_usage: Option<RequestCacheUsage>,
    #[serde(
        rename = "providerKind",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub provider_kind: Option<ProviderKind>,
    #[serde(rename = "contextTokens")]
    pub context_tokens: Option<u64>,
    #[serde(rename = "contextWindow")]
    pub context_window: Option<u64>,
    #[serde(rename = "contextWindowEstimated", default)]
    pub context_window_estimated: bool,
    #[serde(rename = "reservedOutputTokens", default)]
    pub reserved_output_tokens: Option<u64>,
    #[serde(rename = "availableTokens", default)]
    pub available_tokens: Option<u64>,
    #[serde(rename = "inputCost")]
    pub input_cost: Option<f64>,
    #[serde(rename = "outputCost")]
    pub output_cost: Option<f64>,
    #[serde(rename = "totalCost")]
    pub total_cost: Option<f64>,
    pub currency: Option<String>,
    /// Provider-reported reasoning/thinking tokens. Missing means the provider
    /// never reported the counter; it is never inferred from other counts.
    #[serde(
        rename = "reasoningTokens",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub reasoning_tokens: Option<u64>,
    /// P9-E：本轮观测发生在压缩改写请求前缀之后，缓存读数从这里重新起算。
    /// 瞬态标记——参与一次合并后归位，不落盘。
    #[serde(default, skip_serializing)]
    pub cache_baseline_reset: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCacheStats {
    pub source: String,
    #[serde(rename = "inputTokens")]
    pub input_tokens: Option<u64>,
    #[serde(rename = "cachedInputTokens")]
    pub cached_input_tokens: Option<u64>,
    #[serde(rename = "cacheCreationInputTokens")]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(rename = "hitRate")]
    pub hit_rate: Option<f64>,
    #[serde(rename = "providerKind")]
    pub provider_kind: Option<ProviderKind>,
    #[serde(rename = "modelId")]
    pub model_id: Option<String>,
    #[serde(rename = "requestCount")]
    pub request_count: Option<u64>,
    /// P9-E：本轮观测发生在压缩改写了请求前缀之后，缓存读数从这里重新起算。
    /// 只影响本轮合并，落盘后归位为 false。
    #[serde(
        rename = "cacheBaselineReset",
        default,
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub cache_baseline_reset: bool,
}

fn add_optional(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.saturating_add(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

pub fn merge_usage(total: &mut Option<AgentUsage>, next: AgentUsage) {
    let Some(current) = total.as_mut() else {
        *total = Some(next);
        return;
    };
    current.input_tokens = add_optional(current.input_tokens, next.input_tokens);
    current.output_tokens = add_optional(current.output_tokens, next.output_tokens);
    if next.generation_tokens.is_some() || next.generation_duration_ms.is_some() {
        if current.generation_tokens.is_none() && current.generation_duration_ms.is_none() {
            current.generation_timing_source = next.generation_timing_source.clone();
        } else if current.generation_timing_source != next.generation_timing_source {
            current.generation_timing_source = None;
        }
    }
    current.generation_tokens = add_optional(current.generation_tokens, next.generation_tokens);
    current.generation_duration_ms =
        add_optional(current.generation_duration_ms, next.generation_duration_ms);
    current.cached_input_tokens =
        add_optional(current.cached_input_tokens, next.cached_input_tokens);
    current.cache_creation_input_tokens = add_optional(
        current.cache_creation_input_tokens,
        next.cache_creation_input_tokens,
    );
    current.reasoning_tokens = add_optional(current.reasoning_tokens, next.reasoning_tokens);
    current.provider_kind = next.provider_kind.or(current.provider_kind);
    if next.latest_request_usage.is_some() {
        current.latest_request_usage = next.latest_request_usage;
    }
    current.cache_stats_source = match (current.cache_stats_source.take(), next.cache_stats_source)
    {
        (Some(current), Some(next)) => Some(merge_cache_source(&current, &next)),
        (current, next) => next.or(current),
    };
    current.cache_stats = match (current.cache_stats.take(), next.cache_stats) {
        (Some(current), Some(next)) => Some(merge_cache_stats(current, next)),
        // P9-E：压缩后的第一轮即使没带读数，也要把压缩前的旧基线丢掉——
        // 否则下一轮会拿旧前缀的命中率当成本轮前缀的基线。
        (Some(_current), None) if next.cache_baseline_reset => None,
        (current, next) => next.or(current),
    };
    current.context_tokens = next.context_tokens.or(current.context_tokens);
    current.context_window = next.context_window.or(current.context_window);
    current.context_window_estimated =
        current.context_window_estimated || next.context_window_estimated;
    current.reserved_output_tokens = next
        .reserved_output_tokens
        .or(current.reserved_output_tokens);
    current.available_tokens = next.available_tokens.or(current.available_tokens);
    current.input_cost = next.input_cost.or(current.input_cost);
    current.output_cost = next.output_cost.or(current.output_cost);
    current.total_cost = next.total_cost.or(current.total_cost);
    current.currency = next.currency.or(current.currency.take());
}

fn cache_source_rank(source: &str) -> u8 {
    match source {
        "provider" => 2,
        "unsupported" => 1,
        "unreported" => 0,
        _ => 1,
    }
}

fn merge_cache_source(current: &str, next: &str) -> String {
    if cache_source_rank(next) >= cache_source_rank(current) {
        next.to_string()
    } else {
        current.to_string()
    }
}

fn merge_cache_stats(current: AgentCacheStats, next: AgentCacheStats) -> AgentCacheStats {
    // P9-E：压缩改写了请求前缀，压缩之前的缓存读数对「当前前缀的命中率」没有意义，
    // 继续累加会让命中率被压缩前的旧数据长期稀释。本轮带 `cache_baseline_reset`
    // 标记时从这块读数重新起算（顶部 input_tokens 等累计值不动，成本口径不受影响）。
    let previous = if next.cache_baseline_reset {
        None
    } else {
        Some(current)
    };
    let (
        input_tokens,
        cached_input_tokens,
        cache_creation_input_tokens,
        request_count,
        source,
        provider_kind,
        model_id,
    ) = match previous {
        Some(previous) => (
            add_optional(previous.input_tokens, next.input_tokens),
            add_optional(previous.cached_input_tokens, next.cached_input_tokens),
            add_optional(
                previous.cache_creation_input_tokens,
                next.cache_creation_input_tokens,
            ),
            add_optional(previous.request_count, next.request_count),
            merge_cache_source(&previous.source, &next.source),
            next.provider_kind.or(previous.provider_kind),
            next.model_id.or(previous.model_id),
        ),
        None => (
            next.input_tokens,
            next.cached_input_tokens,
            next.cache_creation_input_tokens,
            next.request_count,
            next.source.clone(),
            next.provider_kind,
            next.model_id,
        ),
    };
    let hit_rate = provider_kind
        .and_then(|kind| {
            ProviderCapabilities::for_kind(kind).cache_hit_rate(
                input_tokens,
                cached_input_tokens,
                cache_creation_input_tokens,
            )
        })
        .or(next.hit_rate);
    AgentCacheStats {
        source,
        input_tokens,
        cached_input_tokens,
        cache_creation_input_tokens,
        hit_rate,
        provider_kind,
        model_id,
        request_count,
        cache_baseline_reset: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_metrics_sum_only_measured_requests() {
        let metrics = GenerationMetrics::default()
            .with_request(Some(100), 2000)
            .with_request(Some(200), 3000);
        assert_eq!(metrics.tokens, 300);
        assert_eq!(metrics.duration_ms, 5000);
        assert_eq!(metrics.with_request(None, 1000), metrics);
        assert_eq!(metrics.with_request(Some(50), 0), metrics);
        let zero_output = metrics.with_request(Some(0), 1000);
        assert_eq!(zero_output.tokens, 300);
        assert_eq!(zero_output.duration_ms, 6000);
    }

    #[test]
    fn buffered_output_counts_waiting_before_the_visible_burst() {
        let metrics = GenerationMetrics::default().with_request(Some(2020), 20_000);
        assert_eq!(metrics.tokens * 1000 / metrics.duration_ms, 101);
    }

    #[test]
    fn mixed_legacy_and_request_metrics_cannot_claim_reliable_request_timing() {
        let mut old = usage(None, None);
        old.generation_tokens = Some(100);
        old.generation_duration_ms = Some(1000);
        let mut next = usage(None, None);
        next.generation_tokens = Some(100);
        next.generation_duration_ms = Some(10_000);
        next.generation_timing_source = Some("request".to_string());
        let mut total = Some(old);
        merge_usage(&mut total, next);
        assert_eq!(total.unwrap().generation_timing_source, None);
    }

    fn usage(input_tokens: Option<u64>, cached_input_tokens: Option<u64>) -> AgentUsage {
        AgentUsage {
            input_tokens,
            output_tokens: Some(1),
            generation_tokens: None,
            generation_duration_ms: None,
            generation_timing_source: None,
            cached_input_tokens,
            cache_creation_input_tokens: None,
            cache_stats_source: None,
            cache_stats: None,
            latest_request_usage: None,
            provider_kind: None,
            context_tokens: None,
            context_window: None,
            context_window_estimated: false,
            reserved_output_tokens: None,
            available_tokens: None,
            input_cost: None,
            output_cost: None,
            total_cost: None,
            currency: None,
            reasoning_tokens: None,
            cache_baseline_reset: false,
        }
    }

    #[test]
    fn reasoning_tokens_accumulate_without_inventing_a_missing_report() {
        let mut first = usage(Some(100), None);
        first.reasoning_tokens = Some(40);
        let mut total = Some(first);
        // A round that never reported reasoning must not clear the total.
        merge_usage(&mut total, usage(Some(50), None));
        assert_eq!(total.as_ref().unwrap().reasoning_tokens, Some(40));
        let mut second = usage(Some(10), None);
        second.reasoning_tokens = Some(2);
        merge_usage(&mut total, second);
        assert_eq!(total.unwrap().reasoning_tokens, Some(42));
        // Absent reasoning is omitted from the persisted JSON instead of 0.
        let mut empty = Some(usage(None, None));
        merge_usage(&mut empty, usage(None, None));
        let json = serde_json::to_value(empty.unwrap()).unwrap();
        assert!(json.get("reasoningTokens").is_none());
    }

    #[test]
    fn merges_reported_usage_without_inventing_missing_values() {
        let mut total = Some(usage(Some(100), None));
        merge_usage(&mut total, usage(Some(50), Some(20)));
        let total = total.expect("usage");
        assert_eq!(total.input_tokens, Some(150));
        assert_eq!(total.output_tokens, Some(2));
        assert_eq!(total.cached_input_tokens, Some(20));
    }

    #[test]
    fn latest_request_cache_is_not_accumulated_or_reused_when_unreported() {
        let mut first = usage(Some(1000), Some(800));
        first.latest_request_usage = Some(RequestCacheUsage::from_usage(
            ProviderKind::OpenAi,
            Some(&first),
        ));
        let mut next = usage(Some(2000), Some(0));
        next.latest_request_usage = Some(RequestCacheUsage::from_usage(
            ProviderKind::OpenAi,
            Some(&next),
        ));
        let mut total = Some(first);
        merge_usage(&mut total, next);
        let saved: AgentUsage =
            serde_json::from_value(serde_json::to_value(total.as_ref().unwrap()).unwrap()).unwrap();
        assert_eq!(saved.input_tokens, Some(3000));
        assert_eq!(
            saved.latest_request_usage.as_ref().unwrap().input_tokens,
            Some(2000)
        );
        assert_eq!(
            saved
                .latest_request_usage
                .as_ref()
                .unwrap()
                .cached_input_tokens,
            Some(0)
        );
        let mut unreported = usage(None, None);
        unreported.latest_request_usage =
            Some(RequestCacheUsage::from_usage(ProviderKind::OpenAi, None));
        merge_usage(&mut total, unreported);
        let total = total.unwrap();
        assert_eq!(total.cached_input_tokens, Some(800));
        assert_eq!(
            total.latest_request_usage.unwrap().cached_input_tokens,
            None
        );
    }

    #[test]
    fn accumulates_round_usage_and_keeps_a_reported_cache_source() {
        let mut first = usage(Some(100), Some(80));
        first.cache_stats_source = Some("provider".to_string());
        first.cache_stats = Some(AgentCacheStats {
            source: "provider".to_string(),
            input_tokens: Some(100),
            cached_input_tokens: Some(80),
            cache_creation_input_tokens: None,
            hit_rate: Some(0.8),
            provider_kind: Some(ProviderKind::OpenAi),
            model_id: Some("gpt-4.1".to_string()),
            request_count: Some(1),
            cache_baseline_reset: false,
        });
        let mut second = usage(Some(200), None);
        second.cache_stats_source = Some("unreported".to_string());
        second.cache_stats = Some(AgentCacheStats {
            source: "unreported".to_string(),
            input_tokens: Some(200),
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            hit_rate: None,
            provider_kind: Some(ProviderKind::OpenAi),
            model_id: Some("gpt-4.1".to_string()),
            request_count: Some(1),
            cache_baseline_reset: false,
        });

        let mut total = Some(first);
        merge_usage(&mut total, second);
        let total = total.expect("usage");
        assert_eq!(total.input_tokens, Some(300));
        assert_eq!(total.cache_stats_source.as_deref(), Some("provider"));
        let cache = total.cache_stats.expect("cache stats");
        assert_eq!(cache.input_tokens, Some(300));
        assert_eq!(cache.cached_input_tokens, Some(80));
        assert_eq!(cache.request_count, Some(2));
        assert_eq!(cache.hit_rate, Some(80.0 / 300.0));
    }

    /// P9-E：压缩改写了请求前缀，之后的第一轮读数必须从零起算，
    /// 否则命中率会被压缩前的旧数据长期稀释（表现为「压完还是不高」）。
    #[test]
    fn compaction_resets_the_cache_baseline_before_counting_the_new_prefix() {
        let mut before = usage(Some(10_000), Some(9_000));
        before.cache_stats = Some(AgentCacheStats {
            source: "provider".to_string(),
            input_tokens: Some(10_000),
            cached_input_tokens: Some(9_000),
            cache_creation_input_tokens: None,
            hit_rate: Some(0.9),
            provider_kind: Some(ProviderKind::OpenAi),
            model_id: Some("gpt-4.1".to_string()),
            request_count: Some(4),
            cache_baseline_reset: false,
        });
        let mut total = Some(before);
        let mut after_compaction = usage(Some(1_000), Some(100));
        after_compaction.cache_stats = Some(AgentCacheStats {
            source: "provider".to_string(),
            input_tokens: Some(1_000),
            cached_input_tokens: Some(100),
            cache_creation_input_tokens: None,
            hit_rate: None,
            provider_kind: Some(ProviderKind::OpenAi),
            model_id: Some("gpt-4.1".to_string()),
            request_count: Some(1),
            cache_baseline_reset: true,
        });
        merge_usage(&mut total, after_compaction);
        let cache = total
            .as_ref()
            .unwrap()
            .cache_stats
            .clone()
            .expect("cache stats");
        // 旧基线不再参与：读数只剩压缩后这一轮。
        assert_eq!(cache.input_tokens, Some(1_000));
        assert_eq!(cache.cached_input_tokens, Some(100));
        assert_eq!(cache.request_count, Some(1));
        assert_eq!(cache.hit_rate, Some(0.1));
        assert!(
            !cache.cache_baseline_reset,
            "复位标记只生效一次，落盘后归位"
        );

        // 累计口径（成本）不受影响。
        assert_eq!(total.unwrap().input_tokens, Some(11_000));

        // 压缩后的第一轮没有读数时，旧基线也必须被丢掉，不能残留到下一轮。
        let mut total = Some(AgentUsage {
            cache_stats: Some(AgentCacheStats {
                source: "provider".to_string(),
                input_tokens: Some(10_000),
                cached_input_tokens: Some(9_000),
                cache_creation_input_tokens: None,
                hit_rate: Some(0.9),
                provider_kind: Some(ProviderKind::OpenAi),
                model_id: Some("gpt-4.1".to_string()),
                request_count: Some(4),
                cache_baseline_reset: false,
            }),
            ..usage(None, None)
        });
        let mut silent_round = usage(Some(500), None);
        silent_round.cache_baseline_reset = true;
        merge_usage(&mut total, silent_round);
        assert!(total.unwrap().cache_stats.is_none());
    }
}
