use super::*;

pub(super) async fn prepare(
    app: &AppHandle,
    session_id: &str,
    turn_id: &str,
    request: &AgentChatRequest,
    is_ssh_target: bool,
    is_wsl_target: bool,
    cancellation: &Arc<AtomicBool>,
    cancellation_reason: &Arc<Mutex<Option<String>>>,
    cancellation_token: &tokio_util::sync::CancellationToken,
) -> Result<TurnEnvironments, String> {
    let environments = tokio::select! {
        _ = cancellation_token.cancelled() => {
            return Err(cancellation_message(&cancellation_reason));
        }
        environments = super::turn_prepare::prepare_with_timeout(
            super::turn_prepare::DEFAULT_PREPARE_TIMEOUT,
            &cancellation,
            setup::prepare_turn_environments(
                app,
                &request,
                session_id,
                turn_id,
                &cancellation,
                is_ssh_target,
                is_wsl_target,
            ),
        ) => environments?,
    };
    let TurnEnvironments {
        ssh_context,
        ssh_connection_id: mut_ssh_connection_id,
        wsl_root,
        ) => environments?,
    };
    Ok(environments)
}
