use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{CoreError, CoreResult};

pub type SessionId = Uuid;
pub type TranscriptId = Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionKind {
    Dictation,
    Meeting,
}

impl SessionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dictation => "dictation",
            Self::Meeting => "meeting",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionStatus {
    Capturing,
    Processing,
    Completed,
    Cancelled,
    Failed,
    PendingEnhancement,
}

impl SessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Capturing => "capturing",
            Self::Processing => "processing",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::PendingEnhancement => "pending_enhancement",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProcessingMode {
    Hosted,
    HostedFallback,
    Local,
    Queued,
}

impl ProcessingMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hosted => "hosted",
            Self::HostedFallback => "hosted_fallback",
            Self::Local => "local",
            Self::Queued => "queued",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionSource {
    Microphone,
    OnlineCall,
    InPerson,
    ImportedFile,
}

impl SessionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Microphone => "microphone",
            Self::OnlineCall => "online_call",
            Self::InPerson => "in_person",
            Self::ImportedFile => "imported_file",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingSession {
    pub id: SessionId,
    pub kind: SessionKind,
    pub source: SessionSource,
    pub title: Option<String>,
    pub status: SessionStatus,
    pub processing_mode: Option<ProcessingMode>,
    pub provider: Option<String>,
    pub audio_path: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub pinned: bool,
}

impl RecordingSession {
    pub fn new(kind: SessionKind, source: SessionSource, title: Option<String>) -> Self {
        let now = Utc::now();
        let retention = match kind {
            SessionKind::Dictation => Duration::days(7),
            SessionKind::Meeting => Duration::days(30),
        };
        Self {
            id: Uuid::new_v4(),
            kind,
            source,
            title,
            status: SessionStatus::Capturing,
            processing_mode: None,
            provider: None,
            audio_path: None,
            created_at: now,
            updated_at: now,
            expires_at: now + retention,
            pinned: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSegment {
    pub id: Uuid,
    pub ordinal: u32,
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker: Option<String>,
    pub text: String,
}

impl TranscriptSegment {
    pub fn validate(&self) -> CoreResult<()> {
        if self.end_ms < self.start_ms {
            return Err(CoreError::InvalidInput(format!(
                "segment {} ends before it starts",
                self.id
            )));
        }
        if self.text.trim().is_empty() {
            return Err(CoreError::InvalidInput(format!(
                "segment {} has no text",
                self.id
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawTranscript {
    pub id: TranscriptId,
    pub session_id: SessionId,
    pub language: String,
    pub provider: String,
    pub created_at: DateTime<Utc>,
    pub segments: Vec<TranscriptSegment>,
}

impl RawTranscript {
    pub fn validate(&self) -> CoreResult<()> {
        if self.segments.is_empty() {
            return Err(CoreError::InvalidInput(
                "a transcript requires at least one segment".into(),
            ));
        }
        for segment in &self.segments {
            segment.validate()?;
        }
        for pair in self.segments.windows(2) {
            if pair[0].ordinal >= pair[1].ordinal || pair[0].start_ms > pair[1].start_ms {
                return Err(CoreError::InvalidInput(
                    "transcript segments must be ordered".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn text(&self) -> String {
        self.segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditedSegment {
    pub raw_segment_id: Uuid,
    pub text: String,
    pub speaker: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditedTranscript {
    pub id: Uuid,
    pub raw_transcript_id: TranscriptId,
    pub revision: u32,
    pub created_at: DateTime<Utc>,
    pub segments: Vec<EditedSegment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BriefItemKind {
    Decision,
    ActionItem,
    OpenQuestion,
    NotableMoment,
    PossibleFollowUp,
}

impl BriefItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Decision => "decision",
            Self::ActionItem => "action_item",
            Self::OpenQuestion => "open_question",
            Self::NotableMoment => "notable_moment",
            Self::PossibleFollowUp => "possible_follow_up",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptCitation {
    pub segment_id: Uuid,
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefItem {
    pub id: Uuid,
    pub kind: BriefItemKind,
    pub text: String,
    pub citations: Vec<TranscriptCitation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingBrief {
    pub id: Uuid,
    pub session_id: SessionId,
    pub transcript_id: TranscriptId,
    pub overview: String,
    pub detailed_topics: Vec<String>,
    pub items: Vec<BriefItem>,
    pub created_at: DateTime<Utc>,
}

impl MeetingBrief {
    pub fn validate_against(&self, transcript: &RawTranscript) -> CoreResult<()> {
        if self.transcript_id != transcript.id || self.session_id != transcript.session_id {
            return Err(CoreError::InvalidInput(
                "brief does not belong to the supplied transcript".into(),
            ));
        }
        for item in &self.items {
            if matches!(
                item.kind,
                BriefItemKind::Decision | BriefItemKind::ActionItem
            ) && item.citations.is_empty()
            {
                return Err(CoreError::InvalidInput(format!(
                    "{} items require at least one citation",
                    item.kind.as_str()
                )));
            }
            for citation in &item.citations {
                let segment = transcript
                    .segments
                    .iter()
                    .find(|segment| segment.id == citation.segment_id)
                    .ok_or_else(|| {
                        CoreError::InvalidInput(format!(
                            "citation references unknown segment {}",
                            citation.segment_id
                        ))
                    })?;
                if citation.start_ms < segment.start_ms
                    || citation.end_ms > segment.end_ms
                    || citation.end_ms < citation.start_ms
                {
                    return Err(CoreError::InvalidInput(
                        "citation timestamp is outside its source segment".into(),
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DictationResult {
    pub id: Uuid,
    pub session_id: SessionId,
    pub raw_transcript_id: TranscriptId,
    pub text: String,
    pub corrected: bool,
    pub insertion_status: String,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transcript() -> RawTranscript {
        let session_id = Uuid::new_v4();
        RawTranscript {
            id: Uuid::new_v4(),
            session_id,
            language: "vi".into(),
            provider: "deepgram".into(),
            created_at: Utc::now(),
            segments: vec![TranscriptSegment {
                id: Uuid::new_v4(),
                ordinal: 0,
                start_ms: 100,
                end_ms: 900,
                speaker: Some("You".into()),
                text: "Chao ban".into(),
            }],
        }
    }

    #[test]
    fn decisions_require_valid_citations() {
        let raw = transcript();
        let brief = MeetingBrief {
            id: Uuid::new_v4(),
            session_id: raw.session_id,
            transcript_id: raw.id,
            overview: "Overview".into(),
            detailed_topics: vec![],
            items: vec![BriefItem {
                id: Uuid::new_v4(),
                kind: BriefItemKind::Decision,
                text: "Ship it".into(),
                citations: vec![],
            }],
            created_at: Utc::now(),
        };
        assert!(brief.validate_against(&raw).is_err());
    }

    #[test]
    fn citation_must_stay_inside_source_segment() {
        let raw = transcript();
        let brief = MeetingBrief {
            id: Uuid::new_v4(),
            session_id: raw.session_id,
            transcript_id: raw.id,
            overview: "Overview".into(),
            detailed_topics: vec![],
            items: vec![BriefItem {
                id: Uuid::new_v4(),
                kind: BriefItemKind::ActionItem,
                text: "Send notes".into(),
                citations: vec![TranscriptCitation {
                    segment_id: raw.segments[0].id,
                    start_ms: 0,
                    end_ms: 900,
                }],
            }],
            created_at: Utc::now(),
        };
        assert!(brief.validate_against(&raw).is_err());
    }
}
