pub mod adapters;
pub mod backup;
pub mod commands;
pub mod db;
pub mod diagnostics;
pub mod domain;
pub mod error;
pub mod providers;
pub mod retention;
pub mod routing;
pub mod secrets;
pub mod updates;
pub mod workflow;

use std::{
    collections::HashMap,
    error::Error,
    sync::{Arc, Mutex},
};

use commands::CoreState;
use db::Repository;
use diagnostics::DiagnosticLogger;
use retention::RetentionService;
use secrets::WindowsCredentialStore;
use tauri::{Emitter, Manager};
#[cfg(windows)]
use tauri_plugin_notification::NotificationExt;

#[cfg(windows)]
use adapters::windows::{
    meeting::{detect_meeting_activity, MeetingPromptEvent, MeetingPromptTracker},
    shortcut::{GlobalShortcutService, ShortcutEvent},
    WindowsAudioCapture, WindowsTextInsertion,
};

pub fn run() -> Result<(), Box<dyn Error>> {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            #[cfg(windows)]
            if launch_ready_update(&data_dir)? {
                app.handle().exit(0);
                return Ok(());
            }
            apply_pending_restore(&data_dir)?;
            let media_dir = data_dir.join("media");
            std::fs::create_dir_all(&media_dir)?;
            let preferences = commands::read_preferences(&data_dir)?;
            commands::configure_startup(preferences.launch_at_startup)?;
            let mut repository = Repository::open(&data_dir.join("murmur.sqlite3"))?;
            let retention = RetentionService::new(media_dir.clone());
            let _ = retention.purge(&mut repository, chrono::Utc::now());
            recover_interrupted_work(&mut repository)?;
            #[cfg(windows)]
            let audio = Arc::new(WindowsAudioCapture::new());
            app.manage(CoreState {
                repository: Mutex::new(repository),
                routes: Mutex::new(HashMap::new()),
                retention,
                secrets: Arc::new(WindowsCredentialStore),
                diagnostics: Arc::new(DiagnosticLogger::new(data_dir.join("logs"))),
                data_dir,
                #[cfg(windows)]
                audio,
                #[cfg(windows)]
                insertion: Arc::new(WindowsTextInsertion::new()),
                #[cfg(windows)]
                shortcuts: Mutex::new(None),
                active_dictation: Mutex::new(None),
                active_meeting: Mutex::new(None),
                meeting_jobs: Mutex::new(Default::default()),
                #[cfg(windows)]
                meeting_prompts: Mutex::new(MeetingPromptTracker::default()),
            });
            #[cfg(windows)]
            {
                install_shortcuts(app)?;
                install_meeting_detection(app)?;
                install_update_checks(app)?;
                install_connectivity_monitor(app)?;
            }
            let show =
                tauri::menu::MenuItem::with_id(app, "show", "Show Murmur", true, None::<&str>)?;
            let stop_meeting = tauri::menu::MenuItem::with_id(
                app,
                "stop-meeting",
                "Stop meeting",
                true,
                None::<&str>,
            )?;
            let quit = tauri::menu::MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = tauri::menu::Menu::with_items(app, &[&show, &stop_meeting, &quit])?;
            let handle = app.handle().clone();
            tauri::tray::TrayIconBuilder::with_id("murmur")
                .tooltip("Murmur")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "stop-meeting" => {
                        let state = app.state::<CoreState>();
                        let _ = commands::stop_meeting_inner(app, &state);
                    }
                    "quit" => {
                        let state = app.state::<CoreState>();
                        if has_active_work(&state) {
                            #[cfg(windows)]
                            let _ = app
                                .notification()
                                .builder()
                                .title("Murmur is still working")
                                .body("Stop the recording or wait for meeting processing before quitting.")
                                .show();
                        } else {
                            app.exit(0);
                        }
                    }
                    _ => {}
                })
                .on_tray_icon_event(move |_tray, event| {
                    if matches!(event, tauri::tray::TrayIconEvent::Click { .. }) {
                        if let Some(window) = handle.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::start_dictation,
            commands::stop_dictation,
            commands::cancel_dictation,
            commands::delete_session,
            commands::save_preferences,
            commands::get_preferences,
            commands::get_setup_status,
            commands::run_setup_check,
            commands::install_offline_model,
            commands::validate_provider,
            commands::set_provider_credential,
            commands::choose_context_application,
            commands::list_audio_devices,
            commands::create_backup,
            commands::restore_backup,
            commands::export_diagnostics,
            commands::preview_data_removal,
            commands::remove_user_data,
            commands::check_for_updates,
            commands::list_vocabulary,
            commands::add_vocabulary,
            commands::update_vocabulary,
            commands::delete_vocabulary,
            commands::reset_learned_vocabulary,
            commands::start_meeting,
            commands::stop_meeting,
            commands::import_recording,
            commands::resume_enhancement,
            commands::dismiss_meeting_prompt,
            commands::get_meeting_detail,
            commands::save_transcript_turn,
            commands::rename_speaker,
            commands::export_meeting,
            commands::seek_meeting_audio,
            commands::create_session,
            commands::list_sessions,
            commands::route_state,
            commands::report_provider_failure,
            commands::complete_transcription,
            commands::get_raw_transcript,
            commands::get_dictation_detail,
            commands::transform_dictation,
            commands::save_edited_transcript,
            commands::save_meeting_brief,
            commands::get_meeting_brief,
            commands::search_meetings,
            commands::search_sessions,
            commands::set_meeting_pinned,
            commands::purge_expired,
            commands::diagnostic_report,
            commands::setup_plan,
            commands::build_backup_manifest,
            commands::observe_attributed_correction,
        ])
        .run(tauri::generate_context!())?;
    Ok(())
}

fn has_active_work(state: &CoreState) -> bool {
    state
        .active_dictation
        .lock()
        .map(|active| active.is_some())
        .unwrap_or(true)
        || state
            .active_meeting
            .lock()
            .map(|active| active.is_some())
            .unwrap_or(true)
        || state
            .meeting_jobs
            .lock()
            .map(|jobs| !jobs.is_empty())
            .unwrap_or(true)
}

#[cfg(windows)]
fn launch_ready_update(data_dir: &std::path::Path) -> Result<bool, Box<dyn Error>> {
    let Some(public_key) = option_env!("MURMUR_UPDATER_PUBLIC_KEY") else {
        return Ok(false);
    };
    let update_directory = data_dir.join("updates");
    let decision = updates::plan_restart_install(&update_directory, public_key, false, false)?;
    let updates::RestartInstallDecision::Ready { plan } = decision else {
        return Ok(false);
    };
    let launch_guard = updates::prepare_ready_update_launch(&update_directory, &plan)?;
    let installer = if plan
        .artifact_path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("exe"))
    {
        plan.artifact_path
    } else {
        extract_verified_installer(&plan.artifact_path, &update_directory)?
    };
    std::process::Command::new(installer)
        .args(["/S", "/UPDATE"])
        .spawn()?;
    launch_guard.commit()?;
    Ok(true)
}

#[cfg(windows)]
fn extract_verified_installer(
    archive_path: &std::path::Path,
    update_directory: &std::path::Path,
) -> Result<std::path::PathBuf, Box<dyn Error>> {
    let file = std::fs::File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let executable_indexes = (0..archive.len())
        .filter(|index| {
            archive
                .by_index(*index)
                .ok()
                .and_then(|entry| entry.enclosed_name())
                .is_some_and(|path| {
                    path.components().count() == 1
                        && path
                            .extension()
                            .and_then(|value| value.to_str())
                            .is_some_and(|value| value.eq_ignore_ascii_case("exe"))
                })
        })
        .collect::<Vec<_>>();
    if executable_indexes.len() != 1 {
        return Err("verified update archive must contain exactly one top-level installer".into());
    }
    let mut entry = archive.by_index(executable_indexes[0])?;
    if entry.size() == 0 || entry.size() > 512 * 1024 * 1024 {
        return Err("verified update installer has an invalid size".into());
    }
    let name = entry
        .enclosed_name()
        .and_then(|path| path.file_name().map(std::ffi::OsStr::to_os_string))
        .ok_or("verified update installer name is invalid")?;
    let destination = update_directory.join(name);
    let partial = destination.with_extension("exe.part");
    if partial.is_file() {
        std::fs::remove_file(&partial)?;
    }
    let mut output = std::fs::File::create(&partial)?;
    std::io::copy(&mut entry, &mut output)?;
    output.sync_all()?;
    if destination.is_file() {
        std::fs::remove_file(&destination)?;
    }
    std::fs::rename(partial, &destination)?;
    Ok(destination)
}

fn recover_interrupted_work(repository: &mut Repository) -> Result<(), error::CoreError> {
    let interrupted = repository
        .list_sessions(None)?
        .into_iter()
        .filter(|session| {
            matches!(
                session.status,
                domain::SessionStatus::Capturing | domain::SessionStatus::Processing
            )
        })
        .collect::<Vec<_>>();
    for session in interrupted {
        match session.kind {
            domain::SessionKind::Dictation => {
                repository.set_session_state(
                    session.id,
                    domain::SessionStatus::Failed,
                    None,
                    session.provider.as_deref(),
                )?;
                repository.set_session_expiry(
                    session.id,
                    chrono::Utc::now() + chrono::Duration::hours(24),
                )?;
            }
            domain::SessionKind::Meeting => {
                repository.set_session_state(
                    session.id,
                    domain::SessionStatus::PendingEnhancement,
                    Some(domain::ProcessingMode::Queued),
                    session.provider.as_deref(),
                )?;
                repository.queue_enhancement(
                    session.id,
                    "transcription",
                    Some("recovered after an interrupted application run"),
                )?;
            }
        }
    }
    Ok(())
}

fn apply_pending_restore(data_dir: &std::path::Path) -> Result<(), Box<dyn Error>> {
    let pending = data_dir.with_extension("pending-restore");
    if !pending.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(data_dir)?;
    for suffix in ["sqlite3-wal", "sqlite3-shm"] {
        let stale = data_dir.join(format!("murmur.{suffix}"));
        if stale.is_file() {
            std::fs::remove_file(stale)?;
        }
    }
    copy_restore_tree(&pending, data_dir)?;
    std::fs::remove_dir_all(pending)?;
    Ok(())
}

fn copy_restore_tree(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let target_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            std::fs::create_dir_all(&target_path)?;
            copy_restore_tree(&source_path, &target_path)?;
        } else if source_path.is_file() {
            std::fs::copy(source_path, target_path)?;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn install_meeting_detection(app: &mut tauri::App) -> Result<(), Box<dyn Error>> {
    let handle = app.handle().clone();
    std::thread::Builder::new()
        .name("murmur-meeting-detection".into())
        .spawn(move || {
            let mut was_recording = false;
            loop {
                let state = handle.state::<CoreState>();
                let preferences = commands::read_preferences(&state.data_dir).unwrap_or_default();
                if !preferences.meeting_suggestions {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    continue;
                }
                let active = state
                    .active_meeting
                    .lock()
                    .ok()
                    .and_then(|recording| recording.clone());
                let online_recording = active
                    .as_ref()
                    .is_some_and(|recording| recording.source == domain::SessionSource::OnlineCall);
                let activity = detect_meeting_activity().ok().flatten().filter(|activity| {
                    !preferences
                        .excluded_meeting_applications
                        .iter()
                        .any(|name| name.eq_ignore_ascii_case(activity.application.display_name()))
                });
                let channel_activity = active
                    .as_ref()
                    .and_then(|recording| state.audio.channel_activity(&recording.capture_id).ok());
                let microphone_active = channel_activity
                    .as_ref()
                    .map(|activity| activity.microphone_active)
                    .unwrap_or(true);
                let system_audio_active = channel_activity
                    .as_ref()
                    .map(|activity| activity.system_active)
                    .unwrap_or(true);
                let prompt = state.meeting_prompts.lock().ok().and_then(|mut tracker| {
                    if online_recording != was_recording {
                        tracker.set_recording(online_recording);
                        was_recording = online_recording;
                    }
                    if active.is_some() && !online_recording {
                        None
                    } else {
                        tracker.observe(
                            activity,
                            microphone_active,
                            system_audio_active,
                            std::time::Duration::from_secs(2),
                        )
                    }
                });
                match prompt {
                    Some(MeetingPromptEvent::OfferStart(activity)) => {
                        let _ = handle.emit(
                            "murmur://meeting-prompt",
                            serde_json::json!({
                                "kind": "start",
                                "appName": activity.application.display_name(),
                                "title": activity.window_title.clone()
                            }),
                        );
                        let _ = handle
                            .notification()
                            .builder()
                            .title("Meeting detected")
                            .body(format!(
                                "Open Murmur to record {}.",
                                activity.application.display_name()
                            ))
                            .show();
                    }
                    Some(MeetingPromptEvent::OfferStop) => {
                        let _ = handle.emit(
                            "murmur://meeting-prompt",
                            serde_json::json!({ "kind": "stop", "appName": "Meeting" }),
                        );
                        let _ = handle
                            .notification()
                            .builder()
                            .title("Meeting may have ended")
                            .body("Open Murmur to stop recording.")
                            .show();
                    }
                    None => {}
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        })?;
    Ok(())
}

#[cfg(windows)]
fn install_update_checks(app: &mut tauri::App) -> Result<(), Box<dyn Error>> {
    let handle = app.handle().clone();
    std::thread::Builder::new()
        .name("murmur-update-checks".into())
        .spawn(move || loop {
            let state = handle.state::<CoreState>();
            if let Ok(status) = commands::check_for_updates_inner(&state) {
                if status.update_ready() {
                    let _ = handle
                        .notification()
                        .builder()
                        .title("Murmur update ready")
                        .body("The signed update will install on an ordinary restart.")
                        .show();
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(24 * 60 * 60));
        })?;
    Ok(())
}

#[cfg(windows)]
fn install_connectivity_monitor(app: &mut tauri::App) -> Result<(), Box<dyn Error>> {
    let handle = app.handle().clone();
    std::thread::Builder::new()
        .name("murmur-connectivity".into())
        .spawn(move || {
            let mut previous = None;
            loop {
                let online = "1.1.1.1:443"
                    .parse()
                    .ok()
                    .and_then(|address| {
                        std::net::TcpStream::connect_timeout(
                            &address,
                            std::time::Duration::from_secs(2),
                        )
                        .ok()
                    })
                    .is_some();
                if previous != Some(online) {
                    let _ = handle.emit(
                        "murmur://connectivity",
                        serde_json::json!({ "online": online }),
                    );
                    previous = Some(online);
                }
                // The public probe only drives the mode indicator. Managed networks can block it
                // while configured providers remain reachable, so due work must try on schedule
                // and let provider-specific failures control backoff.
                let state = handle.state::<CoreState>();
                let pending = state
                    .repository
                    .lock()
                    .ok()
                    .and_then(|mut repository| {
                        repository
                            .claim_due_pending_enhancement_sessions(chrono::Utc::now(), 4)
                            .ok()
                    })
                    .unwrap_or_default();
                for session_id in pending {
                    let _ = commands::resume_pending_enhancement(&handle, session_id, &state);
                }
                std::thread::sleep(std::time::Duration::from_secs(15));
            }
        })?;
    Ok(())
}

#[cfg(windows)]
fn install_shortcuts(app: &mut tauri::App) -> Result<(), Box<dyn Error>> {
    let state = app.state::<CoreState>();
    let config = commands::load_shortcut_config(&state.data_dir).unwrap_or_default();
    let (service, receiver) = GlobalShortcutService::start(config)?;
    *app.state::<CoreState>()
        .shortcuts
        .lock()
        .map_err(|_| "shortcut service lock is poisoned")? = Some(service);
    let handle = app.handle().clone();
    std::thread::Builder::new()
        .name("murmur-shortcut-events".into())
        .spawn(move || {
            while let Ok(event) = receiver.recv() {
                let state = handle.state::<CoreState>();
                let _ = match event {
                    ShortcutEvent::BeginDictation => {
                        commands::start_dictation_inner(&handle, &state)
                    }
                    ShortcutEvent::FinishDictation => {
                        commands::stop_dictation_inner(&handle, &state)
                    }
                    ShortcutEvent::CancelDictation => {
                        commands::cancel_dictation_inner(&handle, &state)
                    }
                };
            }
        })?;
    Ok(())
}
