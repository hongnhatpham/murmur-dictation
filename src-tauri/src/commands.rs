use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use chrono::{Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_notification::NotificationExt;
use uuid::Uuid;

use crate::{
    adapters::{AudioCapture, CaptureRequest},
    backup::{BackupEntry, BackupManifest},
    db::{DictationTimingRecord, Repository, VocabularyRecord},
    diagnostics::{
        run_diagnostics_with, DiagnosticInputs, DiagnosticLevel, DiagnosticLogger,
        DiagnosticReport, SetupStep,
    },
    domain::{
        BriefItem, BriefItemKind, DictationResult, EditedSegment, EditedTranscript, MeetingBrief,
        ProcessingMode, RawTranscript, RecordingSession, SessionId, SessionKind, SessionSource,
        SessionStatus, TranscriptCitation, TranscriptSegment,
    },
    error::{CommandError, CoreError},
    providers::{
        download_checked_model, install_local_whisper_runtime_archive, is_empty_brief_entry,
        local_whisper_install_is_present, normalize_meeting_audio_chunks,
        validate_local_whisper_install, AudioRequest, CleanupLevel, CorrectionRequest,
        ExplicitTransformAction, LiveAudioPacket, LiveAudioSamples, LocalWhisperConfig,
        MeetingBriefRequest, SpeechProviders, TranscriptionOutcome, TranscriptionResult,
        VocabularyReplacement, LOCAL_WHISPER_MODEL_NAME, LOCAL_WHISPER_MODEL_SHA256,
        LOCAL_WHISPER_MODEL_URL, LOCAL_WHISPER_RUNTIME_SHA256, LOCAL_WHISPER_RUNTIME_URL,
    },
    retention::{RetentionReport, RetentionService},
    routing::{ProviderFailure, RouteDecision, RouteState},
    secrets::{provider_credentials, secret_statuses, SecretStore},
    updates::{GitHubUpdateClient, GitHubUpdateConfig},
};

#[cfg(windows)]
use crate::adapters::windows::{
    audio::{wave_data_layout, CaptureEvent, WindowsAudioCapture},
    context::{active_window, capture_context, ContextCapturePolicy, ContextSnapshot},
    meeting::{MeetingApplication, MeetingPromptTracker},
    shortcut::{GlobalShortcutService, ShortcutChord, ShortcutConfig, ShortcutEvent},
    InsertionTarget, WindowsTextInsertion,
};
#[cfg(windows)]
use windows::Win32::UI::WindowsAndMessaging::{
    SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
};

pub struct CoreState {
    pub repository: Mutex<Repository>,
    pub routes: Mutex<HashMap<SessionId, RouteState>>,
    pub retention: RetentionService,
    pub secrets: Arc<dyn SecretStore>,
    pub diagnostics: Arc<DiagnosticLogger>,
    pub data_dir: std::path::PathBuf,
    #[cfg(windows)]
    pub audio: Arc<WindowsAudioCapture>,
    #[cfg(windows)]
    pub insertion: Arc<WindowsTextInsertion>,
    #[cfg(windows)]
    pub shortcuts: Mutex<Option<GlobalShortcutService>>,
    pub active_dictation: Mutex<Option<ActiveRecording>>,
    pub active_meeting: Mutex<Option<ActiveRecording>>,
    pub meeting_jobs: Mutex<HashSet<SessionId>>,
    #[cfg(windows)]
    pub meeting_prompts: Mutex<MeetingPromptTracker>,
}

#[derive(Debug, Clone)]
pub struct ActiveRecording {
    pub session_id: SessionId,
    pub capture_id: String,
    pub started: Instant,
    pub source: SessionSource,
    #[cfg(windows)]
    pub streaming_result: Option<Arc<Mutex<mpsc::Receiver<Result<TranscriptionResult, String>>>>>,
    #[cfg(windows)]
    pub context: Option<ContextSnapshot>,
    #[cfg(windows)]
    pub insertion_target: Option<InsertionTarget>,
}

type CommandResult<T> = Result<T, CommandError>;

#[cfg(windows)]
const STREAMING_RESULT_TIMEOUT: Duration = Duration::from_secs(4);
#[cfg(windows)]
const DICTATION_LEVEL_INTERVAL: Duration = Duration::from_millis(50);

fn lock_error(name: &str) -> CommandError {
    CoreError::Unavailable(format!("{name} lock is poisoned")).into()
}

fn has_active_work(state: &CoreState) -> CommandResult<bool> {
    Ok(state
        .active_dictation
        .lock()
        .map_err(|_| lock_error("active dictation"))?
        .is_some()
        || state
            .active_meeting
            .lock()
            .map_err(|_| lock_error("active meeting"))?
            .is_some()
        || !state
            .meeting_jobs
            .lock()
            .map_err(|_| lock_error("meeting jobs"))?
            .is_empty())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    pub kind: SessionKind,
    pub source: SessionSource,
    pub title: Option<String>,
    pub local_stt_available: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteUpdate {
    pub decision: RouteDecision,
    pub route: RouteState,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    pub context_capture: bool,
    pub launch_at_startup: bool,
    pub meeting_suggestions: bool,
    pub offline_model: bool,
    pub dictation_retention_days: u32,
    pub meeting_retention_days: u32,
    #[serde(default = "default_hold_shortcut")]
    pub hold_shortcut: String,
    #[serde(default)]
    pub toggle_shortcut: String,
    #[serde(default)]
    pub microphone_id: Option<String>,
    #[serde(default)]
    pub excluded_applications: Vec<String>,
    #[serde(default)]
    pub excluded_meeting_applications: Vec<String>,
    #[serde(default)]
    pub cleanup_level: CleanupLevel,
}

fn default_hold_shortcut() -> String {
    "Ctrl+Win".into()
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            context_capture: true,
            launch_at_startup: true,
            meeting_suggestions: true,
            offline_model: true,
            dictation_retention_days: 7,
            meeting_retention_days: 30,
            hold_shortcut: default_hold_shortcut(),
            toggle_shortcut: String::new(),
            microphone_id: None,
            excluded_applications: vec![],
            excluded_meeting_applications: vec![],
            cleanup_level: CleanupLevel::Light,
        }
    }
}

#[tauri::command]
pub fn start_dictation(app: AppHandle, state: State<'_, CoreState>) -> CommandResult<()> {
    start_dictation_inner(&app, &state)
}

#[tauri::command]
pub fn delete_session(session_id: SessionId, state: State<'_, CoreState>) -> CommandResult<()> {
    let mut repository = state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?;
    let session = repository.session(session_id)?;
    repository.delete_session_explicit(session_id)?;
    drop(repository);
    if let Some(path) = session.audio_path.map(PathBuf::from) {
        let media_root = state.data_dir.join("media");
        if path.starts_with(&media_root) {
            if path.is_dir() {
                fs::remove_dir_all(path).map_err(CoreError::from)?;
            } else if path.is_file() {
                fs::remove_file(path).map_err(CoreError::from)?;
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub fn save_preferences(
    preferences: Preferences,
    app: AppHandle,
    state: State<'_, CoreState>,
) -> CommandResult<()> {
    if preferences.dictation_retention_days == 0 || preferences.meeting_retention_days == 0 {
        return Err(CoreError::InvalidInput("retention must be at least one day".into()).into());
    }
    #[cfg(windows)]
    {
        let config = shortcut_config(&preferences)?;
        if config
            .toggle
            .as_ref()
            .is_some_and(|toggle| toggle.normalized() == config.hold_to_talk.normalized())
        {
            return Err(CoreError::InvalidInput(
                "hold-to-talk and toggle shortcuts must be different".into(),
            )
            .into());
        }
    }
    write_preferences(&state.data_dir, &preferences)?;
    configure_startup(preferences.launch_at_startup).map_err(CommandError::from)?;
    #[cfg(windows)]
    reconfigure_shortcuts(&app, &state, &preferences)?;
    Ok(())
}

#[tauri::command]
pub fn get_preferences(state: State<'_, CoreState>) -> CommandResult<Preferences> {
    read_preferences(&state.data_dir).map_err(Into::into)
}

#[tauri::command]
pub fn create_session(
    request: CreateSessionRequest,
    state: State<'_, CoreState>,
) -> CommandResult<RecordingSession> {
    if request.kind == SessionKind::Dictation && request.source != SessionSource::Microphone {
        return Err(CoreError::InvalidInput(
            "dictation sessions require microphone capture".into(),
        )
        .into());
    }
    let session = RecordingSession::new(request.kind, request.source, request.title);
    state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .create_session(&session)
        .map_err(CommandError::from)?;
    let route = match request.kind {
        SessionKind::Dictation => RouteState::for_dictation(request.local_stt_available),
        SessionKind::Meeting => RouteState::for_meeting(request.local_stt_available),
    };
    state
        .routes
        .lock()
        .map_err(|_| lock_error("routes"))?
        .insert(session.id, route);
    Ok(session)
}

#[tauri::command]
pub fn list_sessions(
    kind: Option<SessionKind>,
    state: State<'_, CoreState>,
) -> CommandResult<Vec<RecordingSession>> {
    let mut repository = state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?;
    let _ = state.retention.purge(&mut repository, Utc::now());
    repository.list_sessions(kind).map_err(Into::into)
}

#[tauri::command]
pub fn route_state(
    session_id: SessionId,
    state: State<'_, CoreState>,
) -> CommandResult<RouteState> {
    state
        .routes
        .lock()
        .map_err(|_| lock_error("routes"))?
        .get(&session_id)
        .cloned()
        .ok_or_else(|| CoreError::NotFound(format!("route for session {session_id}")).into())
}

#[tauri::command]
pub fn report_provider_failure(
    session_id: SessionId,
    failure: ProviderFailure,
    state: State<'_, CoreState>,
) -> CommandResult<RouteUpdate> {
    let mut routes = state.routes.lock().map_err(|_| lock_error("routes"))?;
    let route = routes.get_mut(&session_id).ok_or_else(|| {
        CommandError::from(CoreError::NotFound(format!(
            "route for session {session_id}"
        )))
    })?;
    let decision = route.fail(failure);
    let provider = route.current_provider().map(|provider| provider.id());
    let session_status = if decision == RouteDecision::QueueEnhancement {
        SessionStatus::PendingEnhancement
    } else {
        SessionStatus::Processing
    };
    let mode = route.processing_mode();
    state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .set_session_state(session_id, session_status, Some(mode), provider)
        .map_err(CommandError::from)?;
    if decision == RouteDecision::QueueEnhancement {
        state
            .repository
            .lock()
            .map_err(|_| lock_error("repository"))?
            .queue_enhancement(
                session_id,
                "transcription",
                Some("all configured STT routes unavailable"),
            )
            .map_err(CommandError::from)?;
    }
    Ok(RouteUpdate {
        decision,
        route: route.clone(),
    })
}

#[tauri::command]
pub fn complete_transcription(
    transcript: RawTranscript,
    state: State<'_, CoreState>,
) -> CommandResult<RawTranscript> {
    let session_id = transcript.session_id;
    state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .insert_raw_transcript(&transcript)
        .map_err(CommandError::from)?;
    if let Some(route) = state
        .routes
        .lock()
        .map_err(|_| lock_error("routes"))?
        .get_mut(&session_id)
    {
        route.succeed();
    }
    state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .set_session_state(
            session_id,
            SessionStatus::Completed,
            None,
            Some(&transcript.provider),
        )
        .map_err(CommandError::from)?;
    Ok(transcript)
}

#[tauri::command]
pub fn get_raw_transcript(
    transcript_id: Uuid,
    state: State<'_, CoreState>,
) -> CommandResult<RawTranscript> {
    state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .raw_transcript(transcript_id)
        .map_err(Into::into)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DictationDetailView {
    id: SessionId,
    result: String,
    raw_transcript: String,
    insertion_status: &'static str,
    processing_mode: ProcessingMode,
    provider: String,
    target_application: Option<String>,
    language: &'static str,
    uncorrected: bool,
    capture_ms: u64,
    transcription_ms: u64,
    correction_ms: u64,
    insertion_ms: u64,
    expires_at: chrono::DateTime<Utc>,
}

fn language_label(language: &str) -> &'static str {
    let value = language.to_ascii_lowercase();
    if value.starts_with("vi") {
        "Vietnamese"
    } else if value.starts_with("en") {
        "English"
    } else {
        "Mixed"
    }
}

fn dictation_detail(
    repository: &Repository,
    session_id: SessionId,
) -> Result<DictationDetailView, CoreError> {
    let session = repository.session(session_id)?;
    if session.kind != SessionKind::Dictation {
        return Err(CoreError::InvalidInput("session is not a dictation".into()));
    }
    let record = repository.dictation_result(session_id)?;
    let legacy_capture_ms = record
        .raw
        .segments
        .iter()
        .map(|segment| segment.end_ms)
        .max()
        .unwrap_or_default();
    let legacy_capture_and_transcription_ms = record
        .raw
        .created_at
        .signed_duration_since(session.created_at)
        .num_milliseconds()
        .max(0) as u64;
    let legacy_correction_and_insertion_ms = record
        .result
        .created_at
        .signed_duration_since(record.raw.created_at)
        .num_milliseconds()
        .max(0) as u64;
    let insertion_status = if record.result.insertion_status == "retained_clipboard" {
        "clipboard"
    } else {
        "inserted"
    };
    let timing = repository
        .dictation_timing(session_id)?
        .unwrap_or(DictationTimingRecord {
            capture_ms: legacy_capture_ms,
            transcription_ms: legacy_capture_and_transcription_ms.saturating_sub(legacy_capture_ms),
            correction_ms: legacy_correction_and_insertion_ms,
            insertion_ms: 0,
        });
    Ok(DictationDetailView {
        id: session_id,
        result: record.result.text,
        raw_transcript: record.raw.text(),
        insertion_status,
        processing_mode: session.processing_mode.unwrap_or(ProcessingMode::Queued),
        provider: record.raw.provider,
        target_application: None,
        language: language_label(&record.raw.language),
        uncorrected: !record.result.corrected,
        capture_ms: timing.capture_ms,
        transcription_ms: timing.transcription_ms,
        correction_ms: timing.correction_ms,
        insertion_ms: timing.insertion_ms,
        expires_at: session.expires_at,
    })
}

#[tauri::command]
pub fn get_dictation_detail(
    session_id: SessionId,
    state: State<'_, CoreState>,
) -> CommandResult<DictationDetailView> {
    let repository = state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?;
    dictation_detail(&repository, session_id).map_err(Into::into)
}

#[tauri::command]
pub fn transform_dictation(
    session_id: SessionId,
    action: String,
    state: State<'_, CoreState>,
) -> CommandResult<DictationDetailView> {
    let mut detail = {
        let repository = state
            .repository
            .lock()
            .map_err(|_| lock_error("repository"))?;
        dictation_detail(&repository, session_id)?
    };
    let action = match action.as_str() {
        "rewrite" => ExplicitTransformAction::Rewrite,
        "translate-english" => ExplicitTransformAction::TranslateEnglish,
        "translate-vietnamese" => ExplicitTransformAction::TranslateVietnamese,
        _ => {
            return Err(
                CoreError::InvalidInput(format!("unknown dictation transform: {action}")).into(),
            )
        }
    };
    let providers = SpeechProviders::new(provider_credentials(state.secrets.as_ref())?)?;
    detail.result = providers
        .transform_text(&detail.result, action)
        .map_err(|error| CoreError::Unavailable(error.message))?;
    detail.uncorrected = false;
    state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .update_dictation_result_text(session_id, &detail.result, true)?;
    Ok(detail)
}

#[tauri::command]
pub fn save_edited_transcript(
    edited: EditedTranscript,
    state: State<'_, CoreState>,
) -> CommandResult<()> {
    state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .insert_edited_transcript(&edited)
        .map_err(Into::into)
}

#[tauri::command]
pub fn save_meeting_brief(brief: MeetingBrief, state: State<'_, CoreState>) -> CommandResult<()> {
    let session_id = brief.session_id;
    let mut repository = state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?;
    repository
        .insert_meeting_brief(&brief)
        .map_err(CommandError::from)?;
    repository
        .rebuild_meeting_search(session_id)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn get_meeting_brief(
    session_id: SessionId,
    state: State<'_, CoreState>,
) -> CommandResult<Option<MeetingBrief>> {
    state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .meeting_brief(session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn search_meetings(
    query: String,
    limit: Option<u32>,
    state: State<'_, CoreState>,
) -> CommandResult<Vec<SessionId>> {
    state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .search_meetings(&query, limit.unwrap_or(50).min(100))
        .map_err(Into::into)
}

#[tauri::command]
pub fn search_sessions(
    query: String,
    limit: Option<u32>,
    state: State<'_, CoreState>,
) -> CommandResult<Vec<SessionId>> {
    state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .search_sessions(&query, limit.unwrap_or(50).min(100))
        .map_err(Into::into)
}

#[tauri::command]
pub fn set_meeting_pinned(
    session_id: SessionId,
    pinned: bool,
    state: State<'_, CoreState>,
) -> CommandResult<()> {
    state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .set_pinned(session_id, pinned)
        .map_err(Into::into)
}

#[tauri::command]
pub fn purge_expired(state: State<'_, CoreState>) -> CommandResult<RetentionReport> {
    let mut repository = state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?;
    state
        .retention
        .purge(&mut repository, Utc::now())
        .map_err(Into::into)
}

#[tauri::command]
pub fn diagnostic_report(state: State<'_, CoreState>) -> CommandResult<DiagnosticReport> {
    runtime_diagnostics(&state).map_err(Into::into)
}

fn runtime_diagnostics(state: &CoreState) -> Result<DiagnosticReport, CoreError> {
    #[cfg(windows)]
    let microphone = match crate::adapters::windows::list_microphones() {
        Ok(devices) if !devices.is_empty() => Some(Ok(())),
        Ok(_) => Some(Err("Windows reports no active microphone endpoint".into())),
        Err(error) => Some(Err(error.to_string())),
    };
    #[cfg(not(windows))]
    let microphone = Some(Err("microphone checks require Windows".into()));
    let model_directory = state.data_dir.join("models");
    run_diagnostics_with(
        &state.data_dir,
        state.secrets.as_ref(),
        DiagnosticInputs {
            microphone,
            insertion: state
                .data_dir
                .join("insertion-check.json")
                .is_file()
                .then_some(Ok(())),
            startup_enabled: Some(startup_enabled()),
            whisper_install_directory: Some(&model_directory),
            updater_public_key_configured: Some(
                option_env!("MURMUR_UPDATER_PUBLIC_KEY").is_some_and(|key| !key.trim().is_empty()),
            ),
        },
    )
}

#[cfg(windows)]
fn startup_enabled() -> bool {
    std::process::Command::new("reg.exe")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            "Murmur",
        ])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn startup_enabled() -> bool {
    false
}

#[tauri::command]
pub fn setup_plan() -> Vec<SetupStep> {
    crate::diagnostics::deterministic_setup_plan()
}

#[tauri::command]
pub fn build_backup_manifest(
    app_version: String,
    entries: Vec<BackupEntry>,
) -> CommandResult<BackupManifest> {
    BackupManifest::new(app_version, entries).map_err(Into::into)
}

#[tauri::command]
pub fn observe_attributed_correction(
    heard: String,
    replacement: String,
    state: State<'_, CoreState>,
) -> CommandResult<bool> {
    state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .observe_attributed_correction(&heard, &replacement)
        .map_err(Into::into)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DictationStateEvent {
    session_id: Option<SessionId>,
    state: &'static str,
    mode: Option<&'static str>,
    provider: Option<String>,
    elapsed_ms: Option<u64>,
    target_app: Option<String>,
    message: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
struct DictationAudioLevelEvent {
    session_id: SessionId,
    rms: f32,
    peak: f32,
}

fn emit_dictation(
    app: &AppHandle,
    session_id: Option<SessionId>,
    state: &'static str,
    mode: Option<&'static str>,
    provider: Option<String>,
    elapsed_ms: Option<u64>,
    target_app: Option<String>,
    message: Option<String>,
) {
    let terminal = matches!(
        state,
        "inserted" | "raw" | "clipboard" | "failed" | "cancelled"
    );
    let _ = app.emit(
        "murmur://dictation-state",
        DictationStateEvent {
            session_id,
            state,
            mode,
            provider,
            elapsed_ms,
            target_app,
            message,
        },
    );
    if let Some(window) = app.get_webview_window("dictation-overlay") {
        if !terminal {
            if let Ok(Some(monitor)) = window.current_monitor() {
                if let Ok(overlay) = window.outer_size() {
                    let monitor_size = monitor.size();
                    let monitor_position = monitor.position();
                    let x = monitor_position.x
                        + (i32::try_from(monitor_size.width).unwrap_or(i32::MAX)
                            - i32::try_from(overlay.width).unwrap_or_default())
                            / 2;
                    let y = monitor_position.y
                        + i32::try_from(monitor_size.height).unwrap_or(i32::MAX)
                        - i32::try_from(overlay.height).unwrap_or_default()
                        - 48;
                    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
                }
            }
            show_dictation_overlay(
                || window.show(),
                || {
                    let window = window.hwnd()?;
                    unsafe {
                        SetWindowPos(
                            window,
                            Some(HWND_TOPMOST),
                            0,
                            0,
                            0,
                            0,
                            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                        )
                    }
                    .map_err(|error| tauri::Error::Anyhow(error.into()))
                },
            );
        } else {
            let handle = app.clone();
            let _ = thread::Builder::new()
                .name("murmur-overlay-hide".into())
                .spawn(move || {
                    thread::sleep(std::time::Duration::from_millis(1_500));
                    let core = handle.state::<CoreState>();
                    let active = core
                        .active_dictation
                        .lock()
                        .map(|recording| recording.is_some())
                        .unwrap_or(true);
                    if !active {
                        if let Some(window) = handle.get_webview_window("dictation-overlay") {
                            let _ = window.hide();
                        }
                    }
                });
        }
    }
}

fn show_dictation_overlay(
    show: impl FnOnce() -> tauri::Result<()>,
    raise_without_activation: impl FnOnce() -> tauri::Result<()>,
) {
    let _ = show();
    let _ = raise_without_activation();
}

fn emit_sessions_changed(app: &AppHandle) {
    let _ = app.emit("murmur://sessions-changed", serde_json::json!({}));
}

#[cfg(windows)]
fn emit_audio_events(app: &AppHandle, session_id: SessionId, capture_id: &str) {
    let state = app.state::<CoreState>();
    let Ok(events) = state.audio.drain_events(capture_id) else {
        return;
    };
    for event in events {
        let (payload, notification) = match event {
            CaptureEvent::ChannelLost {
                channel, reason, ..
            } => (
                serde_json::json!({
                    "channel": channel,
                    "state": "lost",
                    "message": reason,
                    "sessionId": session_id,
                }),
                Some("An audio device disconnected. Murmur kept the audio recorded so far."),
            ),
            CaptureEvent::ChannelRecovered { channel, .. } => (
                serde_json::json!({
                    "channel": channel,
                    "state": "restored",
                    "message": "The original audio endpoint recovered.",
                    "sessionId": session_id,
                }),
                Some("The original audio device reconnected."),
            ),
        };
        let _ = app.emit("murmur://audio-device", payload);
        if let Some(message) = notification {
            let _ = app
                .notification()
                .builder()
                .title("Murmur audio status")
                .body(message)
                .show();
        }
    }
}

fn set_tray(app: &AppHandle, tooltip: &str) {
    if let Some(tray) = app.tray_by_id("murmur") {
        let _ = tray.set_tooltip(Some(tooltip));
    }
}

#[cfg(windows)]
fn set_shortcut_recording(state: &CoreState, active: bool) {
    if let Ok(shortcuts) = state.shortcuts.lock() {
        if let Some(service) = shortcuts.as_ref() {
            let _ = service.set_external_dictation_active(active);
        }
    }
}

#[cfg(windows)]
pub fn start_dictation_inner(app: &AppHandle, state: &CoreState) -> CommandResult<()> {
    let mut active = state
        .active_dictation
        .lock()
        .map_err(|_| lock_error("active dictation"))?;
    if active.is_some() {
        return Ok(());
    }
    if state
        .active_meeting
        .lock()
        .map_err(|_| lock_error("active meeting"))?
        .is_some()
    {
        return Err(CoreError::Unavailable(
            "finish the active meeting before starting dictation".into(),
        )
        .into());
    }
    let preferences = read_preferences(&state.data_dir)?;
    let mut session = RecordingSession::new(
        SessionKind::Dictation,
        SessionSource::Microphone,
        Some("Dictation".into()),
    );
    session.expires_at =
        session.created_at + ChronoDuration::days(i64::from(preferences.dictation_retention_days));
    let output_directory = state.data_dir.join("media").join(session.id.to_string());
    let insertion_target = state.insertion.capture_target().ok();
    set_shortcut_recording(state, true);
    let _ = state.diagnostics.record(
        DiagnosticLevel::Info,
        "dictation.started",
        "microphone capture started",
    );
    let capture_id = match state.audio.start(CaptureRequest {
        microphone_device: preferences.microphone_id.clone(),
        include_system_audio: false,
        output_directory,
    }) {
        Ok(capture_id) => capture_id,
        Err(error) => {
            set_shortcut_recording(state, false);
            emit_dictation(
                app,
                Some(session.id),
                "failed",
                None,
                None,
                Some(0),
                None,
                Some(error.to_string()),
            );
            return Err(error.into());
        }
    };
    let started = Instant::now();
    // Subscribe before context and database work so the hosted stream starts at packet zero.
    // The durable WAV remains authoritative when any packet is missed.
    let streaming_result = start_streaming_capture(state, &capture_id);
    let context = capture_context(&ContextCapturePolicy {
        enabled: preferences.context_capture,
        denied_applications: preferences.excluded_applications,
    })
    .ok();
    emit_dictation(
        app,
        Some(session.id),
        "recording",
        None,
        None,
        Some(0),
        context.as_ref().map(|value| value.process_name.clone()),
        None,
    );
    session.audio_path = Some(
        state
            .data_dir
            .join("media")
            .join(session.id.to_string())
            .to_string_lossy()
            .into(),
    );
    if let Err(error) = state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .create_session(&session)
    {
        let _ = state.audio.cancel(&capture_id);
        set_shortcut_recording(state, false);
        emit_dictation(
            app,
            Some(session.id),
            "failed",
            None,
            None,
            Some(started.elapsed().as_millis() as u64),
            None,
            Some(error.to_string()),
        );
        return Err(error.into());
    }
    let target_app = context.as_ref().map(|value| value.process_name.clone());
    *active = Some(ActiveRecording {
        session_id: session.id,
        capture_id,
        started,
        source: SessionSource::Microphone,
        streaming_result,
        context,
        insertion_target,
    });
    emit_dictation(
        app,
        Some(session.id),
        "recording",
        None,
        None,
        Some(0),
        target_app,
        None,
    );
    set_tray(app, "Murmur - dictating");
    spawn_dictation_ticker(app.clone(), session.id);
    spawn_dictation_level_ticker(app.clone(), session.id);
    emit_sessions_changed(app);
    Ok(())
}

#[cfg(windows)]
fn spawn_dictation_ticker(app: AppHandle, session_id: SessionId) {
    let _ = thread::Builder::new()
        .name("murmur-dictation-ticker".into())
        .spawn(move || loop {
            thread::sleep(std::time::Duration::from_secs(1));
            let state = app.state::<CoreState>();
            let active = state
                .active_dictation
                .lock()
                .ok()
                .and_then(|recording| recording.clone());
            let Some(recording) = active.filter(|recording| recording.session_id == session_id)
            else {
                break;
            };
            emit_audio_events(&app, session_id, &recording.capture_id);
            emit_dictation(
                &app,
                Some(session_id),
                "recording",
                None,
                None,
                Some(recording.started.elapsed().as_millis() as u64),
                recording
                    .context
                    .as_ref()
                    .map(|context| context.process_name.clone()),
                None,
            );
        });
}

#[cfg(windows)]
fn spawn_dictation_level_ticker(app: AppHandle, session_id: SessionId) {
    let _ = thread::Builder::new()
        .name("murmur-dictation-level".into())
        .spawn(move || loop {
            thread::sleep(DICTATION_LEVEL_INTERVAL);
            let state = app.state::<CoreState>();
            let active = state
                .active_dictation
                .lock()
                .ok()
                .and_then(|recording| recording.clone());
            let Some(recording) = active.filter(|recording| recording.session_id == session_id)
            else {
                break;
            };
            let Ok(Some(level)) = state.audio.take_microphone_level(&recording.capture_id) else {
                continue;
            };
            let _ = app.emit(
                "murmur://dictation-audio-level",
                DictationAudioLevelEvent {
                    session_id,
                    rms: level.rms,
                    peak: level.peak,
                },
            );
        });
}

#[cfg(windows)]
fn start_streaming_capture(
    state: &CoreState,
    capture_id: &str,
) -> Option<Arc<Mutex<mpsc::Receiver<Result<TranscriptionResult, String>>>>> {
    let packets = state.audio.subscribe_microphone(capture_id).ok()?;
    let credentials = provider_credentials(state.secrets.as_ref()).ok()?;
    if credentials.deepgram.is_none() && credentials.assemblyai.is_none() {
        return None;
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    let _ = thread::Builder::new()
        .name("murmur-streaming-dictation".into())
        .spawn(move || {
            let result = (|| -> Result<TranscriptionResult, String> {
                let first = packets
                    .recv()
                    .map_err(|_| "microphone stream ended before its first packet".to_string())?;
                let mut expected_sequence = 0_u64;
                validate_stream_packet(&first, expected_sequence)?;
                expected_sequence = expected_sequence.saturating_add(1);
                let first_live = live_packet(first)?;
                let providers =
                    SpeechProviders::new(credentials).map_err(|error| error.to_string())?;
                let mut stream = providers
                    .start_streaming_dictation(first_live.sample_rate_hz, first_live.channels)
                    .map_err(|error| error.message)?;
                stream
                    .push_audio(first_live)
                    .map_err(|error| error.message)?;
                while let Ok(packet) = packets.recv() {
                    validate_stream_packet(&packet, expected_sequence)?;
                    expected_sequence = expected_sequence.saturating_add(1);
                    stream
                        .push_audio(live_packet(packet)?)
                        .map_err(|error| error.message)?;
                }
                stream.finish().map_err(|error| error.message)
            })();
            let _ = sender.send(result);
        })
        .ok()?;
    Some(Arc::new(Mutex::new(receiver)))
}

#[cfg(windows)]
fn validate_stream_packet(
    packet: &crate::adapters::windows::audio::AudioPacket,
    expected_sequence: u64,
) -> Result<(), String> {
    if packet.dropped_before > 0 || packet.sequence != expected_sequence {
        return Err(format!(
            "hosted stream missed audio packets before sequence {}",
            packet.sequence
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn live_packet(
    packet: crate::adapters::windows::audio::AudioPacket,
) -> Result<LiveAudioPacket, String> {
    use crate::adapters::windows::audio::SampleEncoding;

    let samples = match (packet.format.encoding, packet.format.bits_per_sample) {
        (SampleEncoding::Pcm, 16) => LiveAudioSamples::Pcm16(
            packet
                .bytes
                .chunks_exact(2)
                .map(|sample| i16::from_le_bytes([sample[0], sample[1]]))
                .collect(),
        ),
        (SampleEncoding::Float, 32) => LiveAudioSamples::Float32(
            packet
                .bytes
                .chunks_exact(4)
                .map(|sample| f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]))
                .collect(),
        ),
        _ => {
            return Err(format!(
                "streaming does not support WASAPI {:?} {}-bit samples",
                packet.format.encoding, packet.format.bits_per_sample
            ))
        }
    };
    Ok(LiveAudioPacket {
        sample_rate_hz: packet.format.sample_rate,
        channels: packet.format.channels,
        samples,
    })
}

#[cfg(not(windows))]
pub fn start_dictation_inner(_app: &AppHandle, _state: &CoreState) -> CommandResult<()> {
    Err(CoreError::Unavailable("dictation capture requires Windows 11".into()).into())
}

#[tauri::command]
pub fn stop_dictation(app: AppHandle, state: State<'_, CoreState>) -> CommandResult<()> {
    stop_dictation_inner(&app, &state)
}

pub fn stop_dictation_inner(app: &AppHandle, state: &CoreState) -> CommandResult<()> {
    #[cfg(windows)]
    {
        let recording = state
            .active_dictation
            .lock()
            .map_err(|_| lock_error("active dictation"))?
            .take()
            .ok_or_else(|| CoreError::NotFound("active dictation".into()))?;
        set_shortcut_recording(state, false);
        let files = state.audio.stop(&recording.capture_id)?;
        let initial_mode = provider_credentials(state.secrets.as_ref())
            .map(|credentials| {
                if recording.streaming_result.is_some() {
                    "hosted"
                } else if credentials.groq.is_some() {
                    "hosted-fallback"
                } else if local_whisper(&state.data_dir).is_some() {
                    "local"
                } else {
                    "hosted-fallback"
                }
            })
            .unwrap_or("local");
        emit_dictation(
            &app,
            Some(recording.session_id),
            "transcribing",
            Some(initial_mode),
            None,
            Some(recording.started.elapsed().as_millis() as u64),
            recording
                .context
                .as_ref()
                .map(|context| context.process_name.clone()),
            None,
        );
        set_tray(app, "Murmur - processing dictation");
        let app_handle = app.clone();
        thread::Builder::new()
            .name("murmur-dictation-pipeline".into())
            .spawn(move || process_dictation(app_handle, recording, files))
            .map_err(CoreError::Io)?;
        return Ok(());
    }
    #[cfg(not(windows))]
    Err(CoreError::Unavailable("dictation capture requires Windows 11".into()).into())
}

#[tauri::command]
pub fn cancel_dictation(app: AppHandle, state: State<'_, CoreState>) -> CommandResult<()> {
    cancel_dictation_inner(&app, &state)
}

pub fn cancel_dictation_inner(app: &AppHandle, state: &CoreState) -> CommandResult<()> {
    #[cfg(windows)]
    {
        let recording = state
            .active_dictation
            .lock()
            .map_err(|_| lock_error("active dictation"))?
            .take()
            .ok_or_else(|| CoreError::NotFound("active dictation".into()))?;
        set_shortcut_recording(state, false);
        state.audio.cancel(&recording.capture_id)?;
        state
            .repository
            .lock()
            .map_err(|_| lock_error("repository"))?
            .set_session_state(recording.session_id, SessionStatus::Cancelled, None, None)?;
        emit_dictation(
            &app,
            Some(recording.session_id),
            "cancelled",
            None,
            None,
            Some(recording.started.elapsed().as_millis() as u64),
            None,
            None,
        );
        set_tray(app, "Murmur");
        emit_sessions_changed(&app);
        return Ok(());
    }
    #[cfg(not(windows))]
    Err(CoreError::Unavailable("dictation capture requires Windows 11".into()).into())
}

#[cfg(windows)]
fn process_dictation(app: AppHandle, recording: ActiveRecording, files: Vec<PathBuf>) {
    if let Err(error) = process_dictation_result(&app, &recording, &files) {
        let state = app.state::<CoreState>();
        let _ = state.diagnostics.record(
            DiagnosticLevel::Error,
            "dictation.failed",
            "speech processing failed; diagnostic audio retained",
        );
        if let Ok(mut repository) = state.repository.lock() {
            let _ = repository.set_session_state(
                recording.session_id,
                SessionStatus::Failed,
                None,
                None,
            );
            let _ = repository
                .set_session_expiry(recording.session_id, Utc::now() + ChronoDuration::hours(24));
        }
        emit_dictation(
            &app,
            Some(recording.session_id),
            "failed",
            None,
            None,
            Some(recording.started.elapsed().as_millis() as u64),
            None,
            Some(error.to_string()),
        );
        set_tray(&app, "Murmur - needs attention");
        emit_sessions_changed(&app);
    }
}

#[cfg(windows)]
fn process_dictation_result(
    app: &AppHandle,
    recording: &ActiveRecording,
    files: &[PathBuf],
) -> Result<(), CoreError> {
    let processing_started = Instant::now();
    let capture_ms = recording.started.elapsed().as_millis() as u64;
    let state = app.state::<CoreState>();
    let audio_path = merge_audio_chunks(
        files,
        &state
            .data_dir
            .join("media")
            .join(recording.session_id.to_string())
            .join("dictation.wav"),
    )?;
    let credentials = provider_credentials(state.secrets.as_ref())?;
    let providers = SpeechProviders::new(credentials)?;
    let local = local_whisper(&state.data_dir);
    let streamed = recording
        .streaming_result
        .as_ref()
        .and_then(receive_streamed_transcript);
    let outcome = if let Some(transcript) = streamed {
        TranscriptionOutcome::Completed {
            transcript,
            attempts: vec![],
        }
    } else {
        let audio_request = AudioRequest::from_file(&audio_path, "audio/wav")?;
        providers.transcribe_with_fallback_observed(
            SessionKind::Dictation,
            &audio_request,
            local.as_ref(),
            |attempt| {
                emit_dictation(
                    app,
                    Some(recording.session_id),
                    "transcribing",
                    Some(mode_label(attempt.mode)),
                    Some(attempt.provider),
                    Some(recording.started.elapsed().as_millis() as u64),
                    None,
                    None,
                );
            },
        )
    };
    let (transcription, fallback_count) = match outcome {
        TranscriptionOutcome::Completed {
            transcript,
            attempts,
        } => (transcript, attempts.len()),
        TranscriptionOutcome::Failed { attempts } => {
            return Err(CoreError::Unavailable(format!(
                "all speech providers failed; diagnostic audio was kept for recovery: {}",
                attempts
                    .last()
                    .map(|failure| failure.message.as_str())
                    .unwrap_or("no provider is configured")
            )))
        }
        TranscriptionOutcome::PendingEnhancement { .. } => unreachable!(),
    };
    let mode = processing_mode(&transcription.provider, fallback_count);
    let transcription_ms = processing_started.elapsed().as_millis() as u64;
    emit_dictation(
        app,
        Some(recording.session_id),
        "correcting",
        Some(mode_label(mode)),
        Some(transcription.provider.clone()),
        Some(recording.started.elapsed().as_millis() as u64),
        None,
        None,
    );
    set_tray(app, "Murmur - correcting dictation");
    let raw = to_raw_transcript(recording.session_id, transcription);
    let vocabulary = state
        .repository
        .lock()
        .map_err(|_| CoreError::Unavailable("repository lock is poisoned".into()))?
        .dictation_vocabulary()?
        .into_iter()
        .map(|entry| VocabularyReplacement {
            heard: entry.heard,
            replacement: entry.replacement,
        })
        .collect();
    let context = usable_correction_context(recording.context.as_ref());
    let cleanup_level = read_preferences(&state.data_dir)
        .map(|preferences| preferences.cleanup_level)
        .unwrap_or_default();
    let correction_started = Instant::now();
    let correction = providers.correct_dictation(&CorrectionRequest {
        raw_transcript: raw.text(),
        cleanup_level,
        application: context.map(|value| value.process_name.clone()),
        window_title: context.map(|value| value.window_title.clone()),
        selected_text: context.and_then(|value| value.selected_text.clone()),
        surrounding_text: context.and_then(|value| value.surrounding_text.clone()),
        personal_vocabulary: vocabulary,
    });
    let correction_ms = correction_started.elapsed().as_millis() as u64;
    let text = apply_spoken_formatting(&correction.text);
    emit_dictation(
        app,
        Some(recording.session_id),
        "inserting",
        Some(mode_label(mode)),
        correction.provider.clone(),
        Some(recording.started.elapsed().as_millis() as u64),
        None,
        None,
    );
    let insertion_started = Instant::now();
    // Clipboard paste creates the target application's normal undo unit. UI Automation
    // SetValue can report success without changing the visible document in modern Notepad.
    let insertion = state
        .insertion
        .paste_retaining_clipboard_for_target(&text, recording.insertion_target);
    let (insertion_status, terminal_state) = match insertion {
        Ok(()) => (
            "clipboard_paste",
            if correction.corrected {
                "inserted"
            } else {
                "raw"
            },
        ),
        Err(_) => ("retained_clipboard", "clipboard"),
    };
    let insertion_ms = insertion_started.elapsed().as_millis() as u64;
    let result = DictationResult {
        id: Uuid::new_v4(),
        session_id: recording.session_id,
        raw_transcript_id: raw.id,
        text,
        corrected: correction.corrected,
        insertion_status: insertion_status.into(),
        created_at: Utc::now(),
    };
    {
        let mut repository = state
            .repository
            .lock()
            .map_err(|_| CoreError::Unavailable("repository lock is poisoned".into()))?;
        repository.insert_raw_transcript(&raw)?;
        repository.insert_dictation_result(&result)?;
        repository.insert_dictation_timing(
            recording.session_id,
            DictationTimingRecord {
                capture_ms,
                transcription_ms,
                correction_ms,
                insertion_ms,
            },
        )?;
        let title = result.text.chars().take(96).collect::<String>();
        repository.set_session_title(recording.session_id, &title)?;
        repository.set_session_state(
            recording.session_id,
            SessionStatus::Completed,
            Some(mode),
            Some(&raw.provider),
        )?;
        repository.dispose_context(recording.session_id)?;
    }
    remove_success_audio(files, &audio_path);
    if insertion_status != "retained_clipboard" {
        let inserted = result.text.clone();
        let handle = app.clone();
        let _ = thread::Builder::new()
            .name("murmur-attributed-correction".into())
            .spawn(move || {
                if let Ok(Some(correction)) =
                    crate::adapters::windows::observe_attributed_correction(&inserted)
                {
                    let state = handle.state::<CoreState>();
                    if let Ok(mut repository) = state.repository.lock() {
                        let _ = repository.observe_attributed_correction(
                            &correction.heard,
                            &correction.replacement,
                        );
                    };
                }
            });
    }
    emit_dictation(
        app,
        Some(recording.session_id),
        terminal_state,
        Some(mode_label(mode)),
        Some(raw.provider),
        Some(recording.started.elapsed().as_millis() as u64),
        recording
            .context
            .as_ref()
            .map(|value| value.process_name.clone()),
        correction.error,
    );
    let _ = state.diagnostics.record(
        DiagnosticLevel::Info,
        "dictation.completed",
        format!("mode={} insertion={insertion_status}", mode.as_str()),
    );
    set_tray(app, "Murmur");
    emit_sessions_changed(app);
    Ok(())
}

#[cfg(windows)]
fn receive_streamed_transcript(
    result: &Arc<Mutex<mpsc::Receiver<Result<TranscriptionResult, String>>>>,
) -> Option<TranscriptionResult> {
    result
        .lock()
        .ok()
        .and_then(|receiver| receiver.recv_timeout(STREAMING_RESULT_TIMEOUT).ok())
        .and_then(Result::ok)
}

#[cfg(windows)]
fn usable_correction_context(context: Option<&ContextSnapshot>) -> Option<&ContextSnapshot> {
    context.filter(|snapshot| snapshot.excluded_reason.is_none())
}

fn to_raw_transcript(
    session_id: SessionId,
    transcript: crate::providers::TranscriptionResult,
) -> RawTranscript {
    let mut segments = transcript
        .segments
        .into_iter()
        .enumerate()
        .map(|(ordinal, segment)| TranscriptSegment {
            id: Uuid::new_v4(),
            ordinal: ordinal as u32,
            start_ms: segment.start_ms,
            end_ms: segment.end_ms.max(segment.start_ms),
            speaker: segment.speaker,
            text: segment.text,
        })
        .collect::<Vec<_>>();
    if segments.is_empty() {
        segments.push(TranscriptSegment {
            id: Uuid::new_v4(),
            ordinal: 0,
            start_ms: 0,
            end_ms: 0,
            speaker: None,
            text: transcript.text,
        });
    }
    RawTranscript {
        id: Uuid::new_v4(),
        session_id,
        language: transcript.language.unwrap_or_else(|| "en-vi".into()),
        provider: transcript.provider,
        created_at: Utc::now(),
        segments,
    }
}

fn processing_mode(provider: &str, prior_failures: usize) -> ProcessingMode {
    if provider.starts_with("local_") {
        ProcessingMode::Local
    } else if prior_failures > 0 {
        ProcessingMode::HostedFallback
    } else {
        ProcessingMode::Hosted
    }
}

fn mode_label(mode: ProcessingMode) -> &'static str {
    match mode {
        ProcessingMode::Hosted => "hosted",
        ProcessingMode::HostedFallback => "hosted-fallback",
        ProcessingMode::Local => "local",
        ProcessingMode::Queued => "queued",
    }
}

fn local_whisper(data_dir: &Path) -> Option<LocalWhisperConfig> {
    let directory = data_dir.join("models");
    let executable = directory.join("whisper-cli.exe");
    let model = directory.join(LOCAL_WHISPER_MODEL_NAME);
    // Setup and diagnostics verify the checksums. The utterance hot path only checks presence;
    // hashing the 574 MB model here added more than twelve seconds to every dictation.
    (executable.is_file() && model.is_file()).then_some(LocalWhisperConfig { executable, model })
}

fn apply_spoken_formatting(value: &str) -> String {
    let words = value.split_whitespace().collect::<Vec<_>>();
    let mut output = String::new();
    let mut index = 0;
    while index < words.len() {
        let word = words[index];
        let normalized = word
            .trim_matches(|character: char| !character.is_alphanumeric())
            .to_ascii_lowercase();
        let next = words.get(index + 1).map(|word| {
            word.trim_matches(|character: char| !character.is_alphanumeric())
                .to_ascii_lowercase()
        });
        let append = |output: &mut String, text: &str| {
            if !output.is_empty() && !output.ends_with(['\n', ' ']) {
                output.push(' ');
            }
            output.push_str(text);
        };
        if normalized == "literal" {
            if let Some(literal) = words.get(index + 1) {
                append(&mut output, literal);
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        match (normalized.as_str(), next.as_deref()) {
            ("new", Some("line")) => output.push('\n'),
            ("new", Some("paragraph")) => output.push_str("\n\n"),
            ("numbered", Some("item")) => output.push_str("\n1. "),
            ("open", Some("quote")) | ("close", Some("quote")) => output.push('"'),
            ("bullet", _) => {
                if !output.is_empty() && !output.ends_with('\n') {
                    output.push('\n');
                }
                output.push_str("- ");
                index += 1;
                continue;
            }
            _ => {
                append(&mut output, word);
                index += 1;
                continue;
            }
        }
        index += 2;
    }
    output.trim().to_string()
}

/// Concatenates same-format WAV chunks into `destination` for the meeting pipeline and test harness.
pub fn merge_audio_chunks(files: &[PathBuf], destination: &Path) -> Result<PathBuf, CoreError> {
    if files.is_empty() {
        return Err(CoreError::Unavailable(
            "the microphone produced no audio".into(),
        ));
    }
    if files.len() == 1 {
        return Ok(files[0].clone());
    }
    let first = fs::read(&files[0])?;
    let layout = wave_data_layout(&first)?;
    let data_size_offset = layout.data_size_offset;
    let data_offset = layout.data_offset;
    let mut header = first[..data_offset].to_vec();
    let mut payload = first[data_offset..].to_vec();
    for path in &files[1..] {
        let chunk = fs::read(path)?;
        let chunk_layout = wave_data_layout(&chunk)?;
        let chunk_size_offset = chunk_layout.data_size_offset;
        let chunk_data_offset = chunk_layout.data_offset;
        if chunk_data_offset != data_offset
            || chunk_size_offset != data_size_offset
            || chunk[8..data_size_offset] != header[8..data_size_offset]
        {
            return Err(CoreError::InvalidInput(
                "audio chunks use incompatible WAV formats".into(),
            ));
        }
        payload.extend_from_slice(&chunk[chunk_data_offset..]);
    }
    let data_size = u32::try_from(payload.len())
        .map_err(|_| CoreError::Unavailable("combined audio exceeds WAV limits".into()))?;
    let riff_size = u32::try_from(header.len() + payload.len() - 8)
        .map_err(|_| CoreError::Unavailable("combined audio exceeds WAV limits".into()))?;
    header[4..8].copy_from_slice(&riff_size.to_le_bytes());
    header[data_size_offset..data_size_offset + 4].copy_from_slice(&data_size.to_le_bytes());
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut output = fs::File::create(destination)?;
    output.write_all(&header)?;
    output.write_all(&payload)?;
    output.sync_all()?;
    Ok(destination.to_path_buf())
}

fn remove_success_audio(files: &[PathBuf], merged: &Path) {
    for path in files {
        let _ = fs::remove_file(path);
    }
    if !files.iter().any(|path| path == merged) {
        let _ = fs::remove_file(merged);
    }
}

pub(crate) fn read_preferences(data_dir: &Path) -> Result<Preferences, CoreError> {
    let path = data_dir.join("preferences.json");
    match fs::read(&path) {
        Ok(value) => serde_json::from_slice(&value).map_err(Into::into),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Preferences::default()),
        Err(error) => Err(error.into()),
    }
}

fn write_preferences(data_dir: &Path, preferences: &Preferences) -> Result<(), CoreError> {
    let path = data_dir.join("preferences.json");
    let value = serde_json::to_vec_pretty(preferences)?;
    fs::write(path, value)?;
    Ok(())
}

#[cfg(windows)]
pub(crate) fn configure_startup(enabled: bool) -> Result<(), CoreError> {
    let executable = std::env::current_exe()?;
    let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    let mut command = std::process::Command::new("reg.exe");
    if enabled {
        command.args([
            "add",
            key,
            "/v",
            "Murmur",
            "/t",
            "REG_SZ",
            "/d",
            &format!("\"{}\"", executable.display()),
            "/f",
        ]);
    } else {
        command.args(["delete", key, "/v", "Murmur", "/f"]);
    }
    let status = command.status()?;
    if status.success() || !enabled {
        Ok(())
    } else {
        Err(CoreError::Unavailable(format!(
            "Windows startup registration exited with {status}"
        )))
    }
}

#[cfg(windows)]
fn reconfigure_shortcuts(
    app: &AppHandle,
    state: &CoreState,
    preferences: &Preferences,
) -> CommandResult<()> {
    let config = shortcut_config(preferences)?;
    let mut slot = state
        .shortcuts
        .lock()
        .map_err(|_| lock_error("shortcut service"))?;
    if let Some(service) = slot.take() {
        service.stop()?;
    }
    let (service, receiver) = GlobalShortcutService::start(config)?;
    *slot = Some(service);
    let handle = app.clone();
    thread::Builder::new()
        .name("murmur-shortcut-events".into())
        .spawn(move || {
            while let Ok(event) = receiver.recv() {
                let state = handle.state::<CoreState>();
                let _ = match event {
                    ShortcutEvent::BeginDictation => start_dictation_inner(&handle, &state),
                    ShortcutEvent::FinishDictation => stop_dictation_inner(&handle, &state),
                    ShortcutEvent::CancelDictation => cancel_dictation_inner(&handle, &state),
                };
            }
        })
        .map_err(CoreError::Io)?;
    Ok(())
}

#[cfg(windows)]
fn shortcut_config(preferences: &Preferences) -> Result<ShortcutConfig, CoreError> {
    Ok(ShortcutConfig {
        hold_to_talk: parse_shortcut(&preferences.hold_shortcut)?,
        toggle: if preferences.toggle_shortcut.trim().is_empty() {
            None
        } else {
            Some(parse_shortcut(&preferences.toggle_shortcut)?)
        },
    })
}

#[cfg(windows)]
fn parse_shortcut(value: &str) -> Result<ShortcutChord, CoreError> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        VK_CONTROL, VK_LWIN, VK_MENU, VK_SHIFT, VK_SPACE,
    };
    let mut keys = Vec::new();
    for part in value
        .split('+')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let key = match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => VK_CONTROL.0 as u16,
            "win" | "windows" | "super" => VK_LWIN.0 as u16,
            "alt" => VK_MENU.0 as u16,
            "shift" => VK_SHIFT.0 as u16,
            "space" => VK_SPACE.0 as u16,
            single if single.len() == 1 => single.as_bytes()[0].to_ascii_uppercase() as u16,
            _ => {
                return Err(CoreError::InvalidInput(format!(
                    "unsupported shortcut key: {part}"
                )))
            }
        };
        keys.push(key);
    }
    if keys.is_empty() {
        return Err(CoreError::InvalidInput("shortcut cannot be empty".into()));
    }
    Ok(ShortcutChord { keys })
}

#[cfg(windows)]
pub(crate) fn load_shortcut_config(data_dir: &Path) -> Result<ShortcutConfig, CoreError> {
    let preferences = read_preferences(data_dir)?;
    shortcut_config(&preferences)
}

#[cfg(not(windows))]
pub(crate) fn configure_startup(_enabled: bool) -> Result<(), CoreError> {
    Err(CoreError::Unavailable(
        "startup registration requires Windows".into(),
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VocabularyEntry {
    pub id: Uuid,
    pub spoken: String,
    pub replacement: String,
    pub source: String,
    pub uses: u32,
}

impl From<VocabularyRecord> for VocabularyEntry {
    fn from(value: VocabularyRecord) -> Self {
        Self {
            id: value.id,
            spoken: value.heard,
            replacement: value.replacement,
            source: if value.learned { "learned" } else { "manual" }.into(),
            uses: value.observations,
        }
    }
}

#[tauri::command]
pub fn list_vocabulary(state: State<'_, CoreState>) -> CommandResult<Vec<VocabularyEntry>> {
    state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .list_vocabulary()
        .map(|items| items.into_iter().map(Into::into).collect())
        .map_err(Into::into)
}

#[tauri::command]
pub fn add_vocabulary(
    spoken: String,
    replacement: String,
    state: State<'_, CoreState>,
) -> CommandResult<VocabularyEntry> {
    state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .add_vocabulary(&spoken, &replacement)
        .map(Into::into)
        .map_err(Into::into)
}

#[tauri::command]
pub fn update_vocabulary(entry: VocabularyEntry, state: State<'_, CoreState>) -> CommandResult<()> {
    state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .update_vocabulary(entry.id, &entry.spoken, &entry.replacement)
        .map_err(Into::into)
}

#[tauri::command]
pub fn delete_vocabulary(id: Uuid, state: State<'_, CoreState>) -> CommandResult<()> {
    state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .delete_vocabulary(id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn reset_learned_vocabulary(state: State<'_, CoreState>) -> CommandResult<()> {
    state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .reset_learned_vocabulary()
        .map(|_| ())
        .map_err(Into::into)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupCheckView {
    id: String,
    label: String,
    detail: String,
    state: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStatusView {
    id: String,
    label: String,
    configured: bool,
    state: &'static str,
    detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatusView {
    state: &'static str,
    version: Option<String>,
    detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupStatusView {
    checks: Vec<SetupCheckView>,
    providers: Vec<ProviderStatusView>,
    update: UpdateStatusView,
}

impl SetupStatusView {
    pub(crate) fn update_ready(&self) -> bool {
        self.update.state == "ready"
    }
}

fn setup_status(state: &CoreState) -> Result<SetupStatusView, CoreError> {
    setup_status_with_offline(state, None)
}

fn setup_status_with_offline(
    state: &CoreState,
    verified_offline: Option<bool>,
) -> Result<SetupStatusView, CoreError> {
    let configured = secret_statuses(state.secrets.as_ref())?;
    let providers = [("groq", "Groq")]
        .into_iter()
        .map(|(id, label)| {
            let is_configured = configured
                .iter()
                .any(|status| status.name == id && status.configured);
            ProviderStatusView {
                id: id.into(),
                label: label.into(),
                configured: is_configured,
                state: if is_configured { "ready" } else { "missing" },
                detail: if is_configured {
                    "Credential stored in Windows Credential Manager".into()
                } else {
                    "Run murmur-setup secret set and pass the value through stdin".into()
                },
            }
        })
        .collect();
    let offline = verified_offline
        .unwrap_or_else(|| local_whisper_install_is_present(&state.data_dir.join("models")));
    #[cfg(windows)]
    let microphone_ready = crate::adapters::windows::list_microphones()
        .map(|devices| !devices.is_empty())
        .unwrap_or(false);
    #[cfg(not(windows))]
    let microphone_ready = false;
    let platform = cfg!(windows);
    Ok(SetupStatusView {
        checks: vec![
            SetupCheckView {
                id: "platform".into(),
                label: "Windows 11".into(),
                detail: if platform {
                    "Windows native runtime is available".into()
                } else {
                    "Murmur V1 requires Windows 11".into()
                },
                state: if platform { "ready" } else { "error" },
            },
            SetupCheckView {
                id: "microphone".into(),
                label: "Microphone".into(),
                detail: "Murmur follows the current Windows default unless overridden".into(),
                state: if microphone_ready {
                    "ready"
                } else {
                    "unavailable"
                },
            },
            SetupCheckView {
                id: "offline".into(),
                label: "Offline speech".into(),
                detail: if offline {
                    "whisper.cpp and Large V3 Turbo are installed".into()
                } else {
                    "No offline STT. Install whisper-cli.exe and the checksummed model with murmur-setup"
                        .into()
                },
                state: if offline { "ready" } else { "missing" },
            },
        ],
        providers,
        update: UpdateStatusView {
            state: "current",
            version: None,
            detail: "Updates check daily and install on the next restart"
                .into(),
        },
    })
}

#[tauri::command]
pub fn get_setup_status(state: State<'_, CoreState>) -> CommandResult<SetupStatusView> {
    setup_status(&state).map_err(Into::into)
}

#[tauri::command]
pub fn run_setup_check(
    check_id: String,
    state: State<'_, CoreState>,
) -> CommandResult<SetupStatusView> {
    match check_id.as_str() {
        "platform" | "microphone" => setup_status(&state).map_err(Into::into),
        "offline" => {
            let offline = validate_local_whisper_install(&state.data_dir.join("models"))?;
            setup_status_with_offline(&state, Some(offline)).map_err(Into::into)
        }
        _ => Err(CoreError::InvalidInput(format!("unknown setup check: {check_id}")).into()),
    }
}

fn emit_setup_progress(app: &AppHandle, percent: u8, message: &str) {
    let _ = app.emit(
        "murmur://setup-progress",
        serde_json::json!({
            "operation": "offline-model",
            "percent": percent,
            "message": message,
        }),
    );
}

#[tauri::command]
pub fn install_offline_model(
    app: AppHandle,
    state: State<'_, CoreState>,
) -> CommandResult<SetupStatusView> {
    let directory = state.data_dir.join("models");
    if validate_local_whisper_install(&directory)? {
        return setup_status(&state).map_err(Into::into);
    }
    fs::create_dir_all(&directory)?;
    let archive = directory.join("whisper-runtime.zip");
    emit_setup_progress(&app, 0, "Downloading the offline speech runtime");
    download_checked_model(
        LOCAL_WHISPER_RUNTIME_URL,
        &archive,
        LOCAL_WHISPER_RUNTIME_SHA256,
        |received, total| {
            let percent = total
                .filter(|total| *total > 0)
                .map(|total| (received.saturating_mul(20) / total).min(20) as u8)
                .unwrap_or(0);
            emit_setup_progress(&app, percent, "Downloading the offline speech runtime");
        },
    )?;
    install_local_whisper_runtime_archive(&archive, &directory)?;
    let _ = fs::remove_file(&archive);
    emit_setup_progress(
        &app,
        20,
        "Downloading the English and Vietnamese speech model",
    );
    download_checked_model(
        LOCAL_WHISPER_MODEL_URL,
        &directory.join(LOCAL_WHISPER_MODEL_NAME),
        LOCAL_WHISPER_MODEL_SHA256,
        |received, total| {
            let percent = total
                .filter(|total| *total > 0)
                .map(|total| 20 + (received.saturating_mul(80) / total).min(80) as u8)
                .unwrap_or(20);
            emit_setup_progress(
                &app,
                percent,
                "Downloading the English and Vietnamese speech model",
            );
        },
    )?;
    if !validate_local_whisper_install(&directory)? {
        return Err(CoreError::Unavailable(
            "offline speech installation did not pass validation".into(),
        )
        .into());
    }
    emit_setup_progress(&app, 100, "Offline speech is ready");
    setup_status(&state).map_err(Into::into)
}

#[tauri::command]
pub fn validate_provider(
    provider_id: String,
    state: State<'_, CoreState>,
) -> CommandResult<SetupStatusView> {
    let credentials = provider_credentials(state.secrets.as_ref())?;
    SpeechProviders::new(credentials)?
        .validate_provider(&provider_id)
        .map_err(|error| CoreError::Unavailable(error.message))?;
    setup_status(&state).map_err(Into::into)
}

#[tauri::command]
pub fn set_provider_credential(
    provider_id: String,
    secret: String,
    state: State<'_, CoreState>,
) -> CommandResult<SetupStatusView> {
    if !matches!(
        provider_id.as_str(),
        "deepgram" | "assemblyai" | "groq" | "github_updates"
    ) {
        return Err(CoreError::InvalidInput(format!("unknown credential: {provider_id}")).into());
    }
    if secret.trim().is_empty() {
        return Err(CoreError::InvalidInput("credential cannot be empty".into()).into());
    }
    state.secrets.set(&provider_id, secret.trim())?;
    setup_status(&state).map_err(Into::into)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDeviceView {
    id: String,
    label: String,
    is_default: bool,
}

#[tauri::command]
pub fn list_audio_devices() -> CommandResult<Vec<AudioDeviceView>> {
    #[cfg(windows)]
    {
        return crate::adapters::windows::list_microphones()
            .map(|devices| {
                devices
                    .into_iter()
                    .map(|device| AudioDeviceView {
                        id: device.id,
                        label: device.label,
                        is_default: device.is_default,
                    })
                    .collect()
            })
            .map_err(Into::into);
    }
    #[cfg(not(windows))]
    Ok(vec![])
}

#[tauri::command]
pub fn choose_context_application() -> CommandResult<Option<String>> {
    #[cfg(windows)]
    {
        return active_window()
            .map(|window| Some(window.process_name))
            .map_err(Into::into);
    }
    #[cfg(not(windows))]
    Ok(None)
}

#[tauri::command]
pub fn create_backup(state: State<'_, CoreState>) -> CommandResult<Option<String>> {
    if has_active_work(&state)? {
        return Err(CoreError::Unavailable(
            "finish recording and meeting processing before creating a backup".into(),
        )
        .into());
    }
    let Some(path) = rfd::FileDialog::new()
        .set_title("Create Murmur backup")
        .add_filter("ZIP archive", &["zip"])
        .set_file_name("murmur-backup.zip")
        .save_file()
    else {
        return Ok(None);
    };
    crate::backup::create_backup(&state.data_dir, &path, env!("CARGO_PKG_VERSION"))?;
    Ok(Some(path.to_string_lossy().into()))
}

#[tauri::command]
pub fn restore_backup(state: State<'_, CoreState>) -> CommandResult<Option<String>> {
    if has_active_work(&state)? {
        return Err(CoreError::Unavailable(
            "finish recording and meeting processing before restoring a backup".into(),
        )
        .into());
    }
    let Some(path) = rfd::FileDialog::new()
        .set_title("Restore Murmur backup")
        .add_filter("ZIP archive", &["zip"])
        .pick_file()
    else {
        return Ok(None);
    };
    let pending = state.data_dir.with_extension("pending-restore");
    if pending.exists() {
        fs::remove_dir_all(&pending).map_err(CoreError::from)?;
    }
    crate::backup::restore_backup(&path, &pending)?;
    Ok(Some(path.to_string_lossy().into()))
}

#[tauri::command]
pub fn export_diagnostics(state: State<'_, CoreState>) -> CommandResult<Option<String>> {
    let Some(path) = rfd::FileDialog::new()
        .set_title("Export Murmur diagnostics")
        .add_filter("JSON", &["json"])
        .set_file_name("murmur-diagnostics.json")
        .save_file()
    else {
        return Ok(None);
    };
    let report = runtime_diagnostics(&state)?;
    let bytes = serde_json::to_vec_pretty(&report).map_err(CoreError::from)?;
    fs::write(&path, bytes).map_err(CoreError::from)?;
    let log_path = path.with_extension("logs.jsonl");
    state.diagnostics.export(&log_path)?;
    Ok(Some(path.to_string_lossy().into()))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataRemovalPreview {
    paths: Vec<String>,
    bytes: u64,
}

fn tree_size(path: &Path) -> Result<u64, CoreError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(0);
    }
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    let mut total = 0_u64;
    for entry in fs::read_dir(path)? {
        total = total.saturating_add(tree_size(&entry?.path())?);
    }
    Ok(total)
}

#[tauri::command]
pub fn preview_data_removal(state: State<'_, CoreState>) -> CommandResult<DataRemovalPreview> {
    let bytes = if state.data_dir.exists() {
        tree_size(&state.data_dir)?
    } else {
        0
    };
    let mut paths = vec![state.data_dir.to_string_lossy().into_owned()];
    paths.extend(
        ["deepgram", "assemblyai", "groq", "github_updates"]
            .map(|name| format!("Windows Credential Manager: Murmur/{name}")),
    );
    Ok(DataRemovalPreview { paths, bytes })
}

#[tauri::command]
pub fn remove_user_data(confirmation: String, state: State<'_, CoreState>) -> CommandResult<()> {
    if confirmation != "DELETE" {
        return Err(CoreError::InvalidInput("type DELETE to remove Murmur data".into()).into());
    }
    if has_active_work(&state)? {
        return Err(CoreError::Unavailable("stop active work before deleting data".into()).into());
    }
    state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .clear_user_data()?;
    for name in ["media", "models", "updates", "logs", "preferences.json"] {
        let path = state.data_dir.join(name);
        if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else if path.is_file() {
            fs::remove_file(&path)?;
        }
    }
    for name in ["deepgram", "assemblyai", "groq", "github_updates"] {
        state.secrets.delete(name)?;
    }
    Ok(())
}

#[tauri::command]
pub fn check_for_updates(state: State<'_, CoreState>) -> CommandResult<SetupStatusView> {
    check_for_updates_inner(&state).map_err(Into::into)
}

pub(crate) fn check_for_updates_inner(state: &CoreState) -> Result<SetupStatusView, CoreError> {
    let mut status = setup_status(state)?;
    let token = state.secrets.get("github_updates")?.unwrap_or_default();
    let client = GitHubUpdateClient::new(
        token,
        GitHubUpdateConfig::public_repository("hongnhatpham", "murmur-dictation"),
    )?;
    match client.check(env!("CARGO_PKG_VERSION"))? {
        Some(update) => {
            let version = update.version.clone();
            client.download(&update, &state.data_dir.join("updates"), |_, _| {})?;
            status.update = UpdateStatusView {
                state: "ready",
                version: Some(version),
                detail: "Signed update downloaded. Murmur will verify and install it on restart"
                    .into(),
            };
        }
        None => {
            status.update = UpdateStatusView {
                state: "current",
                version: None,
                detail: "Murmur is current".into(),
            };
        }
    }
    Ok(status)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MeetingStateEvent {
    session_id: Option<SessionId>,
    state: &'static str,
    elapsed_ms: Option<u64>,
    message: Option<String>,
}

fn emit_meeting(
    app: &AppHandle,
    session_id: Option<SessionId>,
    state: &'static str,
    elapsed_ms: Option<u64>,
    message: Option<String>,
) {
    let _ = app.emit(
        "murmur://meeting-state",
        MeetingStateEvent {
            session_id,
            state,
            elapsed_ms,
            message,
        },
    );
}

#[tauri::command]
pub fn dismiss_meeting_prompt(
    app_name: String,
    disable_for_app: bool,
    state: State<'_, CoreState>,
) -> CommandResult<()> {
    let app_name = app_name.trim();
    if app_name.is_empty() {
        return Err(CoreError::InvalidInput("meeting application name is empty".into()).into());
    }
    let application = [
        MeetingApplication::Zoom,
        MeetingApplication::Teams,
        MeetingApplication::GoogleMeet,
        MeetingApplication::Slack,
        MeetingApplication::Discord,
    ]
    .into_iter()
    .find(|application| application.display_name().eq_ignore_ascii_case(app_name));
    if let Some(application) = application {
        let mut tracker = state
            .meeting_prompts
            .lock()
            .map_err(|_| lock_error("meeting prompts"))?;
        if disable_for_app {
            tracker.suppress_application(application);
        } else {
            tracker.dismiss_current(application);
        }
    }
    if !disable_for_app {
        return Ok(());
    }
    let mut preferences = read_preferences(&state.data_dir)?;
    if !preferences
        .excluded_meeting_applications
        .iter()
        .any(|value| value.eq_ignore_ascii_case(app_name))
    {
        preferences
            .excluded_meeting_applications
            .push(app_name.to_owned());
        preferences.excluded_meeting_applications.sort();
        write_preferences(&state.data_dir, &preferences)?;
    }
    Ok(())
}

#[tauri::command]
pub fn start_meeting(
    source: String,
    app: AppHandle,
    state: State<'_, CoreState>,
) -> CommandResult<SessionId> {
    #[cfg(windows)]
    {
        let mut active = state
            .active_meeting
            .lock()
            .map_err(|_| lock_error("active meeting"))?;
        if active.is_some() {
            return Err(CoreError::Unavailable("a meeting is already recording".into()).into());
        }
        if state
            .active_dictation
            .lock()
            .map_err(|_| lock_error("active dictation"))?
            .is_some()
        {
            return Err(CoreError::Unavailable(
                "finish dictation before recording a meeting".into(),
            )
            .into());
        }
        let (session_source, include_system_audio, title) = match source.as_str() {
            "online" => (SessionSource::OnlineCall, true, "Online meeting"),
            "in-person" => (SessionSource::InPerson, false, "In-person meeting"),
            _ => {
                return Err(
                    CoreError::InvalidInput(format!("unknown meeting source: {source}")).into(),
                )
            }
        };
        let preferences = read_preferences(&state.data_dir)?;
        let mut session =
            RecordingSession::new(SessionKind::Meeting, session_source, Some(title.into()));
        session.expires_at = session.created_at
            + ChronoDuration::days(i64::from(preferences.meeting_retention_days));
        let output_directory = state.data_dir.join("media").join(session.id.to_string());
        let capture_id = state.audio.start(CaptureRequest {
            microphone_device: preferences.microphone_id,
            include_system_audio,
            output_directory: output_directory.clone(),
        })?;
        session.audio_path = Some(output_directory.to_string_lossy().into());
        if let Err(error) = state
            .repository
            .lock()
            .map_err(|_| lock_error("repository"))?
            .create_session(&session)
        {
            let _ = state.audio.cancel(&capture_id);
            return Err(error.into());
        }
        *active = Some(ActiveRecording {
            session_id: session.id,
            capture_id,
            started: Instant::now(),
            source: session_source,
            streaming_result: None,
            context: None,
            insertion_target: None,
        });
        emit_meeting(&app, Some(session.id), "recording", Some(0), None);
        set_tray(&app, "Murmur - recording meeting");
        spawn_meeting_ticker(app.clone(), session.id);
        let _ = app
            .notification()
            .builder()
            .title("Murmur is recording")
            .body("Use the tray or Murmur window to stop the meeting.")
            .show();
        emit_sessions_changed(&app);
        return Ok(session.id);
    }
    #[cfg(not(windows))]
    Err(CoreError::Unavailable("meeting capture requires Windows 11".into()).into())
}

#[cfg(windows)]
fn spawn_meeting_ticker(app: AppHandle, session_id: SessionId) {
    let _ = thread::Builder::new()
        .name("murmur-meeting-ticker".into())
        .spawn(move || loop {
            thread::sleep(std::time::Duration::from_secs(1));
            let state = app.state::<CoreState>();
            let active = state
                .active_meeting
                .lock()
                .ok()
                .and_then(|recording| recording.clone());
            let Some(recording) = active.filter(|recording| recording.session_id == session_id)
            else {
                break;
            };
            emit_audio_events(&app, session_id, &recording.capture_id);
            emit_meeting(
                &app,
                Some(session_id),
                "recording",
                Some(recording.started.elapsed().as_millis() as u64),
                None,
            );
        });
}

#[tauri::command]
pub fn stop_meeting(app: AppHandle, state: State<'_, CoreState>) -> CommandResult<()> {
    stop_meeting_inner(&app, &state)
}

pub fn stop_meeting_inner(app: &AppHandle, state: &CoreState) -> CommandResult<()> {
    #[cfg(windows)]
    {
        let recording = state
            .active_meeting
            .lock()
            .map_err(|_| lock_error("active meeting"))?
            .take()
            .ok_or_else(|| CoreError::NotFound("active meeting".into()))?;
        let files = state.audio.stop(&recording.capture_id)?;
        state
            .repository
            .lock()
            .map_err(|_| lock_error("repository"))?
            .set_session_state(
                recording.session_id,
                SessionStatus::Processing,
                Some(ProcessingMode::Hosted),
                None,
            )?;
        emit_meeting(
            &app,
            Some(recording.session_id),
            "processing",
            Some(recording.started.elapsed().as_millis() as u64),
            None,
        );
        set_tray(&app, "Murmur - processing meeting");
        let _ = app
            .notification()
            .builder()
            .title("Meeting recording stopped")
            .body("Murmur is creating the transcript and Meeting Brief.")
            .show();
        spawn_meeting_job(
            app.clone(),
            recording.session_id,
            files,
            "murmur-meeting-pipeline",
        )?;
        return Ok(());
    }
    #[cfg(not(windows))]
    Err(CoreError::Unavailable("meeting capture requires Windows 11".into()).into())
}

#[tauri::command]
pub fn import_recording(
    app: AppHandle,
    state: State<'_, CoreState>,
) -> CommandResult<Option<SessionId>> {
    let Some(source) = rfd::FileDialog::new()
        .set_title("Import a meeting recording")
        .add_filter("Audio", &["wav", "mp3", "m4a", "flac", "ogg", "webm"])
        .pick_file()
    else {
        return Ok(None);
    };
    let title = source
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Imported meeting")
        .to_string();
    let mut session = RecordingSession::new(
        SessionKind::Meeting,
        SessionSource::ImportedFile,
        Some(title),
    );
    let preferences = read_preferences(&state.data_dir)?;
    session.expires_at =
        session.created_at + ChronoDuration::days(i64::from(preferences.meeting_retention_days));
    session.status = SessionStatus::Processing;
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("wav");
    let directory = state.data_dir.join("media").join(session.id.to_string());
    fs::create_dir_all(&directory).map_err(CoreError::from)?;
    let destination = directory.join(format!("imported.{extension}"));
    fs::copy(&source, &destination).map_err(CoreError::from)?;
    session.audio_path = Some(destination.to_string_lossy().into());
    state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .create_session(&session)?;
    emit_meeting(&app, Some(session.id), "processing", Some(0), None);
    emit_sessions_changed(&app);
    let session_id = session.id;
    spawn_meeting_job(
        app.clone(),
        session_id,
        vec![destination],
        "murmur-import-pipeline",
    )?;
    Ok(Some(session.id))
}

fn spawn_meeting_job(
    app: AppHandle,
    session_id: SessionId,
    files: Vec<PathBuf>,
    thread_name: &'static str,
) -> Result<(), CoreError> {
    let state = app.state::<CoreState>();
    {
        let mut jobs = state
            .meeting_jobs
            .lock()
            .map_err(|_| CoreError::Unavailable("meeting job lock is poisoned".into()))?;
        if !jobs.insert(session_id) {
            return Err(CoreError::Unavailable(
                "this meeting is already processing".into(),
            ));
        }
    }
    let handle = app.clone();
    if let Err(error) = thread::Builder::new()
        .name(thread_name.into())
        .spawn(move || process_meeting(handle, session_id, files))
    {
        if let Ok(mut jobs) = state.meeting_jobs.lock() {
            jobs.remove(&session_id);
        }
        return Err(CoreError::Io(error));
    }
    Ok(())
}

fn process_meeting(app: AppHandle, session_id: SessionId, files: Vec<PathBuf>) {
    match process_meeting_result(&app, session_id, &files) {
        Ok(()) => {}
        Err(error) => {
            let state = app.state::<CoreState>();
            if let Ok(mut repository) = state.repository.lock() {
                let operation = if repository
                    .raw_transcript_for_session(session_id)
                    .ok()
                    .flatten()
                    .is_some()
                {
                    "brief"
                } else {
                    "transcription"
                };
                let _ = repository.set_session_state(
                    session_id,
                    SessionStatus::PendingEnhancement,
                    Some(ProcessingMode::Queued),
                    None,
                );
                let _ =
                    repository.queue_enhancement(session_id, operation, Some(&error.to_string()));
            }
            emit_meeting(
                &app,
                Some(session_id),
                "pending",
                None,
                Some(error.to_string()),
            );
            set_tray(&app, "Murmur - meeting pending");
            emit_sessions_changed(&app);
        }
    }
    if let Ok(mut jobs) = app.state::<CoreState>().meeting_jobs.lock() {
        jobs.remove(&session_id);
    }
}

fn process_meeting_result(
    app: &AppHandle,
    session_id: SessionId,
    files: &[PathBuf],
) -> Result<(), CoreError> {
    let state = app.state::<CoreState>();
    let session = state
        .repository
        .lock()
        .map_err(|_| CoreError::Unavailable("repository lock is poisoned".into()))?
        .session(session_id)?;
    let existing_raw = {
        let repository = state
            .repository
            .lock()
            .map_err(|_| CoreError::Unavailable("repository lock is poisoned".into()))?;
        repository.raw_transcript_for_session(session_id)?
    };
    if let Some(raw) = existing_raw {
        let providers = SpeechProviders::new(provider_credentials(state.secrets.as_ref())?)?;
        return complete_meeting_brief(app, &session, &raw, &providers);
    }
    let providers = SpeechProviders::new(provider_credentials(state.secrets.as_ref())?)?;
    let local = local_whisper(&state.data_dir);
    let microphone = files
        .iter()
        .filter(|path| !path.to_string_lossy().contains("-system-"))
        .cloned()
        .collect::<Vec<_>>();
    let system = files
        .iter()
        .filter(|path| path.to_string_lossy().contains("-system-"))
        .cloned()
        .collect::<Vec<_>>();
    let groups = if session.source == SessionSource::OnlineCall {
        vec![
            ("microphone", microphone, Some("You")),
            ("system", system, Some("Others")),
        ]
    } else {
        vec![("meeting", files.to_vec(), None)]
    };
    let mut segments = Vec::new();
    let mut provider_names = Vec::new();
    let mut mode = ProcessingMode::Hosted;
    let mut language = None;
    for (label, group, speaker_fallback) in groups {
        if group.is_empty() {
            continue;
        }
        let audio_path = if group.len() == 1 && !is_wav(&group[0]) {
            vec![(group[0].clone(), 0)]
        } else {
            let merged = merge_audio_chunks(
                &group,
                &state
                    .data_dir
                    .join("media")
                    .join(session_id.to_string())
                    .join(format!("{label}.wav")),
            )?;
            normalize_meeting_audio_chunks(
                &merged,
                &state.data_dir.join("media").join(session_id.to_string()),
                label,
            )?
            .into_iter()
            .map(|chunk| (chunk.path, chunk.offset_ms))
            .collect()
        };
        for (audio_path, offset_ms) in audio_path {
            let outcome = providers.transcribe_with_fallback_observed(
                SessionKind::Meeting,
                &AudioRequest::from_file(&audio_path, mime_type(&audio_path))?,
                local.as_ref(),
                |attempt| {
                    emit_meeting(
                        app,
                        Some(session_id),
                        "processing",
                        None,
                        Some(format!(
                            "{} via {}",
                            mode_label(attempt.mode),
                            attempt.provider
                        )),
                    );
                },
            );
            let (transcription, failures) = match outcome {
                TranscriptionOutcome::Completed {
                    transcript,
                    attempts,
                } => (transcript, attempts.len()),
                TranscriptionOutcome::PendingEnhancement { attempts } => {
                    return Err(CoreError::Unavailable(
                        attempts
                            .last()
                            .map(|failure| failure.message.clone())
                            .unwrap_or_else(|| "no speech provider is configured".into()),
                    ))
                }
                TranscriptionOutcome::Failed { .. } => unreachable!(),
            };
            let group_mode = processing_mode(&transcription.provider, failures);
            if group_mode == ProcessingMode::Local {
                mode = ProcessingMode::Local;
            } else if group_mode == ProcessingMode::HostedFallback && mode != ProcessingMode::Local
            {
                mode = ProcessingMode::HostedFallback;
            }
            language = language.or(transcription.language.clone());
            provider_names.push(transcription.provider.clone());
            let mut raw_group = to_raw_transcript(session_id, transcription).segments;
            for segment in &mut raw_group {
                segment.start_ms = segment.start_ms.saturating_add(offset_ms);
                segment.end_ms = segment.end_ms.saturating_add(offset_ms);
                if segment.speaker.is_none() {
                    segment.speaker = speaker_fallback.map(str::to_owned);
                }
            }
            segments.extend(raw_group);
        }
    }
    segments.sort_by_key(|segment| (segment.start_ms, segment.end_ms));
    for (ordinal, segment) in segments.iter_mut().enumerate() {
        segment.ordinal = ordinal as u32;
    }
    let raw = RawTranscript {
        id: Uuid::new_v4(),
        session_id,
        language: language.unwrap_or_else(|| "en-vi".into()),
        provider: provider_names.join(" + "),
        created_at: Utc::now(),
        segments,
    };
    {
        let mut repository = state
            .repository
            .lock()
            .map_err(|_| CoreError::Unavailable("repository lock is poisoned".into()))?;
        repository.insert_raw_transcript(&raw)?;
        repository.set_session_state(
            session_id,
            SessionStatus::Processing,
            Some(mode),
            Some(&raw.provider),
        )?;
    }
    complete_meeting_brief(app, &session, &raw, &providers)
}

fn complete_meeting_brief(
    app: &AppHandle,
    session: &RecordingSession,
    raw: &RawTranscript,
    providers: &SpeechProviders,
) -> Result<(), CoreError> {
    let state = app.state::<CoreState>();
    let generated = providers
        .create_meeting_brief(&MeetingBriefRequest {
            title: session.title.clone(),
            segments: raw
                .segments
                .iter()
                .map(|segment| crate::providers::TranscriptSegment {
                    start_ms: segment.start_ms,
                    end_ms: segment.end_ms,
                    text: segment.text.clone(),
                    speaker: segment.speaker.clone(),
                    confidence: None,
                })
                .collect(),
        })
        .map_err(|error| CoreError::Unavailable(error.message))?;
    let brief = parse_generated_brief(session.id, raw, &generated.markdown);
    {
        let mut repository = state
            .repository
            .lock()
            .map_err(|_| CoreError::Unavailable("repository lock is poisoned".into()))?;
        repository.insert_meeting_brief(&brief)?;
        repository.set_session_state(
            session.id,
            SessionStatus::Completed,
            session.processing_mode.or(Some(ProcessingMode::Hosted)),
            Some(&raw.provider),
        )?;
        repository.rebuild_meeting_search(session.id)?;
        repository.clear_pending_enhancements(session.id)?;
    }
    emit_meeting(app, Some(session.id), "ready", None, None);
    set_tray(app, "Murmur");
    emit_sessions_changed(app);
    Ok(())
}

fn is_wav(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("wav"))
}

fn mime_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "webm" => "audio/webm",
        _ => "audio/wav",
    }
}

fn parse_generated_brief(
    session_id: SessionId,
    transcript: &RawTranscript,
    markdown: &str,
) -> MeetingBrief {
    let mut section = "overview";
    let mut overview = Vec::new();
    let mut topics = Vec::new();
    let mut items = Vec::new();
    for line in markdown
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if line.starts_with('#') {
            let heading = line.trim_start_matches('#').trim().to_ascii_lowercase();
            section = if heading.contains("detailed topic") {
                "topics"
            } else if heading.contains("decision") {
                "decision"
            } else if heading.contains("action") {
                "action"
            } else if heading.contains("open question") {
                "question"
            } else if heading.contains("possible follow") {
                "follow"
            } else if heading.contains("notable") {
                "notable"
            } else {
                "overview"
            };
            continue;
        }
        let text = line.trim_start_matches(['-', '*']).trim();
        if is_empty_brief_entry(text) {
            continue;
        }
        match section {
            "overview" => overview.push(text.to_string()),
            "topics" => topics.push(text.to_string()),
            kind => {
                let item_kind = match kind {
                    "decision" => BriefItemKind::Decision,
                    "action" => BriefItemKind::ActionItem,
                    "question" => BriefItemKind::OpenQuestion,
                    "follow" => BriefItemKind::PossibleFollowUp,
                    _ => BriefItemKind::NotableMoment,
                };
                let citations = timestamp_from_text(text)
                    .and_then(|millis| {
                        transcript
                            .segments
                            .iter()
                            .find(|segment| millis >= segment.start_ms && millis <= segment.end_ms)
                            .or_else(|| {
                                transcript
                                    .segments
                                    .iter()
                                    .rev()
                                    .find(|segment| segment.start_ms <= millis)
                            })
                            .map(|segment| TranscriptCitation {
                                segment_id: segment.id,
                                start_ms: segment.start_ms,
                                end_ms: segment.end_ms,
                            })
                    })
                    .into_iter()
                    .collect::<Vec<_>>();
                if matches!(
                    item_kind,
                    BriefItemKind::Decision | BriefItemKind::ActionItem
                ) && citations.is_empty()
                {
                    continue;
                }
                items.push(BriefItem {
                    id: Uuid::new_v4(),
                    kind: item_kind,
                    text: text.to_string(),
                    citations,
                });
            }
        }
    }
    MeetingBrief {
        id: Uuid::new_v4(),
        session_id,
        transcript_id: transcript.id,
        overview: overview.join("\n"),
        detailed_topics: topics,
        items,
        created_at: Utc::now(),
    }
}

fn timestamp_from_text(text: &str) -> Option<u64> {
    let start = text.find('[')? + 1;
    let end = text[start..].find(']')? + start;
    let parts = text[start..end]
        .split(':')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let seconds = match parts.as_slice() {
        [hours, minutes, seconds] => hours * 3600 + minutes * 60 + seconds,
        [minutes, seconds] => minutes * 60 + seconds,
        _ => return None,
    };
    Some(seconds * 1000)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingDetailView {
    id: SessionId,
    title: String,
    date: String,
    duration: String,
    duration_seconds: u64,
    speakers: usize,
    expires_at: String,
    pinned: bool,
    status: &'static str,
    pending_reason: Option<String>,
    audio_paths: Vec<String>,
    overview: String,
    topics: Vec<BriefItemView>,
    decisions: Vec<BriefItemView>,
    actions: Vec<BriefItemView>,
    questions: Vec<BriefItemView>,
    notable_moments: Vec<BriefItemView>,
    follow_ups: Vec<BriefItemView>,
    turns: Vec<TranscriptTurnView>,
}

fn meeting_audio_paths(path: Option<&str>) -> Vec<String> {
    let Some(path) = path.map(PathBuf::from) else {
        return vec![];
    };
    if path.is_file() {
        return vec![path.to_string_lossy().into_owned()];
    }
    let preferred = ["meeting.wav", "microphone.wav", "system.wav"]
        .into_iter()
        .map(|name| path.join(name))
        .filter(|candidate| candidate.is_file())
        .collect::<Vec<_>>();
    let mut files = if preferred.is_empty() {
        fs::read_dir(&path)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|candidate| candidate.is_file() && is_wav(candidate))
            .collect::<Vec<_>>()
    } else {
        preferred
    };
    files.sort();
    files
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BriefItemView {
    text: String,
    citation_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptTurnView {
    id: Uuid,
    speaker: String,
    timestamp_seconds: u64,
    end_seconds: u64,
    text: String,
    raw_text: String,
    edited: bool,
}

#[tauri::command]
pub fn get_meeting_detail(
    session_id: SessionId,
    state: State<'_, CoreState>,
) -> CommandResult<MeetingDetailView> {
    let repository = state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?;
    let session = repository.session(session_id)?;
    if session.kind != SessionKind::Meeting {
        return Err(CoreError::InvalidInput("session is not a meeting".into()).into());
    }
    let raw = repository.raw_transcript_for_session(session_id)?;
    let edited = raw
        .as_ref()
        .map(|raw| repository.latest_edited_transcript(raw.id))
        .transpose()?
        .flatten();
    let brief = repository.meeting_brief(session_id)?;
    let audio_paths = meeting_audio_paths(session.audio_path.as_deref());
    let turns = raw
        .as_ref()
        .map(|raw| {
            raw.segments
                .iter()
                .map(|segment| {
                    let current = edited.as_ref().and_then(|edited| {
                        edited
                            .segments
                            .iter()
                            .find(|item| item.raw_segment_id == segment.id)
                    });
                    TranscriptTurnView {
                        id: segment.id,
                        speaker: current
                            .and_then(|item| item.speaker.clone())
                            .or_else(|| segment.speaker.clone())
                            .unwrap_or_else(|| "Speaker".into()),
                        timestamp_seconds: segment.start_ms / 1000,
                        end_seconds: segment.end_ms / 1000,
                        text: current
                            .map(|item| item.text.clone())
                            .unwrap_or_else(|| segment.text.clone()),
                        raw_text: segment.text.clone(),
                        edited: current.is_some(),
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let speakers = turns
        .iter()
        .map(|turn| turn.speaker.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len();
    let duration_seconds = turns.last().map(|turn| turn.end_seconds).unwrap_or(0);
    let item_views = |kind: BriefItemKind| {
        brief
            .as_ref()
            .map(|brief| {
                brief
                    .items
                    .iter()
                    .filter(|item| item.kind == kind)
                    .map(|item| BriefItemView {
                        text: item.text.clone(),
                        citation_seconds: item
                            .citations
                            .first()
                            .map(|citation| citation.start_ms / 1000),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    Ok(MeetingDetailView {
        id: session.id,
        title: session.title.unwrap_or_else(|| "Untitled meeting".into()),
        date: session.created_at.format("%B %-d, %Y").to_string(),
        duration: format!("{}:{:02}", duration_seconds / 60, duration_seconds % 60),
        duration_seconds,
        speakers,
        expires_at: session.expires_at.to_rfc3339(),
        pinned: session.pinned,
        status: match session.status {
            SessionStatus::Capturing => "recording",
            SessionStatus::Processing => "processing",
            SessionStatus::PendingEnhancement => "pending",
            SessionStatus::Completed => "ready",
            _ => "failed",
        },
        pending_reason: (session.status == SessionStatus::PendingEnhancement)
            .then(|| "Waiting for a configured speech or brief provider".into()),
        audio_paths,
        overview: brief
            .as_ref()
            .map(|brief| brief.overview.clone())
            .unwrap_or_default(),
        topics: brief
            .as_ref()
            .map(|brief| {
                brief
                    .detailed_topics
                    .iter()
                    .map(|text| BriefItemView {
                        text: text.clone(),
                        citation_seconds: None,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        decisions: item_views(BriefItemKind::Decision),
        actions: item_views(BriefItemKind::ActionItem),
        questions: item_views(BriefItemKind::OpenQuestion),
        notable_moments: item_views(BriefItemKind::NotableMoment),
        follow_ups: item_views(BriefItemKind::PossibleFollowUp),
        turns,
    })
}

fn next_edited(
    repository: &Repository,
    session_id: SessionId,
) -> Result<(RawTranscript, EditedTranscript), CoreError> {
    let raw = repository
        .raw_transcript_for_session(session_id)?
        .ok_or_else(|| CoreError::NotFound("meeting transcript".into()))?;
    let previous = repository.latest_edited_transcript(raw.id)?;
    let revision = previous
        .as_ref()
        .map(|value| value.revision + 1)
        .unwrap_or(1);
    let segments = raw
        .segments
        .iter()
        .map(|segment| {
            previous
                .as_ref()
                .and_then(|edited| {
                    edited
                        .segments
                        .iter()
                        .find(|item| item.raw_segment_id == segment.id)
                })
                .cloned()
                .unwrap_or_else(|| EditedSegment {
                    raw_segment_id: segment.id,
                    text: segment.text.clone(),
                    speaker: segment.speaker.clone(),
                })
        })
        .collect();
    Ok((
        raw.clone(),
        EditedTranscript {
            id: Uuid::new_v4(),
            raw_transcript_id: raw.id,
            revision,
            created_at: Utc::now(),
            segments,
        },
    ))
}

#[tauri::command]
pub fn save_transcript_turn(
    session_id: SessionId,
    turn_id: Uuid,
    text: String,
    state: State<'_, CoreState>,
) -> CommandResult<()> {
    if text.trim().is_empty() {
        return Err(CoreError::InvalidInput("transcript turn cannot be empty".into()).into());
    }
    let mut repository = state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?;
    let (_, mut edited) = next_edited(&repository, session_id)?;
    let turn = edited
        .segments
        .iter_mut()
        .find(|turn| turn.raw_segment_id == turn_id)
        .ok_or_else(|| CoreError::NotFound(format!("transcript turn {turn_id}")))?;
    turn.text = text.trim().into();
    repository.insert_edited_transcript(&edited)?;
    repository.rebuild_meeting_search(session_id)?;
    Ok(())
}

#[tauri::command]
pub fn rename_speaker(
    session_id: SessionId,
    from: String,
    to: String,
    state: State<'_, CoreState>,
) -> CommandResult<()> {
    if to.trim().is_empty() {
        return Err(CoreError::InvalidInput("speaker label cannot be empty".into()).into());
    }
    let mut repository = state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?;
    let (_, mut edited) = next_edited(&repository, session_id)?;
    let mut changed = false;
    for turn in &mut edited.segments {
        if turn.speaker.as_deref().unwrap_or("Speaker") == from {
            turn.speaker = Some(to.trim().into());
            changed = true;
        }
    }
    if !changed {
        return Err(CoreError::NotFound(format!("speaker {from}")).into());
    }
    repository.insert_edited_transcript(&edited)?;
    repository.rebuild_meeting_search(session_id)?;
    Ok(())
}

#[tauri::command]
pub fn resume_enhancement(
    session_id: SessionId,
    app: AppHandle,
    state: State<'_, CoreState>,
) -> CommandResult<()> {
    resume_pending_enhancement(&app, session_id, &state)
}

pub(crate) fn resume_pending_enhancement(
    app: &AppHandle,
    session_id: SessionId,
    state: &CoreState,
) -> CommandResult<()> {
    let session = state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .session(session_id)?;
    let path = session
        .audio_path
        .map(PathBuf::from)
        .ok_or_else(|| CoreError::NotFound("meeting audio".into()))?;
    let files = if path.is_dir() {
        meeting_source_files(&path)?
    } else {
        vec![path]
    };
    emit_meeting(app, Some(session_id), "processing", None, None);
    spawn_meeting_job(app.clone(), session_id, files, "murmur-resume-meeting")?;
    Ok(())
}

/// Returns only authoritative capture chunks or an imported source. Generated merge outputs such
/// as `microphone.wav` and `system.wav` must never be fed back into a retry.
fn meeting_source_files(directory: &Path) -> Result<Vec<PathBuf>, CoreError> {
    let mut files = fs::read_dir(directory)
        .map_err(CoreError::from)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            if !path.is_file() {
                return false;
            }
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            name.starts_with("imported.")
                || name.contains("-microphone-")
                || name.contains("-system-")
        })
        .collect::<Vec<_>>();
    files.sort();
    if files.is_empty() {
        return Err(CoreError::NotFound("meeting source audio".into()));
    }
    Ok(files)
}

#[tauri::command]
pub fn export_meeting(
    session_id: SessionId,
    state: State<'_, CoreState>,
) -> CommandResult<Option<String>> {
    let detail = get_meeting_detail(session_id, state)?;
    let Some(path) = rfd::FileDialog::new()
        .set_title("Export Meeting Brief")
        .add_filter("Markdown", &["md"])
        .set_file_name(format!("{}.md", safe_file_name(&detail.title)))
        .save_file()
    else {
        return Ok(None);
    };
    let mut markdown = format!("# {}\n\n{}\n\n", detail.title, detail.overview);
    append_brief_markdown(&mut markdown, "Decisions", &detail.decisions);
    append_brief_markdown(&mut markdown, "Action items", &detail.actions);
    append_brief_markdown(&mut markdown, "Open questions", &detail.questions);
    append_brief_markdown(&mut markdown, "Possible follow-ups", &detail.follow_ups);
    markdown.push_str("## Transcript\n\n");
    for turn in detail.turns {
        markdown.push_str(&format!(
            "- [{}:{:02}] **{}:** {}\n",
            turn.timestamp_seconds / 60,
            turn.timestamp_seconds % 60,
            turn.speaker,
            turn.text
        ));
    }
    fs::write(&path, markdown).map_err(CoreError::from)?;
    Ok(Some(path.to_string_lossy().into()))
}

fn append_brief_markdown(markdown: &mut String, title: &str, items: &[BriefItemView]) {
    if items.is_empty() {
        return;
    }
    markdown.push_str(&format!("## {title}\n\n"));
    for item in items {
        let citation = item
            .citation_seconds
            .map(|seconds| format!(" [{}:{:02}]", seconds / 60, seconds % 60))
            .unwrap_or_default();
        markdown.push_str(&format!("- {}{}\n", item.text, citation));
    }
    markdown.push('\n');
}

fn safe_file_name(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            ) {
                '_'
            } else {
                character
            }
        })
        .collect()
}

#[tauri::command]
pub fn seek_meeting_audio(
    session_id: SessionId,
    _seconds: u64,
    state: State<'_, CoreState>,
) -> CommandResult<()> {
    let session = state
        .repository
        .lock()
        .map_err(|_| lock_error("repository"))?
        .session(session_id)?;
    let path = session
        .audio_path
        .ok_or_else(|| CoreError::NotFound("meeting audio".into()))?;
    std::process::Command::new("explorer.exe")
        .arg(path)
        .spawn()
        .map_err(CoreError::Io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{cell::RefCell, rc::Rc};

    #[test]
    fn preferences_without_cleanup_level_default_to_light() {
        let preferences: Preferences = serde_json::from_str(
            r#"{"contextCapture":true,"launchAtStartup":true,"meetingSuggestions":true,"offlineModel":true,"dictationRetentionDays":7,"meetingRetentionDays":30}"#,
        )
        .unwrap();
        assert_eq!(preferences.cleanup_level, CleanupLevel::Light);

        let medium: Preferences =
            serde_json::from_value(serde_json::json!({"contextCapture":true,"launchAtStartup":true,"meetingSuggestions":true,"offlineModel":true,"dictationRetentionDays":7,"meetingRetentionDays":30,"cleanupLevel":"medium"})).unwrap();
        assert_eq!(medium.cleanup_level, CleanupLevel::Medium);
    }

    #[test]
    fn showing_dictation_overlay_raises_visible_window_without_activation() {
        let actions = Rc::new(RefCell::new(Vec::new()));
        show_dictation_overlay(
            {
                let actions = Rc::clone(&actions);
                move || {
                    actions.borrow_mut().push("show");
                    Ok(())
                }
            },
            {
                let actions = Rc::clone(&actions);
                move || {
                    actions.borrow_mut().push("raise without activation");
                    Ok(())
                }
            },
        );
        assert_eq!(
            actions.borrow().as_slice(),
            ["show", "raise without activation"]
        );
    }

    #[cfg(windows)]
    use crate::adapters::windows::{
        audio::{AudioChannel, AudioPacket, SampleEncoding, WasapiFormat},
        context::ContextExclusion,
    };

    #[test]
    fn spoken_formatting_preserves_literal_phrases() {
        assert_eq!(
            apply_spoken_formatting("hello new line bullet ship literal bullet"),
            "hello\n- ship bullet"
        );
    }

    #[cfg(windows)]
    #[test]
    fn excluded_context_is_never_available_to_correction() {
        let snapshot = ContextSnapshot {
            process_name: "PasswordManager.exe".into(),
            window_title: "Private vault".into(),
            selected_text: Some("secret".into()),
            surrounding_text: Some("private text".into()),
            excluded_reason: Some(ContextExclusion::PasswordManager),
        };
        assert!(usable_correction_context(Some(&snapshot)).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn hosted_stream_rejects_missing_or_dropped_audio_packets() {
        let packet = |sequence, dropped_before| AudioPacket {
            sequence,
            dropped_before,
            channel: AudioChannel::Microphone,
            captured_millis: 0,
            frames: 1,
            flags: 0,
            device_position: 0,
            qpc_position: 0,
            format: WasapiFormat {
                raw: vec![],
                format_tag: 1,
                channels: 1,
                sample_rate: 16_000,
                average_bytes_per_second: 32_000,
                block_align: 2,
                bits_per_sample: 16,
                encoding: SampleEncoding::Pcm,
            },
            bytes: vec![0, 0],
        };
        assert!(validate_stream_packet(&packet(0, 0), 0).is_ok());
        assert!(validate_stream_packet(&packet(2, 0), 1).is_err());
        assert!(validate_stream_packet(&packet(1, 1), 1).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn dictation_audio_level_event_has_stable_camel_case_shape() {
        let session_id = Uuid::nil();
        assert_eq!(
            serde_json::to_value(DictationAudioLevelEvent {
                session_id,
                rms: 0.25,
                peak: 0.5,
            })
            .unwrap(),
            serde_json::json!({
                "sessionId": session_id,
                "rms": 0.25,
                "peak": 0.5,
            })
        );
    }

    #[cfg(windows)]
    #[test]
    fn hosted_stream_accepts_final_transcript_after_network_round_trip() {
        let (sender, receiver) = mpsc::sync_channel(1);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(500));
            sender
                .send(Ok(TranscriptionResult {
                    text: "ready".into(),
                    language: Some("en".into()),
                    provider: "deepgram_flux_streaming".into(),
                    segments: vec![],
                    diarization: false,
                }))
                .unwrap();
        });
        let receiver = Arc::new(Mutex::new(receiver));

        let transcript = receive_streamed_transcript(&receiver)
            .expect("stream finalization should survive a normal network round trip");

        assert_eq!(transcript.text, "ready");
    }

    #[cfg(windows)]
    #[test]
    fn merge_audio_chunks_preserves_extensible_wave_header() {
        fn wave(payload: &[u8]) -> Vec<u8> {
            let mut bytes = b"RIFF".to_vec();
            bytes.extend_from_slice(&0_u32.to_le_bytes());
            bytes.extend_from_slice(b"WAVEfmt ");
            bytes.extend_from_slice(&40_u32.to_le_bytes());
            bytes.extend_from_slice(&[0_u8; 40]);
            bytes.extend_from_slice(b"data");
            bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            bytes.extend_from_slice(payload);
            let riff_size = (bytes.len() as u32) - 8;
            bytes[4..8].copy_from_slice(&riff_size.to_le_bytes());
            bytes
        }

        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.wav");
        let second = directory.path().join("second.wav");
        let merged = directory.path().join("merged.wav");
        fs::write(&first, wave(&[1, 2])).unwrap();
        fs::write(&second, wave(&[3, 4])).unwrap();

        merge_audio_chunks(&[first, second], &merged).unwrap();
        let bytes = fs::read(merged).unwrap();
        let layout = wave_data_layout(&bytes).unwrap();
        assert_eq!(layout.data_offset, 68);
        assert_eq!(&bytes[layout.data_offset..], &[1, 2, 3, 4]);
        assert_eq!(u32::from_le_bytes(bytes[64..68].try_into().unwrap()), 4);
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 64);
    }

    #[test]
    fn local_whisper_hot_path_does_not_rehash_the_installed_model() {
        let directory = tempfile::tempdir().unwrap();
        let models = directory.path().join("models");
        fs::create_dir_all(&models).unwrap();
        fs::write(models.join("whisper-cli.exe"), b"installed runtime").unwrap();
        fs::write(models.join(LOCAL_WHISPER_MODEL_NAME), b"installed model").unwrap();

        let config = local_whisper(directory.path()).expect("installed files are available");
        assert_eq!(config.executable, models.join("whisper-cli.exe"));
        assert_eq!(config.model, models.join(LOCAL_WHISPER_MODEL_NAME));
    }

    #[test]
    fn meeting_retry_ignores_generated_merge_outputs() {
        let directory = tempfile::tempdir().unwrap();
        let microphone_chunk = directory.path().join("capture-microphone-00000.wav");
        let system_chunk = directory.path().join("capture-system-00000.wav");
        fs::write(&microphone_chunk, b"source").unwrap();
        fs::write(&system_chunk, b"source").unwrap();
        fs::write(directory.path().join("microphone.wav"), b"generated").unwrap();
        fs::write(directory.path().join("system.wav"), b"generated").unwrap();

        let files = meeting_source_files(directory.path()).unwrap();

        assert_eq!(files, vec![microphone_chunk, system_chunk]);
    }
}
