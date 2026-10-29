use super::super::*;

pub(in crate::agent::turn) struct UsageFinalization<'a> {
    pub(in crate::agent::turn) protocol: ProviderKind,
    pub(in crate::agent::turn) provider: &'a AgentProviderSettings,
    pub(in crate::agent::turn) selected_model: &'a AgentModelSettings,
    pub(in crate::agent::turn) model_supports_usage: bool,
    pub(in crate::agent::turn) provider_capabilities: ProviderCapabilities,
    pub(in crate::agent::turn) context_window: Option<u32>,
    pub(in crate::agent::turn) context_window_estimated: bool,
    pub(in crate::agent::turn) final_response_requested: bool,
    pub(in crate::agent::turn) generation_metrics: &'a agent_core::GenerationMetrics,
    pub(in crate::agent::turn) last_context_tokens: &'a AtomicU64,
    pub(in crate::agent::turn) model_request_count: &'a std::cell::Cell<u64>,
    pub(in crate::agent::turn) cache_baseline_reset: bool,
}

/// Attach the turn's measured context, generation and provider cache figures
/// to the cumulative usage. Missing usage remains missing rather than zero.
pub(in crate::agent::turn) fn finalize_usage(
    mut usage: Option<AgentUsage>,
    input: UsageFinalization<'_>,
) -> Option<AgentUsage> {
    if usage.is_none() {
        usage = Some(AgentUsage {
            input_tokens: None,
            output_tokens: None,
            generation_tokens: None,
            generation_duration_ms: None,
            generation_timing_source: None,
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            cache_stats_source: None,
            cache_stats: None,
            latest_request_usage: Some(agent_core::RequestCacheUsage::from_usage(
                input.protocol,
                None,
            )),
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
            // A turn where the provider never reported usage has no reasoning counter;
            // it stays absent instead of being invented as 0.
            reasoning_tokens: None,
            cache_baseline_reset: false,
        });
    }
    if input.cache_baseline_reset {
        if let Some(total) = usage.as_mut() {
            total.cache_baseline_reset = true;
        }
    }
    if let Some(usage) = usage.as_mut() {
        if input.generation_metrics.duration_ms > 0 {
            usage.generation_tokens = Some(input.generation_metrics.tokens);
            usage.generation_duration_ms = Some(input.generation_metrics.duration_ms);
            usage.generation_timing_source = Some("request".to_string());
        }
        let provider_reported_cache =
            usage.cached_input_tokens.is_some() || usage.cache_creation_input_tokens.is_some();
        // A stable request prefix is not evidence of a provider's KV cache.
        // Only usage fields returned by the provider may contribute to this
        // metric; servers without usage capability therefore remain explicitly
        // unsupported instead of showing a fabricated hit rate.
        usage.provider_kind = Some(input.protocol);
        usage.context_tokens = Some(input.last_context_tokens.load(Ordering::Relaxed));
        usage.context_window = input.context_window.map(u64::from);
        usage.context_window_estimated = input.context_window_estimated;
        let reserved_output_tokens =
            agent_output_reserve(input.context_window, input.final_response_requested);
        usage.reserved_output_tokens = Some(reserved_output_tokens as u64);
        usage.available_tokens = Some(available_agent_context_tokens(
            input.context_window,
            input.last_context_tokens.load(Ordering::Relaxed) as usize,
            reserved_output_tokens,
        ) as u64);
        let input_tokens = usage.input_tokens.map(|tokens| {
            if input.protocol == ProviderKind::Anthropic {
                tokens
                    .saturating_add(usage.cached_input_tokens.unwrap_or(0))
                    .saturating_add(usage.cache_creation_input_tokens.unwrap_or(0))
            } else {
                tokens
            }
        });
        usage.input_cost = input
            .provider
            .input_price_per_million
            .zip(input_tokens)
            .map(|(price, tokens)| price * tokens as f64 / 1_000_000.0);
        usage.output_cost = input
            .provider
            .output_price_per_million
            .zip(usage.output_tokens)
            .map(|(price, tokens)| price * tokens as f64 / 1_000_000.0);
        usage.total_cost = match (usage.input_cost, usage.output_cost) {
            (Some(input), Some(output)) => Some(input + output),
            (Some(input), None) => Some(input),
            (None, Some(output)) => Some(output),
            (None, None) => None,
        };
        usage.currency = input.provider.currency.clone();
        let cache_source = if provider_reported_cache {
            "provider"
        } else if !input.model_supports_usage {
            "unsupported"
        } else {
            input
                .provider_capabilities
                .cache_stats_source(usage.cached_input_tokens, usage.cache_creation_input_tokens)
        };
        usage.cache_stats_source = Some(cache_source.to_string());
        usage.cache_stats = Some(AgentCacheStats {
            source: cache_source.to_string(),
            input_tokens: usage.input_tokens,
            cached_input_tokens: usage.cached_input_tokens,
            cache_creation_input_tokens: usage.cache_creation_input_tokens,
            hit_rate: input.provider_capabilities.cache_hit_rate(
                usage.input_tokens,
                usage.cached_input_tokens,
                usage.cache_creation_input_tokens,
            ),
            provider_kind: Some(input.protocol),
            model_id: Some(input.selected_model.id.clone()),
            request_count: Some(input.model_request_count.get()),
            cache_baseline_reset: input.cache_baseline_reset,
        });
    }
    usage
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (AgentProviderSettings, AgentModelSettings) {
        let provider: AgentProviderSettings = serde_json::from_value(json!({
            "id": "fixture", "name": "Fixture", "kind": "open_ai_compatible",
            "baseUrl": "https://fixture.example/v1", "model": "model",
            "inputPricePerMillion": 1.0, "outputPricePerMillion": 2.0,
            "currency": "测试币"
        }))
        .unwrap();
        let model =
            agent_core::resolve_agent_model(std::slice::from_ref(&provider), "fixture:model")
                .unwrap()
                .1;
        (provider, model)
    }

    #[test]
    fn usage_keeps_an_unreported_reasoning_counter_absent_and_preserves_costs() {
        let (provider, model) = fixture();
        let last_context_tokens = AtomicU64::new(123);
        let requests = std::cell::Cell::new(3);
        let usage = AgentUsage {
            input_tokens: Some(1_000),
            output_tokens: Some(50),
            generation_tokens: None,
            generation_duration_ms: None,
            generation_timing_source: None,
            cached_input_tokens: Some(200),
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
        };
        let usage = finalize_usage(
            Some(usage),
            UsageFinalization {
                protocol: ProviderKind::OpenAiCompatible,
                provider: &provider,
                selected_model: &model,
                model_supports_usage: true,
                provider_capabilities: ProviderCapabilities::for_kind(
                    ProviderKind::OpenAiCompatible,
                ),
                context_window: Some(16_000),
                context_window_estimated: false,
                final_response_requested: false,
                generation_metrics: &agent_core::GenerationMetrics::default(),
                last_context_tokens: &last_context_tokens,
                model_request_count: &requests,
                cache_baseline_reset: true,
            },
        )
        .unwrap();
        assert_eq!(usage.reasoning_tokens, None);
        assert_eq!(usage.context_tokens, Some(123));
        assert_eq!(usage.input_cost, Some(0.001));
        assert_eq!(usage.output_cost, Some(0.0001));
        assert_eq!(usage.cache_stats.unwrap().request_count, Some(3));
        assert!(usage.cache_baseline_reset);
    }
}
