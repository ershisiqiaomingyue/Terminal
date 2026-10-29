use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const INLINE_BINARY_ARTIFACT_STATUS: &str = "inline_binary";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JiraIssueContext {
    pub url: String,
    pub host: String,
    pub key: String,
    pub summary: String,
    pub description: String,
    pub status: Option<String>,
    pub priority: Option<String>,
    #[serde(rename = "issueType")]
    pub issue_type: Option<String>,
    pub components: Vec<String>,
    pub labels: Vec<String>,
    pub environment: Option<String>,
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalArtifact {
    /// A virtual browser attachment uses attachment://... as its path. For
    /// text files, content contains bounded UTF-8 text. For binary browser
    /// files, content contains a bounded data:*;base64,... payload and
    /// status is inline_binary.
    pub path: String,
    pub name: String,
    pub kind: String,
    pub size: Option<u64>,
    #[serde(rename = "modifiedAt")]
    pub modified_at: Option<String>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(rename = "contentTruncated", default)]
    pub content_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentImageAttachment {
    pub name: String,
    #[serde(rename = "mediaType")]
    pub media_type: String,
    #[serde(rename = "dataUrl", default, skip_serializing_if = "String::is_empty")]
    pub data_url: String,
    /// Desktop-owned immutable snapshot. Resolved only at the provider boundary;
    /// durable history never needs another copy of the base64 payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentContextTurn {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(rename = "messageId", default)]
    pub message_id: Option<String>,
    #[serde(rename = "turnId", default)]
    pub turn_id: Option<String>,
    #[serde(rename = "modelId", default)]
    pub model_id: Option<String>,
    /// Images belonging to this durable message. Desktop history uses immutable
    /// cache references; legacy inline data remains readable. Checkpoints only
    /// retain the owner message id.
    #[serde(default)]
    pub attachments: Vec<AgentImageAttachment>,
    /// Metadata of files/directories explicitly supplied in this user message.
    /// Historical selections retain location, not file contents or permissions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<LocalArtifact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolPolicy {
    Auto,
    Disabled,
}

impl Default for AgentToolPolicy {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatRequest {
    #[serde(rename = "modelId")]
    pub model_id: String,
    pub message: String,
    #[serde(rename = "jiraContext")]
    pub jira_context: Option<JiraIssueContext>,
    pub artifacts: Vec<LocalArtifact>,
    #[serde(rename = "workspacePath", default)]
    pub workspace_path: Option<String>,
    #[serde(rename = "sessionId", default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub attachments: Vec<AgentImageAttachment>,
    #[serde(rename = "turnId", default)]
    pub turn_id: Option<String>,
    #[serde(rename = "permissionMode", default)]
    pub permission_mode: Option<String>,
    #[serde(rename = "analysisMode", default)]
    pub analysis_mode: bool,
    #[serde(default)]
    pub context: Vec<AgentContextTurn>,
    #[serde(rename = "toolPolicy", default)]
    pub tool_policy: AgentToolPolicy,
    #[serde(rename = "skillIds", default)]
    pub skill_ids: Vec<String>,
    #[serde(rename = "requestedTool", default)]
    pub requested_tool: Option<String>,
    #[serde(rename = "reasoningEffort", default)]
    pub reasoning_effort: Option<String>,
    #[serde(rename = "taskPlanMode", default)]
    pub task_plan_mode: Option<String>,
    #[serde(rename = "taskPlanMinSteps", default)]
    pub task_plan_min_steps: Option<u8>,
    #[serde(rename = "executionTarget", default)]
    pub execution_target: Option<AgentExecutionTarget>,
    /// Remote environments bound to this conversation. The first entry mirrors
    /// `execution_target` (the turn default); later entries stay addressable
    /// through the optional `target` argument of the `ssh__*` tools.
    #[serde(rename = "executionTargets", default)]
    pub execution_targets: Vec<AgentExecutionTarget>,
    /// G2-D：本 turn 作为子智能体时的委派层数（0 = 用户直接发起的 turn）。
    /// 只有子智能体运行时会填，普通 turn 缺省 0。
    #[serde(rename = "collaborationDepth", default)]
    pub collaboration_depth: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentExecutionTarget {
    #[serde(default = "default_execution_target_kind")]
    pub kind: String,
    #[serde(rename = "connectionId", default)]
    pub connection_id: Option<String>,
    #[serde(rename = "profileId", default)]
    pub profile_id: Option<String>,
    #[serde(default)]
    pub root: Option<String>,
}

fn default_execution_target_kind() -> String {
    "local".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSteerRequest {
    pub message: String,
    #[serde(default)]
    pub attachments: Vec<AgentImageAttachment>,
    #[serde(default)]
    pub artifacts: Vec<LocalArtifact>,
    /// Runtime-only id derived from the durable steer row. Keeping this out of
    /// the persisted request JSON preserves exact inbox matching while giving
    /// images a stable owner once the steer is delivered.
    #[serde(skip)]
    pub source_message_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentContextCheckpoint {
    pub id: String,
    pub summary: String,
    #[serde(rename = "evidencePolicyVersion", default)]
    pub evidence_policy_version: u8,
    #[serde(
        rename = "coveredThroughMessageId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub covered_through_message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<AgentContextCheckpointPayload>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentContextCheckpointPayload {
    pub format_version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer_model_id: Option<String>,
    #[serde(default)]
    pub entries: Vec<AgentContextCheckpointEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentContextCheckpointEntry {
    SystemConstraint {
        content: String,
        #[serde(
            rename = "sourceMessageId",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        source_message_id: Option<String>,
    },
    UserConstraint {
        content: String,
        #[serde(
            rename = "sourceMessageId",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        source_message_id: Option<String>,
    },
    AssistantConclusion {
        content: String,
        #[serde(
            rename = "sourceMessageId",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        source_message_id: Option<String>,
    },
    ToolEvidence {
        #[serde(rename = "callId")]
        call_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        input: Value,
        output: String,
        #[serde(
            rename = "sourceMessageId",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        source_message_id: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCancellationStatus {
    pub requested: bool,
    pub completed: bool,
    /// P1-C 兜底：true 表示软取消宽限（1500ms）超时后已硬中止回合任务，
    /// 回合终态由后台收尾落成 cancelled。
    #[serde(default)]
    pub aborted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AnalysisReport {
    pub conclusion: String,
    #[serde(rename = "suspectedCauses")]
    pub suspected_causes: Vec<String>,
    pub evidence: Vec<String>,
    pub timeline: Vec<String>,
    #[serde(rename = "nextSteps")]
    pub next_steps: Vec<String>,
}
