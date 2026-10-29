use crate::ProviderKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputUsageSemantics {
    TotalPrompt,
    UncachedPrompt,
}

#[derive(Debug, Clone, Copy)]
pub struct ProviderCapabilities {
    pub supports_tools: bool,
    pub supports_usage: bool,
    pub supports_structured_output: bool,
    pub input_usage: InputUsageSemantics,
}

impl ProviderCapabilities {
    pub fn for_kind(kind: ProviderKind) -> Self {
        match kind {
            ProviderKind::Anthropic => Self {
                supports_tools: true,
                supports_usage: true,
                supports_structured_output: false,
                input_usage: InputUsageSemantics::UncachedPrompt,
            },
            ProviderKind::OpenAi | ProviderKind::OpenAiResponses => Self {
                supports_tools: true,
                supports_usage: true,
                supports_structured_output: true,
                input_usage: InputUsageSemantics::TotalPrompt,
            },
            ProviderKind::OpenAiCompatible => Self {
                supports_tools: true,
                // Most compatible relays expose usage when the optional
                // stream_options.include_usage field is present. The desktop
                // runtime retries once without it for older servers.
                supports_usage: true,
                supports_structured_output: false,
                input_usage: InputUsageSemantics::TotalPrompt,
            },
            ProviderKind::DeepSeek
            | ProviderKind::Kimi
            | ProviderKind::Glm
            | ProviderKind::MiMo => Self {
                supports_tools: true,
                supports_usage: true,
                supports_structured_output: false,
                input_usage: InputUsageSemantics::TotalPrompt,
            },
        }
    }

    pub fn cache_stats_source(
        self,
        cached_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
    ) -> &'static str {
        if cached_input_tokens.is_some() || cache_creation_input_tokens.is_some() {
            "provider"
        } else {
            "unreported"
        }
    }

    /// Normalize a provider's input-token fields without changing their
    /// meaning. Anthropic reports uncached prompt tokens in `input_tokens`,
    /// while OpenAI reports the complete prompt in `prompt_tokens`/`input_tokens`.
    /// A relay may omit that total and expose cache-hit/miss fields instead.
    pub fn input_tokens_from_usage(
        self,
        reported_input_tokens: Option<u64>,
        cached_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
        uncached_input_tokens: Option<u64>,
    ) -> Option<u64> {
        match self.input_usage {
            InputUsageSemantics::UncachedPrompt => reported_input_tokens.or(uncached_input_tokens),
            InputUsageSemantics::TotalPrompt => reported_input_tokens.or_else(|| {
                match (
                    cached_input_tokens,
                    uncached_input_tokens,
                    cache_creation_input_tokens,
                ) {
                    (Some(cached), Some(uncached), _) => Some(cached.saturating_add(uncached)),
                    (Some(cached), None, Some(created)) => Some(cached.saturating_add(created)),
                    (None, Some(uncached), Some(created)) => Some(uncached.saturating_add(created)),
                    (None, Some(uncached), None) => Some(uncached),
                    _ => None,
                }
            }),
        }
    }

    pub fn cache_input_total(
        self,
        input_tokens: Option<u64>,
        cached_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
    ) -> Option<u64> {
        match self.input_usage {
            InputUsageSemantics::UncachedPrompt => {
                if input_tokens.is_none()
                    && cached_input_tokens.is_none()
                    && cache_creation_input_tokens.is_none()
                {
                    None
                } else {
                    Some(
                        input_tokens
                            .unwrap_or_default()
                            .saturating_add(cached_input_tokens.unwrap_or_default())
                            .saturating_add(cache_creation_input_tokens.unwrap_or_default()),
                    )
                }
            }
            InputUsageSemantics::TotalPrompt => input_tokens.or_else(|| {
                cached_input_tokens
                    .zip(cache_creation_input_tokens)
                    .map(|(cached, created)| cached.saturating_add(created))
            }),
        }
    }

    pub fn cache_hit_rate(
        self,
        input_tokens: Option<u64>,
        cached_input_tokens: Option<u64>,
        cache_creation_input_tokens: Option<u64>,
    ) -> Option<f64> {
        // Anthropic reports cache reads separately from uncached input and
        // does not always include a cache-creation field. That is still
        // enough to calculate a rate when uncached input is present. Only a
        // cache-read-only payload is ambiguous and must remain unreported.
        if matches!(self.input_usage, InputUsageSemantics::UncachedPrompt)
            && input_tokens.is_none()
            && cached_input_tokens.is_some()
            && cache_creation_input_tokens.is_none()
        {
            return None;
        }
        let total = self.cache_input_total(
            input_tokens,
            cached_input_tokens,
            cache_creation_input_tokens,
        )?;
        let cached = cached_input_tokens?;
        (total > 0 && cached <= total).then(|| cached as f64 / total as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::{InputUsageSemantics, ProviderCapabilities};
    use crate::ProviderKind;

    #[test]
    fn calculates_anthropic_cache_rate_without_cache_creation_field() {
        let capabilities = ProviderCapabilities {
            input_usage: InputUsageSemantics::UncachedPrompt,
            ..ProviderCapabilities::for_kind(ProviderKind::Anthropic)
        };
        assert_eq!(
            capabilities.cache_hit_rate(Some(100), Some(900), None),
            Some(0.9)
        );
    }

    #[test]
    fn keeps_total_prompt_semantics_for_openai_chat_and_responses() {
        for kind in [ProviderKind::OpenAi, ProviderKind::OpenAiResponses] {
            let capabilities = ProviderCapabilities::for_kind(kind);
            assert_eq!(
                capabilities.input_tokens_from_usage(Some(1_000), Some(700), None, None),
                Some(1_000)
            );
            assert_eq!(
                capabilities.input_tokens_from_usage(None, Some(700), None, Some(300)),
                Some(1_000)
            );
            assert_eq!(
                capabilities.cache_hit_rate(Some(1_000), Some(700), None),
                Some(0.7)
            );
        }
    }

    #[test]
    fn includes_anthropic_cache_creation_in_the_uncached_prompt_total() {
        let capabilities = ProviderCapabilities::for_kind(ProviderKind::Anthropic);
        assert_eq!(
            capabilities.input_tokens_from_usage(Some(100), Some(800), Some(100), None),
            Some(100)
        );
        assert_eq!(
            capabilities.cache_hit_rate(Some(100), Some(800), Some(100)),
            Some(0.8)
        );
    }

    #[test]
    fn does_not_treat_cache_read_only_usage_as_full_hit() {
        let capabilities = ProviderCapabilities::for_kind(ProviderKind::Anthropic);
        assert_eq!(capabilities.cache_hit_rate(None, Some(900), None), None);
    }
}
