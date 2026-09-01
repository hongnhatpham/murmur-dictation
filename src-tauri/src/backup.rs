use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

use crate::error::{CoreError, CoreResult};

pub const BACKUP_FORMAT_VERSION: u32 = 1;
pub const PRIVACY_WARNING: &str = "This backup contains private transcripts, vocabulary, settings, and meeting audio in plain files. It never contains provider or update credentials.";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackupEntryKind {
    Database,
    Settings,
    Vocabulary,
    MeetingFile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupEntry {
    pub relative_path: String,
    pub kind: BackupEntryKind,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupResult {
    pub archive_path: PathBuf,
    pub manifest: BackupManifest,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreResult {
    pub destination: PathBuf,
    pub restored_entries: usize,
}

/// Create a plain ZIP from supported application files. Credential Manager is external to the
/// data directory, and credential-like file names are rejected as a second line of defense.
pub fn create_backup(
    data_dir: &Path,
    archive_path: &Path,
    app_version: &str,
) -> CoreResult<BackupResult> {
    let database = data_dir.join("murmur.sqlite3");
    if !database.is_file() {
        return create_backup_from_sources(data_dir, archive_path, app_version, None);
    }
    let snapshot = data_dir.join(format!(
        ".murmur-backup-snapshot-{}.sqlite3",
        Uuid::new_v4()
    ));
    let connection = Connection::open(&database)?;
    connection.execute("VACUUM main INTO ?1", [snapshot.to_string_lossy().as_ref()])?;
    drop(connection);
    let result = create_backup_from_sources(data_dir, archive_path, app_version, Some(&snapshot));
    let cleanup = fs::remove_file(&snapshot);
    if let Err(error) = cleanup {
        if error.kind() != std::io::ErrorKind::NotFound && result.is_ok() {
            return Err(error.into());
        }
    }
    result
}

fn create_backup_from_sources(
    data_dir: &Path,
    archive_path: &Path,
    app_version: &str,
    database_snapshot: Option<&Path>,
) -> CoreResult<BackupResult> {
    let archive_absolute = absolute_path(archive_path)?;
    let mut files = collect_backup_files(data_dir, &archive_absolute)?;
    if let Some(snapshot) = database_snapshot {
        files.retain(|(relative, _, _)| {
            let name = path_to_zip_name(relative).unwrap_or_default();
            !matches!(
                name.as_str(),
                "murmur.sqlite3" | "murmur.sqlite3-wal" | "murmur.sqlite3-shm"
            ) && !name.starts_with(".murmur-backup-snapshot-")
        });
        files.push((
            PathBuf::from("murmur.sqlite3"),
            snapshot.to_path_buf(),
            BackupEntryKind::Database,
        ));
        files.sort_by(|left, right| left.0.cmp(&right.0));
    }
    if files.is_empty() {
        return Err(CoreError::NotFound(
            "no Murmur data was found to back up".into(),
        ));
    }
    let entries = files
        .iter()
        .map(|(relative, source, kind)| {
            Ok(BackupEntry {
                relative_path: path_to_zip_name(relative)?,
                kind: kind.clone(),
                size_bytes: fs::metadata(source)?.len(),
                sha256: Some(file_sha256(source)?),
            })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    let manifest = BackupManifest::new(app_version, entries)?;
    if let Some(parent) = archive_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let partial = archive_path.with_extension("zip.part");
    let file = fs::File::create(&partial)?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    zip.start_file("manifest.json", options)
        .map_err(zip_error)?;
    zip.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
    for (relative, source, _) in &files {
        zip.start_file(path_to_zip_name(relative)?, options)
            .map_err(zip_error)?;
        let mut input = fs::File::open(source)?;
        std::io::copy(&mut input, &mut zip)?;
    }
    let file = zip.finish().map_err(zip_error)?;
    file.sync_all()?;
    fs::rename(&partial, archive_path)?;
    Ok(BackupResult {
        archive_path: archive_path.to_path_buf(),
        manifest,
        size_bytes: fs::metadata(archive_path)?.len(),
    })
}

/// Restore only manifest-listed files through a staging directory. ZIP traversal, symlinks,
/// duplicates, unlisted files, size mismatches, and checksum mismatches all stop the restore.
pub fn restore_backup(archive_path: &Path, destination: &Path) -> CoreResult<RestoreResult> {
    let file = fs::File::open(archive_path)?;
    let mut zip = ZipArchive::new(file).map_err(zip_error)?;
    let manifest: BackupManifest = {
        let mut entry = zip.by_name("manifest.json").map_err(zip_error)?;
        if entry.size() > 4 * 1024 * 1024 {
            return Err(CoreError::InvalidInput(
                "backup manifest is unexpectedly large".into(),
            ));
        }
        let mut text = String::new();
        entry.read_to_string(&mut text)?;
        serde_json::from_str(&text)?
    };
    manifest.validate()?;
    if manifest.entries.iter().any(|entry| entry.sha256.is_none()) {
        return Err(CoreError::InvalidInput(
            "restorable backup entries require SHA-256 checksums".into(),
        ));
    }
    let expected = manifest
        .entries
        .iter()
        .map(|entry| (entry.relative_path.clone(), entry))
        .collect::<HashMap<_, _>>();
    let mut seen = HashSet::new();
    let staging = destination
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".murmur-restore-{}", Uuid::new_v4()));
    fs::create_dir_all(&staging)?;
    let extraction = (|| -> CoreResult<()> {
        for index in 0..zip.len() {
            let mut archived = zip.by_index(index).map_err(zip_error)?;
            let name = archived.name().to_owned();
            if name == "manifest.json" {
                continue;
            }
            validate_relative_path(&name)?;
            let manifest_entry = expected.get(&name).ok_or_else(|| {
                CoreError::InvalidInput(format!("backup contains unlisted entry: {name}"))
            })?;
            if !seen.insert(name.clone()) {
                return Err(CoreError::InvalidInput(format!(
                    "backup contains duplicate entry: {name}"
                )));
            }
            if archived.is_dir()
                || archived
                    .unix_mode()
                    .is_some_and(|mode| mode & 0o170000 == 0o120000)
            {
                return Err(CoreError::InvalidInput(format!(
                    "backup entry is not a regular file: {name}"
                )));
            }
            if archived.size() != manifest_entry.size_bytes {
                return Err(CoreError::InvalidInput(format!(
                    "backup size mismatch for {name}"
                )));
            }
            let target = staging.join(Path::new(&name));
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut output = fs::File::create(&target)?;
            std::io::copy(&mut archived, &mut output)?;
            output.sync_all()?;
            if let Some(expected_hash) = &manifest_entry.sha256 {
                if !file_sha256(&target)?.eq_ignore_ascii_case(expected_hash) {
                    return Err(CoreError::InvalidInput(format!(
                        "backup checksum mismatch for {name}"
                    )));
                }
            }
        }
        if seen.len() != expected.len() {
            return Err(CoreError::InvalidInput(
                "backup is missing one or more manifest entries".into(),
            ));
        }
        Ok(())
    })();
    if let Err(error) = extraction {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    fs::create_dir_all(destination)?;
    for entry in &manifest.entries {
        let relative = Path::new(&entry.relative_path);
        let source = staging.join(relative);
        let target = destination.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source, target)?;
    }
    fs::remove_dir_all(staging)?;
    Ok(RestoreResult {
        destination: destination.to_path_buf(),
        restored_entries: manifest.entries.len(),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupManifest {
    pub format_version: u32,
    pub app_version: String,
    pub created_at: DateTime<Utc>,
    pub credentials_excluded: bool,
    pub privacy_warning: String,
    pub entries: Vec<BackupEntry>,
}

impl BackupManifest {
    pub fn new(app_version: impl Into<String>, entries: Vec<BackupEntry>) -> CoreResult<Self> {
        let manifest = Self {
            format_version: BACKUP_FORMAT_VERSION,
            app_version: app_version.into(),
            created_at: Utc::now(),
            credentials_excluded: true,
            privacy_warning: PRIVACY_WARNING.into(),
            entries,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> CoreResult<()> {
        if self.format_version != BACKUP_FORMAT_VERSION {
            return Err(CoreError::InvalidInput(format!(
                "unsupported backup format {}",
                self.format_version
            )));
        }
        if !self.credentials_excluded {
            return Err(CoreError::InvalidInput(
                "backup manifests must exclude credentials".into(),
            ));
        }
        let mut paths = HashSet::new();
        for entry in &self.entries {
            validate_relative_path(&entry.relative_path)?;
            if !paths.insert(entry.relative_path.to_ascii_lowercase()) {
                return Err(CoreError::InvalidInput(format!(
                    "duplicate backup path: {}",
                    entry.relative_path
                )));
            }
            let lower = entry.relative_path.to_ascii_lowercase();
            if lower.contains("credential")
                || lower.contains("secret")
                || lower.contains("api_key")
                || lower.contains("token")
            {
                return Err(CoreError::InvalidInput(format!(
                    "credential-like backup entry rejected: {}",
                    entry.relative_path
                )));
            }
            if let Some(checksum) = &entry.sha256 {
                if checksum.len() != 64
                    || !checksum
                        .chars()
                        .all(|character| character.is_ascii_hexdigit())
                {
                    return Err(CoreError::InvalidInput(format!(
                        "invalid backup checksum for {}",
                        entry.relative_path
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn restore_targets(&self, destination: &Path) -> CoreResult<Vec<PathBuf>> {
        self.validate()?;
        Ok(self
            .entries
            .iter()
            .map(|entry| destination.join(&entry.relative_path))
            .collect())
    }
}

fn validate_relative_path(value: &str) -> CoreResult<()> {
    let path = Path::new(value);
    if path.is_absolute()
        || value.is_empty()
        || path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(CoreError::InvalidInput(format!(
            "unsafe backup path: {value}"
        )));
    }
    Ok(())
}

fn collect_backup_files(
    data_dir: &Path,
    archive_absolute: &Path,
) -> CoreResult<Vec<(PathBuf, PathBuf, BackupEntryKind)>> {
    let mut files = Vec::new();
    if !data_dir.exists() {
        return Ok(files);
    }
    let mut directories = vec![data_dir.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for item in fs::read_dir(directory)? {
            let item = item?;
            let file_type = item.file_type()?;
            let source = item.path();
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                directories.push(source);
                continue;
            }
            if !file_type.is_file() || absolute_path(&source)? == archive_absolute {
                continue;
            }
            let relative = source
                .strip_prefix(data_dir)
                .map_err(|_| {
                    CoreError::InvalidInput("backup source escaped data directory".into())
                })?
                .to_path_buf();
            let name = path_to_zip_name(&relative)?;
            if credential_like(&name) || name.ends_with(".log") || name.contains("diagnostic") {
                continue;
            }
            let kind = if name.ends_with(".sqlite3")
                || name.ends_with(".db")
                || name.ends_with(".sqlite3-wal")
                || name.ends_with(".sqlite3-shm")
                || name.ends_with(".db-wal")
                || name.ends_with(".db-shm")
            {
                BackupEntryKind::Database
            } else if name == "preferences.json" || name.starts_with("settings/") {
                BackupEntryKind::Settings
            } else if name.contains("vocabulary") {
                BackupEntryKind::Vocabulary
            } else if name.starts_with("media/") || name.starts_with("meetings/") {
                BackupEntryKind::MeetingFile
            } else {
                continue;
            };
            files.push((relative, source, kind));
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(files)
}

fn credential_like(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    ["credential", "secret", "api_key", "api-key", "token"]
        .iter()
        .any(|needle| lower.contains(needle))
}

fn path_to_zip_name(path: &Path) -> CoreResult<String> {
    let value = path
        .components()
        .map(|component| match component {
            Component::Normal(value) => value.to_str().map(str::to_owned),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| CoreError::InvalidInput("backup path is not valid Unicode".into()))?
        .join("/");
    validate_relative_path(&value)?;
    Ok(value)
}

fn file_sha256(path: &Path) -> CoreResult<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn absolute_path(path: &Path) -> CoreResult<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn zip_error(error: zip::result::ZipError) -> CoreError {
    CoreError::InvalidInput(format!("invalid backup ZIP: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_rejects_credentials_and_traversal() {
        assert!(BackupManifest::new(
            "0.1.0",
            vec![BackupEntry {
                relative_path: "credentials/token".into(),
                kind: BackupEntryKind::Settings,
                size_bytes: 1,
                sha256: None,
            }]
        )
        .is_err());
        assert!(BackupManifest::new(
            "0.1.0",
            vec![BackupEntry {
                relative_path: "../outside.db".into(),
                kind: BackupEntryKind::Database,
                size_bytes: 1,
                sha256: None,
            }]
        )
        .is_err());
    }

    #[test]
    fn manifest_always_marks_credentials_excluded() {
        let manifest = BackupManifest::new(
            "0.1.0",
            vec![BackupEntry {
                relative_path: "murmur.db".into(),
                kind: BackupEntryKind::Database,
                size_bytes: 20,
                sha256: None,
            }],
        )
        .unwrap();
        assert!(manifest.credentials_excluded);
        assert!(manifest.privacy_warning.contains("plain files"));
    }

    #[test]
    fn zip_round_trip_restores_unicode_and_excludes_credentials() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        fs::create_dir_all(source.join("media")).unwrap();
        let live = Connection::open(source.join("murmur.sqlite3")).unwrap();
        live.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;
             CREATE TABLE notes(value TEXT NOT NULL);
             INSERT INTO notes VALUES ('committed while live');",
        )
        .unwrap();
        fs::write(
            source.join("preferences.json"),
            r#"{"language":"Ti\u1ebfng Vi\u1ec7t"}"#.as_bytes(),
        )
        .unwrap();
        let meeting_name = "h\u{ecd}p.wav";
        fs::write(source.join("media").join(meeting_name), b"audio").unwrap();
        fs::write(source.join("provider-token.txt"), b"never archive this").unwrap();
        let archive = root.path().join("backup.zip");
        let backup = create_backup(&source, &archive, "0.1.0").unwrap();
        assert_eq!(backup.manifest.entries.len(), 3);
        assert!(!backup
            .manifest
            .entries
            .iter()
            .any(|entry| entry.relative_path.ends_with("-wal")
                || entry.relative_path.ends_with("-shm")));
        let destination = root.path().join("restored");
        let restored = restore_backup(&archive, &destination).unwrap();
        assert_eq!(restored.restored_entries, 3);
        assert_eq!(
            fs::read(destination.join("media").join(meeting_name)).unwrap(),
            b"audio"
        );
        assert!(!destination.join("provider-token.txt").exists());
        let restored_db = Connection::open(destination.join("murmur.sqlite3")).unwrap();
        let value: String = restored_db
            .query_row("SELECT value FROM notes", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "committed while live");
    }

    #[test]
    fn restore_rejects_legacy_entries_without_integrity_hashes() {
        let directory = tempfile::tempdir().unwrap();
        let manifest = BackupManifest::new(
            "0.1.0",
            vec![BackupEntry {
                relative_path: "murmur.sqlite3".into(),
                kind: BackupEntryKind::Database,
                size_bytes: 1,
                sha256: None,
            }],
        )
        .unwrap();
        let archive_path = directory.path().join("legacy.zip");
        let file = fs::File::create(&archive_path).unwrap();
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default();
        archive.start_file("manifest.json", options).unwrap();
        archive
            .write_all(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        archive.start_file("murmur.sqlite3", options).unwrap();
        archive.write_all(b"x").unwrap();
        archive.finish().unwrap();
        let error = restore_backup(&archive_path, &directory.path().join("restore"))
            .expect_err("unhashed entries must not restore");
        assert!(error.to_string().contains("require SHA-256"));
    }
}
