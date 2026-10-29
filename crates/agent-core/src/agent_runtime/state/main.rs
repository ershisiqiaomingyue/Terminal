use super::queue::{QueuedAgentClaim, QueuedAgentRequest, QueuedThreadSnapshot};
use super::requests::RequestState;
use super::threads::ThreadProjectionState;
use crate::{ThreadEvent, ThreadSnapshot};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct AgentRuntimeState {
    pub requests: Arc<Mutex<HashMap<String, super::requests::AgentRequestControl>>>,
    pub threads: Arc<Mutex<HashMap<String, ThreadSnapshot>>>,
    pub(super) thread_event_ids:
        Arc<Mutex<HashMap<String, VecDeque<(std::mem::Discriminant<ThreadEvent>, String)>>>>,
    pub(super) pending_thread_publications: Arc<Mutex<HashSet<String>>>,
    pub(super) thread_updates: Arc<Mutex<HashMap<String, super::threads::ThreadUpdateJournal>>>,
    pub(super) scheduler: Arc<Mutex<ThreadScheduler>>,
    pub permissions: Arc<Mutex<HashMap<(String, String), super::requests::PendingPermission>>>,
    pub session_approvals: Arc<Mutex<HashSet<(String, String)>>>,
    pub one_time_approvals: Arc<Mutex<HashSet<(String, String)>>>,
    /// P8-C：per-(session, model) 的 token 估算漂移校准器（EMA）。key 为
    /// "{session_id}:{model_id}"。内存态，重启重置——几轮报告后重新收敛。
    pub token_calibrators: Arc<Mutex<HashMap<String, crate::models::TokenEstimateCalibrator>>>,
    /// P8-E：per-session 的压缩触发滞回状态。压缩过一次之后触发线下移一段
    /// 安全带，避免「压完立刻又跨线」的抖动。内存态，重启重置。
    pub compaction_hysteresis: Arc<Mutex<HashMap<String, crate::models::CompactionHysteresis>>>,
}

impl ThreadProjectionState for AgentRuntimeState {
    fn threads_slot(&self) -> &Mutex<HashMap<String, ThreadSnapshot>> {
        &self.threads
    }
    fn updates_slot(&self) -> &Mutex<HashMap<String, super::threads::ThreadUpdateJournal>> {
        &self.thread_updates
    }
}

impl RequestState for AgentRuntimeState {
    fn requests_slot(&self) -> &Mutex<HashMap<String, super::requests::AgentRequestControl>> {
        &self.requests
    }
    fn permissions_slot(
        &self,
    ) -> &Mutex<HashMap<(String, String), super::requests::PendingPermission>> {
        &self.permissions
    }
    fn approvals_slot(&self) -> &Mutex<HashSet<(String, String)>> {
        &self.one_time_approvals
    }
}

impl QueuedThreadSnapshot for AgentRuntimeState {
    fn threads_slot(&self) -> &Mutex<HashMap<String, ThreadSnapshot>> {
        &self.threads
    }
}

#[derive(Default)]
pub(super) struct ThreadScheduler {
    pub(super) queued: HashMap<String, VecDeque<QueuedAgentRequest>>,
    pub(super) claimed: HashMap<String, QueuedAgentClaim>,
    pub(super) dispatching: HashSet<String>,
}
