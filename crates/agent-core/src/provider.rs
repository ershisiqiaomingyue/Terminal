use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    OpenAi,
    OpenAiResponses,
    OpenAiCompatible,
    #[serde(rename = "deepseek")]
    DeepSeek,
    Kimi,
    Glm,
    #[serde(rename = "mimo")]
    MiMo,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProviderConfigError {
    #[error("provider id must not be empty")]
    EmptyId,
    #[error("provider name must not be empty")]
    EmptyName,
    #[error("provider base URL must use http or https")]
    InvalidBaseUrl,
    #[error("provider credential reference must not be empty")]
    EmptyCredentialReference,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub api_key_ref: String,
}

impl ProviderConfig {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        kind: ProviderKind,
        base_url: impl Into<String>,
        api_key_ref: impl Into<String>,
    ) -> Result<Self, ProviderConfigError> {
        let config = Self {
            id: id.into(),
            name: name.into(),
            kind,
            base_url: base_url.into(),
            api_key_ref: api_key_ref.into(),
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ProviderConfigError> {
        if self.id.trim().is_empty() {
            return Err(ProviderConfigError::EmptyId);
        }
        if self.name.trim().is_empty() {
            return Err(ProviderConfigError::EmptyName);
        }
        if !(self.base_url.starts_with("http://") || self.base_url.starts_with("https://")) {
            return Err(ProviderConfigError::InvalidBaseUrl);
        }
        if self.api_key_ref.trim().is_empty() {
            return Err(ProviderConfigError::EmptyCredentialReference);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelProfile {
    pub id: String,
    pub provider_id: String,
    pub model: String,
    pub protocol: ProviderKind,
    pub context_window: Option<u32>,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub supports_structured_output: bool,
    pub supports_usage: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ModelCompatibilityError {
    #[error("model does not support tool calling")]
    ToolsUnsupported,
    #[error("model does not support vision input")]
    VisionUnsupported,
    #[error("model does not support structured output")]
    StructuredOutputUnsupported,
    #[error("estimated request exceeds model context window {limit}")]
    ContextWindowExceeded { limit: u32 },
}

impl ModelProfile {
    pub fn new(
        id: impl Into<String>,
        provider_id: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            provider_id: provider_id.into(),
            model: model.into(),
            protocol: ProviderKind::OpenAiCompatible,
            context_window: None,
            supports_tools: false,
            supports_vision: false,
            supports_structured_output: false,
            supports_usage: false,
        }
    }

    pub fn with_protocol(mut self, protocol: ProviderKind) -> Self {
        self.protocol = protocol;
        self
    }

    pub fn with_capabilities(
        mut self,
        context_window: Option<u32>,
        supports_tools: bool,
        supports_vision: bool,
        supports_structured_output: bool,
        supports_usage: bool,
    ) -> Self {
        self.context_window = context_window;
        self.supports_tools = supports_tools;
        self.supports_vision = supports_vision;
        self.supports_structured_output = supports_structured_output;
        self.supports_usage = supports_usage;
        self
    }

    pub fn check_compatibility(
        &self,
        tools_requested: bool,
        structured_output_requested: bool,
        estimated_input_tokens: Option<u32>,
    ) -> Result<(), ModelCompatibilityError> {
        if tools_requested && !self.supports_tools {
            return Err(ModelCompatibilityError::ToolsUnsupported);
        }
        if structured_output_requested && !self.supports_structured_output {
            return Err(ModelCompatibilityError::StructuredOutputUnsupported);
        }
        if let (Some(limit), Some(estimated)) = (self.context_window, estimated_input_tokens) {
            if estimated > limit {
                return Err(ModelCompatibilityError::ContextWindowExceeded { limit });
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ConnectionStatus {
    NotConfigured,
    Checking,
    Connected { message: String },
    Error { message: String },
}

impl ConnectionStatus {
    pub fn connected(message: impl Into<String>) -> Self {
        Self::Connected {
            message: message.into(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: message.into(),
        }
    }

    pub fn is_healthy(&self) -> bool {
        matches!(self, Self::Connected { .. })
    }

    pub fn message(&self) -> &str {
        match self {
            Self::NotConfigured => "not configured",
            Self::Checking => "checking",
            Self::Connected { message } | Self::Error { message } => message,
        }
    }
}
