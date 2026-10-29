use super::*;
use crate::file_changes::FileSnapshot;

/// Publish cumulative edits after each mutating tool. Await the final snapshot
/// before the terminal is persisted, so queued turns cannot steal its changes.
pub(super) async fn refresh_turn_file_changes(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    paths: &[String],
    before: &[FileSnapshot],
    previous: &[FileSnapshot],
    final_snapshot: bool,
) -> Option<Vec<FileSnapshot>> {
    let work_app = app.clone();
    let work_session = session_id.to_string();
    let work_turn = turn_id.to_string();
    let paths = paths.to_vec();
    let before = before.to_vec();
    let previous = previous.to_vec();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let after = agent_core::file_changes::capture_file_snapshot_since(
            &paths,
            &previous,
            &AtomicBool::new(false),
        )?;
        let mut changes = diff_file_snapshots(&before, &after);
        let backups = if final_snapshot {
            crate::file_changes::retain_changed_file_backups(
                &work_app,
                &work_session,
                &work_turn,
                &mut changes,
                &after,
            )
        } else {
            crate::file_changes::update_turn_file_backups(
                &work_app,
                &work_session,
                &work_turn,
                &mut changes,
                &after,
                false,
            )
        };
        if let Err(error) = backups {
            emit_agent_payload_for_turn(
                &work_app,
                json!({"type":"file_backup_error", "session_id":work_session, "message":error}),
                &work_turn,
            );
        }
        let mut payload = serde_json::to_value(AgentEvent::FileChanges {
            session_id: work_session.clone(),
            created: changes.created,
            modified: changes.modified,
            deleted: changes.deleted,
            added_lines: changes.added_lines,
            removed_lines: changes.removed_lines,
            entries: changes.entries,
        })
        .map_err(|error| error.to_string())?;
        payload["snapshot_complete"] = Value::Bool(true);
        emit_agent_payload_for_turn(&work_app, payload, &work_turn);
        Ok::<_, String>(after)
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|result| result);
    match result {
        Ok(after) => Some(after),
        Err(error) => {
            emit_agent_payload_for_turn(
                app,
                json!({"type":"file_snapshot_error", "session_id":session_id, "phase":"after", "code":"file_snapshot", "retryable":false, "message":error}),
                turn_id,
            );
            None
        }
    }
}
