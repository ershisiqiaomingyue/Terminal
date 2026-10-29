use super::*;

/// G1：goal 一等公民的持久层。目标是一个跨多轮的生命周期对象——
/// 创建它的 turn 可能早已结束，goal 仍要活着并被下一个 turn 查询/推进。
/// 状态机对照 codex `thread_goals`（六态 + 预算），外加 parent_id 树。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalRecord {
    pub id: String,
    pub session_id: String,
    pub parent_id: Option<String>,
    pub objective: String,
    /// JSON 数组字符串（Vec<String> 的验收条件）。
    pub acceptance: String,
    pub status: String,
    pub blocked_reason: Option<String>,
    pub blocked_rounds: i64,
    pub round: i64,
    pub max_rounds: i64,
    pub token_budget: Option<i64>,
    pub tokens_used: i64,
    pub time_used_seconds: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

impl GoalRecord {
    pub fn acceptance_list(&self) -> Vec<String> {
        serde_json::from_str(&self.acceptance).unwrap_or_default()
    }
}

fn row_to_goal(row: &rusqlite::Row<'_>) -> rusqlite::Result<GoalRecord> {
    Ok(GoalRecord {
        id: row.get("id")?,
        session_id: row.get("session_id")?,
        parent_id: row.get("parent_id")?,
        objective: row.get("objective")?,
        acceptance: row.get("acceptance")?,
        status: row.get("status")?,
        blocked_reason: row.get("blocked_reason")?,
        blocked_rounds: row.get("blocked_rounds")?,
        round: row.get("round")?,
        max_rounds: row.get("max_rounds")?,
        token_budget: row.get("token_budget")?,
        tokens_used: row.get("tokens_used")?,
        time_used_seconds: row.get("time_used_seconds")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

impl HistoryStore {
    pub(super) fn create_goal(
        &self,
        goal: &GoalRecord,
    ) -> Result<GoalRecord, String> {
        self.connection
            .execute(
                "INSERT INTO goals(id, session_id, parent_id, objective, acceptance, status,
                    blocked_reason, blocked_rounds, round, max_rounds, token_budget,
                    tokens_used, time_used_seconds, created_at, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
                params![
                    goal.id, goal.session_id, goal.parent_id, goal.objective, goal.acceptance,
                    goal.status, goal.blocked_reason, goal.blocked_rounds, goal.round,
                    goal.max_rounds, goal.token_budget, goal.tokens_used, goal.time_used_seconds,
                    goal.created_at, goal.updated_at,
                ],
            )
            .map_err(|error| format!("无法创建目标: {error}"))?;
        Ok(goal.clone())
    }

    /// 每会话同时最多一个 active goal（对齐 codex 的 thread 级单 goal）。
    pub(super) fn active_goal(&self, session_id: &str) -> Result<Option<GoalRecord>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT * FROM goals WHERE session_id = ?1 AND status IN ('active','paused','blocked')
                 ORDER BY created_at DESC LIMIT 1",
            )
            .map_err(|error| format!("无法读取目标: {error}"))?;
        statement
            .query_row(params![session_id], row_to_goal)
            .optional()
            .map_err(|error| format!("无法读取目标: {error}"))
    }

    pub(super) fn goal_by_id(&self, goal_id: &str) -> Result<Option<GoalRecord>, String> {
        let mut statement = self
            .connection
            .prepare("SELECT * FROM goals WHERE id = ?1")
            .map_err(|error| format!("无法读取目标: {error}"))?;
        statement
            .query_row(params![goal_id], row_to_goal)
            .optional()
            .map_err(|error| format!("无法读取目标: {error}"))
    }

    /// 状态推进。终态（complete/dropped 及预算终态）不可逆——codex 同款约束，
    /// 防止模型把已宣布完成的目标悄悄改回 active。
    pub(super) fn update_goal_status(
        &self,
        goal_id: &str,
        status: &str,
        blocked_reason: Option<&str>,
        round_delta: i64,
        tokens_used_delta: i64,
        time_used_delta_seconds: i64,
    ) -> Result<bool, String> {
        if !matches!(
            status,
            "active" | "paused" | "blocked" | "usage_limited" | "budget_limited" | "complete" | "dropped"
        ) {
            return Err("未知的目标状态".to_string());
        }
        let changed = self
            .connection
            .execute(
                "UPDATE goals
                 SET status = ?2,
                     blocked_reason = ?3,
                     blocked_rounds = CASE WHEN ?2 = 'blocked' THEN blocked_rounds + 1 ELSE 0 END,
                     round = round + ?4,
                     tokens_used = tokens_used + ?5,
                     time_used_seconds = time_used_seconds + ?6,
                     updated_at = ?7
                 WHERE id = ?1 AND status NOT IN ('complete','dropped')",
                params![goal_id, status, blocked_reason, round_delta, tokens_used_delta, time_used_delta_seconds, now_ms()],
            )
            .map_err(|error| format!("无法更新目标: {error}"))?;
        Ok(changed == 1)
    }

    /// G2 预算归属：turn 的 token 花费累计（与状态推进解耦的独立通道）。
    /// 终态 goal 不再累计；goal 不存在时静默（job 可能先于 goal 被删）。
    pub(super) fn record_goal_usage(
        &self,
        goal_id: &str,
        tokens_delta: i64,
        seconds_delta: i64,
    ) -> Result<(), String> {
        self.connection
            .execute(
                "UPDATE goals
                 SET tokens_used = tokens_used + ?2,
                     time_used_seconds = time_used_seconds + ?3,
                     updated_at = ?4
                 WHERE id = ?1 AND status NOT IN ('complete','dropped')",
                params![goal_id, tokens_delta.max(0), seconds_delta.max(0), now_ms()],
            )
            .map_err(|error| format!("无法累计目标用量: {error}"))?;
        Ok(())
    }

    pub(super) fn goals_for_session(&self, session_id: &str) -> Result<Vec<GoalRecord>, String> {
        let mut statement = self
            .connection
            .prepare("SELECT * FROM goals WHERE session_id = ?1 ORDER BY created_at ASC")
            .map_err(|error| format!("无法读取目标: {error}"))?;
        let rows = statement
            .query_map(params![session_id], row_to_goal)
            .map_err(|error| format!("无法读取目标: {error}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("无法读取目标: {error}"))
    }
}

#[cfg(test)]
mod goal_tests {
    use super::*;

    fn goal(id: &str, session: &str) -> GoalRecord {
        GoalRecord {
            id: id.into(),
            session_id: session.into(),
            parent_id: None,
            objective: "把日志分析完".into(),
            acceptance: "[\"给出根因\"]".into(),
            status: "active".into(),
            blocked_reason: None,
            blocked_rounds: 0,
            round: 0,
            max_rounds: 8,
            token_budget: None,
            tokens_used: 0,
            time_used_seconds: 0,
            created_at: 1,
            updated_at: 1,
        }
    }

    #[test]
    fn goals_are_persisted_terminal_states_are_final_and_children_link_parents() {
        let store = HistoryStore::open_memory().unwrap();
        let mut first = goal("g1", "s");
        first.created_at = 1;
        store.create_goal(&first).unwrap();
        let mut child = goal("g2", "s");
        child.parent_id = Some("g1".into());
        child.created_at = 2;
        child.updated_at = 2;
        store.create_goal(&child).unwrap();

        // active goal = 最新创建的非终态目标（g2 后创建）。
        let active = store.active_goal("s").unwrap().unwrap();
        assert_eq!(active.id, "g2");
        assert_eq!(store.goals_for_session("s").unwrap().len(), 2);
        assert_eq!(
            store.goals_for_session("s").unwrap()[1].parent_id.as_deref(),
            Some("g1")
        );

        assert!(store.update_goal_status("g1", "complete", None, 1, 100, 10).unwrap());
        let completed = store.goal_by_id("g1").unwrap().unwrap();
        assert_eq!(completed.status, "complete");
        assert_eq!(completed.round, 1);
        // 终态不可逆：再 update 不生效。
        assert!(!store.update_goal_status("g1", "active", None, 0, 0, 0).unwrap());
        assert_eq!(store.goal_by_id("g1").unwrap().unwrap().status, "complete");
        // g1 终态后 active 落到仍存活的 g2。
        assert_eq!(store.active_goal("s").unwrap().unwrap().id, "g2");
        // 连续 blocked 累计轮次，解除后清零。
        assert!(store.update_goal_status("g2", "blocked", Some("等待用户"), 0, 0, 0).unwrap());
        assert_eq!(store.goal_by_id("g2").unwrap().unwrap().blocked_rounds, 1);
        assert!(store.update_goal_status("g2", "active", None, 0, 0, 0).unwrap());
        assert_eq!(store.goal_by_id("g2").unwrap().unwrap().blocked_rounds, 0);
    }
}
