use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use uuid::Uuid;

use crate::{
    domain::{
        BriefItem, BriefItemKind, DictationResult, EditedTranscript, MeetingBrief, ProcessingMode,
        RawTranscript, RecordingSession, SessionId, SessionKind, SessionSource, SessionStatus,
        TranscriptCitation, TranscriptSegment,
    },
    error::{CoreError, CoreResult},
};

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('dictation', 'meeting')),
    source TEXT NOT NULL,
    title TEXT,
    status TEXT NOT NULL,
    processing_mode TEXT,
    provider TEXT,
    audio_path TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    pinned INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS sessions_expiry ON sessions(expires_at, pinned);

CREATE TABLE IF NOT EXISTS raw_transcripts (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL UNIQUE REFERENCES sessions(id) ON DELETE CASCADE,
    language TEXT NOT NULL,
    provider TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS raw_segments (
    id TEXT PRIMARY KEY,
    transcript_id TEXT NOT NULL REFERENCES raw_transcripts(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    start_ms INTEGER NOT NULL,
    end_ms INTEGER NOT NULL,
    speaker TEXT,
    text TEXT NOT NULL,
    UNIQUE(transcript_id, ordinal)
);

CREATE TABLE IF NOT EXISTS edited_transcripts (
    id TEXT PRIMARY KEY,
    raw_transcript_id TEXT NOT NULL REFERENCES raw_transcripts(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(raw_transcript_id, revision)
);
CREATE TABLE IF NOT EXISTS edited_segments (
    edited_transcript_id TEXT NOT NULL REFERENCES edited_transcripts(id) ON DELETE CASCADE,
    raw_segment_id TEXT NOT NULL REFERENCES raw_segments(id),
    text TEXT NOT NULL,
    speaker TEXT,
    PRIMARY KEY(edited_transcript_id, raw_segment_id)
);

CREATE TABLE IF NOT EXISTS dictation_results (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL UNIQUE REFERENCES sessions(id) ON DELETE CASCADE,
    raw_transcript_id TEXT NOT NULL REFERENCES raw_transcripts(id),
    text TEXT NOT NULL,
    corrected INTEGER NOT NULL,
    insertion_status TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS dictation_timings (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    capture_ms INTEGER NOT NULL,
    transcription_ms INTEGER NOT NULL,
    correction_ms INTEGER NOT NULL,
    insertion_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS meeting_briefs (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL UNIQUE REFERENCES sessions(id) ON DELETE CASCADE,
    transcript_id TEXT NOT NULL REFERENCES raw_transcripts(id),
    overview TEXT NOT NULL,
    topics_json TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS brief_items (
    id TEXT PRIMARY KEY,
    brief_id TEXT NOT NULL REFERENCES meeting_briefs(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    text TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS brief_citations (
    item_id TEXT NOT NULL REFERENCES brief_items(id) ON DELETE CASCADE,
    segment_id TEXT NOT NULL REFERENCES raw_segments(id),
    start_ms INTEGER NOT NULL,
    end_ms INTEGER NOT NULL,
    PRIMARY KEY(item_id, segment_id, start_ms, end_ms)
);

CREATE TABLE IF NOT EXISTS provider_attempts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    processing_mode TEXT NOT NULL,
    outcome TEXT NOT NULL,
    latency_ms INTEGER,
    created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS pending_enhancements (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    operation TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TEXT NOT NULL,
    last_error TEXT,
    PRIMARY KEY(session_id, operation)
);

CREATE TABLE IF NOT EXISTS personal_vocabulary (
    id TEXT PRIMARY KEY,
    heard TEXT NOT NULL,
    replacement TEXT NOT NULL,
    observations INTEGER NOT NULL DEFAULT 1,
    learned INTEGER NOT NULL DEFAULT 0,
    updated_at TEXT NOT NULL,
    UNIQUE(heard, replacement)
);
CREATE TABLE IF NOT EXISTS context_snapshots (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    process_name TEXT NOT NULL,
    window_title TEXT,
    selected_text TEXT,
    surrounding_text TEXT,
    created_at TEXT NOT NULL
);

CREATE VIRTUAL TABLE IF NOT EXISTS meeting_search USING fts5(
    session_id UNINDEXED,
    title,
    transcript,
    speakers,
    brief
);

PRAGMA user_version = 1;
"#;

pub struct Repository {
    connection: Connection,
}

#[derive(Debug, Clone)]
pub struct VocabularyRecord {
    pub id: Uuid,
    pub heard: String,
    pub replacement: String,
    pub observations: u32,
    pub learned: bool,
}

#[derive(Debug, Clone)]
pub struct DictationRecord {
    pub result: DictationResult,
    pub raw: RawTranscript,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DictationTimingRecord {
    pub capture_ms: u64,
    pub transcription_ms: u64,
    pub correction_ms: u64,
    pub insertion_ms: u64,
}

impl Repository {
    pub fn open(path: &Path) -> CoreResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        let mut repository = Self { connection };
        repository.migrate()?;
        Ok(repository)
    }

    #[cfg(test)]
    pub fn in_memory() -> CoreResult<Self> {
        let connection = Connection::open_in_memory()?;
        let mut repository = Self { connection };
        repository.migrate()?;
        Ok(repository)
    }

    fn migrate(&mut self) -> CoreResult<()> {
        self.connection.execute_batch(SCHEMA)?;
        Ok(())
    }

    pub fn create_session(&mut self, session: &RecordingSession) -> CoreResult<()> {
        self.connection.execute(
            "INSERT INTO sessions (id, kind, source, title, status, processing_mode, provider, audio_path, created_at, updated_at, expires_at, pinned)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                session.id.to_string(),
                session.kind.as_str(),
                session.source.as_str(),
                session.title,
                session.status.as_str(),
                session.processing_mode.map(ProcessingMode::as_str),
                session.provider,
                session.audio_path,
                timestamp(session.created_at),
                timestamp(session.updated_at),
                timestamp(session.expires_at),
                session.pinned,
            ],
        )?;
        Ok(())
    }

    pub fn list_sessions(&self, kind: Option<SessionKind>) -> CoreResult<Vec<RecordingSession>> {
        let sql = if kind.is_some() {
            "SELECT id, kind, source, title, status, processing_mode, provider, audio_path, created_at, updated_at, expires_at, pinned FROM sessions WHERE kind = ?1 ORDER BY created_at DESC"
        } else {
            "SELECT id, kind, source, title, status, processing_mode, provider, audio_path, created_at, updated_at, expires_at, pinned FROM sessions ORDER BY created_at DESC"
        };
        let mut statement = self.connection.prepare(sql)?;
        let map = |row: &rusqlite::Row<'_>| read_session(row);
        let rows = if let Some(kind) = kind {
            statement
                .query_map([kind.as_str()], map)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            statement
                .query_map([], map)?
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(rows)
    }

    pub fn session(&self, session_id: SessionId) -> CoreResult<RecordingSession> {
        self.connection
            .query_row(
                "SELECT id, kind, source, title, status, processing_mode, provider, audio_path, created_at, updated_at, expires_at, pinned FROM sessions WHERE id = ?1",
                [session_id.to_string()],
                read_session,
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("session {session_id}")))
    }

    pub fn set_session_audio_path(
        &mut self,
        session_id: SessionId,
        audio_path: &Path,
    ) -> CoreResult<()> {
        let changed = self.connection.execute(
            "UPDATE sessions SET audio_path = ?2, updated_at = ?3 WHERE id = ?1",
            params![
                session_id.to_string(),
                audio_path.to_string_lossy(),
                timestamp(Utc::now())
            ],
        )?;
        ensure_changed(changed, "session", session_id)
    }

    pub fn set_session_expiry(
        &mut self,
        session_id: SessionId,
        expires_at: DateTime<Utc>,
    ) -> CoreResult<()> {
        let changed = self.connection.execute(
            "UPDATE sessions SET expires_at = ?2, updated_at = ?3 WHERE id = ?1",
            params![
                session_id.to_string(),
                timestamp(expires_at),
                timestamp(Utc::now())
            ],
        )?;
        ensure_changed(changed, "session", session_id)
    }

    pub fn set_session_title(&mut self, session_id: SessionId, title: &str) -> CoreResult<()> {
        let changed = self.connection.execute(
            "UPDATE sessions SET title = ?2, updated_at = ?3 WHERE id = ?1",
            params![session_id.to_string(), title, timestamp(Utc::now())],
        )?;
        ensure_changed(changed, "session", session_id)
    }

    pub fn set_session_state(
        &mut self,
        session_id: SessionId,
        status: SessionStatus,
        mode: Option<ProcessingMode>,
        provider: Option<&str>,
    ) -> CoreResult<()> {
        let changed = self.connection.execute(
            "UPDATE sessions SET status = ?2, processing_mode = ?3, provider = ?4, updated_at = ?5 WHERE id = ?1",
            params![
                session_id.to_string(),
                status.as_str(),
                mode.map(ProcessingMode::as_str),
                provider,
                timestamp(Utc::now()),
            ],
        )?;
        ensure_changed(changed, "session", session_id)
    }

    pub fn set_pinned(&mut self, session_id: SessionId, pinned: bool) -> CoreResult<()> {
        let kind: Option<String> = self
            .connection
            .query_row(
                "SELECT kind FROM sessions WHERE id = ?1",
                [session_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        match kind.as_deref() {
            Some("meeting") => {
                self.connection.execute(
                    "UPDATE sessions SET pinned = ?2, updated_at = ?3 WHERE id = ?1",
                    params![session_id.to_string(), pinned, timestamp(Utc::now())],
                )?;
                Ok(())
            }
            Some(_) => Err(CoreError::InvalidInput(
                "dictation sessions cannot be pinned".into(),
            )),
            None => Err(CoreError::NotFound(format!("session {session_id}"))),
        }
    }

    /// Raw transcripts are insert-only. The unique session constraint prevents replacement.
    pub fn insert_raw_transcript(&mut self, transcript: &RawTranscript) -> CoreResult<()> {
        transcript.validate()?;
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO raw_transcripts (id, session_id, language, provider, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                transcript.id.to_string(),
                transcript.session_id.to_string(),
                transcript.language,
                transcript.provider,
                timestamp(transcript.created_at),
            ],
        )?;
        for segment in &transcript.segments {
            transaction.execute(
                "INSERT INTO raw_segments (id, transcript_id, ordinal, start_ms, end_ms, speaker, text) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    segment.id.to_string(), transcript.id.to_string(), segment.ordinal,
                    segment.start_ms, segment.end_ms, segment.speaker, segment.text,
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn raw_transcript(&self, transcript_id: Uuid) -> CoreResult<RawTranscript> {
        let header = self
            .connection
            .query_row(
                "SELECT session_id, language, provider, created_at FROM raw_transcripts WHERE id = ?1",
                [transcript_id.to_string()],
                |row| Ok((row.get::<_, String>(0)?, row.get(1)?, row.get(2)?, row.get::<_, String>(3)?)),
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("raw transcript {transcript_id}")))?;
        let mut statement = self.connection.prepare(
            "SELECT id, ordinal, start_ms, end_ms, speaker, text FROM raw_segments WHERE transcript_id = ?1 ORDER BY ordinal",
        )?;
        let segments = statement
            .query_map([transcript_id.to_string()], |row| {
                Ok(TranscriptSegment {
                    id: parse_uuid_sql(row.get::<_, String>(0)?)?,
                    ordinal: row.get(1)?,
                    start_ms: row.get(2)?,
                    end_ms: row.get(3)?,
                    speaker: row.get(4)?,
                    text: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(RawTranscript {
            id: transcript_id,
            session_id: parse_uuid(&header.0)?,
            language: header.1,
            provider: header.2,
            created_at: parse_timestamp(&header.3)?,
            segments,
        })
    }

    pub fn raw_transcript_for_session(
        &self,
        session_id: SessionId,
    ) -> CoreResult<Option<RawTranscript>> {
        let transcript_id = self
            .connection
            .query_row(
                "SELECT id FROM raw_transcripts WHERE session_id = ?1",
                [session_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        transcript_id
            .map(|id| parse_uuid(&id).and_then(|id| self.raw_transcript(id)))
            .transpose()
    }

    pub fn latest_edited_transcript(
        &self,
        raw_transcript_id: Uuid,
    ) -> CoreResult<Option<EditedTranscript>> {
        let header = self
            .connection
            .query_row(
                "SELECT id, revision, created_at FROM edited_transcripts WHERE raw_transcript_id = ?1 ORDER BY revision DESC LIMIT 1",
                [raw_transcript_id.to_string()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?, row.get::<_, String>(2)?)),
            )
            .optional()?;
        let Some((id, revision, created_at)) = header else {
            return Ok(None);
        };
        let mut statement = self.connection.prepare(
            "SELECT raw_segment_id, text, speaker FROM edited_segments WHERE edited_transcript_id = ?1 ORDER BY rowid",
        )?;
        let segments = statement
            .query_map([id.clone()], |row| {
                Ok(crate::domain::EditedSegment {
                    raw_segment_id: parse_uuid_sql(row.get::<_, String>(0)?)?,
                    text: row.get(1)?,
                    speaker: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(EditedTranscript {
            id: parse_uuid(&id)?,
            raw_transcript_id,
            revision,
            created_at: parse_timestamp(&created_at)?,
            segments,
        }))
    }

    pub fn insert_edited_transcript(&mut self, edited: &EditedTranscript) -> CoreResult<()> {
        let raw = self.raw_transcript(edited.raw_transcript_id)?;
        let raw_ids = raw
            .segments
            .iter()
            .map(|segment| segment.id)
            .collect::<std::collections::HashSet<_>>();
        if edited.segments.len() != raw_ids.len()
            || edited
                .segments
                .iter()
                .any(|segment| !raw_ids.contains(&segment.raw_segment_id))
        {
            return Err(CoreError::InvalidInput(
                "an edited transcript must preserve every raw segment link".into(),
            ));
        }
        let expected: u32 = self.connection.query_row(
            "SELECT COALESCE(MAX(revision), 0) + 1 FROM edited_transcripts WHERE raw_transcript_id = ?1",
            [edited.raw_transcript_id.to_string()],
            |row| row.get(0),
        )?;
        if edited.revision != expected {
            return Err(CoreError::InvalidInput(format!(
                "expected transcript revision {expected}, got {}",
                edited.revision
            )));
        }
        let transaction = self.connection.transaction()?;
        insert_edited(&transaction, edited)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn insert_dictation_result(&mut self, result: &DictationResult) -> CoreResult<()> {
        self.connection.execute(
            "INSERT INTO dictation_results (id, session_id, raw_transcript_id, text, corrected, insertion_status, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![result.id.to_string(), result.session_id.to_string(), result.raw_transcript_id.to_string(), result.text, result.corrected, result.insertion_status, timestamp(result.created_at)],
        )?;
        Ok(())
    }

    pub fn insert_dictation_timing(
        &mut self,
        session_id: SessionId,
        timing: DictationTimingRecord,
    ) -> CoreResult<()> {
        self.connection.execute(
            "INSERT INTO dictation_timings (session_id, capture_ms, transcription_ms, correction_ms, insertion_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session_id.to_string(),
                timing.capture_ms,
                timing.transcription_ms,
                timing.correction_ms,
                timing.insertion_ms,
            ],
        )?;
        Ok(())
    }

    pub fn dictation_timing(
        &self,
        session_id: SessionId,
    ) -> CoreResult<Option<DictationTimingRecord>> {
        self.connection
            .query_row(
                "SELECT capture_ms, transcription_ms, correction_ms, insertion_ms FROM dictation_timings WHERE session_id = ?1",
                [session_id.to_string()],
                |row| {
                    Ok(DictationTimingRecord {
                        capture_ms: row.get(0)?,
                        transcription_ms: row.get(1)?,
                        correction_ms: row.get(2)?,
                        insertion_ms: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn dictation_result(&self, session_id: SessionId) -> CoreResult<DictationRecord> {
        let row = self
            .connection
            .query_row(
                "SELECT id, raw_transcript_id, text, corrected, insertion_status, created_at
                 FROM dictation_results WHERE session_id = ?1",
                [session_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, bool>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| CoreError::NotFound(format!("dictation result for {session_id}")))?;
        let raw_transcript_id = parse_uuid(&row.1)?;
        Ok(DictationRecord {
            result: DictationResult {
                id: parse_uuid(&row.0)?,
                session_id,
                raw_transcript_id,
                text: row.2,
                corrected: row.3,
                insertion_status: row.4,
                created_at: parse_timestamp(&row.5)?,
            },
            raw: self.raw_transcript(raw_transcript_id)?,
        })
    }

    pub fn update_dictation_result_text(
        &mut self,
        session_id: SessionId,
        text: &str,
        corrected: bool,
    ) -> CoreResult<()> {
        if text.trim().is_empty() {
            return Err(CoreError::InvalidInput(
                "transformed dictation text cannot be empty".into(),
            ));
        }
        let changed = self.connection.execute(
            "UPDATE dictation_results SET text = ?2, corrected = ?3 WHERE session_id = ?1",
            params![session_id.to_string(), text, corrected],
        )?;
        ensure_changed(changed, "dictation result", session_id)
    }

    /// Clears user-owned records while keeping the open SQLite connection valid.
    pub fn clear_user_data(&mut self) -> CoreResult<()> {
        let transaction = self.connection.transaction()?;
        transaction.execute("DELETE FROM pending_enhancements", [])?;
        transaction.execute("DELETE FROM provider_attempts", [])?;
        transaction.execute("DELETE FROM personal_vocabulary", [])?;
        transaction.execute("DELETE FROM meeting_search", [])?;
        transaction.execute("DELETE FROM sessions", [])?;
        transaction.commit()?;
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }

    pub fn insert_meeting_brief(&mut self, brief: &MeetingBrief) -> CoreResult<()> {
        let transcript = self.raw_transcript(brief.transcript_id)?;
        brief.validate_against(&transcript)?;
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO meeting_briefs (id, session_id, transcript_id, overview, topics_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![brief.id.to_string(), brief.session_id.to_string(), brief.transcript_id.to_string(), brief.overview, serde_json::to_string(&brief.detailed_topics)?, timestamp(brief.created_at)],
        )?;
        for item in &brief.items {
            transaction.execute(
                "INSERT INTO brief_items (id, brief_id, kind, text) VALUES (?1, ?2, ?3, ?4)",
                params![
                    item.id.to_string(),
                    brief.id.to_string(),
                    item.kind.as_str(),
                    item.text
                ],
            )?;
            for citation in &item.citations {
                transaction.execute(
                    "INSERT INTO brief_citations (item_id, segment_id, start_ms, end_ms) VALUES (?1, ?2, ?3, ?4)",
                    params![item.id.to_string(), citation.segment_id.to_string(), citation.start_ms, citation.end_ms],
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn meeting_brief(&self, session_id: SessionId) -> CoreResult<Option<MeetingBrief>> {
        let header = self.connection.query_row(
            "SELECT id, transcript_id, overview, topics_json, created_at FROM meeting_briefs WHERE session_id = ?1",
            [session_id.to_string()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?, row.get::<_, String>(4)?)),
        ).optional()?;
        let Some((id, transcript_id, overview, topics, created_at)) = header else {
            return Ok(None);
        };
        let brief_id = parse_uuid(&id)?;
        let mut statement = self
            .connection
            .prepare("SELECT id, kind, text FROM brief_items WHERE brief_id = ?1")?;
        let item_rows = statement
            .query_map([id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut items = Vec::with_capacity(item_rows.len());
        for (item_id, kind, text) in item_rows {
            let mut citations_statement = self.connection.prepare(
                "SELECT segment_id, start_ms, end_ms FROM brief_citations WHERE item_id = ?1",
            )?;
            let citations = citations_statement
                .query_map([item_id.clone()], |row| {
                    Ok(TranscriptCitation {
                        segment_id: parse_uuid_sql(row.get::<_, String>(0)?)?,
                        start_ms: row.get(1)?,
                        end_ms: row.get(2)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            items.push(BriefItem {
                id: parse_uuid(&item_id)?,
                kind: parse_brief_kind(&kind)?,
                text,
                citations,
            });
        }
        Ok(Some(MeetingBrief {
            id: brief_id,
            session_id,
            transcript_id: parse_uuid(&transcript_id)?,
            overview,
            detailed_topics: serde_json::from_str(&topics)?,
            items,
            created_at: parse_timestamp(&created_at)?,
        }))
    }

    pub fn queue_enhancement(
        &mut self,
        session_id: SessionId,
        operation: &str,
        error: Option<&str>,
    ) -> CoreResult<()> {
        let previous_attempts = self
            .connection
            .query_row(
                "SELECT attempts FROM pending_enhancements WHERE session_id = ?1 AND operation = ?2",
                params![session_id.to_string(), operation],
                |row| row.get::<_, u32>(0),
            )
            .optional()?;
        let attempts = previous_attempts
            .map(|value| value.saturating_add(1))
            .unwrap_or(0);
        let delay_seconds = if attempts == 0 {
            0
        } else {
            15_i64
                .saturating_mul(1_i64 << attempts.saturating_sub(1).min(6))
                .min(15 * 60)
        };
        let next_attempt = Utc::now() + Duration::seconds(delay_seconds);
        self.connection.execute(
            "INSERT INTO pending_enhancements (session_id, operation, attempts, next_attempt_at, last_error) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(session_id, operation) DO UPDATE SET attempts = excluded.attempts, next_attempt_at = excluded.next_attempt_at, last_error = excluded.last_error",
            params![session_id.to_string(), operation, attempts, timestamp(next_attempt), error],
        )?;
        Ok(())
    }

    pub fn pending_enhancement_sessions(&self) -> CoreResult<Vec<SessionId>> {
        let mut statement = self.connection.prepare(
            "SELECT DISTINCT session_id FROM pending_enhancements WHERE next_attempt_at <= ?1 ORDER BY next_attempt_at",
        )?;
        let sessions = statement
            .query_map([timestamp(Utc::now())], |row| {
                parse_uuid_sql(row.get::<_, String>(0)?)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(sessions)
    }

    /// Atomically lease due sessions so connectivity and manual retry workers cannot start the
    /// same pending enhancement concurrently. A crashed worker becomes due again after ten minutes.
    pub fn claim_due_pending_enhancement_sessions(
        &mut self,
        now: DateTime<Utc>,
        limit: u32,
    ) -> CoreResult<Vec<SessionId>> {
        let limit = limit.clamp(1, 100);
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let sessions = {
            let mut statement = transaction.prepare(
                "SELECT DISTINCT session_id FROM pending_enhancements
                 WHERE next_attempt_at <= ?1 ORDER BY next_attempt_at LIMIT ?2",
            )?;
            let sessions = statement
                .query_map(params![timestamp(now), limit], |row| {
                    parse_uuid_sql(row.get::<_, String>(0)?)
                })?
                .collect::<Result<Vec<_>, _>>()?;
            sessions
        };
        let leased_until = timestamp(now + Duration::minutes(10));
        for session_id in &sessions {
            transaction.execute(
                "UPDATE pending_enhancements SET next_attempt_at = ?2 WHERE session_id = ?1",
                params![session_id.to_string(), leased_until],
            )?;
        }
        transaction.commit()?;
        Ok(sessions)
    }

    pub fn clear_pending_enhancements(&mut self, session_id: SessionId) -> CoreResult<()> {
        self.connection.execute(
            "DELETE FROM pending_enhancements WHERE session_id = ?1",
            [session_id.to_string()],
        )?;
        Ok(())
    }

    pub fn dispose_context(&mut self, session_id: SessionId) -> CoreResult<()> {
        self.connection.execute(
            "DELETE FROM context_snapshots WHERE session_id = ?1",
            [session_id.to_string()],
        )?;
        Ok(())
    }

    /// Counts only corrections attributed to Murmur's inserted range. The caller owns the
    /// 30-second and focus-leave observation boundary.
    pub fn observe_attributed_correction(
        &mut self,
        heard: &str,
        replacement: &str,
    ) -> CoreResult<bool> {
        let heard = heard.trim();
        let replacement = replacement.trim();
        if heard.is_empty() || replacement.is_empty() || heard == replacement {
            return Err(CoreError::InvalidInput(
                "an attributed correction requires two different non-empty values".into(),
            ));
        }
        self.connection.execute(
            "INSERT INTO personal_vocabulary (id, heard, replacement, observations, learned, updated_at) VALUES (?1, ?2, ?3, 1, 0, ?4)
             ON CONFLICT(heard, replacement) DO UPDATE SET observations = CASE WHEN observations = 0 THEN 0 ELSE observations + 1 END, learned = CASE WHEN observations + 1 >= 3 THEN 1 ELSE learned END, updated_at = excluded.updated_at",
            params![Uuid::new_v4().to_string(), heard, replacement, timestamp(Utc::now())],
        )?;
        self.connection
            .query_row(
                "SELECT learned FROM personal_vocabulary WHERE heard = ?1 AND replacement = ?2",
                params![heard, replacement],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn list_vocabulary(&self) -> CoreResult<Vec<VocabularyRecord>> {
        let mut statement = self.connection.prepare(
            "SELECT id, heard, replacement, observations, learned FROM personal_vocabulary ORDER BY updated_at DESC",
        )?;
        let records = statement
            .query_map([], |row| {
                Ok(VocabularyRecord {
                    id: parse_uuid_sql(row.get::<_, String>(0)?)?,
                    heard: row.get(1)?,
                    replacement: row.get(2)?,
                    observations: row.get(3)?,
                    learned: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    /// Manually supplied entries are immediate; observed edits activate after three matches.
    pub fn dictation_vocabulary(&self) -> CoreResult<Vec<VocabularyRecord>> {
        Ok(self
            .list_vocabulary()?
            .into_iter()
            .filter(|entry| entry.observations == 0 || entry.learned)
            .collect())
    }

    pub fn add_vocabulary(
        &mut self,
        heard: &str,
        replacement: &str,
    ) -> CoreResult<VocabularyRecord> {
        let heard = heard.trim();
        let replacement = replacement.trim();
        if heard.is_empty() || replacement.is_empty() {
            return Err(CoreError::InvalidInput(
                "a vocabulary entry requires both phrases".into(),
            ));
        }
        let record = VocabularyRecord {
            id: Uuid::new_v4(),
            heard: heard.into(),
            replacement: replacement.into(),
            observations: 0,
            learned: false,
        };
        self.connection.execute(
            "INSERT INTO personal_vocabulary (id, heard, replacement, observations, learned, updated_at) VALUES (?1, ?2, ?3, 0, 0, ?4)",
            params![record.id.to_string(), record.heard, record.replacement, timestamp(Utc::now())],
        )?;
        Ok(record)
    }

    pub fn update_vocabulary(
        &mut self,
        id: Uuid,
        heard: &str,
        replacement: &str,
    ) -> CoreResult<()> {
        let heard = heard.trim();
        let replacement = replacement.trim();
        if heard.is_empty() || replacement.is_empty() {
            return Err(CoreError::InvalidInput(
                "a vocabulary entry requires both phrases".into(),
            ));
        }
        let changed = self.connection.execute(
            "UPDATE personal_vocabulary SET heard = ?2, replacement = ?3, observations = CASE WHEN heard = ?2 AND replacement = ?3 THEN observations ELSE 0 END, learned = CASE WHEN heard = ?2 AND replacement = ?3 THEN learned ELSE 0 END, updated_at = ?4 WHERE id = ?1",
            params![id.to_string(), heard, replacement, timestamp(Utc::now())],
        )?;
        ensure_changed(changed, "vocabulary entry", id)
    }

    pub fn delete_vocabulary(&mut self, id: Uuid) -> CoreResult<()> {
        let changed = self.connection.execute(
            "DELETE FROM personal_vocabulary WHERE id = ?1",
            [id.to_string()],
        )?;
        ensure_changed(changed, "vocabulary entry", id)
    }

    pub fn reset_learned_vocabulary(&mut self) -> CoreResult<usize> {
        self.connection
            .execute("DELETE FROM personal_vocabulary WHERE learned = 1", [])
            .map_err(Into::into)
    }

    pub fn expired_sessions(
        &self,
        now: DateTime<Utc>,
    ) -> CoreResult<Vec<(SessionId, Option<String>)>> {
        let mut statement = self
            .connection
            .prepare("SELECT id, audio_path FROM sessions WHERE pinned = 0 AND expires_at <= ?1")?;
        let values = statement
            .query_map([timestamp(now)], |row| {
                Ok((parse_uuid_sql(row.get::<_, String>(0)?)?, row.get(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(values)
    }

    pub fn delete_sessions(&mut self, ids: &[SessionId]) -> CoreResult<usize> {
        let transaction = self.connection.transaction()?;
        let mut count = 0;
        for id in ids {
            count += transaction.execute(
                "DELETE FROM sessions WHERE id = ?1 AND pinned = 0",
                [id.to_string()],
            )?;
        }
        transaction.commit()?;
        Ok(count)
    }

    pub fn delete_session_explicit(&mut self, id: SessionId) -> CoreResult<()> {
        let changed = self
            .connection
            .execute("DELETE FROM sessions WHERE id = ?1", [id.to_string()])?;
        ensure_changed(changed, "session", id)
    }

    pub fn rebuild_meeting_search(&mut self, session_id: SessionId) -> CoreResult<()> {
        self.connection.execute(
            "DELETE FROM meeting_search WHERE session_id = ?1",
            [session_id.to_string()],
        )?;
        self.connection.execute(
            "INSERT INTO meeting_search(session_id, title, transcript, speakers, brief)
             SELECT s.id, COALESCE(s.title, ''), COALESCE(group_concat(rs.text, ' '), ''), COALESCE(group_concat(rs.speaker, ' '), ''),
                    COALESCE(mb.overview, '') || ' ' || COALESCE(group_concat(bi.text, ' '), '')
             FROM sessions s
             LEFT JOIN raw_transcripts rt ON rt.session_id = s.id
             LEFT JOIN raw_segments rs ON rs.transcript_id = rt.id
             LEFT JOIN meeting_briefs mb ON mb.session_id = s.id
             LEFT JOIN brief_items bi ON bi.brief_id = mb.id
             WHERE s.id = ?1 AND s.kind = 'meeting' GROUP BY s.id",
            [session_id.to_string()],
        )?;
        Ok(())
    }

    pub fn search_meetings(&self, query: &str, limit: u32) -> CoreResult<Vec<SessionId>> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut statement = self.connection.prepare(
            "SELECT session_id FROM meeting_search WHERE meeting_search MATCH ?1 LIMIT ?2",
        )?;
        let ids = statement
            .query_map(params![query, limit], |row| {
                parse_uuid_sql(row.get::<_, String>(0)?)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    pub fn search_sessions(&self, query: &str, limit: u32) -> CoreResult<Vec<SessionId>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(vec![]);
        }
        let pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
        let mut statement = self.connection.prepare(
            "SELECT DISTINCT s.id FROM sessions s
             LEFT JOIN raw_transcripts rt ON rt.session_id = s.id
             LEFT JOIN raw_segments rs ON rs.transcript_id = rt.id
             LEFT JOIN dictation_results dr ON dr.session_id = s.id
             LEFT JOIN edited_transcripts et ON et.raw_transcript_id = rt.id
             LEFT JOIN edited_segments es ON es.edited_transcript_id = et.id
             LEFT JOIN meeting_briefs mb ON mb.session_id = s.id
             LEFT JOIN brief_items bi ON bi.brief_id = mb.id
             WHERE COALESCE(s.title, '') LIKE ?1 ESCAPE '\\'
                OR COALESCE(rs.text, '') LIKE ?1 ESCAPE '\\'
                OR COALESCE(dr.text, '') LIKE ?1 ESCAPE '\\'
                OR COALESCE(es.text, '') LIKE ?1 ESCAPE '\\'
                OR COALESCE(es.speaker, '') LIKE ?1 ESCAPE '\\'
                OR COALESCE(mb.overview, '') LIKE ?1 ESCAPE '\\'
                OR COALESCE(bi.text, '') LIKE ?1 ESCAPE '\\'
             ORDER BY s.created_at DESC LIMIT ?2",
        )?;
        let sessions = statement
            .query_map(params![pattern, limit], |row| {
                parse_uuid_sql(row.get::<_, String>(0)?)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(sessions)
    }
}

fn insert_edited(transaction: &Transaction<'_>, edited: &EditedTranscript) -> CoreResult<()> {
    transaction.execute("INSERT INTO edited_transcripts (id, raw_transcript_id, revision, created_at) VALUES (?1, ?2, ?3, ?4)", params![edited.id.to_string(), edited.raw_transcript_id.to_string(), edited.revision, timestamp(edited.created_at)])?;
    for segment in &edited.segments {
        transaction.execute("INSERT INTO edited_segments (edited_transcript_id, raw_segment_id, text, speaker) VALUES (?1, ?2, ?3, ?4)", params![edited.id.to_string(), segment.raw_segment_id.to_string(), segment.text, segment.speaker])?;
    }
    Ok(())
}

fn read_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<RecordingSession> {
    let kind: String = row.get(1)?;
    let source: String = row.get(2)?;
    let status: String = row.get(4)?;
    let mode: Option<String> = row.get(5)?;
    Ok(RecordingSession {
        id: parse_uuid_sql(row.get::<_, String>(0)?)?,
        kind: parse_session_kind_sql(&kind)?,
        source: parse_source_sql(&source)?,
        title: row.get(3)?,
        status: parse_status_sql(&status)?,
        processing_mode: mode.as_deref().map(parse_mode_sql).transpose()?,
        provider: row.get(6)?,
        audio_path: row.get(7)?,
        created_at: parse_timestamp_sql(row.get::<_, String>(8)?)?,
        updated_at: parse_timestamp_sql(row.get::<_, String>(9)?)?,
        expires_at: parse_timestamp_sql(row.get::<_, String>(10)?)?,
        pinned: row.get(11)?,
    })
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339()
}
fn parse_uuid(value: &str) -> CoreResult<Uuid> {
    Uuid::parse_str(value).map_err(|e| CoreError::InvalidInput(e.to_string()))
}
fn parse_timestamp(value: &str) -> CoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|v| v.with_timezone(&Utc))
        .map_err(|e| CoreError::InvalidInput(e.to_string()))
}
fn sql_conversion_error(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}
fn parse_uuid_sql(value: String) -> rusqlite::Result<Uuid> {
    Uuid::parse_str(&value).map_err(sql_conversion_error)
}
fn parse_timestamp_sql(value: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|v| v.with_timezone(&Utc))
        .map_err(sql_conversion_error)
}
fn unknown_enum(value: &str) -> rusqlite::Error {
    sql_conversion_error(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("unknown stored enum: {value}"),
    ))
}
fn parse_session_kind_sql(value: &str) -> rusqlite::Result<SessionKind> {
    match value {
        "dictation" => Ok(SessionKind::Dictation),
        "meeting" => Ok(SessionKind::Meeting),
        _ => Err(unknown_enum(value)),
    }
}
fn parse_source_sql(value: &str) -> rusqlite::Result<SessionSource> {
    match value {
        "microphone" => Ok(SessionSource::Microphone),
        "online_call" => Ok(SessionSource::OnlineCall),
        "in_person" => Ok(SessionSource::InPerson),
        "imported_file" => Ok(SessionSource::ImportedFile),
        _ => Err(unknown_enum(value)),
    }
}
fn parse_status_sql(value: &str) -> rusqlite::Result<SessionStatus> {
    match value {
        "capturing" => Ok(SessionStatus::Capturing),
        "processing" => Ok(SessionStatus::Processing),
        "completed" => Ok(SessionStatus::Completed),
        "cancelled" => Ok(SessionStatus::Cancelled),
        "failed" => Ok(SessionStatus::Failed),
        "pending_enhancement" => Ok(SessionStatus::PendingEnhancement),
        _ => Err(unknown_enum(value)),
    }
}
fn parse_mode_sql(value: &str) -> rusqlite::Result<ProcessingMode> {
    match value {
        "hosted" => Ok(ProcessingMode::Hosted),
        "hosted_fallback" => Ok(ProcessingMode::HostedFallback),
        "local" => Ok(ProcessingMode::Local),
        "queued" => Ok(ProcessingMode::Queued),
        _ => Err(unknown_enum(value)),
    }
}
fn parse_brief_kind(value: &str) -> CoreResult<BriefItemKind> {
    match value {
        "decision" => Ok(BriefItemKind::Decision),
        "action_item" => Ok(BriefItemKind::ActionItem),
        "open_question" => Ok(BriefItemKind::OpenQuestion),
        "notable_moment" => Ok(BriefItemKind::NotableMoment),
        "possible_follow_up" => Ok(BriefItemKind::PossibleFollowUp),
        _ => Err(CoreError::InvalidInput(format!(
            "unknown brief item kind: {value}"
        ))),
    }
}
fn ensure_changed(changed: usize, entity: &str, id: Uuid) -> CoreResult<()> {
    if changed == 0 {
        Err(CoreError::NotFound(format!("{entity} {id}")))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::EditedSegment;
    use chrono::Duration;

    fn sample_raw(session_id: Uuid) -> RawTranscript {
        RawTranscript {
            id: Uuid::new_v4(),
            session_id,
            language: "en-vi".into(),
            provider: "groq".into(),
            created_at: Utc::now(),
            segments: vec![TranscriptSegment {
                id: Uuid::new_v4(),
                ordinal: 0,
                start_ms: 0,
                end_ms: 500,
                speaker: Some("You".into()),
                text: "Xin chao world".into(),
            }],
        }
    }

    #[test]
    fn raw_transcript_cannot_be_replaced() {
        let mut repository = Repository::in_memory().unwrap();
        let session =
            RecordingSession::new(SessionKind::Dictation, SessionSource::Microphone, None);
        repository.create_session(&session).unwrap();
        let raw = sample_raw(session.id);
        repository.insert_raw_transcript(&raw).unwrap();
        let mut replacement = raw.clone();
        replacement.id = Uuid::new_v4();
        assert!(repository.insert_raw_transcript(&replacement).is_err());
        assert_eq!(
            repository.raw_transcript(raw.id).unwrap().text(),
            "Xin chao world"
        );
    }

    #[test]
    fn edited_revisions_preserve_raw_segment_links() {
        let mut repository = Repository::in_memory().unwrap();
        let session = RecordingSession::new(
            SessionKind::Meeting,
            SessionSource::ImportedFile,
            Some("Review".into()),
        );
        repository.create_session(&session).unwrap();
        let raw = sample_raw(session.id);
        repository.insert_raw_transcript(&raw).unwrap();
        let edited = EditedTranscript {
            id: Uuid::new_v4(),
            raw_transcript_id: raw.id,
            revision: 1,
            created_at: Utc::now(),
            segments: vec![EditedSegment {
                raw_segment_id: raw.segments[0].id,
                text: "Xin chao, world".into(),
                speaker: Some("Nhat".into()),
            }],
        };
        repository.insert_edited_transcript(&edited).unwrap();
        assert!(repository.insert_edited_transcript(&edited).is_err());
    }

    #[test]
    fn expiry_skips_pinned_meetings() {
        let mut repository = Repository::in_memory().unwrap();
        let mut expired =
            RecordingSession::new(SessionKind::Meeting, SessionSource::OnlineCall, None);
        expired.expires_at = Utc::now() - Duration::days(1);
        repository.create_session(&expired).unwrap();
        repository.set_pinned(expired.id, true).unwrap();
        assert!(repository.expired_sessions(Utc::now()).unwrap().is_empty());
    }

    #[test]
    fn vocabulary_learns_on_third_attributed_correction() {
        let mut repository = Repository::in_memory().unwrap();
        assert!(!repository
            .observe_attributed_correction("knat", "Nhat")
            .unwrap());
        assert!(repository.dictation_vocabulary().unwrap().is_empty());
        assert!(!repository
            .observe_attributed_correction("knat", "Nhat")
            .unwrap());
        assert!(repository.dictation_vocabulary().unwrap().is_empty());
        assert!(repository
            .observe_attributed_correction("knat", "Nhat")
            .unwrap());
        assert_eq!(repository.dictation_vocabulary().unwrap().len(), 1);
        let learned = repository.dictation_vocabulary().unwrap().remove(0);
        repository
            .update_vocabulary(learned.id, "knat", "Nhat")
            .unwrap();
        assert!(repository.dictation_vocabulary().unwrap()[0].learned);
    }

    #[test]
    fn manual_vocabulary_remains_active_after_observation_and_edit() {
        let mut repository = Repository::in_memory().unwrap();
        let manual = repository.add_vocabulary("knat", "Nhat").unwrap();
        repository
            .observe_attributed_correction("knat", "Nhat")
            .unwrap();
        assert_eq!(repository.dictation_vocabulary().unwrap().len(), 1);
        repository
            .update_vocabulary(manual.id, "knat", "Nhat Pham")
            .unwrap();
        assert_eq!(
            repository.dictation_vocabulary().unwrap()[0].replacement,
            "Nhat Pham"
        );
        repository
            .observe_attributed_correction("an old sentence", "A past dictation")
            .unwrap();
        assert_eq!(repository.list_vocabulary().unwrap().len(), 2);
        assert_eq!(repository.dictation_vocabulary().unwrap().len(), 1);
        let candidate = repository
            .list_vocabulary()
            .unwrap()
            .into_iter()
            .find(|entry| entry.heard == "an old sentence")
            .unwrap();
        repository
            .update_vocabulary(candidate.id, &candidate.heard, &candidate.replacement)
            .unwrap();
        assert_eq!(repository.dictation_vocabulary().unwrap().len(), 1);
        repository
            .update_vocabulary(candidate.id, &candidate.heard, "An edited phrase")
            .unwrap();
        assert_eq!(repository.dictation_vocabulary().unwrap().len(), 2);
    }

    #[test]
    fn dictation_detail_round_trips_and_clear_removes_user_records() {
        let mut repository = Repository::in_memory().unwrap();
        let session =
            RecordingSession::new(SessionKind::Dictation, SessionSource::Microphone, None);
        repository.create_session(&session).unwrap();
        let raw = sample_raw(session.id);
        repository.insert_raw_transcript(&raw).unwrap();
        let result = DictationResult {
            id: Uuid::new_v4(),
            session_id: session.id,
            raw_transcript_id: raw.id,
            text: "Xin chao, world".into(),
            corrected: false,
            insertion_status: "accessibility".into(),
            created_at: Utc::now(),
        };
        repository.insert_dictation_result(&result).unwrap();
        let timing = DictationTimingRecord {
            capture_ms: 1_500,
            transcription_ms: 900,
            correction_ms: 120,
            insertion_ms: 30,
        };
        repository
            .insert_dictation_timing(session.id, timing)
            .unwrap();

        let stored = repository.dictation_result(session.id).unwrap();
        assert_eq!(stored.result.text, result.text);
        assert_eq!(stored.raw.text(), "Xin chao world");
        assert_eq!(
            repository.dictation_timing(session.id).unwrap(),
            Some(timing)
        );

        repository
            .update_dictation_result_text(session.id, "Hello, world", true)
            .unwrap();
        let transformed = repository.dictation_result(session.id).unwrap();
        assert_eq!(transformed.result.text, "Hello, world");
        assert!(transformed.result.corrected);

        repository.clear_user_data().unwrap();
        assert!(repository.list_sessions(None).unwrap().is_empty());
        assert!(repository.list_vocabulary().unwrap().is_empty());
    }

    #[test]
    fn pending_enhancement_claim_is_atomic_and_failure_backs_off() {
        let mut repository = Repository::in_memory().unwrap();
        let session = RecordingSession::new(
            SessionKind::Meeting,
            SessionSource::ImportedFile,
            Some("Queued".into()),
        );
        repository.create_session(&session).unwrap();
        repository
            .queue_enhancement(session.id, "brief", Some("offline"))
            .unwrap();
        let now = Utc::now();
        assert_eq!(
            repository
                .claim_due_pending_enhancement_sessions(now, 10)
                .unwrap(),
            vec![session.id]
        );
        assert!(repository
            .claim_due_pending_enhancement_sessions(now, 10)
            .unwrap()
            .is_empty());

        repository
            .queue_enhancement(session.id, "brief", Some("still offline"))
            .unwrap();
        assert!(repository
            .claim_due_pending_enhancement_sessions(Utc::now() + Duration::seconds(14), 10)
            .unwrap()
            .is_empty());
        assert_eq!(
            repository
                .claim_due_pending_enhancement_sessions(Utc::now() + Duration::seconds(16), 10)
                .unwrap(),
            vec![session.id]
        );
    }
}
