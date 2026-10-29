/// G5-C：turn 结束时随响应带回的上下文检查点状态（摘要 + 结构化载荷 + 覆盖边界）。
pub(in crate::agent::turn) struct CheckpointState {
    pub(in crate::agent::turn) summary: Option<String>,
    pub(in crate::agent::turn) payload: Option<agent_core::AgentContextCheckpointPayload>,
    pub(in crate::agent::turn) covered_through_message_id: Option<String>,
}
