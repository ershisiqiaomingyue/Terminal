use super::ThreadProjectionState;
use crate::{AgentRuntimeState, ThreadEvent, ThreadSnapshot, ThreadUpdate};

impl AgentRuntimeState {
    pub fn reset_idle_thread(&self, session_id: &str) -> Option<ThreadSnapshot> {
        if self.requests.lock().ok()?.contains_key(session_id) {
            return None;
        }
        self.thread_event_ids.lock().ok()?.remove(session_id);
        let mut threads = self.threads.lock().ok()?;
        let revision = threads
            .get(session_id)
            .map_or(1, |snapshot| snapshot.revision.saturating_add(1));
        let mut snapshot = ThreadSnapshot::new(session_id);
        snapshot.revision = revision;
        threads.insert(session_id.into(), snapshot.clone());
        self.thread_updates.lock().ok()?.remove(session_id);
        self.pending_thread_publications
            .lock()
            .ok()?
            .remove(session_id);
        Some(snapshot)
    }

    pub fn read_thread_snapshot(&self, session_id: &str) -> Option<ThreadSnapshot> {
        self.read_thread_snapshot_after(session_id, None)
    }

    pub fn read_thread_snapshot_after(
        &self,
        session_id: &str,
        after_revision: Option<u64>,
    ) -> Option<ThreadSnapshot> {
        let threads = self.threads.lock().ok()?;
        let snapshot = threads.get(session_id)?;
        if after_revision.is_some_and(|revision| snapshot.revision <= revision) {
            return None;
        }
        Some(snapshot.clone())
    }

    pub fn apply_thread_event(&self, event: ThreadEvent) -> Option<ThreadSnapshot> {
        self.apply_thread_event_once(event, None)
    }

    pub fn apply_thread_event_once(
        &self,
        event: ThreadEvent,
        event_id: Option<&str>,
    ) -> Option<ThreadSnapshot> {
        self.update_thread_event(event, event_id, |snapshot| snapshot.clone())
    }

    /// Apply high-frequency deltas without copying the growing conversation.
    pub fn apply_thread_event_deferred(&self, event: ThreadEvent, event_id: Option<&str>) -> bool {
        self.update_thread_event(event, event_id, |_| ()).is_some()
    }

    pub fn schedule_thread_publication(&self, session_id: &str) -> bool {
        self.pending_thread_publications
            .lock()
            .is_ok_and(|mut pending| pending.insert(session_id.to_string()))
    }

    pub fn take_thread_publication(&self, session_id: &str) -> Option<ThreadSnapshot> {
        self.pending_thread_publications
            .lock()
            .ok()?
            .remove(session_id);
        self.read_thread_snapshot(session_id)
    }

    pub fn take_thread_update(&self, session_id: &str) -> Option<ThreadUpdate> {
        self.pending_thread_publications
            .lock()
            .ok()?
            .remove(session_id);
        let threads = self.threads.lock().ok()?;
        let snapshot = threads.get(session_id)?;
        let mut journals = self.thread_updates.lock().ok()?;
        let journal = journals.entry(session_id.to_string()).or_default();
        let previous = journal.published_revision;
        if previous == Some(snapshot.revision) {
            return None;
        }
        journal.published_revision = Some(snapshot.revision);
        if let Some(base_revision) = previous {
            let events = journal
                .events
                .iter()
                .filter(|(revision, _, _)| *revision > base_revision)
                .collect::<Vec<_>>();
            let contiguous = events.len() as u64 == snapshot.revision.saturating_sub(base_revision)
                && events
                    .first()
                    .is_some_and(|(revision, _, _)| *revision == base_revision + 1);
            if contiguous {
                return Some(ThreadUpdate::Delta {
                    id: snapshot.id.clone(),
                    base_revision,
                    revision: snapshot.revision,
                    metadata: ThreadSnapshot {
                        items: Vec::new(),
                        id: snapshot.id.clone(),
                        revision: snapshot.revision,
                        status: snapshot.status,
                        active_turn: snapshot.active_turn.clone(),
                        pending_permission: snapshot.pending_permission.clone(),
                        runtime: snapshot.runtime.clone(),
                        queue: snapshot.queue.clone(),
                    },
                    events: events
                        .into_iter()
                        .map(|(_, _, event)| event.clone())
                        .collect(),
                });
            }
        }
        Some(ThreadUpdate::Snapshot {
            snapshot: snapshot.clone(),
        })
    }

    fn update_thread_event<T>(
        &self,
        event: ThreadEvent,
        event_id: Option<&str>,
        project: impl FnOnce(&ThreadSnapshot) -> T,
    ) -> Option<T> {
        let session_id = match &event {
            ThreadEvent::TurnStarted { thread_id, .. }
            | ThreadEvent::ItemAdded { thread_id, .. }
            | ThreadEvent::ToolCallInputDelta { thread_id, .. }
            | ThreadEvent::ToolCallOutput { thread_id, .. }
            | ThreadEvent::AgentDelta { thread_id, .. }
            | ThreadEvent::AgentFinal { thread_id, .. }
            | ThreadEvent::PermissionRequested { thread_id, .. }
            | ThreadEvent::PermissionResolved { thread_id, .. }
            | ThreadEvent::TurnCompleted { thread_id, .. }
            | ThreadEvent::RuntimeUpdated { thread_id, .. }
            | ThreadEvent::UsageUpdated { thread_id, .. }
            | ThreadEvent::ExecutionTargetUpdated { thread_id, .. } => thread_id,
        };
        if let Some(event_id) = event_id.filter(|value| !value.trim().is_empty()) {
            // One source event can update both timeline and usage projections.
            let projection = std::mem::discriminant(&event);
            let mut event_ids = self.thread_event_ids.lock().ok()?;
            let ids = event_ids.entry(session_id.clone()).or_default();
            if ids
                .iter()
                .any(|(kind, existing)| *kind == projection && existing == event_id)
            {
                return None;
            }
            ids.push_back((projection, event_id.to_string()));
            if ids.len() > 2_048 {
                ids.pop_front();
            }
        }
        let mut threads = self.threads_slot().lock().ok()?;
        let snapshot = threads
            .entry(session_id.clone())
            .or_insert_with(|| ThreadSnapshot::new(session_id));
        let retained = event.clone();
        if !snapshot.apply(event) {
            return None;
        }
        if let Ok(mut journals) = self.updates_slot().lock() {
            let journal = journals.entry(snapshot.id.clone()).or_default();
            // Bound replay memory independently of the durable history.
            let bytes = serde_json::to_vec(&retained).map_or(512 * 1024 + 1, |value| value.len());
            journal.bytes += bytes;
            journal
                .events
                .push_back((snapshot.revision, bytes, retained));
            while journal.events.len() > 256 || journal.bytes > 512 * 1024 {
                if let Some((_, bytes, _)) = journal.events.pop_front() {
                    journal.bytes -= bytes;
                } else {
                    break;
                }
            }
        }
        Some(project(snapshot))
    }
}
