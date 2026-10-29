use super::*;

#[tauri::command]
pub fn index_local_logs(paths: Vec<String>) -> Result<Vec<LogIndexEntry>, String> {
    index_log_paths(&paths)
}

#[tauri::command]
pub fn search_local_logs(
    paths: Vec<String>,
    query: String,
    max_results: Option<usize>,
    max_excerpt_bytes: Option<usize>,
) -> Result<Vec<LogSearchHit>, String> {
    search_log_paths(
        &paths,
        &query,
        max_results.unwrap_or(32),
        max_excerpt_bytes.unwrap_or(512),
    )
}

#[tauri::command]
pub fn read_local_log_range(request: LogRangeRequest) -> Result<LogRangeResult, String> {
    read_log_range(
        &request.paths,
        request.source.as_deref(),
        request.member.as_deref(),
        request.start_line,
        request.max_lines,
        request.max_bytes,
    )
}

#[tauri::command]
pub fn rebuild_local_log_timeline(
    paths: Vec<String>,
    max_events: Option<usize>,
) -> Result<Vec<LogTimelineEvent>, String> {
    rebuild_timeline(&paths, max_events.unwrap_or(64))
}
