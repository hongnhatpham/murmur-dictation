use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

use chrono::{DateTime, Utc};

use serde::{Deserialize, Serialize};

use crate::{
    error::CoreResult,
    providers::validate_local_whisper_install,
    secrets::{SecretStore, ALLOWED_SECRET_NAMES},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckStatus {
    Pass,
    Warning,
    Fail,
    NotRun,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticCheck {
    pub id: String,
    pub status: CheckStatus,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticReport {
    pub schema_version: u32,
    pub checks: Vec<DiagnosticCheck>,
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStep {
    pub id: String,
    pub required: bool,
    pub description: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticLogEntry {
    pub at: DateTime<Utc>,
    pub level: DiagnosticLevel,
    pub code: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metrics: BTreeMap<String, i64>,
}

pub struct DiagnosticLogger {
    directory: PathBuf,
    max_bytes: u64,
    generations: usize,
    write_lock: Mutex<()>,
}

impl DiagnosticLogger {
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            max_bytes: 1024 * 1024,
            generations: 4,
            write_lock: Mutex::new(()),
        }
    }

    #[cfg(test)]
    fn with_limits(directory: PathBuf, max_bytes: u64, generations: usize) -> Self {
        Self {
            directory,
            max_bytes,
            generations,
            write_lock: Mutex::new(()),
        }
    }

    /// Record an event code while discarding free-form detail. Callers remain source-compatible,
    /// but transcript, context, and secret text cannot cross this boundary.
    pub fn record(
        &self,
        level: DiagnosticLevel,
        code: impl Into<String>,
        detail: impl AsRef<str>,
    ) -> CoreResult<()> {
        let _guard = self.write_lock.lock().map_err(|_| {
            crate::error::CoreError::Unavailable("diagnostic logger lock is poisoned".into())
        })?;
        fs::create_dir_all(&self.directory)?;
        let code = code.into();
        if code.is_empty()
            || !code
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
        {
            return Err(crate::error::CoreError::InvalidInput(
                "diagnostic event code contains unsupported characters".into(),
            ));
        }
        let entry = DiagnosticLogEntry {
            at: Utc::now(),
            level,
            code,
            detail: redact_diagnostic_text(detail.as_ref()),
            metrics: BTreeMap::new(),
        };
        self.write_entry(entry)
    }

    /// Record numeric operational fields without accepting user-bearing strings.
    pub fn record_metrics(
        &self,
        level: DiagnosticLevel,
        code: impl Into<String>,
        metrics: impl IntoIterator<Item = (String, i64)>,
    ) -> CoreResult<()> {
        let _guard = self.write_lock.lock().map_err(|_| {
            crate::error::CoreError::Unavailable("diagnostic logger lock is poisoned".into())
        })?;
        fs::create_dir_all(&self.directory)?;
        let code = validate_event_code(code.into())?;
        let mut safe_metrics = BTreeMap::new();
        for (name, value) in metrics {
            if name.is_empty()
                || !name
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
            {
                return Err(crate::error::CoreError::InvalidInput(
                    "diagnostic metric name contains unsupported characters".into(),
                ));
            }
            safe_metrics.insert(name, value);
        }
        self.write_entry(DiagnosticLogEntry {
            at: Utc::now(),
            level,
            code,
            detail: "[REDACTED DETAIL]".into(),
            metrics: safe_metrics,
        })
    }

    fn write_entry(&self, entry: DiagnosticLogEntry) -> CoreResult<()> {
        let mut line = serde_json::to_vec(&entry)?;
        line.push(b'\n');
        self.rotate_if_needed(line.len() as u64)?;
        let path = self.directory.join("murmur.log");
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        file.write_all(&line)?;
        file.flush()?;
        Ok(())
    }

    /// Export this logger's generations while excluding concurrent writes and rotation.
    pub fn export(&self, destination: &Path) -> CoreResult<usize> {
        let _guard = self.write_lock.lock().map_err(|_| {
            crate::error::CoreError::Unavailable("diagnostic logger lock is poisoned".into())
        })?;
        export_redacted_log_generations(&self.directory, destination, self.generations)
    }

    fn rotate_if_needed(&self, incoming: u64) -> CoreResult<()> {
        let current = self.directory.join("murmur.log");
        let length = fs::metadata(&current).map(|value| value.len()).unwrap_or(0);
        if length == 0 || length + incoming <= self.max_bytes {
            return Ok(());
        }
        for generation in (1..=self.generations).rev() {
            let destination = self.directory.join(format!("murmur.log.{generation}"));
            if generation == self.generations && destination.exists() {
                fs::remove_file(&destination)?;
            }
            let source = if generation == 1 {
                current.clone()
            } else {
                self.directory
                    .join(format!("murmur.log.{}", generation - 1))
            };
            if source.exists() {
                fs::rename(source, destination)?;
            }
        }
        Ok(())
    }
}

pub fn redact_diagnostic_text(value: &str) -> String {
    let _ = value;
    "[REDACTED DETAIL]".into()
}

/// Export already-redacted rotating logs as a bounded JSON array. Each line is parsed and its
/// detail is redacted again, so manually altered log files cannot inject transcript content.
pub fn export_redacted_logs(log_directory: &Path, destination: &Path) -> CoreResult<usize> {
    export_redacted_log_generations(log_directory, destination, 4)
}

fn export_redacted_log_generations(
    log_directory: &Path,
    destination: &Path,
    generations: usize,
) -> CoreResult<usize> {
    let mut entries = Vec::new();
    for generation in (0..=generations).rev() {
        let path = if generation == 0 {
            log_directory.join("murmur.log")
        } else {
            log_directory.join(format!("murmur.log.{generation}"))
        };
        if !path.is_file() {
            continue;
        }
        let metadata = fs::metadata(&path)?;
        if metadata.len() > 2 * 1024 * 1024 {
            return Err(crate::error::CoreError::InvalidInput(
                "diagnostic log is unexpectedly large".into(),
            ));
        }
        for line in fs::read_to_string(path)?.lines() {
            let mut entry: DiagnosticLogEntry = serde_json::from_str(line)?;
            entry.detail = redact_diagnostic_text(&entry.detail);
            entries.push(entry);
        }
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(destination, serde_json::to_vec_pretty(&entries)?)?;
    Ok(entries.len())
}

fn validate_event_code(code: String) -> CoreResult<String> {
    if code.is_empty()
        || !code
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
    {
        return Err(crate::error::CoreError::InvalidInput(
            "diagnostic event code contains unsupported characters".into(),
        ));
    }
    Ok(code)
}

#[derive(Debug, Clone, Default)]
pub struct DiagnosticInputs<'a> {
    pub microphone: Option<Result<(), String>>,
    pub insertion: Option<Result<(), String>>,
    pub startup_enabled: Option<bool>,
    pub whisper_install_directory: Option<&'a Path>,
    pub updater_public_key_configured: Option<bool>,
}

pub fn deterministic_setup_plan() -> Vec<SetupStep> {
    vec![
        SetupStep {
            id: "data_directory".into(),
            required: true,
            description: "Create the per-user Murmur data directory and SQLite database.".into(),
        },
        SetupStep {
            id: "microphone".into(),
            required: true,
            description: "Check the Windows default microphone without overriding it.".into(),
        },
        SetupStep {
            id: "insertion".into(),
            required: true,
            description: "Check accessibility insertion and clipboard fallback.".into(),
        },
        SetupStep {
            id: "providers".into(),
            required: true,
            description:
                "Store and validate hosted provider credentials through Windows Credential Manager."
                    .into(),
        },
        SetupStep {
            id: "local_model".into(),
            required: false,
            description:
                "Download and verify the checksummed local Whisper model, or record No offline STT."
                    .into(),
        },
        SetupStep {
            id: "startup".into(),
            required: true,
            description: "Enable launch with Windows.".into(),
        },
        SetupStep {
            id: "private_updates".into(),
            required: true,
            description: "Validate private GitHub update access and Tauri signature configuration."
                .into(),
        },
    ]
}

pub fn run_diagnostics(data_dir: &Path, secrets: &dyn SecretStore) -> CoreResult<DiagnosticReport> {
    run_diagnostics_with(data_dir, secrets, DiagnosticInputs::default())
}

/// Build a report from probes that were actually run. Callers must not pass `Ok(())` until the
/// target-machine adapter completed the corresponding check.
pub fn run_diagnostics_with(
    data_dir: &Path,
    secrets: &dyn SecretStore,
    inputs: DiagnosticInputs<'_>,
) -> CoreResult<DiagnosticReport> {
    fs::create_dir_all(data_dir)?;
    let mut checks = vec![DiagnosticCheck {
        id: "platform".into(),
        status: if cfg!(windows) {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        message: if cfg!(windows) {
            "Windows platform detected.".into()
        } else {
            "Murmur V1 requires Windows 11.".into()
        },
    }];
    let probe = data_dir.join(".write-probe");
    let storage = match fs::write(&probe, b"murmur").and_then(|_| fs::remove_file(&probe)) {
        Ok(()) => DiagnosticCheck {
            id: "storage".into(),
            status: CheckStatus::Pass,
            message: "Application storage is writable.".into(),
        },
        Err(error) => DiagnosticCheck {
            id: "storage".into(),
            status: CheckStatus::Fail,
            message: format!("Application storage is not writable: {error}"),
        },
    };
    checks.push(storage);
    let mut configured_secrets = std::collections::HashSet::new();
    for name in ALLOWED_SECRET_NAMES {
        let (status, message) = match secrets.get(name) {
            Ok(Some(_)) => {
                configured_secrets.insert(*name);
                (CheckStatus::Pass, "Credential is configured.".to_string())
            }
            Ok(None) => (
                CheckStatus::Warning,
                "Credential is not configured.".to_string(),
            ),
            Err(error) => (
                CheckStatus::Warning,
                format!("Credential status unavailable: {error}"),
            ),
        };
        checks.push(DiagnosticCheck {
            id: format!("credential_{name}"),
            status,
            message,
        });
    }
    checks.push(DiagnosticCheck {
        id: "hosted_stt".into(),
        status: if ["deepgram", "assemblyai", "groq"]
            .iter()
            .any(|name| configured_secrets.contains(name))
        {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        message: if ["deepgram", "assemblyai", "groq"]
            .iter()
            .any(|name| configured_secrets.contains(name))
        {
            "At least one hosted STT provider is configured.".into()
        } else {
            "No hosted STT provider is configured.".into()
        },
    });
    checks.push(probe_check(
        "microphone",
        inputs.microphone,
        "Run the target-machine audio probe before accepting microphone capture.",
    ));
    checks.push(probe_check(
        "insertion",
        inputs.insertion,
        "Run the target application insertion check before accepting insertion.",
    ));
    checks.push(boolean_check(
        "startup",
        inputs.startup_enabled,
        "Launch with Windows has not been checked.",
        "Launch with Windows is enabled.",
        "Launch with Windows is disabled.",
    ));
    checks.push(match inputs.whisper_install_directory {
        Some(directory) => match validate_local_whisper_install(directory) {
            Ok(true) => DiagnosticCheck {
                id: "local_whisper_install".into(),
                status: CheckStatus::Pass,
                message: "Local whisper.cpp runtime and model checksums are valid.".into(),
            },
            Ok(false) => DiagnosticCheck {
                id: "local_whisper_install".into(),
                status: CheckStatus::Warning,
                message: "Local whisper.cpp runtime or model is missing. No offline STT.".into(),
            },
            Err(error) => DiagnosticCheck {
                id: "local_whisper_install".into(),
                status: CheckStatus::Warning,
                message: format!("Could not validate the local whisper.cpp installation: {error}"),
            },
        },
        None => DiagnosticCheck {
            id: "local_whisper_install".into(),
            status: CheckStatus::NotRun,
            message: "Local whisper.cpp installation was not checked.".into(),
        },
    });
    checks.push(boolean_check(
        "updater_signature",
        inputs.updater_public_key_configured,
        "Tauri updater signature configuration was not checked.",
        "Tauri updater public key is configured.",
        "Tauri updater public key is missing.",
    ));
    // Optional providers and local STT may warn without blocking setup. Hardware, insertion,
    // startup, hosted STT, Groq correction, and signed private updates are required.
    let required = [
        "platform",
        "storage",
        "hosted_stt",
        "credential_groq",
        "credential_github_updates",
        "microphone",
        "insertion",
        "startup",
        "updater_signature",
    ];
    let ok = required.iter().all(|id| {
        checks
            .iter()
            .any(|check| check.id == *id && check.status == CheckStatus::Pass)
    });
    Ok(DiagnosticReport {
        schema_version: 1,
        checks,
        ok,
    })
}

fn probe_check(id: &str, result: Option<Result<(), String>>, not_run: &str) -> DiagnosticCheck {
    match result {
        Some(Ok(())) => DiagnosticCheck {
            id: id.into(),
            status: CheckStatus::Pass,
            message: format!("{} check passed.", sentence_case(id)),
        },
        Some(Err(message)) => DiagnosticCheck {
            id: id.into(),
            status: CheckStatus::Fail,
            message,
        },
        None => DiagnosticCheck {
            id: id.into(),
            status: CheckStatus::NotRun,
            message: not_run.into(),
        },
    }
}

fn boolean_check(
    id: &str,
    value: Option<bool>,
    not_run: &str,
    passed: &str,
    failed: &str,
) -> DiagnosticCheck {
    match value {
        Some(true) => DiagnosticCheck {
            id: id.into(),
            status: CheckStatus::Pass,
            message: passed.into(),
        },
        Some(false) => DiagnosticCheck {
            id: id.into(),
            status: CheckStatus::Fail,
            message: failed.into(),
        },
        None => DiagnosticCheck {
            id: id.into(),
            status: CheckStatus::NotRun,
            message: not_run.into(),
        },
    }
}

fn sentence_case(value: &str) -> String {
    let mut characters = value.chars();
    characters
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + characters.as_str())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CoreResult;
    use std::sync::Arc;

    struct ConfiguredSecrets;

    impl SecretStore for ConfiguredSecrets {
        fn set(&self, _name: &str, _secret: &str) -> CoreResult<()> {
            Ok(())
        }

        fn get(&self, _name: &str) -> CoreResult<Option<String>> {
            Ok(Some("configured".into()))
        }

        fn delete(&self, _name: &str) -> CoreResult<()> {
            Ok(())
        }
    }

    #[test]
    fn not_run_checks_keep_setup_incomplete() {
        let directory = tempfile::tempdir().expect("temporary data directory");
        let report = run_diagnostics(directory.path(), &ConfiguredSecrets)
            .expect("diagnostics should complete");

        assert!(!report.ok);
        assert!(report
            .checks
            .iter()
            .any(|check| check.status == CheckStatus::NotRun));
    }

    #[test]
    fn rotating_logs_redact_content_and_credentials_on_write_and_export() {
        let directory = tempfile::tempdir().unwrap();
        let logger = DiagnosticLogger::with_limits(directory.path().join("logs"), 160, 2);
        logger
            .record(
                DiagnosticLevel::Error,
                "provider.failure",
                "authorization: secret-value",
            )
            .unwrap();
        logger
            .record(
                DiagnosticLevel::Warning,
                "correction.failure",
                "Raw Transcript: Xin chào Hong",
            )
            .unwrap();
        let export = directory.path().join("diagnostics.json");
        let count = export_redacted_logs(&directory.path().join("logs"), &export).unwrap();
        let text = fs::read_to_string(export).unwrap();
        assert_eq!(count, 2);
        assert!(!text.contains("secret-value"));
        assert!(!text.contains("Xin chào Hong"));
        assert!(text.contains("[REDACTED"));
    }

    #[test]
    fn logger_serializes_concurrent_rotation_and_redacts_sensitive_categories() {
        let directory = tempfile::tempdir().unwrap();
        let logger = Arc::new(DiagnosticLogger::with_limits(
            directory.path().join("logs"),
            220,
            8,
        ));
        let handles = (0..8)
            .map(|index| {
                let logger = Arc::clone(&logger);
                std::thread::spawn(move || {
                    let detail = match index % 3 {
                        0 => "Context Snapshot: private window text",
                        1 => "Selected Text: private selection",
                        _ => "token=private-token-value",
                    };
                    logger
                        .record(DiagnosticLevel::Warning, "runtime.failure", detail)
                        .unwrap();
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().unwrap();
        }

        let export = directory.path().join("diagnostics.json");
        assert_eq!(logger.export(&export).unwrap(), 8);
        let text = fs::read_to_string(export).unwrap();
        for secret in [
            "private window text",
            "private selection",
            "private-token-value",
        ] {
            assert!(!text.contains(secret));
        }
    }

    #[test]
    fn logger_discards_unlabelled_user_text_but_keeps_numeric_metrics() {
        let directory = tempfile::tempdir().unwrap();
        let logger = DiagnosticLogger::new(directory.path().join("logs"));
        logger
            .record(
                DiagnosticLevel::Info,
                "dictation.completed",
                "Xin chao Hong, this has no sensitive label",
            )
            .unwrap();
        logger
            .record_metrics(
                DiagnosticLevel::Info,
                "dictation.timing",
                [("elapsed_ms".into(), 420), ("fallbacks".into(), 1)],
            )
            .unwrap();
        let export = directory.path().join("diagnostics.json");
        logger.export(&export).unwrap();
        let text = fs::read_to_string(export).unwrap();
        assert!(!text.contains("Xin chao Hong"));
        assert!(text.contains("elapsed_ms"));
        assert!(text.contains("420"));
    }
}
