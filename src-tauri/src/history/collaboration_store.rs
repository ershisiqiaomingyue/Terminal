use super::*;

impl HistoryStore {
    pub(super) fn normalized_collaboration_permission_mode(
        value: Option<&str>,
    ) -> Result<String, String> {
        match value.unwrap_or("ask").trim() {
            "ask" | "risk" | "full" => Ok(value.unwrap_or("ask").trim().to_string()),
            _ => Err("协作 Agent 权限模式必须是 ask、risk 或 full".to_string()),
        }
    }

    pub(super) fn collaboration_job_summary(
        &self,
        job_id: &str,
    ) -> Result<Option<CollaborationJobSummary>, String> {
        self.connection
            .query_row(
                "SELECT id, thread_id, parent_turn_id, goal, workspace_path, permission_mode, execution_target, agent_config,
                        aggregate_summary, status, created_at, updated_at, finished_at
                 FROM collaboration_jobs WHERE id = ?1",
                params![job_id],
                |row| {
                    Ok(CollaborationJobSummary {
                        id: row.get(0)?,
                        thread_id: row.get(1)?,
                        parent_turn_id: row.get(2)?,
                        goal: row.get(3)?,
                        workspace_path: row.get(4)?,
                        permission_mode: row.get(5)?,
                        execution_target: decode_optional(row.get::<_, Option<String>>(6)?),
                        agent_config: serde_json::from_str(&row.get::<_, String>(7)?)
                            .map_err(|error| rusqlite::Error::FromSqlConversionFailure(
                                7,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            ))?,
                        aggregate_summary: row.get(8)?,
                        status: row.get(9)?,
                        created_at: row.get(10)?,
                        updated_at: row.get(11)?,
                        finished_at: row.get(12)?,
                    })
                },
            )
            .optional()
            .map_err(|error| format!("无法读取协作作业: {error}"))
    }

    pub(super) fn collaboration_agents(
        &self,
        job_id: &str,
    ) -> Result<Vec<CollaborationAgentSummary>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id, job_id, title, status, session_id, turn_id, workspace_path, permission_mode,
                        summary, error, created_at, updated_at, finished_at
                 FROM collaboration_agents WHERE job_id = ?1 ORDER BY created_at ASC",
            )
            .map_err(|error| format!("无法读取协作 Agent: {error}"))?;
        let rows = statement
            .query_map(params![job_id], |row| {
                Ok(CollaborationAgentSummary {
                    id: row.get(0)?,
                    job_id: row.get(1)?,
                    title: row.get(2)?,
                    status: row.get(3)?,
                    session_id: row.get(4)?,
                    turn_id: row.get(5)?,
                    workspace_path: row.get(6)?,
                    permission_mode: row.get(7)?,
                    summary: row.get(8)?,
                    error: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                    finished_at: row.get(12)?,
                })
            })
            .map_err(|error| format!("无法读取协作 Agent: {error}"))?;
        rows.map(|row| row.map_err(|error| format!("协作 Agent 数据无效: {error}")))
            .collect()
    }

    pub(super) fn collaboration_job_view(
        &self,
        job_id: &str,
    ) -> Result<Option<CollaborationJobView>, String> {
        let Some(job) = self.collaboration_job_summary(job_id)? else {
            return Ok(None);
        };
        Ok(Some(CollaborationJobView {
            agents: self.collaboration_agents(job_id)?,
            job,
        }))
    }

    pub(super) fn collaboration_fan_in_state(
        &self,
        job_id: &str,
    ) -> Result<Option<CollaborationFanInState>, String> {
        self.collaboration_job_view(job_id)
            .map(|view| view.map(CollaborationFanInState::from))
    }

    pub(super) fn list_collaboration_jobs(
        &self,
        thread_id: Option<&str>,
    ) -> Result<Vec<CollaborationJobView>, String> {
        let query = if thread_id.is_some() {
            "SELECT id FROM collaboration_jobs WHERE thread_id = ?1 ORDER BY updated_at DESC"
        } else {
            "SELECT id FROM collaboration_jobs ORDER BY updated_at DESC"
        };
        let mut statement = self
            .connection
            .prepare(query)
            .map_err(|error| format!("无法读取协作作业: {error}"))?;
        let ids = if let Some(thread_id) = thread_id {
            statement
                .query_map(params![thread_id], |row| row.get::<_, String>(0))
                .map_err(|error| format!("无法读取协作作业: {error}"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|error| format!("协作作业数据无效: {error}"))?
        } else {
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|error| format!("无法读取协作作业: {error}"))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|error| format!("协作作业数据无效: {error}"))?
        };
        ids.into_iter()
            .map(|id| {
                self.collaboration_job_view(&id)?
                    .ok_or_else(|| "协作作业在读取期间被删除".to_string())
            })
            .collect()
    }

    pub(super) fn has_collaboration_jobs(&self, thread_id: &str) -> Result<bool, String> {
        self.connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM collaboration_jobs WHERE thread_id = ?1)",
                params![thread_id],
                |row| row.get(0),
            )
            .map_err(|error| format!("无法检查对话关联的协作作业: {error}"))
    }

    pub(super) fn collaboration_job_goal_id(
        &self,
        job_id: &str,
    ) -> Result<Option<String>, String> {
        self.connection
            .query_row(
                "SELECT goal_id FROM collaboration_jobs WHERE id = ?1",
                params![job_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map(|row| row.flatten())
            .map_err(|error| format!("无法读取协作作业的目标关联: {error}"))
    }

    pub(super) fn create_collaboration_job(
        &self,
        input: CollaborationJobInput,
    ) -> Result<CollaborationJobView, String> {
        let thread_id = input.thread_id.trim();
        let goal = input.goal.trim();
        if thread_id.is_empty() || goal.is_empty() {
            return Err("协作作业必须指定对话线程和目标".to_string());
        }
        if input.agents.is_empty() || input.agents.len() > 8 {
            return Err("每个协作作业必须包含 1 到 8 个子 Agent".to_string());
        }
        if self.conversation_thread(thread_id)?.is_none() {
            return Err("协作作业所属的对话线程不存在".to_string());
        }
        let permission_mode =
            Self::normalized_collaboration_permission_mode(input.permission_mode.as_deref())?;
        let agent_config = input.agent_config;
        if agent_config.model_id.trim().is_empty() {
            return Err("协作作业必须指定子 Agent 使用的模型".to_string());
        }
        let workspace_path = input
            .workspace_path
            .filter(|path| !path.trim().is_empty())
            .map(|path| path.trim().to_string());
        let job_id = next_history_id("collab");
        let timestamp = now_ms();
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法开始创建协作作业: {error}"))?;
        let result = (|| {
            self.connection
                .execute(
                    "INSERT INTO collaboration_jobs
                        (id, thread_id, parent_turn_id, goal, goal_id, workspace_path, permission_mode, execution_target, agent_config, status, created_at, updated_at, finished_at)
                     VALUES (?1, ?2, ?3, ?4, ?10, ?5, ?6, ?7, ?8, 'queued', ?9, ?9, NULL)",
                    params![
                        job_id,
                        thread_id,
                        input.parent_turn_id.filter(|value| !value.trim().is_empty()),
                        goal,
                        workspace_path,
                        permission_mode,
                        input.execution_target.as_ref().filter(|value| !value.is_null()).map(encode).transpose()?,
                        encode(&serde_json::to_value(&agent_config).map_err(|error| format!("无法序列化协作 Agent 配置: {error}"))?)?,
                        timestamp,
                        input.goal_id.filter(|value| !value.trim().is_empty()),
                    ],
                )
                .map_err(|error| format!("无法保存协作作业: {error}"))?;
            for agent in &input.agents {
                let title = agent.title.trim();
                let instruction = agent.instruction.trim();
                if title.is_empty() || instruction.is_empty() {
                    return Err("每个子 Agent 都必须包含名称和任务说明".to_string());
                }
                let agent_workspace = agent
                    .workspace_path
                    .as_ref()
                    .filter(|path| !path.trim().is_empty())
                    .map(|path| path.trim().to_string())
                    .or_else(|| workspace_path.clone());
                let agent_permission = Self::normalized_collaboration_permission_mode(
                    agent
                        .permission_mode
                        .as_deref()
                        .or(Some(permission_mode.as_str())),
                )?;
                let agent_id = next_history_id("collab-agent");
                self.connection
                    .execute(
                        "INSERT INTO collaboration_agents
                            (id, job_id, title, instruction, workspace_path, permission_mode, session_id, turn_id, status, summary, error, created_at, updated_at, finished_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'queued', NULL, NULL, ?9, ?9, NULL)",
                        params![
                            agent_id,
                            job_id,
                            title,
                            instruction,
                            agent_workspace,
                            agent_permission,
                            next_history_id("collab-session"),
                            next_history_id("collab-turn"),
                            timestamp,
                        ],
                    )
                    .map_err(|error| format!("无法保存子 Agent: {error}"))?;
            }
            Ok::<(), String>(())
        })();
        match result {
            Ok(()) => self.commit_transaction("无法提交协作作业")?,
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                return Err(error);
            }
        }
        self.collaboration_job_view(&job_id)?
            .ok_or_else(|| "协作作业创建后无法读取".to_string())
    }

    pub(super) fn claim_collaboration_agent(
        &self,
        job_id: &str,
        agent_id: &str,
        lease_owner: &str,
    ) -> Result<CollaborationDispatch, String> {
        if lease_owner.trim().is_empty() {
            return Err("领取协作子 Agent 必须指定 lease owner".to_string());
        }
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法锁定协作子 Agent: {error}"))?;
        let result = (|| {
            let row = self
            .connection
            .query_row(
                "SELECT a.id, a.job_id, a.title, a.instruction, a.workspace_path, a.permission_mode,
                        a.session_id, a.turn_id, a.status, a.summary, a.error, a.created_at, a.updated_at, a.finished_at,
                        a.attempt, a.cancel_requested,
                        j.goal, j.execution_target, j.agent_config, j.cancel_requested
                 FROM collaboration_agents a JOIN collaboration_jobs j ON j.id = a.job_id
                 WHERE a.id = ?1 AND a.job_id = ?2",
                params![agent_id, job_id],
                |row| {
                    Ok((
                        CollaborationAgentSummary {
                            id: row.get(0)?,
                            job_id: row.get(1)?,
                            title: row.get(2)?,
                            status: row.get(8)?,
                            session_id: row.get(6)?,
                            turn_id: row.get(7)?,
                            workspace_path: row.get(4)?,
                            permission_mode: row.get(5)?,
                            summary: row.get(9)?,
                            error: row.get(10)?,
                            created_at: row.get(11)?,
                            updated_at: row.get(12)?,
                            finished_at: row.get(13)?,
                        },
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(14)?,
                        row.get::<_, bool>(15)?,
                        row.get::<_, String>(16)?,
                        decode_optional(row.get::<_, Option<String>>(17)?),
                        serde_json::from_str(&row.get::<_, String>(18)?)
                            .map_err(|error| rusqlite::Error::FromSqlConversionFailure(
                                18,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            ))?,
                        row.get::<_, bool>(19)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| format!("无法读取子 Agent: {error}"))?
            .ok_or_else(|| "没有找到指定的协作子 Agent".to_string())?;
            if row.3 || row.7 || (row.0.status != "queued" && row.0.status != "interrupted") {
                if row.3 || row.7 {
                    return Err("协作作业已请求取消，不能再调度子 Agent".to_string());
                }
                return Err("该子 Agent 当前不可调度".to_string());
            }
            let running: usize = self
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM collaboration_agents WHERE status = 'running'",
                    [],
                    |row| row.get(0),
                )
                .map_err(|error| format!("无法检查全局协作额度: {error}"))?;
            if running >= MAX_COLLABORATION_CONCURRENCY {
                return Err(COLLABORATION_CAPACITY_ERROR.to_string());
            }
            let timestamp = now_ms();
            let lease_id = next_history_id("collab-lease");
            let lease_expires_at = timestamp.saturating_add(COLLABORATION_LEASE_MS);
            let attempt = row.2.saturating_add(1);
            // A renewed lease is a new native submission, not a content edit
            // of the old inbound message. Keep the session and prior run intact.
            let turn_id = if row.2 > 0 {
                next_history_id("collab-turn")
            } else {
                row.0.turn_id.clone()
            };
            if turn_id != row.0.turn_id {
                self.connection
                    .execute(
                        "UPDATE agent_runs SET status = 'interrupted',
                        status_message = '旧尝试已中断；后续执行使用独立的新轮次。',
                        updated_at = ?1, finished_at = ?1
                     WHERE session_id = ?2 AND turn_id = ?3 AND finished_at IS NULL",
                        params![timestamp, row.0.session_id, row.0.turn_id],
                    )
                    .map_err(|error| format!("无法保存旧协作尝试的中断状态: {error}"))?;
            }
            let changed = self
                .connection
                .execute(
                    "UPDATE collaboration_agents
                     SET status = 'running', error = NULL, updated_at = ?1, finished_at = NULL,
                         attempt = attempt + 1, lease_id = ?2, lease_owner = ?3,
                         lease_expires_at = ?4, cancel_requested = 0, turn_id = ?7
                     WHERE id = ?5 AND job_id = ?6 AND status IN ('queued', 'interrupted')
                       AND cancel_requested = 0
                       AND EXISTS(SELECT 1 FROM collaboration_jobs
                                  WHERE id = ?6 AND cancel_requested = 0)",
                    params![
                        timestamp,
                        lease_id,
                        lease_owner,
                        lease_expires_at,
                        agent_id,
                        job_id,
                        turn_id,
                    ],
                )
                .map_err(|error| format!("无法领取子 Agent: {error}"))?;
            if changed == 0 {
                return Err("该子 Agent 已被其他调度器领取".to_string());
            }
            self.connection
                .execute(
                    "UPDATE collaboration_jobs SET status = 'running', updated_at = ?1, finished_at = NULL
                     WHERE id = ?2 AND cancel_requested = 0",
                    params![timestamp, job_id],
                )
                .map_err(|error| format!("无法更新协作作业状态: {error}"))?;
            Ok(CollaborationDispatch {
                agent: CollaborationAgentSummary {
                    turn_id,
                    status: "running".to_string(),
                    updated_at: timestamp,
                    finished_at: None,
                    error: None,
                    ..row.0
                },
                instruction: row.1,
                attempt,
                lease_id,
                lease_owner: lease_owner.to_string(),
                lease_expires_at,
                job_goal: row.4,
                execution_target: row.5,
                agent_config: row.6,
            })
        })();
        match result {
            Ok(value) => {
                self.commit_transaction("无法提交协作子 Agent 领取")?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub(super) fn refresh_collaboration_job_state_locked(
        &self,
        job_id: &str,
        timestamp: i64,
    ) -> Result<(), String> {
        let agents = self.collaboration_agents(job_id)?;
        let cancel_requested = self
            .connection
            .query_row(
                "SELECT cancel_requested FROM collaboration_jobs WHERE id = ?1",
                params![job_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|error| format!("无法读取协作作业取消状态: {error}"))?;
        let queued = agents.iter().any(|agent| agent.status == "queued");
        let running = agents.iter().any(|agent| agent.status == "running");
        let active = queued || running;
        let interrupted = agents.iter().any(|agent| agent.status == "interrupted");
        let awaiting_completion = active || interrupted;
        let failed = agents.iter().any(|agent| agent.status == "failed");
        let cancelled = agents.iter().any(|agent| agent.status == "cancelled");
        let job_status = if cancel_requested {
            if awaiting_completion {
                "cancelling"
            } else {
                "cancelled"
            }
        } else if running {
            "running"
        } else if interrupted {
            "interrupted"
        } else if queued {
            "queued"
        } else if failed {
            "completed_with_errors"
        } else if cancelled {
            "cancelled"
        } else {
            "completed"
        };
        let aggregate_summary = if awaiting_completion {
            None
        } else {
            Some(
                agents
                    .iter()
                    .map(|agent| {
                        let result = agent
                            .summary
                            .as_deref()
                            .or(agent.error.as_deref())
                            .unwrap_or("没有返回结果")
                            .chars()
                            .take(2_000)
                            .collect::<String>();
                        format!("[{}] {}\n{}", agent.title, agent.status, result)
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n")
                    .chars()
                    .take(16_000)
                    .collect::<String>(),
            )
        };
        self.connection
            .execute(
                "UPDATE collaboration_jobs
                 SET aggregate_summary = ?1, status = ?2, updated_at = ?3,
                     finished_at = CASE WHEN ?4 THEN ?3 ELSE NULL END
                 WHERE id = ?5",
                params![
                    aggregate_summary,
                    job_status,
                    timestamp,
                    !awaiting_completion,
                    job_id,
                ],
            )
            .map_err(|error| format!("无法更新协作作业终态: {error}"))?;
        Ok(())
    }

    pub(super) fn finish_collaboration_agent(
        &self,
        job_id: &str,
        agent_id: &str,
        attempt: i64,
        lease_id: &str,
        status: &str,
        summary: Option<&str>,
        error: Option<&str>,
    ) -> Result<CollaborationJobView, String> {
        if !matches!(status, "completed" | "failed" | "cancelled") {
            return Err("协作子 Agent 终态无效".to_string());
        }
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法锁定协作作业终态: {error}"))?;
        let result = (|| {
            let timestamp = now_ms();
            let changed = self
                .connection
                .execute(
                    "UPDATE collaboration_agents
                 SET status = ?1, summary = ?2, error = ?3, updated_at = ?4, finished_at = ?4,
                     lease_id = NULL, lease_owner = NULL, lease_expires_at = NULL
                 WHERE id = ?5 AND job_id = ?6 AND status = 'running'
                   AND attempt = ?7 AND lease_id = ?8",
                    params![
                        status,
                        summary.map(|value| value.chars().take(12_000).collect::<String>()),
                        error.map(|value| value.chars().take(4_000).collect::<String>()),
                        timestamp,
                        agent_id,
                        job_id,
                        attempt,
                        lease_id,
                    ],
                )
                .map_err(|error| format!("无法结束协作子 Agent: {error}"))?;
            if changed == 0 {
                let existing = self
                    .connection
                    .query_row(
                        "SELECT status, attempt FROM collaboration_agents WHERE id = ?1 AND job_id = ?2",
                        params![agent_id, job_id],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                    )
                    .optional()
                    .map_err(|error| format!("无法检查协作子 Agent 终态: {error}"))?
                    .ok_or_else(|| "没有找到指定的协作子 Agent".to_string())?;
                if existing.0 != status || existing.1 != attempt {
                    return Err("协作子 Agent attempt 已失效或已处于另一终态".to_string());
                }
            }
            self.refresh_collaboration_job_state_locked(job_id, timestamp)?;
            Ok::<(), String>(())
        })();
        match result {
            Ok(()) => self.commit_transaction("无法提交协作作业终态")?,
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                return Err(error);
            }
        }
        self.collaboration_job_view(job_id)?
            .ok_or_else(|| "协作作业结束后无法读取".to_string())
    }

    /// Releases a live claim whose task future disappeared before committing a
    /// terminal state. The attempt and lease fence prevents an old future from
    /// interrupting a replacement worker after recovery.
    pub(super) fn release_collaboration_agent_claim(
        &self,
        job_id: &str,
        agent_id: &str,
        attempt: i64,
        lease_id: &str,
    ) -> Result<bool, String> {
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法锁定协作子 Agent lease: {error}"))?;
        let result = (|| {
            let current = self
                .connection
                .query_row(
                    "SELECT a.session_id, a.turn_id,
                            (a.cancel_requested = 1 OR j.cancel_requested = 1)
                     FROM collaboration_agents a
                     JOIN collaboration_jobs j ON j.id = a.job_id
                     WHERE a.id = ?1 AND a.job_id = ?2 AND a.status = 'running'
                       AND a.attempt = ?3 AND a.lease_id = ?4",
                    params![agent_id, job_id, attempt, lease_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, bool>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(|error| format!("无法读取协作子 Agent lease: {error}"))?;
            let Some((session_id, turn_id, cancellation_requested)) = current else {
                return Ok(false);
            };
            let (status, message) = if cancellation_requested {
                ("cancelled", "父任务已取消")
            } else {
                (
                    "interrupted",
                    "协作子 Agent 执行已中断；释放 lease 后可重新调度。",
                )
            };
            let timestamp = now_ms();
            let changed = self
                .connection
                .execute(
                    "UPDATE collaboration_agents
                     SET status = ?1, error = ?2, updated_at = ?3,
                         finished_at = CASE WHEN ?1 = 'cancelled' THEN ?3 ELSE NULL END,
                         cancel_requested = CASE WHEN ?1 = 'cancelled' THEN 1 ELSE cancel_requested END,
                         lease_id = NULL, lease_owner = NULL, lease_expires_at = NULL
                     WHERE id = ?4 AND job_id = ?5 AND status = 'running'
                       AND attempt = ?6 AND lease_id = ?7",
                    params![status, message, timestamp, agent_id, job_id, attempt, lease_id],
                )
                .map_err(|error| format!("无法释放协作子 Agent lease: {error}"))?;
            if changed == 0 {
                return Ok(false);
            }
            self.connection
                .execute(
                    "UPDATE agent_runs
                     SET status = ?1, status_message = ?2, updated_at = ?3,
                         finished_at = CASE WHEN ?1 = 'cancelled' THEN ?3 ELSE NULL END
                     WHERE session_id = ?4 AND turn_id = ?5 AND finished_at IS NULL",
                    params![status, message, timestamp, session_id, turn_id],
                )
                .map_err(|error| format!("无法释放协作 Agent 运行记录: {error}"))?;
            self.refresh_collaboration_job_state_locked(job_id, timestamp)?;
            Ok(true)
        })();
        match result {
            Ok(released) => {
                self.commit_transaction("无法提交协作子 Agent lease 释放")?;
                Ok(released)
            }
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub(super) fn start_collaboration_agent_run(
        &self,
        job_id: &str,
        agent_id: &str,
        attempt: i64,
        lease_id: &str,
        session_id: &str,
        turn_id: &str,
        request: &AgentChatRequest,
    ) -> Result<(), String> {
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法锁定协作 Agent 运行记录: {error}"))?;
        let result = (|| {
            let current = self
                .connection
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM collaboration_agents a
                        JOIN collaboration_jobs j ON j.id = a.job_id
                        WHERE a.id = ?1 AND a.job_id = ?2 AND a.session_id = ?3 AND a.turn_id = ?4
                          AND a.status = 'running' AND a.attempt = ?5 AND a.lease_id = ?6
                          AND a.cancel_requested = 0 AND j.cancel_requested = 0
                    )",
                    params![agent_id, job_id, session_id, turn_id, attempt, lease_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(|error| format!("无法验证协作 Agent attempt: {error}"))?;
            if !current {
                return Err("协作子 Agent attempt 已失效或已请求取消".to_string());
            }
            self.start_agent_run_locked(session_id, turn_id, request)
        })();
        match result {
            Ok(()) => self.commit_transaction("无法提交协作 Agent 运行记录"),
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn finish_collaboration_agent_turn(
        &self,
        job_id: &str,
        agent_id: &str,
        attempt: i64,
        lease_id: &str,
        session_id: &str,
        turn_id: &str,
        status: &str,
        message: Option<&str>,
        assistant_content: Option<&str>,
        summary: Option<&str>,
        error: Option<&str>,
    ) -> Result<CollaborationJobView, String> {
        if !matches!(status, "completed" | "failed" | "cancelled") {
            return Err("协作子 Agent 终态无效".to_string());
        }
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法锁定协作 Agent 原子终态: {error}"))?;
        let result = (|| {
            let timestamp = now_ms();
            let changed = self
                .connection
                .execute(
                    "UPDATE collaboration_agents
                     SET status = ?1, summary = ?2, error = ?3, updated_at = ?4,
                         finished_at = ?4, lease_id = NULL, lease_owner = NULL,
                         lease_expires_at = NULL
                     WHERE id = ?5 AND job_id = ?6 AND session_id = ?7 AND turn_id = ?8
                       AND status = 'running' AND attempt = ?9 AND lease_id = ?10",
                    params![
                        status,
                        summary.map(|value| value.chars().take(12_000).collect::<String>()),
                        error.map(|value| value.chars().take(4_000).collect::<String>()),
                        timestamp,
                        agent_id,
                        job_id,
                        session_id,
                        turn_id,
                        attempt,
                        lease_id,
                    ],
                )
                .map_err(|error| format!("无法结束协作子 Agent: {error}"))?;
            if changed == 0 {
                let existing = self
                    .connection
                    .query_row(
                        "SELECT status, attempt FROM collaboration_agents
                         WHERE id = ?1 AND job_id = ?2 AND session_id = ?3 AND turn_id = ?4",
                        params![agent_id, job_id, session_id, turn_id],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                    )
                    .optional()
                    .map_err(|error| format!("无法检查协作 Agent 原子终态: {error}"))?
                    .ok_or_else(|| "没有找到指定的协作子 Agent".to_string())?;
                if existing.0 != status || existing.1 != attempt {
                    return Err("协作子 Agent attempt 已失效或已处于另一终态".to_string());
                }
            }
            self.upsert_agent_turn_messages(session_id, turn_id, "", assistant_content)?;
            self.finish_agent_run_locked(session_id, turn_id, status, message)?;
            self.refresh_collaboration_job_state_locked(job_id, timestamp)
        })();
        match result {
            Ok(()) => self.commit_transaction("无法提交协作 Agent 原子终态")?,
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                return Err(error);
            }
        }
        self.collaboration_job_view(job_id)?
            .ok_or_else(|| "协作作业结束后无法读取".to_string())
    }

    pub(super) fn request_collaboration_cancellation(
        &self,
        thread_id: &str,
        parent_turn_id: Option<&str>,
    ) -> Result<Vec<CollaborationCancellationTarget>, String> {
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法锁定协作取消状态: {error}"))?;
        let result = (|| {
            let job_ids = {
                let (sql, bind_turn) = if parent_turn_id.is_some() {
                    (
                        "SELECT id FROM collaboration_jobs
                         WHERE thread_id = ?1 AND parent_turn_id = ?2
                           AND status IN ('queued', 'running', 'interrupted', 'cancelling')",
                        true,
                    )
                } else {
                    (
                        "SELECT id FROM collaboration_jobs
                         WHERE thread_id = ?1
                           AND status IN ('queued', 'running', 'interrupted', 'cancelling')",
                        false,
                    )
                };
                let mut statement = self
                    .connection
                    .prepare(sql)
                    .map_err(|error| format!("无法读取待取消协作作业: {error}"))?;
                if bind_turn {
                    statement
                        .query_map(params![thread_id, parent_turn_id], |row| {
                            row.get::<_, String>(0)
                        })
                        .map_err(|error| format!("无法读取待取消协作作业: {error}"))?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(|error| format!("待取消协作作业无效: {error}"))?
                } else {
                    statement
                        .query_map(params![thread_id], |row| row.get::<_, String>(0))
                        .map_err(|error| format!("无法读取待取消协作作业: {error}"))?
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .map_err(|error| format!("待取消协作作业无效: {error}"))?
                }
            };
            let timestamp = now_ms();
            let mut targets = Vec::new();
            for job_id in job_ids {
                {
                    let mut statement = self
                        .connection
                        .prepare(
                            "SELECT job_id, id, session_id, turn_id FROM collaboration_agents
                             WHERE job_id = ?1 AND status IN ('queued', 'running', 'interrupted')",
                        )
                        .map_err(|error| format!("无法读取运行中的协作 Agent: {error}"))?;
                    targets.extend(
                        statement
                            .query_map(params![job_id], |row| {
                                Ok(CollaborationCancellationTarget {
                                    job_id: row.get(0)?,
                                    agent_id: row.get(1)?,
                                    session_id: row.get(2)?,
                                    turn_id: row.get(3)?,
                                })
                            })
                            .map_err(|error| format!("无法读取运行中的协作 Agent: {error}"))?
                            .collect::<rusqlite::Result<Vec<_>>>()
                            .map_err(|error| format!("运行中的协作 Agent 无效: {error}"))?,
                    );
                }
                self.connection
                    .execute(
                        "UPDATE collaboration_agents
                         SET status = 'cancelled', cancel_requested = 1,
                             error = COALESCE(error, '父任务已取消'), updated_at = ?1,
                             finished_at = ?1, lease_id = NULL, lease_owner = NULL,
                             lease_expires_at = NULL
                         WHERE job_id = ?2 AND status IN ('queued', 'interrupted')",
                        params![timestamp, job_id],
                    )
                    .map_err(|error| format!("无法取消待运行协作 Agent: {error}"))?;
                self.connection
                    .execute(
                        "UPDATE collaboration_agents SET cancel_requested = 1, updated_at = ?1
                         WHERE job_id = ?2 AND status = 'running'",
                        params![timestamp, job_id],
                    )
                    .map_err(|error| format!("无法请求取消运行中的协作 Agent: {error}"))?;
                self.connection
                    .execute(
                        "UPDATE collaboration_jobs SET cancel_requested = 1, updated_at = ?1
                         WHERE id = ?2",
                        params![timestamp, job_id],
                    )
                    .map_err(|error| format!("无法请求取消协作作业: {error}"))?;
                self.refresh_collaboration_job_state_locked(&job_id, timestamp)?;
            }
            Ok::<_, String>(targets)
        })();
        match result {
            Ok(targets) => {
                self.commit_transaction("无法提交协作取消状态")?;
                Ok(targets)
            }
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    pub(super) fn recover_collaboration_agents(
        &self,
        active_sessions: &HashSet<String>,
        lease_owner: &str,
    ) -> Result<Vec<CollaborationAgentSummary>, String> {
        if lease_owner.trim().is_empty() {
            return Err("恢复协作 Agent 必须指定当前 lease owner".to_string());
        }
        self.connection
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| format!("无法锁定协作恢复状态: {error}"))?;
        let result = (|| {
            let running_agents = {
                let mut statement = self
                    .connection
                    .prepare(
                        "SELECT a.id, a.job_id, a.session_id, a.turn_id,
                                a.cancel_requested, j.cancel_requested,
                                a.lease_owner, a.lease_expires_at
                         FROM collaboration_agents a
                         JOIN collaboration_jobs j ON j.id = a.job_id
                         WHERE a.status = 'running'",
                    )
                    .map_err(|error| format!("无法读取待恢复协作 Agent: {error}"))?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, bool>(4)?,
                            row.get::<_, bool>(5)?,
                            row.get::<_, Option<String>>(6)?,
                            row.get::<_, Option<i64>>(7)?,
                        ))
                    })
                    .map_err(|error| format!("无法读取待恢复协作 Agent: {error}"))?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|error| format!("待恢复协作 Agent 数据无效: {error}"))?;
                rows
            };
            let timestamp = now_ms();
            for (
                agent_id,
                _job_id,
                session_id,
                turn_id,
                agent_cancel_requested,
                job_cancel_requested,
                prior_lease_owner,
                lease_expires_at,
            ) in running_agents
            {
                if active_sessions.contains(&session_id) {
                    continue;
                }
                let cancellation_requested = agent_cancel_requested || job_cancel_requested;
                let stale_lease = prior_lease_owner.as_deref() != Some(lease_owner)
                    || lease_expires_at.is_none_or(|expires_at| expires_at <= timestamp);
                if !cancellation_requested && !stale_lease {
                    continue;
                }
                if cancellation_requested {
                    self.connection
                        .execute(
                            "UPDATE collaboration_agents
                             SET status = 'cancelled', cancel_requested = 1,
                                 error = COALESCE(error, '父任务已取消'), updated_at = ?1,
                                 finished_at = ?1, lease_id = NULL, lease_owner = NULL,
                                 lease_expires_at = NULL
                             WHERE id = ?2 AND status = 'running'",
                            params![timestamp, agent_id],
                        )
                        .map_err(|error| format!("无法恢复已取消的协作 Agent: {error}"))?;
                    self.connection
                        .execute(
                            "UPDATE agent_runs
                             SET status = 'cancelled', status_message = '父任务已取消',
                                 updated_at = ?1, finished_at = ?1
                             WHERE session_id = ?2 AND turn_id = ?3 AND finished_at IS NULL",
                            params![timestamp, session_id, turn_id],
                        )
                        .map_err(|error| format!("无法取消协作 Agent 运行记录: {error}"))?;
                } else {
                    self.connection
                        .execute(
                            "UPDATE collaboration_agents
                             SET status = 'interrupted',
                                 error = '应用关闭前子 Agent 未收到终态；可重新调度。',
                                 updated_at = ?1, finished_at = NULL,
                                 lease_id = NULL, lease_owner = NULL, lease_expires_at = NULL
                             WHERE id = ?2 AND status = 'running'",
                            params![timestamp, agent_id],
                        )
                        .map_err(|error| format!("无法恢复协作 Agent: {error}"))?;
                    self.connection
                        .execute(
                            "UPDATE agent_runs
                             SET status = 'interrupted',
                                 status_message = '应用关闭前该任务未收到终态；可基于当前工作区继续处理。',
                                 updated_at = ?1
                             WHERE session_id = ?2 AND turn_id = ?3 AND finished_at IS NULL",
                            params![timestamp, session_id, turn_id],
                        )
                        .map_err(|error| format!("无法中断协作 Agent 运行记录: {error}"))?;
                }
            }
            self.connection
                .execute(
                    "UPDATE collaboration_agents
                     SET status = 'cancelled', cancel_requested = 1,
                         error = COALESCE(error, '父任务已取消'), updated_at = ?1,
                         finished_at = ?1, lease_id = NULL, lease_owner = NULL,
                         lease_expires_at = NULL
                     WHERE status IN ('queued', 'interrupted')
                       AND (cancel_requested = 1 OR job_id IN (
                           SELECT id FROM collaboration_jobs WHERE cancel_requested = 1
                       ))",
                    params![timestamp],
                )
                .map_err(|error| format!("无法恢复已取消的待运行协作 Agent: {error}"))?;
            let job_ids = {
                let mut statement = self
                    .connection
                    .prepare(
                        "SELECT id FROM collaboration_jobs
                         WHERE status IN ('queued', 'running', 'interrupted', 'cancelling')
                         ORDER BY updated_at DESC",
                    )
                    .map_err(|error| format!("无法读取待恢复协作作业: {error}"))?;
                let ids = statement
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(|error| format!("无法读取待恢复协作作业: {error}"))?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(|error| format!("待恢复协作作业数据无效: {error}"))?;
                ids
            };
            for job_id in &job_ids {
                self.refresh_collaboration_job_state_locked(job_id, timestamp)?;
            }
            let mut recovered = Vec::new();
            for job_id in job_ids {
                recovered.extend(
                    self.collaboration_agents(&job_id)?
                        .into_iter()
                        .filter(|agent| matches!(agent.status.as_str(), "queued" | "interrupted")),
                );
            }
            Ok::<_, String>(recovered)
        })();
        match result {
            Ok(value) => {
                self.commit_transaction("无法提交协作恢复状态")?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.connection.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }
}
