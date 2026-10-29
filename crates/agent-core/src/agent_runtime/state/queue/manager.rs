use super::claim::{QueuedAgentClaim, QueuedAgentRequest};
use super::preview::queue_items;
use crate::{AgentChatRequest, AgentRuntimeState, ThreadSnapshot};
use std::collections::{HashSet, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

impl AgentRuntimeState {
    pub fn enqueue_agent_request(
        &self,
        session_id: &str,
        id: String,
        request: AgentChatRequest,
    ) -> Result<(ThreadSnapshot, bool), String> {
        let request_is_active = self.has_active_agent_request(session_id);
        let (queue, should_dispatch) = {
            let mut scheduler = self
                .scheduler
                .lock()
                .map_err(|_| "请求队列不可用".to_string())?;
            let queue = {
                let queued = scheduler.queued.entry(session_id.to_string()).or_default();
                if queued.len() >= 20 {
                    return Err("待发送队列最多保留 20 条消息。".to_string());
                }
                queued.push_back(QueuedAgentRequest { id, request });
                queue_items(queued)
            };
            let should_dispatch =
                !request_is_active && scheduler.dispatching.insert(session_id.to_string());
            (queue, should_dispatch)
        };
        Ok((
            self.replace_thread_queue(session_id, queue),
            should_dispatch,
        ))
    }

    pub fn dispatch_queued_request(&self, session_id: &str) -> bool {
        if self.has_active_agent_request(session_id) {
            return false;
        }
        let Ok(mut scheduler) = self.scheduler.lock() else {
            return false;
        };
        scheduler
            .queued
            .get(session_id)
            .is_some_and(|queue| !queue.is_empty())
            && scheduler.dispatching.insert(session_id.to_string())
    }

    /// Claims the oldest pending request without deleting it. This is the
    /// in-memory half of the durable queue lease; callers should use the same
    /// lease id in SQLite and call `ack_agent_queue_claim` only after the turn
    /// has acquired its request guard.
    pub fn claim_next_queued_agent_request(
        &self,
        session_id: &str,
        lease_id: String,
    ) -> Result<Option<(QueuedAgentClaim, ThreadSnapshot)>, String> {
        let queue_id = {
            let scheduler = self
                .scheduler
                .lock()
                .map_err(|_| "请求队列不可用".to_string())?;
            if scheduler.claimed.contains_key(session_id) {
                return Ok(None);
            }
            scheduler
                .queued
                .get(session_id)
                .and_then(|queue| queue.front())
                .map(|item| item.id.clone())
        };
        let Some(queue_id) = queue_id else {
            if let Ok(mut scheduler) = self.scheduler.lock() {
                scheduler.dispatching.remove(session_id);
            }
            return Ok(None);
        };
        self.claim_queued_agent_request(session_id, &queue_id, lease_id)
    }

    /// Claims one exact pending request. Using this operation for queued steer
    /// messages makes reservation and removal from the dispatcher-visible
    /// queue atomic, so the same message cannot also start a later turn.
    pub fn claim_queued_agent_request(
        &self,
        session_id: &str,
        queue_id: &str,
        lease_id: String,
    ) -> Result<Option<(QueuedAgentClaim, ThreadSnapshot)>, String> {
        if lease_id.trim().is_empty() {
            return Err("队列 lease id 不能为空".to_string());
        }
        let (claim, queue) = {
            let mut scheduler = self
                .scheduler
                .lock()
                .map_err(|_| "请求队列不可用".to_string())?;
            if scheduler.claimed.contains_key(session_id) {
                return Ok(None);
            }
            let Some(queued) = scheduler.queued.get_mut(session_id) else {
                return Ok(None);
            };
            let Some(index) = queued.iter().position(|item| item.id == queue_id) else {
                return Ok(None);
            };
            let queued_request = queued
                .remove(index)
                .expect("queue index was obtained from the same collection");
            let claim = QueuedAgentClaim {
                id: queued_request.id,
                lease_id,
                request: queued_request.request,
                original_index: index,
            };
            let queue = queue_items(queued);
            if queued.is_empty() {
                scheduler.queued.remove(session_id);
            }
            scheduler
                .claimed
                .insert(session_id.to_string(), claim.clone());
            (claim, queue)
        };
        let snapshot = self.replace_thread_queue(session_id, queue);
        Ok(Some((claim, snapshot)))
    }

    /// Permanently removes a claimed request from the live queue cache.
    pub fn ack_agent_queue_claim(
        &self,
        session_id: &str,
        queue_id: &str,
        lease_id: &str,
    ) -> Result<ThreadSnapshot, String> {
        let queue = {
            let mut scheduler = self
                .scheduler
                .lock()
                .map_err(|_| "请求队列不可用".to_string())?;
            let matches = scheduler
                .claimed
                .get(session_id)
                .is_some_and(|claim| claim.id == queue_id && claim.lease_id == lease_id);
            if !matches {
                return Err("队列 lease 已失效或不属于该消息".to_string());
            }
            scheduler.claimed.remove(session_id);
            scheduler
                .queued
                .get(session_id)
                .map(queue_items)
                .unwrap_or_default()
        };
        Ok(self.replace_thread_queue(session_id, queue))
    }

    /// Releases a failed handoff and restores the message at its original
    /// position. The dispatcher flag is cleared so a later trigger can retry.
    pub fn nack_agent_queue_claim(
        &self,
        session_id: &str,
        queue_id: &str,
        lease_id: &str,
    ) -> Result<ThreadSnapshot, String> {
        let queue = {
            let mut scheduler = self
                .scheduler
                .lock()
                .map_err(|_| "请求队列不可用".to_string())?;
            let matches = scheduler
                .claimed
                .get(session_id)
                .is_some_and(|claim| claim.id == queue_id && claim.lease_id == lease_id);
            if !matches {
                return Err("队列 lease 已失效或不属于该消息".to_string());
            }
            let claim = scheduler
                .claimed
                .remove(session_id)
                .expect("claim existence was checked above");
            let queued = scheduler.queued.entry(session_id.to_string()).or_default();
            let index = claim.original_index.min(queued.len());
            queued.insert(
                index,
                QueuedAgentRequest {
                    id: claim.id,
                    request: claim.request,
                },
            );
            let queue = queue_items(queued);
            scheduler.dispatching.remove(session_id);
            queue
        };
        Ok(self.replace_thread_queue(session_id, queue))
    }

    /// Rebuilds one session's pending queue from the durable store. Claimed
    /// items must be recovered in SQLite before this method is called.
    pub fn replace_queued_agent_requests(
        &self,
        session_id: &str,
        requests: Vec<(String, AgentChatRequest)>,
    ) -> Result<ThreadSnapshot, String> {
        if requests.len() > 20 {
            return Err("待发送队列最多保留 20 条消息。".to_string());
        }
        let queue = {
            let mut scheduler = self
                .scheduler
                .lock()
                .map_err(|_| "请求队列不可用".to_string())?;
            let mut ids = HashSet::new();
            let mut queued = VecDeque::with_capacity(requests.len());
            for (id, request) in requests {
                if id.trim().is_empty() || !ids.insert(id.clone()) {
                    return Err("持久化队列包含空 id 或重复消息".to_string());
                }
                queued.push_back(QueuedAgentRequest { id, request });
            }
            let queue = queue_items(&queued);
            if queued.is_empty() {
                scheduler.queued.remove(session_id);
            } else {
                scheduler.queued.insert(session_id.to_string(), queued);
            }
            scheduler.claimed.remove(session_id);
            scheduler.dispatching.remove(session_id);
            queue
        };
        Ok(self.replace_thread_queue(session_id, queue))
    }

    pub fn dequeue_agent_request(
        &self,
        session_id: &str,
    ) -> Option<(AgentChatRequest, ThreadSnapshot)> {
        let (request, queue) = {
            let mut scheduler = self.scheduler.lock().ok()?;
            let request = scheduler
                .queued
                .get_mut(session_id)
                .and_then(VecDeque::pop_front);
            if request.is_none() {
                scheduler.queued.remove(session_id);
                scheduler.dispatching.remove(session_id);
            }
            let queue = scheduler
                .queued
                .get(session_id)
                .map(queue_items)
                .unwrap_or_default();
            (request, queue)
        };
        let snapshot = self.replace_thread_queue(session_id, queue);
        request.map(|queued| (queued.request, snapshot))
    }

    pub fn restore_queued_agent_request(
        &self,
        session_id: &str,
        request: AgentChatRequest,
    ) -> ThreadSnapshot {
        let queue = {
            let mut scheduler = self
                .scheduler
                .lock()
                .expect("thread scheduler lock should be available");
            let queue = {
                let queued = scheduler.queued.entry(session_id.to_string()).or_default();
                queued.push_front(QueuedAgentRequest {
                    id: format!(
                        "retry-{}",
                        SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_nanos()
                    ),
                    request,
                });
                queue_items(queued)
            };
            scheduler.dispatching.remove(session_id);
            queue
        };
        self.replace_thread_queue(session_id, queue)
    }

    pub fn queued_agent_request(
        &self,
        session_id: &str,
        queue_id: &str,
    ) -> Option<AgentChatRequest> {
        self.scheduler
            .lock()
            .ok()?
            .queued
            .get(session_id)?
            .iter()
            .find(|item| item.id == queue_id)
            .map(|item| item.request.clone())
    }

    pub fn queued_session_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .scheduler
            .lock()
            .map(|scheduler| {
                scheduler
                    .queued
                    .iter()
                    .filter(|(_, queue)| !queue.is_empty())
                    .map(|(session_id, _)| session_id.clone())
                    .collect()
            })
            .unwrap_or_default();
        ids.sort();
        ids
    }

    pub fn remove_queued_agent_request(
        &self,
        session_id: &str,
        queue_id: &str,
    ) -> Option<ThreadSnapshot> {
        let queue = {
            let mut scheduler = self.scheduler.lock().ok()?;
            let queued = scheduler.queued.get_mut(session_id)?;
            let index = queued.iter().position(|item| item.id == queue_id)?;
            queued.remove(index);
            let queue = queue_items(queued);
            if queued.is_empty() {
                scheduler.queued.remove(session_id);
            }
            queue
        };
        Some(self.replace_thread_queue(session_id, queue))
    }

    pub fn reorder_queued_agent_requests(
        &self,
        session_id: &str,
        queue_ids: &[String],
    ) -> Result<ThreadSnapshot, String> {
        let queue = {
            let mut scheduler = self
                .scheduler
                .lock()
                .map_err(|_| "请求队列不可用".to_string())?;
            if scheduler.claimed.contains_key(session_id) {
                return Err("队列正在发送，稍后再调整顺序。".to_string());
            }
            let queued = scheduler
                .queued
                .get_mut(session_id)
                .ok_or_else(|| "待发送队列已变化，请重试。".to_string())?;
            if queue_ids.len() != queued.len() {
                return Err("待发送队列已变化，请重试。".to_string());
            }
            let mut reordered = VecDeque::with_capacity(queued.len());
            for id in queue_ids {
                let Some(index) = queued.iter().position(|item| &item.id == id) else {
                    return Err("待发送队列已变化，请重试。".to_string());
                };
                if reordered
                    .iter()
                    .any(|item: &QueuedAgentRequest| &item.id == id)
                {
                    return Err("队列顺序包含重复消息。".to_string());
                }
                reordered.push_back(queued[index].clone());
            }
            *queued = reordered;
            queue_items(queued)
        };
        Ok(self.replace_thread_queue(session_id, queue))
    }
}
