use super::*;

/// 未落地的终态描述。`status` 与 `agent_runs` 的终态取值一致。
#[derive(Debug, Clone)]
pub(crate) struct UnpersistedTerminal {
    pub session_id: String,
    pub turn_id: String,
    pub status: String,
    pub message: Option<String>,
}

impl HistoryStore {
    /// N12-A：找出「终态只在 live 会话里、没进库」的回合。
    ///
    /// 病灶：`finish_admitted_agent_turn_with_retry` 三次写库都失败时，run 的
    /// `finished_at` 仍是 NULL。下次启动 `recover_interrupted_runs` 会把它标成
    /// `interrupted`——用户明明看到「失败/已完成」，重启后却变成「被中断」，
    /// 而且终态文本彻底丢失（N12 的原始诉求就是「终态落库失败要能自愈」）。
    ///
    /// 判据刻意收得很窄，避免把正常的活回合误修：
    /// 1. run 未 finished 且状态不是终态（活跃回合本来就不该被修）；
    /// 2. run 不在 `active`（当前无内存控制项）；
    /// 3. 该会话在库里没有任何事件带 `terminal_persistence_failed` 标记——
    ///    这条把「上次启动已经由本函数修过、但标记还在」的行挡住，不会重复改写入库。
    pub(super) fn unpersisted_terminal_turns(
        &self,
        active: &HashSet<(String, String)>,
    ) -> Result<Vec<UnpersistedTerminal>, String> {
        let candidates = {
            let mut statement = self
                .connection
                .prepare(
                    "SELECT session_id, turn_id, status FROM agent_runs
                     WHERE finished_at IS NULL
                       AND status NOT IN ('completed', 'cancelled', 'failed')",
                )
                .map_err(|error| format!("无法读取待补偿的 Agent 终态: {error}"))?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(|error| format!("无法读取待补偿的 Agent 终态: {error}"))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|error| format!("无法读取待补偿的 Agent 终态: {error}"))?
        };
        let mut repaired = Vec::new();
        for (session_id, turn_id, _status) in candidates {
            if active.contains(&(session_id.clone(), turn_id.clone())) {
                continue;
            }
            let flagged = self
                .connection
                .query_row(
                    "SELECT 1 FROM session_events
                     WHERE session_id = ?1
                       AND COALESCE(json_extract(payload, '$.turn_id'),
                                    json_extract(payload, '$.turnId')) = ?2
                       AND json_extract(payload, '$.terminal_persistence_failed') = 1
                     LIMIT 1",
                    params![session_id, turn_id],
                    |_| Ok(()),
                )
                .optional()
                .map_err(|error| format!("无法检查终态补偿标记: {error}"))?
                .is_some();
            if !flagged {
                continue;
            }
            // 从 live 会话事件里还原终态类型：优先用最接近终态的耐久事件，
            // 没有事件时退回 failed（不谎报成功）。
            let recovered = self.recover_terminal_payload(&session_id, &turn_id)?;
            repaired.push(recovered);
        }
        Ok(repaired)
    }

    /// 从事件流里还原终态：看最后一条 durable 终态事件；若缺失则按失败处理。
    fn recover_terminal_payload(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<UnpersistedTerminal, String> {
        let payload: Option<String> = self
            .connection
            .query_row(
                "SELECT payload FROM session_events
                 WHERE session_id = ?1
                   AND COALESCE(json_extract(payload, '$.turn_id'),
                                json_extract(payload, '$.turnId')) = ?2
                   AND json_extract(payload, '$.durable_terminal') IS NOT NULL
                 ORDER BY timestamp_ms DESC, rowid DESC LIMIT 1",
                params![session_id, turn_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("无法读取终态事件: {error}"))?;
        let value = payload
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .unwrap_or(Value::Null);
        let status = match value.get("type").and_then(Value::as_str) {
            Some("completed") => "completed",
            Some("cancelled") => "cancelled",
            _ => "failed",
        };
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                value
                    .get("termination_reason")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        Ok(UnpersistedTerminal {
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            status: status.to_string(),
            message,
        })
    }

    /// N12-A：补偿落库。只在 `unpersisted_terminal_turns` 之后调用。
    pub(super) fn repair_unpersisted_terminal(
        &self,
        terminal: &UnpersistedTerminal,
    ) -> Result<bool, String> {
        let changed = self
            .connection
            .execute(
                "UPDATE agent_runs
                 SET status = ?1, status_message = ?2, updated_at = ?3, finished_at = ?3
                 WHERE session_id = ?4 AND turn_id = ?5 AND finished_at IS NULL",
                params![
                    terminal.status,
                    terminal.message,
                    now_ms(),
                    terminal.session_id,
                    terminal.turn_id
                ],
            )
            .map_err(|error| format!("无法补偿 Agent 终态: {error}"))?;
        if changed == 0 {
            return Ok(false);
        }
        // 终态一旦落地，该轮悬挂的追加要求也该收口（`reject_agent_steers_for_turn`
        // 自己会返回并回收 id；这里只关心落库，不需要再广播——启动时没有 live UI）。
        let _ = self.reject_agent_steers_for_turn(&terminal.session_id, &terminal.turn_id)?;
        Ok(true)
    }
}
