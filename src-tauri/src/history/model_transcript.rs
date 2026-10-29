use super::*;
use agent_core::ModelTranscript;

impl HistoryStore {
    pub(super) fn model_transcript_bytes(
        &self,
        session_id: &str,
    ) -> Result<Option<Vec<u8>>, String> {
        self.connection
            .query_row(
                "SELECT payload FROM model_transcripts WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())
    }

    pub(super) fn install_model_transcript(
        &self,
        session_id: &str,
        payload: Option<&[u8]>,
    ) -> Result<(), String> {
        if let Some(payload) = payload {
            self.connection
                .execute(
                    "INSERT INTO model_transcripts(session_id, payload) VALUES (?1, ?2)
                ON CONFLICT(session_id) DO UPDATE SET payload = excluded.payload",
                    params![session_id, payload],
                )
                .map_err(|e| e.to_string())?;
        } else {
            self.connection
                .execute(
                    "DELETE FROM model_transcripts WHERE session_id = ?1",
                    [session_id],
                )
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub(super) fn save_model_transcript(
        &self,
        session_id: &str,
        transcript: &ModelTranscript,
    ) -> Result<(), String> {
        use std::io::Write;
        if !transcript.is_replayable(&transcript.scope) {
            return Err("模型记录包含未配对的工具调用".into());
        }
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        serde_json::to_writer(&mut encoder, transcript).map_err(|e| e.to_string())?;
        encoder.flush().map_err(|e| e.to_string())?;
        self.install_model_transcript(
            session_id,
            Some(&encoder.finish().map_err(|e| e.to_string())?),
        )
    }

    pub(super) fn load_model_transcript(
        &self,
        session_id: &str,
        scope: &str,
        current_turn: &str,
    ) -> Result<Option<ModelTranscript>, String> {
        use std::io::Read;
        let Some(bytes) = self.model_transcript_bytes(session_id)? else {
            return Ok(None);
        };
        // Bound decoding of corrupt/imported snapshots. Raw images are already
        // bounded at admission; oversized snapshots fall back to display history.
        let decoder = flate2::read::GzDecoder::new(bytes.as_slice()).take(64 * 1024 * 1024);
        let Ok(transcript) = serde_json::from_reader::<_, ModelTranscript>(decoder) else {
            return Ok(None);
        };
        if !transcript.is_replayable(scope) {
            return Ok(None);
        }
        let Some(record) = self.get_record(session_id)? else {
            return Ok(None);
        };
        let latest_turn = ordered_context_messages(&record)
            .into_iter()
            .rev()
            .find_map(|message| {
                let id = message
                    .get("turnId")
                    .or_else(|| message.get("turn_id"))?
                    .as_str()?;
                (id != current_turn).then(|| id.to_string())
            });
        // A saved snapshot isn't committed history until its durable successful
        // terminal exists. This also rejects stale snapshots after edited input.
        // Validate against the durable table, not the UI's bounded event page.
        // Do not load all historical tool payloads on the native snapshot path.
        let completed: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_events WHERE session_id = ?1
             AND COALESCE(json_extract(payload, '$.turn_id'), json_extract(payload, '$.turnId')) = ?2
             AND json_extract(payload, '$.type') = 'completed'
             AND json_extract(payload, '$.durable_terminal') = 1)",
            params![session_id, transcript.turn_id], |row| row.get(0),
        ).map_err(|error| error.to_string())?;
        if latest_turn.as_deref() != Some(transcript.turn_id.as_str()) || !completed {
            return Ok(None);
        }
        Ok(Some(transcript))
    }
}

pub(crate) fn restore_model_transcript(
    app: &AppHandle,
    session_id: &str,
    scope: &str,
    current_turn: &str,
) -> Result<Option<ModelTranscript>, String> {
    let _guard = history_write_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    open_store(app)?.load_model_transcript(session_id, scope, current_turn)
}

pub(crate) fn save_model_transcript(
    app: &AppHandle,
    session_id: &str,
    transcript: &ModelTranscript,
) -> Result<(), String> {
    let _guard = history_write_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let store = open_store(app)?;
    if store.get_record(session_id)?.is_none() {
        return Err("历史会话不存在，无法保存模型上下文".into());
    }
    store.save_model_transcript(session_id, transcript)
}
