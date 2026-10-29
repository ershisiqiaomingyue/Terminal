use crate::{AgentContextCheckpoint, AgentUsage, AnalysisReport};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogIndexEntry {
    pub source: String,
    pub name: String,
    pub kind: String,
    pub size: u64,
    #[serde(rename = "compressedSize")]
    pub compressed_size: Option<u64>,
    #[serde(rename = "isArchive")]
    pub is_archive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogSearchHit {
    pub source: String,
    pub name: String,
    #[serde(rename = "lineStart")]
    pub line_start: usize,
    #[serde(rename = "lineEnd")]
    pub line_end: usize,
    #[serde(rename = "byteStart")]
    pub byte_start: u64,
    #[serde(rename = "byteEnd")]
    pub byte_end: u64,
    pub excerpt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogRangeResult {
    pub source: String,
    pub name: String,
    #[serde(rename = "lineStart")]
    pub line_start: usize,
    #[serde(rename = "lineEnd")]
    pub line_end: usize,
    #[serde(rename = "byteStart")]
    pub byte_start: u64,
    #[serde(rename = "byteEnd")]
    pub byte_end: u64,
    pub content: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogTimelineEvent {
    pub source: String,
    pub name: String,
    pub line: usize,
    pub timestamp: Option<String>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentLogEvidence {
    pub index: Vec<LogIndexEntry>,
    #[serde(rename = "searchHits")]
    pub search_hits: Vec<LogSearchHit>,
    pub ranges: Vec<LogRangeResult>,
    pub timeline: Vec<LogTimelineEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatResponse {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub text: String,
    pub usage: Option<AgentUsage>,
    pub report: Option<AnalysisReport>,
    #[serde(rename = "reportError")]
    pub report_error: Option<String>,
    pub evidence: Option<AgentLogEvidence>,
    #[serde(rename = "contextCheckpoint", skip_serializing_if = "Option::is_none")]
    pub context_checkpoint: Option<AgentContextCheckpoint>,
    #[serde(rename = "turnId")]
    pub turn_id: String,
    /// A stable terminal classification for the desktop timeline. It answers
    /// why an otherwise successful turn stopped without exposing provider
    /// internals or requiring users to infer it from the last tool event.
    #[serde(rename = "terminationReason")]
    pub termination_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogRangeRequest {
    pub paths: Vec<String>,
    pub source: Option<String>,
    pub member: Option<String>,
    #[serde(rename = "startLine", default)]
    pub start_line: usize,
    #[serde(rename = "maxLines", default)]
    pub max_lines: usize,
    #[serde(rename = "maxBytes", default)]
    pub max_bytes: usize,
}
