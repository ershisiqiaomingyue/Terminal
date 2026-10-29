use super::support::*;
use super::*;

#[test]
fn collaboration_capacity_is_global_across_jobs_and_released_by_terminal() {
    let store = HistoryStore::open_memory().expect("history database should open");
    seed_parent_thread(&store);
    let first_job = store
        .create_collaboration_job(collaboration_input(3))
        .expect("first collaboration job should be created");
    let second_job = store
        .create_collaboration_job(collaboration_input(2))
        .expect("second collaboration job should be created");
    let mut running = Vec::new();
    for agent in &first_job.agents {
        running.push(
            store
                .claim_collaboration_agent(&first_job.job.id, &agent.id, "owner-a")
                .expect("first job worker should claim"),
        );
    }
    running.push(
        store
            .claim_collaboration_agent(&second_job.job.id, &second_job.agents[0].id, "owner-a")
            .expect("fourth global worker should claim"),
    );

    let capacity_error = store
        .claim_collaboration_agent(&second_job.job.id, &second_job.agents[1].id, "owner-a")
        .expect_err("fifth worker across jobs must remain queued");
    assert_eq!(capacity_error, COLLABORATION_CAPACITY_ERROR);
    let queued: (String, i64, Option<String>) = store
        .connection
        .query_row(
            "SELECT status, attempt, lease_id FROM collaboration_agents WHERE id = ?1",
            params![second_job.agents[1].id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("queued worker should load");
    assert_eq!(queued, ("queued".to_string(), 0, None));

    let completed = &running[0];
    store
        .finish_collaboration_agent(
            &completed.agent.job_id,
            &completed.agent.id,
            completed.attempt,
            &completed.lease_id,
            "completed",
            Some("done"),
            None,
        )
        .expect("one worker should release durable capacity");
    let fifth = store
        .claim_collaboration_agent(&second_job.job.id, &second_job.agents[1].id, "owner-a")
        .expect("queued cross-job worker should claim after capacity is released");
    assert_eq!(fifth.agent.status, "running");
}

#[test]
fn dropped_collaboration_claim_releases_lease_and_stale_release_is_fenced() {
    let store = HistoryStore::open_memory().expect("history database should open");
    seed_parent_thread(&store);
    let job = store
        .create_collaboration_job(collaboration_input(1))
        .expect("collaboration job should be created");
    let agent_id = job.agents[0].id.clone();
    let first = store
        .claim_collaboration_agent(&job.job.id, &agent_id, "owner-a")
        .expect("first attempt should claim");
    let mut request = queued_request(&first.agent.session_id, "worker task");
    request.turn_id = Some(first.agent.turn_id.clone());
    store
        .start_collaboration_agent_run(
            &job.job.id,
            &agent_id,
            first.attempt,
            &first.lease_id,
            &first.agent.session_id,
            &first.agent.turn_id,
            &request,
        )
        .expect("worker run should start");

    assert!(store
        .release_collaboration_agent_claim(&job.job.id, &agent_id, first.attempt, &first.lease_id,)
        .expect("dropped claim should release"));
    let interrupted = store
        .collaboration_job_view(&job.job.id)
        .expect("job should load")
        .expect("job should exist");
    assert_eq!(interrupted.job.status, "interrupted");
    assert_eq!(interrupted.agents[0].status, "interrupted");
    let run = store
        .agent_run(&first.agent.session_id, &first.agent.turn_id)
        .expect("run should load")
        .expect("run should exist");
    assert_eq!(run.summary.status, "interrupted");
    assert!(run.summary.finished_at.is_none());

    let second = store
        .claim_collaboration_agent(&job.job.id, &agent_id, "owner-a")
        .expect("released worker should be claimable again");
    assert!(second.attempt > first.attempt);
    assert!(!store
        .release_collaboration_agent_claim(&job.job.id, &agent_id, first.attempt, &first.lease_id,)
        .expect("stale release should be a no-op"));
    let current = store
        .collaboration_agents(&job.job.id)
        .expect("agent should load")
        .into_iter()
        .next()
        .expect("agent should exist");
    assert_eq!(current.status, "running");
}

#[test]
fn collaboration_claim_rolls_back_agent_when_job_update_fails() {
    let store = HistoryStore::open_memory().expect("history database should open");
    seed_parent_thread(&store);
    let job = store
        .create_collaboration_job(collaboration_input(1))
        .expect("collaboration job should be created");
    let agent_id = job.agents[0].id.clone();
    store
        .connection
        .execute_batch(
            "CREATE TRIGGER fail_job_claim BEFORE UPDATE ON collaboration_jobs
                 BEGIN SELECT RAISE(ABORT, 'job update failed'); END;",
        )
        .expect("failure trigger should be installed");
    assert!(store
        .claim_collaboration_agent(&job.job.id, &agent_id, "owner-a")
        .is_err());
    let agent = store
        .collaboration_agents(&job.job.id)
        .expect("agents should load")
        .into_iter()
        .next()
        .expect("agent should exist");
    assert_eq!(agent.status, "queued");
}

#[test]
fn collaboration_run_worker_and_parent_terminal_commit_together() {
    let store = HistoryStore::open_memory().expect("history database should open");
    seed_parent_thread(&store);
    let job = store
        .create_collaboration_job(collaboration_input(1))
        .expect("collaboration job should be created");
    let agent_id = job.agents[0].id.clone();
    let dispatch = store
        .claim_collaboration_agent(&job.job.id, &agent_id, "owner-a")
        .expect("agent should be claimed");
    let mut request = queued_request(&dispatch.agent.session_id, "worker task");
    request.turn_id = Some(dispatch.agent.turn_id.clone());
    store
        .start_collaboration_agent_run(
            &job.job.id,
            &agent_id,
            dispatch.attempt,
            &dispatch.lease_id,
            &dispatch.agent.session_id,
            &dispatch.agent.turn_id,
            &request,
        )
        .expect("worker run should start");
    store
        .connection
        .execute_batch(
            "CREATE TRIGGER fail_job_finish BEFORE UPDATE ON collaboration_jobs
                 BEGIN SELECT RAISE(ABORT, 'job finish failed'); END;",
        )
        .expect("failure trigger should be installed");
    assert!(store
        .finish_collaboration_agent_turn(
            &job.job.id,
            &agent_id,
            dispatch.attempt,
            &dispatch.lease_id,
            &dispatch.agent.session_id,
            &dispatch.agent.turn_id,
            "completed",
            None,
            Some("done"),
            Some("done"),
            None,
        )
        .is_err());
    let agent = store
        .collaboration_agents(&job.job.id)
        .expect("agents should load")
        .into_iter()
        .next()
        .expect("agent should exist");
    assert_eq!(agent.status, "running");
    assert!(agent.finished_at.is_none());
    let run = store
        .agent_run(&dispatch.agent.session_id, &dispatch.agent.turn_id)
        .expect("run should load")
        .expect("run should exist");
    assert_eq!(run.summary.status, "preparing");
    assert!(run.summary.finished_at.is_none());
}

#[test]
fn collaboration_recovery_returns_each_interrupted_agent_once() {
    let store = HistoryStore::open_memory().expect("history database should open");
    seed_parent_thread(&store);
    let job = store
        .create_collaboration_job(collaboration_input(2))
        .expect("collaboration job should be created");
    for agent in &job.agents {
        store
            .claim_collaboration_agent(&job.job.id, &agent.id, "process-a")
            .expect("agent should be claimed");
    }
    let recovered = store
        .recover_collaboration_agents(&HashSet::new(), "process-b")
        .expect("running agents should recover");
    assert_eq!(recovered.len(), 2);
    let ids = recovered
        .iter()
        .map(|agent| agent.id.as_str())
        .collect::<HashSet<_>>();
    assert_eq!(ids.len(), 2);
    let fan_in = store
        .collaboration_fan_in_state(&job.job.id)
        .expect("fan-in should load")
        .expect("job should exist");
    assert!(!fan_in.terminal);
    assert_eq!(fan_in.pending_count, 2);
}

#[test]
fn collaboration_recovery_includes_never_started_queued_agents() {
    let store = HistoryStore::open_memory().expect("history database should open");
    seed_parent_thread(&store);
    let job = store
        .create_collaboration_job(collaboration_input(2))
        .expect("collaboration job should be created");

    let recovered = store
        .recover_collaboration_agents(&HashSet::new(), "process-b")
        .expect("queued agents should be recoverable");

    assert_eq!(recovered.len(), 2);
    assert!(recovered.iter().all(|agent| agent.status == "queued"));
    assert_eq!(
        store
            .collaboration_job_summary(&job.job.id)
            .expect("job should load")
            .expect("job should exist")
            .status,
        "queued"
    );
}

#[test]
fn stale_collaboration_attempt_cannot_commit_after_recovery() {
    let store = HistoryStore::open_memory().expect("history database should open");
    seed_parent_thread(&store);
    let job = store
        .create_collaboration_job(collaboration_input(1))
        .expect("collaboration job should be created");
    let agent_id = job.agents[0].id.clone();
    let first = store
        .claim_collaboration_agent(&job.job.id, &agent_id, "process-a")
        .expect("first attempt should claim");
    store
        .recover_collaboration_agents(&HashSet::new(), "process-b")
        .expect("first attempt should recover");
    let second = store
        .claim_collaboration_agent(&job.job.id, &agent_id, "process-b")
        .expect("second attempt should claim");
    assert!(second.attempt > first.attempt);

    assert!(store
        .finish_collaboration_agent(
            &job.job.id,
            &agent_id,
            first.attempt,
            &first.lease_id,
            "completed",
            Some("stale result"),
            None,
        )
        .is_err());
    let finished = store
        .finish_collaboration_agent(
            &job.job.id,
            &agent_id,
            second.attempt,
            &second.lease_id,
            "completed",
            Some("current result"),
            None,
        )
        .expect("current attempt should finish");
    assert_eq!(
        finished.agents[0].summary.as_deref(),
        Some("current result")
    );
}

#[test]
fn collaboration_retry_has_a_new_native_identity_without_replacing_prior_history() {
    let store = HistoryStore::open_memory().unwrap();
    seed_parent_thread(&store);
    let job = store
        .create_collaboration_job(collaboration_input(1))
        .unwrap();
    let agent = &job.agents[0];
    let first = store
        .claim_collaboration_agent(&job.job.id, &agent.id, "process-a")
        .unwrap();
    let mut request = queued_request(&first.agent.session_id, "original worker input");
    request.turn_id = Some(first.agent.turn_id.clone());
    store
        .start_collaboration_agent_run(
            &job.job.id,
            &agent.id,
            first.attempt,
            &first.lease_id,
            &first.agent.session_id,
            &first.agent.turn_id,
            &request,
        )
        .unwrap();
    store
        .recover_collaboration_agents(&HashSet::new(), "process-b")
        .unwrap();
    let interrupted = store
        .agent_run(&first.agent.session_id, &first.agent.turn_id)
        .unwrap()
        .unwrap();
    store
        .connection
        .execute_batch(
            "CREATE TRIGGER native_retry_claim_failure BEFORE UPDATE ON collaboration_jobs
            BEGIN SELECT RAISE(ABORT, 'retry claim rollback'); END;",
        )
        .unwrap();
    assert!(store
        .claim_collaboration_agent(&job.job.id, &agent.id, "process-b")
        .is_err());
    let rolled_back = store.collaboration_agents(&job.job.id).unwrap();
    assert_eq!(rolled_back[0].status, "interrupted");
    assert_eq!(rolled_back[0].turn_id, first.agent.turn_id);
    assert_eq!(
        store
            .agent_run(&first.agent.session_id, &first.agent.turn_id)
            .unwrap()
            .unwrap()
            .summary,
        interrupted.summary
    );
    store
        .connection
        .execute_batch("DROP TRIGGER native_retry_claim_failure;")
        .unwrap();
    let second = store
        .claim_collaboration_agent(&job.job.id, &agent.id, "process-b")
        .unwrap();
    assert_ne!(second.agent.turn_id, first.agent.turn_id);
    assert_eq!(second.agent.session_id, first.agent.session_id);
    let previous = store
        .agent_run(&first.agent.session_id, &first.agent.turn_id)
        .unwrap()
        .unwrap();
    assert_eq!(previous.summary.status, "interrupted");
    assert!(previous.summary.finished_at.is_some());
    request.turn_id = Some(second.agent.turn_id.clone());
    request.message = "resume using the durable outcomes".into();
    store
        .start_collaboration_agent_run(
            &job.job.id,
            &agent.id,
            second.attempt,
            &second.lease_id,
            &second.agent.session_id,
            &second.agent.turn_id,
            &request,
        )
        .unwrap();
    assert!(store
        .finish_collaboration_agent_turn(
            &job.job.id,
            &agent.id,
            first.attempt,
            &first.lease_id,
            &first.agent.session_id,
            &first.agent.turn_id,
            "completed",
            None,
            Some("late old result"),
            None,
            None
        )
        .is_err());
    store
        .finish_collaboration_agent_turn(
            &job.job.id,
            &agent.id,
            second.attempt,
            &second.lease_id,
            &second.agent.session_id,
            &second.agent.turn_id,
            "completed",
            None,
            Some("new result"),
            Some("new result"),
            None,
        )
        .unwrap();
    let retained = store
        .agent_run(&first.agent.session_id, &first.agent.turn_id)
        .unwrap()
        .unwrap();
    assert_eq!(retained.summary, previous.summary);
}

#[test]
fn parent_cancellation_cascades_to_queued_and_running_workers() {
    let store = HistoryStore::open_memory().expect("history database should open");
    seed_parent_thread(&store);
    let job = store
        .create_collaboration_job(collaboration_input(2))
        .expect("collaboration job should be created");
    let running = store
        .claim_collaboration_agent(&job.job.id, &job.agents[0].id, "owner-a")
        .expect("first worker should claim");

    let targets = store
        .request_collaboration_cancellation("parent-thread", Some("parent-turn"))
        .expect("parent cancellation should persist");
    assert_eq!(targets.len(), 2);
    let agents = store
        .collaboration_agents(&job.job.id)
        .expect("agents should load");
    assert_eq!(agents[0].status, "running");
    assert_eq!(agents[1].status, "cancelled");
    let running_cancel_requested: bool = store
        .connection
        .query_row(
            "SELECT cancel_requested FROM collaboration_agents WHERE id = ?1",
            params![running.agent.id],
            |row| row.get(0),
        )
        .expect("running cancellation flag should load");
    assert!(running_cancel_requested);
    let finished = store
        .finish_collaboration_agent(
            &job.job.id,
            &running.agent.id,
            running.attempt,
            &running.lease_id,
            "cancelled",
            None,
            Some("request cancelled"),
        )
        .expect("running worker should acknowledge cancellation");
    assert_eq!(finished.job.status, "cancelled");
    assert!(finished.job.finished_at.is_some());
}
