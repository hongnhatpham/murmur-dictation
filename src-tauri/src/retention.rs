use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Duration, Utc};
use serde::Serialize;

use crate::{
    db::Repository,
    error::{CoreError, CoreResult},
};

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionReport {
    pub sessions_deleted: usize,
    pub audio_files_deleted: usize,
    pub skipped_paths: Vec<String>,
}

pub struct RetentionService {
    media_root: PathBuf,
}

pub const FAILED_DIAGNOSTIC_AUDIO_HOURS: i64 = 24;

pub fn failed_diagnostic_audio_expires_at(created_at: DateTime<Utc>) -> DateTime<Utc> {
    created_at + Duration::hours(FAILED_DIAGNOSTIC_AUDIO_HOURS)
}

impl RetentionService {
    pub fn new(media_root: PathBuf) -> Self {
        Self { media_root }
    }

    pub fn purge(
        &self,
        repository: &mut Repository,
        now: DateTime<Utc>,
    ) -> CoreResult<RetentionReport> {
        fs::create_dir_all(&self.media_root)?;
        let root = self.media_root.canonicalize()?;
        let expired = repository.expired_sessions(now)?;
        let mut report = RetentionReport::default();
        for (_, path) in &expired {
            let Some(path) = path else {
                continue;
            };
            let candidate = Path::new(path);
            if !candidate.exists() {
                continue;
            }
            let resolved = candidate.canonicalize()?;
            if !resolved.starts_with(&root) || resolved == root {
                report.skipped_paths.push(path.clone());
                continue;
            }
            if resolved.is_file() {
                fs::remove_file(&resolved)?;
                report.audio_files_deleted += 1;
            } else if resolved.is_dir() {
                report.audio_files_deleted += count_regular_files(&resolved)?;
                fs::remove_dir_all(&resolved)?;
            } else {
                report.skipped_paths.push(path.clone());
            }
        }
        let ids = expired.into_iter().map(|(id, _)| id).collect::<Vec<_>>();
        report.sessions_deleted = repository.delete_sessions(&ids)?;
        Ok(report)
    }

    pub fn validate_media_path(&self, path: &Path) -> CoreResult<()> {
        let root = self.media_root.canonicalize()?;
        let resolved = path.canonicalize()?;
        if resolved == root || !resolved.starts_with(root) {
            return Err(CoreError::InvalidInput(
                "media path is outside Murmur storage".into(),
            ));
        }
        Ok(())
    }
}

fn count_regular_files(directory: &Path) -> CoreResult<usize> {
    let mut count = 0;
    let mut pending = vec![directory.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in fs::read_dir(current)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                count += 1;
            }
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use tempfile::tempdir;

    use super::*;
    use crate::domain::{RecordingSession, SessionKind, SessionSource};

    #[test]
    fn retention_deletes_only_files_inside_media_root() {
        let temp = tempdir().unwrap();
        let media = temp.path().join("media");
        fs::create_dir_all(&media).unwrap();
        let audio = media.join("expired.wav");
        fs::write(&audio, b"audio").unwrap();
        let mut repository = Repository::in_memory().unwrap();
        let mut session =
            RecordingSession::new(SessionKind::Dictation, SessionSource::Microphone, None);
        session.audio_path = Some(audio.to_string_lossy().into_owned());
        session.expires_at = Utc::now() - Duration::hours(1);
        repository.create_session(&session).unwrap();
        let report = RetentionService::new(media)
            .purge(&mut repository, Utc::now())
            .unwrap();
        assert_eq!(report.sessions_deleted, 1);
        assert_eq!(report.audio_files_deleted, 1);
        assert!(!audio.exists());
    }

    #[test]
    fn failed_diagnostic_audio_expires_after_twenty_four_hours() {
        let created = Utc::now();
        assert_eq!(
            failed_diagnostic_audio_expires_at(created),
            created + Duration::hours(24)
        );
    }
}
