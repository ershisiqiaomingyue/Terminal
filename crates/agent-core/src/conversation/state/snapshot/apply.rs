use serde_json::Value;

use super::super::events::ThreadEvent;
use super::super::models::{
    epoch_millis, PermissionSnapshot, ThreadItem, ThreadRuntimeSnapshot, ThreadStatus,
    ThreadUsageSnapshot, TurnSnapshot, TurnStatus,
};
use super::ThreadSnapshot;

impl ThreadSnapshot {
    pub(super) fn reduce(&mut self, event: ThreadEvent) -> bool {
        self.apply_event(event)
    }

    fn apply_event(&mut self, event: ThreadEvent) -> bool {
        if event.thread_id() != self.id {
            return false;
        }
        if let Some(turn_id) = event.turn_id() {
            if self.active_turn.as_ref().map(|turn| turn.id.as_str()) != Some(turn_id)
                && !matches!(event, ThreadEvent::TurnStarted { .. })
            {
                return false;
            }
        }
        // A terminal turn must not resume streaming or request permissions again.
        // N1-B 例外：迟到的 `permission_resolved` 放行——它只清权限条，handler
        // 仍要求 request_id 匹配，不会复活回合或改动终态。
        if self
            .active_turn
            .as_ref()
            .is_some_and(|turn| turn.status != TurnStatus::Running)
            && matches!(
                event,
                ThreadEvent::AgentDelta { .. }
                    | ThreadEvent::ToolCallInputDelta { .. }
                    | ThreadEvent::ToolCallOutput { .. }
                    | ThreadEvent::PermissionRequested { .. }
                    | ThreadEvent::RuntimeUpdated { .. }
            )
        {
            return false;
        }
        match event {
            ThreadEvent::TurnStarted { turn_id, .. } => {
                self.status = ThreadStatus::Running;
                self.active_turn = Some(TurnSnapshot {
                    id: turn_id,
                    status: TurnStatus::Running,
                    error: None,
                    started_at_ms: epoch_millis(),
                    finished_at_ms: None,
                });
                // Live snapshots intentionally contain only the current turn.
                // Durable history is owned by the history store while this
                // runtime state is the authoritative rendering state for an
                // in-flight turn.
                self.pending_permission = None;
                self.runtime = ThreadRuntimeSnapshot {
                    activity: String::new(),
                    tool_call_count: 0,
                    usage: ThreadUsageSnapshot::default(),
                    execution_target: None,
                };
                self.items.clear();
            }
            ThreadEvent::ItemAdded { item, .. } => {
                // Reasoning deltas form one contiguous stream, not one item per
                // token. Keep boundaries around tools and narration intact.
                if let ThreadItem::TimelineEvent { id, data } = &item {
                    if data.get("type").and_then(Value::as_str) == Some("reasoning_delta") {
                        if let Some(ThreadItem::TimelineEvent {
                            id: previous_id,
                            data: previous,
                        }) = self.items.last_mut()
                        {
                            if previous_id != id
                                && previous.get("type").and_then(Value::as_str)
                                    == Some("reasoning_delta")
                                && previous.get("origin") == data.get("origin")
                            {
                                if let (Some(Value::String(text)), Some(delta)) = (
                                    previous.get_mut("text"),
                                    data.get("text").and_then(Value::as_str),
                                ) {
                                    text.push_str(delta);
                                    self.revision += 1;
                                    return true;
                                }
                            }
                        }
                    }
                }
                if let Some(existing) = self
                    .items
                    .iter_mut()
                    .find(|existing| existing.id() == item.id())
                {
                    let mut item = item;
                    if let (
                        ThreadItem::ToolCall {
                            tool_group_id: previous_group,
                            queued_at_ms: previous_queued,
                            started_at_ms: previous_started,
                            ..
                        },
                        ThreadItem::ToolCall {
                            tool_group_id,
                            queued_at_ms,
                            started_at_ms,
                            ..
                        },
                    ) = (&*existing, &mut item)
                    {
                        if tool_group_id.is_none() {
                            *tool_group_id = previous_group.clone();
                        }
                        if queued_at_ms.is_none() {
                            *queued_at_ms = *previous_queued;
                        }
                        if started_at_ms.is_none() {
                            *started_at_ms = *previous_started;
                        }
                    }
                    *existing = item;
                } else {
                    self.items.push(item);
                }
            }
            ThreadEvent::ToolCallInputDelta { call_id, delta, .. } => {
                if delta.is_empty() {
                    return false;
                }
                let Some(ThreadItem::ToolCall {
                    input_buffer,
                    output,
                    ..
                }) = self
                    .items
                    .iter_mut()
                    .find(|item| matches!(item, ThreadItem::ToolCall { id, .. } if id == &call_id))
                else {
                    return false;
                };
                if output.is_some() {
                    return false;
                }
                let buffer = input_buffer.get_or_insert_with(String::new);
                buffer.push_str(&delta);
            }
            ThreadEvent::AgentDelta { item_id, text, .. } => {
                // Providers can emit an empty text chunk between tool calls.
                // It has no visible content and must not create a new empty
                // assistant segment after a tool item.
                if text.is_empty() {
                    return false;
                }
                if let Some(ThreadItem::AgentMessage {
                    content, pending, ..
                }) = self.items.last_mut()
                {
                    content.push_str(&text);
                    *pending = true;
                } else {
                    self.items.push(ThreadItem::AgentMessage {
                        id: format!("{item_id}:{}", self.items.len()),
                        content: text,
                        pending: true,
                    });
                }
            }
            ThreadEvent::AgentFinal {
                item_id,
                content,
                preserve_streamed,
                ..
            } => {
                let matching_message_index = self.items.iter().position(
                    |item| matches!(item, ThreadItem::AgentMessage { id, .. } if id == &item_id),
                );
                let has_boundary_after_matching_message =
                    matching_message_index.is_some_and(|index| {
                        self.items[index + 1..].iter().any(|item| {
                            matches!(
                                item,
                                ThreadItem::ToolCall { .. } | ThreadItem::UserMessage { .. }
                            )
                        })
                    });
                let streamed_content = self
                    .items
                    .iter()
                    .filter_map(|item| {
                        let ThreadItem::AgentMessage { content, .. } = item else {
                            return None;
                        };
                        Some(content.as_str())
                    })
                    .collect::<String>();
                if matching_message_index.is_some()
                    && !streamed_content.is_empty()
                    && (preserve_streamed || streamed_content == content)
                {
                    for item in &mut self.items {
                        if let ThreadItem::AgentMessage { pending, .. } = item {
                            *pending = false;
                        }
                    }
                } else if !has_boundary_after_matching_message {
                    if let Some(ThreadItem::AgentMessage {
                        content: current,
                        pending,
                        ..
                    }) = matching_message_index.and_then(|index| self.items.get_mut(index))
                    {
                        *current = content;
                        *pending = false;
                        if let Some(index) = matching_message_index {
                            let mut position = 0;
                            self.items.retain(|item| {
                                let keep = position <= index
                                    || !matches!(item, ThreadItem::AgentMessage { .. });
                                position += 1;
                                keep
                            });
                        }
                    }
                } else {
                    // The final response is the aggregate of every streamed text
                    // segment in this turn. When a tool call separates segments,
                    // writing that aggregate as a new tail item duplicates the
                    // already rendered reply.
                    if let Some(ThreadItem::AgentMessage {
                        content: current,
                        pending,
                        ..
                    }) = self.items.last_mut()
                    {
                        *current = content;
                        *pending = false;
                    } else {
                        self.items.push(ThreadItem::AgentMessage {
                            id: format!("{item_id}:final"),
                            content,
                            pending: false,
                        });
                    }
                }
            }
            ThreadEvent::PermissionRequested {
                request_id,
                tool_name,
                expires_at_ms,
                ..
            } => {
                self.status = ThreadStatus::WaitingForPermission;
                self.pending_permission = Some(PermissionSnapshot {
                    request_id,
                    tool_name,
                    expires_at_ms,
                });
            }
            ThreadEvent::PermissionResolved { request_id, .. } => {
                if self
                    .pending_permission
                    .as_ref()
                    .map(|permission| permission.request_id.as_str())
                    != Some(request_id.as_str())
                {
                    return false;
                }
                self.pending_permission = None;
                // N1-B：终态回合上的迟到 resolved 只清权限条，不把线程状态
                // 复活成 Running。
                let turn_running = self
                    .active_turn
                    .as_ref()
                    .map(|turn| turn.status == TurnStatus::Running)
                    .unwrap_or(true);
                if turn_running {
                    self.status = ThreadStatus::Running;
                }
            }
            ThreadEvent::TurnCompleted { status, error, .. } => {
                let Some(turn) = self.active_turn.as_mut() else {
                    return false;
                };
                if turn.status != TurnStatus::Running {
                    return false;
                }
                turn.status = status;
                turn.error = error;
                turn.finished_at_ms = Some(epoch_millis());
                self.pending_permission = None;
                self.status = match status {
                    TurnStatus::Completed => ThreadStatus::Completed,
                    TurnStatus::Failed => ThreadStatus::Failed,
                    TurnStatus::Cancelled => ThreadStatus::Cancelled,
                    TurnStatus::Running => ThreadStatus::Running,
                };
                for item in &mut self.items {
                    if let ThreadItem::AgentMessage { pending, .. } = item {
                        *pending = false;
                    }
                }
            }
            ThreadEvent::RuntimeUpdated {
                activity,
                tool_call_count,
                ..
            } => {
                self.runtime.activity = activity;
                self.runtime.tool_call_count = tool_call_count;
            }
            ThreadEvent::ToolCallOutput {
                call_id, output, ..
            } => {
                let Some(ThreadItem::ToolCall {
                    output: final_output,
                    live_output,
                    ..
                }) = self
                    .items
                    .iter_mut()
                    .find(|item| matches!(item, ThreadItem::ToolCall { id, .. } if id == &call_id))
                else {
                    return false;
                };
                // Late process callbacks cannot replace an authoritative result.
                if final_output.is_some() {
                    return false;
                }
                super::tool::merge_live_tool_output(live_output, output);
            }
            ThreadEvent::UsageUpdated { usage, .. } => {
                self.runtime.usage.merge(usage);
            }
            ThreadEvent::ExecutionTargetUpdated {
                execution_target, ..
            } => {
                self.runtime.execution_target = execution_target;
            }
        }
        self.revision += 1;
        true
    }
}
