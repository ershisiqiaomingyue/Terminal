#[cfg(test)]
mod tests {
    use crate::conversation::state::snapshot::MAX_SNAPSHOT_TOOL_OUTPUT_BYTES;
    use crate::{
        ConversationCommand, ThreadEvent, ThreadItem, ThreadSnapshot, ThreadStatus,
        ThreadUsageSnapshot, TurnStatus,
    };
    use serde_json::Value;

    #[test]
    fn terminal_turn_rejects_late_text_and_permission_events() {
        let mut thread = ThreadSnapshot::new("s");
        thread.apply(ThreadEvent::TurnStarted {
            thread_id: "s".into(),
            turn_id: "t".into(),
        });
        thread.apply(ThreadEvent::TurnCompleted {
            thread_id: "s".into(),
            turn_id: "t".into(),
            status: TurnStatus::Cancelled,
            error: None,
        });
        let revision = thread.revision;
        for event in [
            ThreadEvent::AgentDelta {
                thread_id: "s".into(),
                turn_id: "t".into(),
                item_id: "a".into(),
                text: "late".into(),
            },
            ThreadEvent::PermissionRequested {
                thread_id: "s".into(),
                turn_id: "t".into(),
                request_id: "p".into(),
                tool_name: "shell".into(),
                expires_at_ms: None,
            },
            ThreadEvent::PermissionResolved {
                thread_id: "s".into(),
                turn_id: "t".into(),
                request_id: "p".into(),
            },
            ThreadEvent::RuntimeUpdated {
                thread_id: "s".into(),
                turn_id: "t".into(),
                activity: "running again".into(),
                tool_call_count: 1,
            },
        ] {
            assert!(!thread.apply(event));
        }
        assert_eq!(thread.status, ThreadStatus::Cancelled);
        assert_eq!(thread.revision, revision);
        assert!(thread.pending_permission.is_none());
    }

    #[test]
    fn streamed_final_preserves_runtime_notes_steer_boundaries_and_trailing_context() {
        for content in ["firstlast", "last", ""] {
            let mut thread = ThreadSnapshot::new("a");
            thread.apply(ThreadEvent::TurnStarted {
                thread_id: "a".into(),
                turn_id: "t".into(),
            });
            let items = serde_json::json!([
                {"kind":"agent_message", "id":"assistant:t", "content":"first", "pending":true},
                {"kind":"tool_call", "id":"c", "name":"shell", "input":{}, "output":"ok", "status":"done", "is_error":false},
                {"kind":"agent_message", "id":"runtime", "content":"runtime note", "pending":true},
                {"kind":"user_message", "id":"steer", "content":"continue"},
                {"kind":"agent_message", "id":"tail", "content":"last", "pending":true},
                {"kind":"context", "id":"ctx", "data":{}}
            ]);
            thread.items = serde_json::from_value(items).unwrap();
            let mut expected = thread.items.clone();
            for item in &mut expected {
                if let ThreadItem::AgentMessage { pending, .. } = item {
                    *pending = false;
                }
            }
            thread.apply(ThreadEvent::AgentFinal {
                thread_id: "a".into(),
                turn_id: "t".into(),
                item_id: "assistant:t".into(),
                content: content.into(),
                preserve_streamed: true,
            });
            assert_eq!(thread.items, expected);
        }
    }

    #[test]
    fn legacy_agent_final_defaults_to_replacement_semantics() {
        let event: ThreadEvent = serde_json::from_value(serde_json::json!({
            "event":"agent_final", "thread_id":"a", "turn_id":"t", "item_id":"m", "content":"final"
        }))
        .unwrap();
        assert!(matches!(
            event,
            ThreadEvent::AgentFinal {
                preserve_streamed: false,
                ..
            }
        ));
    }

    #[test]
    fn user_message_attachment_metadata_is_backward_compatible() {
        let legacy: ThreadItem = serde_json::from_value(serde_json::json!({
            "kind":"user_message", "id":"user:t", "content":"hello"
        }))
        .unwrap();
        assert_eq!(
            serde_json::to_value(legacy).unwrap(),
            serde_json::json!({
                "kind":"user_message", "id":"user:t", "content":"hello"
            })
        );
        let value = serde_json::json!({
            "kind":"user_message", "id":"user:t:steer:s", "content":"look",
            "attachments":[{"name":"image.png", "mediaType":"image/png", "dataUrl":"data:image/png;base64,aA=="}],
            "artifacts":[{"name":"file.txt", "path":"attachment://file.txt", "kind":"file"}]
        });
        let item: ThreadItem = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(item).unwrap(), value);
    }

    #[test]
    fn reasoning_keeps_runtime_fallback_separate_from_model_text() {
        let mut thread = ThreadSnapshot::new("a");
        thread.apply(ThreadEvent::TurnStarted {
            thread_id: "a".into(),
            turn_id: "t".into(),
        });
        for (id, origin) in [
            ("model", None),
            ("fallback", Some("runtime_fallback")),
            ("model2", None),
            ("continued", None),
        ] {
            let mut data = serde_json::json!({"type": "reasoning_delta", "text": id});
            if let Some(origin) = origin {
                data["origin"] = serde_json::json!(origin);
            }
            thread.apply(ThreadEvent::ItemAdded {
                thread_id: "a".into(),
                turn_id: "t".into(),
                item: ThreadItem::TimelineEvent {
                    id: id.into(),
                    data,
                },
            });
        }
        assert_eq!(thread.items.len(), 3);
        let ThreadItem::TimelineEvent { data, .. } = &thread.items[1] else {
            panic!("expected runtime fallback");
        };
        assert_eq!(data["origin"], "runtime_fallback");
        assert_eq!(data["text"], "fallback");
        let ThreadItem::TimelineEvent { data, .. } = &thread.items[2] else {
            panic!("expected model reasoning");
        };
        assert_eq!(data["text"], "model2continued");
    }

    #[test]
    fn live_tool_output_does_not_settle_and_is_bounded() {
        let mut thread = ThreadSnapshot::new("a");
        thread.apply(ThreadEvent::TurnStarted {
            thread_id: "a".into(),
            turn_id: "t".into(),
        });
        let tool = |output| ThreadItem::ToolCall {
            id: "c".into(),
            name: "shell".into(),
            input: Value::Null,
            input_buffer: None,
            output,
            live_output: None,
            status: "pending".into(),
            tool_group_id: None,
            queued_at_ms: None,
            started_at_ms: None,
            finished_at_ms: None,
            is_error: false,
        };
        thread.apply(ThreadEvent::ItemAdded {
            thread_id: "a".into(),
            turn_id: "t".into(),
            item: tool(None),
        });
        let initial = serde_json::to_value(&thread.items[0]).unwrap();
        assert!(initial.get("output").is_none());
        assert!(initial.get("live_output").is_none());
        let delta = |output| ThreadEvent::ToolCallOutput {
            thread_id: "a".into(),
            turn_id: "t".into(),
            call_id: "c".into(),
            output,
        };
        for output in [
            serde_json::json!({"phase":"started"}),
            serde_json::json!({"phase":"chunk", "text":"alpha"}),
            serde_json::json!({"phase":"chunk", "text":"\n  "}),
            serde_json::json!({"phase":"chunk", "text":"beta"}),
            serde_json::json!({"phase":"progress", "progress":{"files":2}}),
            serde_json::json!({"phase":"finished"}),
        ] {
            assert!(thread.apply(delta(output)));
        }
        let live = serde_json::to_value(&thread.items[0]).unwrap();
        assert!(live.get("output").is_none());
        assert_eq!(live["live_output"]["text"], "alpha\n  beta");
        assert!(thread.apply(delta(
            serde_json::json!({"phase":"chunk", "text":"界".repeat(MAX_SNAPSHOT_TOOL_OUTPUT_BYTES)})
        )));
        let bounded = serde_json::to_value(&thread.items[0]).unwrap();
        assert!(
            bounded["live_output"]["text"].as_str().unwrap().len()
                <= MAX_SNAPSHOT_TOOL_OUTPUT_BYTES
        );
        assert_eq!(bounded["live_output"]["truncated"], true);
        thread.apply(ThreadEvent::ItemAdded {
            thread_id: "a".into(),
            turn_id: "t".into(),
            item: tool(Some(Value::Null)),
        });
        let settled = serde_json::to_value(&thread.items[0]).unwrap();
        assert_eq!(settled.get("output"), Some(&Value::Null));
        assert!(settled.get("live_output").is_none());
        assert!(!thread.apply(delta(serde_json::json!({"phase":"chunk", "text":"late"}))));
        assert_eq!(serde_json::to_value(&thread.items[0]).unwrap(), settled);
    }

    #[test]
    fn legacy_thread_read_deserializes_without_revision_hint() {
        let command: ConversationCommand = serde_json::from_value(
            serde_json::json!({"method":"thread_read", "params":{"thread_id":"a"}}),
        )
        .unwrap();
        assert_eq!(
            command,
            ConversationCommand::ThreadRead {
                thread_id: "a".into(),
                after_revision: None
            }
        );
    }

    #[test]
    fn conversation_commands_use_a_transport_stable_envelope() {
        let command = ConversationCommand::TurnInterrupt {
            thread_id: "session-a".into(),
            turn_id: "turn-a".into(),
            source: None,
        };
        assert_eq!(
            serde_json::to_value(command).expect("command should serialize"),
            serde_json::json!({
                "method": "turn_interrupt",
                "params": { "thread_id": "session-a", "turn_id": "turn-a" },
            }),
        );
    }

    #[test]
    fn interrupt_source_is_optional_for_old_clients_and_preserved_for_new_clients() {
        for source in [None, Some("stop_button"), Some("delete_session")] {
            let mut wire = serde_json::json!({"method":"turn_interrupt", "params":{
                "thread_id":"s", "turn_id":"t"
            }});
            if let Some(source) = source {
                wire["params"]["source"] = source.into();
            }
            let decoded: ConversationCommand = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(
                decoded,
                ConversationCommand::TurnInterrupt {
                    thread_id: "s".into(),
                    turn_id: "t".into(),
                    source: source.map(str::to_string),
                }
            );
            assert_eq!(serde_json::to_value(decoded).unwrap(), wire);
        }
    }

    #[test]
    fn turn_start_carries_the_full_transport_request() {
        let command = ConversationCommand::TurnStart {
            thread_id: "session-a".into(),
            turn_id: "turn-a".into(),
            request: serde_json::json!({ "modelId": "provider:model", "message": "inspect", "executionTarget": { "kind": "ssh", "connectionId": "ssh-a" } }),
        };
        assert_eq!(
            serde_json::to_value(command).expect("command should serialize"),
            serde_json::json!({
                "method": "turn_start",
                "params": {
                    "thread_id": "session-a",
                    "turn_id": "turn-a",
                    "request": { "modelId": "provider:model", "message": "inspect", "executionTarget": { "kind": "ssh", "connectionId": "ssh-a" } },
                },
            }),
        );
    }

    #[test]
    fn turn_steer_carries_attachments_and_artifacts() {
        let command = ConversationCommand::TurnSteer {
            thread_id: "session-a".into(),
            turn_id: "turn-a".into(),
            request: serde_json::json!({ "message": "also inspect this", "attachments": [{ "name": "screen.png" }], "artifacts": [{ "path": "attachment://note.txt" }] }),
        };
        assert_eq!(
            serde_json::to_value(command).expect("command should serialize"),
            serde_json::json!({
                "method": "turn_steer",
                "params": {
                    "thread_id": "session-a",
                    "turn_id": "turn-a",
                    "request": { "message": "also inspect this", "attachments": [{ "name": "screen.png" }], "artifacts": [{ "path": "attachment://note.txt" }] },
                },
            }),
        );
    }

    #[test]
    fn turn_enqueue_carries_a_full_request_without_an_active_turn() {
        let command = ConversationCommand::TurnEnqueue {
            thread_id: "session-a".into(),
            request: serde_json::json!({ "modelId": "provider:model", "message": "queue this", "sessionId": "session-a" }),
        };
        assert_eq!(
            serde_json::to_value(command).expect("command should serialize"),
            serde_json::json!({
                "method": "turn_enqueue",
                "params": {
                    "thread_id": "session-a",
                    "request": { "modelId": "provider:model", "message": "queue this", "sessionId": "session-a" },
                },
            }),
        );
    }

    #[test]
    fn queue_actions_are_scoped_to_a_thread_and_queue_item() {
        let command = ConversationCommand::TurnQueueRemove {
            thread_id: "session-a".into(),
            queue_id: "queue-a".into(),
        };
        assert_eq!(
            serde_json::to_value(command).expect("command should serialize"),
            serde_json::json!({
                "method": "turn_queue_remove",
                "params": { "thread_id": "session-a", "queue_id": "queue-a" },
            }),
        );
    }

    #[test]
    fn turn_resume_identifies_the_source_and_optional_new_execution() {
        let command = ConversationCommand::TurnResume {
            thread_id: "session-a".into(),
            turn_id: "turn-a".into(),
            resume_turn_id: None,
            permission_mode: None,
        };
        assert_eq!(
            serde_json::to_value(command).expect("command should serialize"),
            serde_json::json!({ "method": "turn_resume", "params": { "thread_id": "session-a", "turn_id": "turn-a" } }),
        );
        let old = serde_json::json!({ "method": "turn_resume", "params": { "thread_id": "session-a", "turn_id": "turn-a" } });
        let mut command: ConversationCommand = serde_json::from_value(old.clone()).unwrap();
        let ConversationCommand::TurnResume { resume_turn_id, .. } = &mut command else {
            panic!("resume command expected")
        };
        assert_eq!(*resume_turn_id, None);
        *resume_turn_id = Some("resume-a".into());
        let mut expected = old;
        expected["params"]["resume_turn_id"] = Value::String("resume-a".into());
        assert_eq!(serde_json::to_value(command).unwrap(), expected);
        for mode in ["ask", "risk", "full"] {
            expected["params"]["permission_mode"] = Value::String(mode.into());
            let command: ConversationCommand = serde_json::from_value(expected.clone()).unwrap();
            assert_eq!(serde_json::to_value(command).unwrap(), expected);
        }
    }

    #[test]
    fn snapshot_rejects_stale_turn_events_and_never_duplicates_deltas() {
        let mut thread = ThreadSnapshot::new("thread-a");
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into()
        }));
        assert!(thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item: ThreadItem::AgentMessage {
                id: "message-a".into(),
                content: String::new(),
                pending: true
            }
        }));
        assert!(thread.apply(ThreadEvent::AgentDelta {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item_id: "message-a".into(),
            text: "hello".into()
        }));
        assert!(!thread.apply(ThreadEvent::AgentDelta {
            thread_id: "thread-a".into(),
            turn_id: "old-turn".into(),
            item_id: "message-a".into(),
            text: " duplicate".into()
        }));
        assert_eq!(
            thread.items,
            vec![ThreadItem::AgentMessage {
                id: "message-a".into(),
                content: "hello".into(),
                pending: true
            }]
        );
    }

    #[test]
    fn task_plan_updates_and_permission_resolution_replace_runtime_state() {
        let mut thread = ThreadSnapshot::new("thread-a");
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into()
        }));
        assert!(thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item: ThreadItem::TaskPlan {
                id: "plan-a".into(),
                goal: "first".into(),
                steps: vec![]
            }
        }));
        assert!(thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item: ThreadItem::TaskPlan {
                id: "plan-a".into(),
                goal: "updated".into(),
                steps: vec![]
            }
        }));
        assert_eq!(thread.items.len(), 1);
        assert!(thread.apply(ThreadEvent::PermissionRequested {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            request_id: "request-a".into(),
            tool_name: "apply_patch".into(),
            expires_at_ms: None
        }));
        assert_eq!(thread.status, ThreadStatus::WaitingForPermission);
        let revision = thread.revision;
        assert!(!thread.apply(ThreadEvent::PermissionResolved {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            request_id: "expired-request".into(),
        }));
        assert_eq!(thread.revision, revision);
        assert_eq!(
            thread.pending_permission.as_ref().unwrap().request_id,
            "request-a"
        );
        assert!(thread.apply(ThreadEvent::PermissionResolved {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            request_id: "request-a".into()
        }));
        assert_eq!(thread.status, ThreadStatus::Running);
        assert_eq!(thread.pending_permission, None);
    }

    #[test]
    fn tool_completion_replaces_its_started_item() {
        let mut thread = ThreadSnapshot::new("thread-a");
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into()
        }));
        assert!(thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item: ThreadItem::ToolCall {
                id: "call-a".into(),
                name: "read_file".into(),
                input: Value::Null,
                input_buffer: None,
                output: None,
                live_output: None,
                status: "pending".into(),
                tool_group_id: None,
                queued_at_ms: None,
                started_at_ms: None,
                finished_at_ms: None,
                is_error: false
            }
        }));
        assert!(thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item: ThreadItem::ToolCall {
                id: "call-a".into(),
                name: "read_file".into(),
                input: Value::Null,
                input_buffer: None,
                output: Some(Value::String("ok".into())),
                live_output: None,
                status: "done".into(),
                tool_group_id: None,
                queued_at_ms: None,
                started_at_ms: None,
                finished_at_ms: None,
                is_error: false
            }
        }));
        assert_eq!(thread.items.len(), 1);
        assert!(matches!(
            thread.items[0],
            ThreadItem::ToolCall {
                output: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn empty_delta_after_a_tool_does_not_create_an_empty_message_segment() {
        let mut thread = ThreadSnapshot::new("thread-a");
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into()
        }));
        assert!(thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item: ThreadItem::ToolCall {
                id: "call-a".into(),
                name: "shell".into(),
                input: Value::Null,
                input_buffer: None,
                output: None,
                live_output: None,
                status: "pending".into(),
                tool_group_id: None,
                queued_at_ms: None,
                started_at_ms: None,
                finished_at_ms: None,
                is_error: false
            },
        }));
        assert!(!thread.apply(ThreadEvent::AgentDelta {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item_id: "assistant:turn-a".into(),
            text: String::new(),
        }));
        assert_eq!(thread.items.len(), 1);
        assert!(matches!(thread.items[0], ThreadItem::ToolCall { .. }));
    }

    #[test]
    fn agent_deltas_after_a_tool_start_a_new_ordered_text_segment() {
        let mut thread = ThreadSnapshot::new("thread-a");
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into()
        }));
        assert!(thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item: ThreadItem::AgentMessage {
                id: "assistant:turn-a".into(),
                content: "A".into(),
                pending: true
            },
        }));
        assert!(thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item: ThreadItem::ToolCall {
                id: "call-a".into(),
                name: "run_local_command".into(),
                input: Value::Null,
                input_buffer: None,
                output: None,
                live_output: None,
                status: "pending".into(),
                tool_group_id: None,
                queued_at_ms: None,
                started_at_ms: None,
                finished_at_ms: None,
                is_error: false
            },
        }));
        assert!(thread.apply(ThreadEvent::AgentDelta {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item_id: "assistant:turn-a".into(),
            text: "C".into(),
        }));
        assert!(matches!(&thread.items[..], [
            ThreadItem::AgentMessage { content, .. },
            ThreadItem::ToolCall { .. },
            ThreadItem::AgentMessage { content: next_content, .. },
        ] if content == "A" && next_content == "C"));
    }

    #[test]
    fn final_response_updates_the_last_text_segment_after_a_tool() {
        let mut thread = ThreadSnapshot::new("thread-a");
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into()
        }));
        assert!(thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item: ThreadItem::AgentMessage {
                id: "assistant:turn-a".into(),
                content: "before tool".into(),
                pending: true
            },
        }));
        assert!(thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item: ThreadItem::ToolCall {
                id: "call-a".into(),
                name: "run_local_command".into(),
                input: Value::Null,
                input_buffer: None,
                output: None,
                live_output: None,
                status: "pending".into(),
                tool_group_id: None,
                queued_at_ms: None,
                started_at_ms: None,
                finished_at_ms: None,
                is_error: false
            },
        }));
        assert!(thread.apply(ThreadEvent::AgentFinal {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item_id: "assistant:turn-a".into(),
            content: "after tool".into(),
            preserve_streamed: false,
        }));
        assert!(matches!(&thread.items[..], [
            ThreadItem::AgentMessage { content: first, .. },
            ThreadItem::ToolCall { .. },
            ThreadItem::AgentMessage { content: last, pending, .. },
        ] if first == "before tool" && last == "after tool" && !pending));
    }

    #[test]
    fn final_response_matching_streamed_text_does_not_duplicate_after_a_tool() {
        let mut thread = ThreadSnapshot::new("thread-a");
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into()
        }));
        assert!(thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item: ThreadItem::AgentMessage {
                id: "assistant:turn-a".into(),
                content: "测试通过。".into(),
                pending: true
            },
        }));
        assert!(thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item: ThreadItem::ToolCall {
                id: "call-a".into(),
                name: "run_project_check".into(),
                input: Value::Null,
                input_buffer: None,
                output: None,
                live_output: None,
                status: "pending".into(),
                tool_group_id: None,
                queued_at_ms: None,
                started_at_ms: None,
                finished_at_ms: None,
                is_error: false
            },
        }));
        assert!(thread.apply(ThreadEvent::AgentFinal {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item_id: "assistant:turn-a".into(),
            content: "测试通过。".into(),
            preserve_streamed: false,
        }));
        assert!(matches!(&thread.items[..], [
            ThreadItem::AgentMessage { id, content, pending },
            ThreadItem::ToolCall { .. },
        ] if id == "assistant:turn-a" && content == "测试通过。" && !pending));
        assert_eq!(thread.revision, 4);
    }

    #[test]
    fn final_response_updates_its_initial_message_after_context_events() {
        let mut thread = ThreadSnapshot::new("thread-a");
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into()
        }));
        assert!(thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item: ThreadItem::AgentMessage {
                id: "assistant:turn-a".into(),
                content: String::new(),
                pending: true
            },
        }));
        assert!(thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item: ThreadItem::Context {
                id: "context:turn-a".into(),
                data: Value::Null
            },
        }));
        assert!(thread.apply(ThreadEvent::AgentFinal {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item_id: "assistant:turn-a".into(),
            content: "测试通过。".into(),
            preserve_streamed: false,
        }));
        assert!(matches!(&thread.items[..], [
            ThreadItem::AgentMessage { id, content, pending },
            ThreadItem::Context { .. },
        ] if id == "assistant:turn-a" && content == "测试通过。" && !pending));
    }

    #[test]
    fn final_response_after_context_split_does_not_repeat_streamed_text() {
        for final_text in ["OK", "corrected"] {
            let mut thread = ThreadSnapshot::new("a");
            thread.apply(ThreadEvent::TurnStarted {
                thread_id: "a".into(),
                turn_id: "t".into(),
            });
            thread.apply(ThreadEvent::ItemAdded {
                thread_id: "a".into(),
                turn_id: "t".into(),
                item: ThreadItem::AgentMessage {
                    id: "assistant:t".into(),
                    content: String::new(),
                    pending: true,
                },
            });
            thread.apply(ThreadEvent::ItemAdded {
                thread_id: "a".into(),
                turn_id: "t".into(),
                item: ThreadItem::Context {
                    id: "ctx".into(),
                    data: Value::Null,
                },
            });
            thread.apply(ThreadEvent::AgentDelta {
                thread_id: "a".into(),
                turn_id: "t".into(),
                item_id: "assistant:t".into(),
                text: "OK".into(),
            });
            thread.apply(ThreadEvent::AgentFinal {
                thread_id: "a".into(),
                turn_id: "t".into(),
                item_id: "assistant:t".into(),
                content: final_text.into(),
                preserve_streamed: false,
            });
            let text = thread
                .items
                .iter()
                .filter_map(|item| match item {
                    ThreadItem::AgentMessage {
                        content, pending, ..
                    } => {
                        assert!(!pending);
                        Some(content.as_str())
                    }
                    _ => None,
                })
                .collect::<String>();
            assert_eq!(text, final_text);
            assert!(thread
                .items
                .iter()
                .any(|item| matches!(item, ThreadItem::Context { .. })));
        }
    }

    #[test]
    fn text_only_steer_boundary_keeps_the_final_reply_in_its_original_position() {
        for (final_text, expected_tail) in [
            ("beforeafter", "after"),
            ("corrected after", "corrected after"),
        ] {
            let mut thread = ThreadSnapshot::new("a");
            thread.apply(ThreadEvent::TurnStarted {
                thread_id: "a".into(),
                turn_id: "t".into(),
            });
            thread.apply(ThreadEvent::ItemAdded {
                thread_id: "a".into(),
                turn_id: "t".into(),
                item: ThreadItem::AgentMessage {
                    id: "assistant:t".into(),
                    content: "before".into(),
                    pending: true,
                },
            });
            thread.apply(ThreadEvent::ItemAdded {
                thread_id: "a".into(),
                turn_id: "t".into(),
                item: ThreadItem::UserMessage {
                    id: "user:t:steer:s".into(),
                    content: "steer".into(),
                    attachments: vec![],
                    artifacts: vec![],
                },
            });
            thread.apply(ThreadEvent::AgentDelta {
                thread_id: "a".into(),
                turn_id: "t".into(),
                item_id: "assistant:t".into(),
                text: "after".into(),
            });
            thread.apply(ThreadEvent::AgentFinal {
                thread_id: "a".into(),
                turn_id: "t".into(),
                item_id: "assistant:t".into(),
                content: final_text.into(),
                preserve_streamed: false,
            });
            assert!(
                matches!(&thread.items[0], ThreadItem::AgentMessage {content, ..} if content=="before")
            );
            assert!(
                matches!(&thread.items[1], ThreadItem::UserMessage {content, ..} if content=="steer")
            );
            assert!(
                matches!(&thread.items[2], ThreadItem::AgentMessage {content, ..} if content==expected_tail)
            );
        }
    }

    #[test]
    fn tool_output_updates_only_its_active_call() {
        let mut thread = ThreadSnapshot::new("thread-a");
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into()
        }));
        assert!(thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item: ThreadItem::ToolCall {
                id: "call-a".into(),
                name: "shell".into(),
                input: Value::Null,
                input_buffer: None,
                output: None,
                live_output: None,
                status: "running".into(),
                tool_group_id: None,
                queued_at_ms: None,
                started_at_ms: None,
                finished_at_ms: None,
                is_error: false
            }
        }));
        assert!(thread.apply(ThreadEvent::ToolCallOutput {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            call_id: "call-a".into(),
            output: Value::String("running".into())
        }));
        assert!(!thread.apply(ThreadEvent::ToolCallOutput {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            call_id: "call-b".into(),
            output: Value::String("wrong".into())
        }));
        assert!(
            matches!(&thread.items[0], ThreadItem::ToolCall { output: None, live_output: Some(Value::String(value)), .. } if value == "running")
        );
    }

    #[test]
    fn file_changes_replace_the_current_turn_snapshot_item() {
        let mut thread = ThreadSnapshot::new("thread-a");
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into()
        }));
        for value in ["first", "latest"] {
            assert!(thread.apply(ThreadEvent::ItemAdded {
                thread_id: "thread-a".into(),
                turn_id: "turn-a".into(),
                item: ThreadItem::FileChanges {
                    id: "file-changes:turn-a".into(),
                    data: Value::String(value.into())
                }
            }));
        }
        assert_eq!(thread.items.len(), 1);
        assert!(
            matches!(&thread.items[0], ThreadItem::FileChanges { data: Value::String(value), .. } if value == "latest")
        );
    }

    #[test]
    fn context_replaces_the_current_turn_snapshot_item_and_rejects_stale_turns() {
        let mut thread = ThreadSnapshot::new("thread-a");
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into()
        }));
        for value in ["first", "latest"] {
            assert!(thread.apply(ThreadEvent::ItemAdded {
                thread_id: "thread-a".into(),
                turn_id: "turn-a".into(),
                item: ThreadItem::Context {
                    id: "context:turn-a".into(),
                    data: Value::String(value.into())
                }
            }));
        }
        assert!(!thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "old-turn".into(),
            item: ThreadItem::Context {
                id: "context:old-turn".into(),
                data: Value::String("stale".into())
            }
        }));
        assert_eq!(thread.items.len(), 1);
        assert!(
            matches!(&thread.items[0], ThreadItem::Context { data: Value::String(value), .. } if value == "latest")
        );
    }

    #[test]
    fn final_report_replaces_the_current_turn_snapshot_item_and_rejects_stale_turns() {
        let mut thread = ThreadSnapshot::new("thread-a");
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into()
        }));
        for value in ["first", "latest"] {
            assert!(thread.apply(ThreadEvent::ItemAdded {
                thread_id: "thread-a".into(),
                turn_id: "turn-a".into(),
                item: ThreadItem::FinalReport {
                    id: "final-report:turn-a".into(),
                    data: Value::String(value.into())
                }
            }));
        }
        assert!(!thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "old-turn".into(),
            item: ThreadItem::FinalReport {
                id: "final-report:old-turn".into(),
                data: Value::String("stale".into())
            }
        }));
        assert_eq!(thread.items.len(), 1);
        assert!(
            matches!(&thread.items[0], ThreadItem::FinalReport { data: Value::String(value), .. } if value == "latest")
        );
    }

    #[test]
    fn timeline_events_append_for_the_current_turn_and_reject_stale_turns() {
        let mut thread = ThreadSnapshot::new("thread-a");
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into()
        }));
        for (id, value) in [("event-1", "first"), ("event-2", "latest")] {
            assert!(thread.apply(ThreadEvent::ItemAdded {
                thread_id: "thread-a".into(),
                turn_id: "turn-a".into(),
                item: ThreadItem::TimelineEvent {
                    id: id.into(),
                    data: Value::String(value.into())
                }
            }));
        }
        assert!(!thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "old-turn".into(),
            item: ThreadItem::TimelineEvent {
                id: "event-stale".into(),
                data: Value::String("stale".into())
            }
        }));
        assert_eq!(thread.items.len(), 2);
        assert!(
            matches!(&thread.items[1], ThreadItem::TimelineEvent { data: Value::String(value), .. } if value == "latest")
        );
    }

    #[test]
    fn final_agent_message_replaces_streamed_text_before_completion() {
        let mut thread = ThreadSnapshot::new("thread-a");
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into()
        }));
        assert!(thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item: ThreadItem::AgentMessage {
                id: "assistant:turn-a".into(),
                content: "partial".into(),
                pending: true
            }
        }));
        assert!(thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item: ThreadItem::AgentMessage {
                id: "assistant:turn-a".into(),
                content: "complete response".into(),
                pending: false
            }
        }));
        assert!(thread.apply(ThreadEvent::TurnCompleted {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            status: TurnStatus::Completed,
            error: None
        }));
        assert!(
            matches!(&thread.items[0], ThreadItem::AgentMessage { content, pending, .. } if content == "complete response" && !pending)
        );
    }

    #[test]
    fn recoverable_completion_keeps_the_streamed_agent_text() {
        let mut thread = ThreadSnapshot::new("thread-a");
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into()
        }));
        assert!(thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item: ThreadItem::AgentMessage {
                id: "assistant:turn-a".into(),
                content: String::new(),
                pending: true
            }
        }));
        assert!(thread.apply(ThreadEvent::AgentDelta {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item_id: "assistant:turn-a".into(),
            text: "已完成的有效内容。".into()
        }));
        assert!(thread.apply(ThreadEvent::AgentFinal {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item_id: "assistant:turn-a".into(),
            content: "已完成的有效内容。".into(),
            preserve_streamed: false,
        }));
        assert!(thread.apply(ThreadEvent::TurnCompleted {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            status: TurnStatus::Completed,
            error: None
        }));
        assert!(
            matches!(&thread.items[0], ThreadItem::AgentMessage { content, pending, .. } if content == "已完成的有效内容。" && !pending)
        );
        assert_eq!(thread.status, ThreadStatus::Completed);
    }

    #[test]
    fn starting_a_turn_replaces_the_previous_live_items() {
        let mut thread = ThreadSnapshot::new("thread-a");
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into()
        }));
        assert!(thread.apply(ThreadEvent::ItemAdded {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            item: ThreadItem::ToolCall {
                id: "call-a".into(),
                name: "read_file".into(),
                input: Value::Null,
                input_buffer: None,
                output: None,
                live_output: None,
                status: "pending".into(),
                tool_group_id: None,
                queued_at_ms: None,
                started_at_ms: None,
                finished_at_ms: None,
                is_error: false
            }
        }));
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-b".into()
        }));
        assert!(thread.items.is_empty());
    }

    #[test]
    fn runtime_updates_belong_only_to_the_active_turn() {
        let mut thread = ThreadSnapshot::new("thread-a");
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into()
        }));
        assert!(thread.apply(ThreadEvent::RuntimeUpdated {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            activity: "正在执行 read file".into(),
            tool_call_count: 1,
        }));
        assert_eq!(thread.runtime.activity, "正在执行 read file");
        assert_eq!(thread.runtime.tool_call_count, 1);
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-b".into()
        }));
        assert!(!thread.apply(ThreadEvent::RuntimeUpdated {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            activity: "stale".into(),
            tool_call_count: 99,
        }));
        assert!(thread.runtime.activity.is_empty());
        assert_eq!(thread.runtime.tool_call_count, 0);
    }

    #[test]
    fn execution_target_belongs_only_to_the_active_turn() {
        let mut thread = ThreadSnapshot::new("thread-a");
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into()
        }));
        assert!(thread.apply(ThreadEvent::ExecutionTargetUpdated {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            execution_target: Some(serde_json::json!({ "kind": "ssh", "connectionId": "ssh-a" })),
        }));
        assert_eq!(
            thread.runtime.execution_target,
            Some(serde_json::json!({ "kind": "ssh", "connectionId": "ssh-a" }))
        );
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-b".into()
        }));
        assert_eq!(thread.runtime.execution_target, None);
        assert!(!thread.apply(ThreadEvent::ExecutionTargetUpdated {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            execution_target: Some(serde_json::json!({ "kind": "local" })),
        }));
        assert_eq!(thread.runtime.execution_target, None);
    }

    #[test]
    fn usage_updates_merge_for_the_active_turn_only() {
        let mut thread = ThreadSnapshot::new("thread-a");
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into()
        }));
        assert!(thread.apply(ThreadEvent::UsageUpdated {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            usage: ThreadUsageSnapshot {
                input_tokens: Some(120),
                output_tokens: Some(30),
                generation_tokens: Some(100),
                generation_duration_ms: Some(2000),
                generation_timing_source: Some("request".to_string()),
                latest_request_usage: Some(crate::RequestCacheUsage {
                    provider_kind: Some(crate::ProviderKind::OpenAi),
                    input_tokens: Some(120),
                    cached_input_tokens: Some(100),
                    cache_creation_input_tokens: None,
                }),
                ..ThreadUsageSnapshot::default()
            },
        }));
        assert!(thread.apply(ThreadEvent::UsageUpdated {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            usage: ThreadUsageSnapshot {
                context_window: Some(128_000),
                context_tokens: Some(450),
                ..ThreadUsageSnapshot::default()
            },
        }));
        assert_eq!(thread.runtime.usage.input_tokens, Some(120));
        assert_eq!(thread.runtime.usage.context_tokens, Some(450));
        assert_eq!(
            thread
                .runtime
                .usage
                .latest_request_usage
                .as_ref()
                .unwrap()
                .cached_input_tokens,
            Some(100)
        );
        assert_eq!(thread.runtime.usage.generation_tokens, Some(100));
        assert_eq!(thread.runtime.usage.generation_duration_ms, Some(2000));
        assert_eq!(
            thread.runtime.usage.generation_timing_source.as_deref(),
            Some("request")
        );
        assert!(thread.apply(ThreadEvent::UsageUpdated {
            thread_id: "thread-a".into(),
            turn_id: "turn-a".into(),
            usage: ThreadUsageSnapshot {
                generation_tokens: Some(250),
                generation_duration_ms: Some(5000),
                generation_timing_source: Some("request".to_string()),
                ..ThreadUsageSnapshot::default()
            },
        }));
        assert_eq!(thread.runtime.usage.generation_tokens, Some(250));
        assert_eq!(thread.runtime.usage.generation_duration_ms, Some(5000));
        assert!(!thread.apply(ThreadEvent::UsageUpdated {
            thread_id: "thread-a".into(),
            turn_id: "stale-turn".into(),
            usage: ThreadUsageSnapshot {
                output_tokens: Some(99),
                ..ThreadUsageSnapshot::default()
            },
        }));
        assert_eq!(thread.runtime.usage.output_tokens, Some(30));
        assert!(thread.apply(ThreadEvent::TurnStarted {
            thread_id: "thread-a".into(),
            turn_id: "turn-b".into(),
        }));
        assert_eq!(thread.runtime.usage.generation_tokens, None);
        assert_eq!(thread.runtime.usage.generation_duration_ms, None);
        assert_eq!(thread.runtime.usage.generation_timing_source, None);
        assert_eq!(thread.runtime.usage.latest_request_usage, None);
    }
}
