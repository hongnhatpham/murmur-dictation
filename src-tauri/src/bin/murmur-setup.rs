use std::{
    env, fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::Command,
};

use murmur_core::{
    adapters::{
        windows::{list_microphones, WindowsTextInsertion},
        TextInsertion,
    },
    backup::{create_backup, restore_backup},
    diagnostics::{deterministic_setup_plan, run_diagnostics_with, DiagnosticInputs},
    providers::{
        download_checked_model, install_local_whisper_runtime_archive, validate_local_model,
        validate_local_whisper_install, ProviderError, SpeechProviders, LOCAL_WHISPER_MODEL_NAME,
        LOCAL_WHISPER_MODEL_SHA256, LOCAL_WHISPER_MODEL_URL, LOCAL_WHISPER_RUNTIME_RELEASE,
        LOCAL_WHISPER_RUNTIME_SHA256, LOCAL_WHISPER_RUNTIME_URL,
    },
    secrets::{provider_credentials, secret_statuses, SecretStore, WindowsCredentialStore},
    updates::{verify_ready_update, GitHubUpdateClient, GitHubUpdateConfig},
};
use serde::Serialize;

struct CliError(String);

impl From<String> for CliError {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for CliError {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

impl From<std::io::Error> for CliError {
    fn from(value: std::io::Error) -> Self {
        Self(value.to_string())
    }
}

impl From<murmur_core::error::CoreError> for CliError {
    fn from(value: murmur_core::error::CoreError) -> Self {
        Self(value.to_string())
    }
}

impl From<ProviderError> for CliError {
    fn from(value: ProviderError) -> Self {
        Self(value.message)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CliOutput<T: Serialize> {
    schema_version: u32,
    ok: bool,
    command: String,
    result: Option<T>,
    error: Option<String>,
}

#[allow(dead_code)]
fn main() {
    run_from(env::args().skip(1).collect())
}

pub fn run_from(args: Vec<String>) -> ! {
    match execute(args) {
        Ok((command, value)) => {
            print_json(&CliOutput {
                schema_version: 1,
                ok: true,
                command,
                result: Some(value),
                error: None,
            });
            std::process::exit(0)
        }
        Err((command, error)) => {
            print_json(&CliOutput::<serde_json::Value> {
                schema_version: 1,
                ok: false,
                command,
                result: None,
                error: Some(error),
            });
            std::process::exit(1)
        }
    }
}

fn execute(args: Vec<String>) -> Result<(String, serde_json::Value), (String, String)> {
    let command = command_name(&args);
    let result = (|| -> Result<serde_json::Value, CliError> {
        Ok(match args.as_slice() {
            [value] if value == "plan" => json_value(deterministic_setup_plan()),
            [value] if value == "diagnose" => {
                let data_dir = default_data_dir()?;
                json_value(diagnose(&data_dir)?)
            }
            [diagnose_command, flag, path]
                if diagnose_command == "diagnose" && flag == "--data-dir" =>
            {
                json_value(diagnose(Path::new(path))?)
            }
            [secret, set, name] if secret == "secret" && set == "set" => {
                let mut value = String::new();
                io::stdin().read_to_string(&mut value)?;
                let value = value.trim_end_matches(['\r', '\n']);
                WindowsCredentialStore.set(name, value)?;
                serde_json::json!({ "name": name, "stored": true })
            }
            [secret, delete, name] if secret == "secret" && delete == "delete" => {
                WindowsCredentialStore.delete(name)?;
                serde_json::json!({ "name": name, "deleted": true })
            }
            [secret, status, name] if secret == "secret" && status == "status" => {
                let configured = WindowsCredentialStore.get(name)?.is_some();
                serde_json::json!({ "name": name, "configured": configured })
            }
            [secret, status] if secret == "secret" && status == "status" => {
                json_value(secret_statuses(&WindowsCredentialStore)?)
            }
            [provider, validate, name] if provider == "provider" && validate == "validate" => {
                let credentials = provider_credentials(&WindowsCredentialStore)?;
                SpeechProviders::new(credentials)?
                    .validate_provider(name)
                    .map_err(|error| {
                        CliError(format!("{} validation failed: {}", name, error.message))
                    })?;
                serde_json::json!({ "provider": name, "valid": true })
            }
            [insertion, check, confirmation]
                if insertion == "insertion"
                    && check == "check"
                    && confirmation == "--confirm-target-ready" =>
            {
                insertion_check(&default_data_dir()?)?
            }
            [insertion, check, confirmation, data_dir]
                if insertion == "insertion"
                    && check == "check"
                    && confirmation == "--confirm-target-ready" =>
            {
                insertion_check(Path::new(data_dir))?
            }
            [model, install] if model == "model" && install == "install" => {
                install_local_model(&default_data_dir()?)?
            }
            [model, install, data_dir] if model == "model" && install == "install" => {
                install_local_model(Path::new(data_dir))?
            }
            [model, download, url, checksum, destination]
                if model == "model" && download == "download" =>
            {
                let result = download_checked_model(
                    url,
                    Path::new(destination),
                    checksum,
                    |received, total| {
                        eprintln!(
                            "{}",
                            serde_json::json!({
                                "event": "progress",
                                "operation": "model.download",
                                "receivedBytes": received,
                                "totalBytes": total
                            })
                        );
                    },
                )?;
                json_value(result)
            }
            [model, validate, path, checksum] if model == "model" && validate == "validate" => {
                serde_json::json!({
                    "path": path,
                    "valid": validate_local_model(Path::new(path), checksum)?
                })
            }
            [backup, create, data_dir, archive] if backup == "backup" && create == "create" => {
                json_value(create_backup(
                    Path::new(data_dir),
                    Path::new(archive),
                    env!("CARGO_PKG_VERSION"),
                )?)
            }
            [backup, restore, archive, destination]
                if backup == "backup" && restore == "restore" =>
            {
                json_value(restore_backup(Path::new(archive), Path::new(destination))?)
            }
            [startup, enable, executable] if startup == "startup" && enable == "enable" => {
                enable_startup(Path::new(executable))?;
                serde_json::json!({ "enabled": true, "executable": executable })
            }
            [updates, check, owner, repository, current]
                if updates == "updates" && check == "check" =>
            {
                let token = required_secret("github_updates")?;
                let client = GitHubUpdateClient::new(
                    token,
                    GitHubUpdateConfig::private_repository(owner, repository),
                )?;
                json_value(client.check(current)?)
            }
            [updates, download, owner, repository, current, directory]
                if updates == "updates" && download == "download" =>
            {
                let token = required_secret("github_updates")?;
                let client = GitHubUpdateClient::new(
                    token,
                    GitHubUpdateConfig::private_repository(owner, repository),
                )?;
                let update = client
                    .check(current)?
                    .ok_or_else(|| CliError("no newer update is available".into()))?;
                let handoff =
                    client.download(&update, Path::new(directory), |received, total| {
                        eprintln!(
                            "{}",
                            serde_json::json!({
                                "event": "progress",
                                "operation": "updates.download",
                                "receivedBytes": received,
                                "totalBytes": total
                            })
                        );
                    })?;
                json_value(handoff)
            }
            [updates, verify, directory, public_key]
                if updates == "updates" && verify == "verify" =>
            {
                json_value(verify_ready_update(Path::new(directory), public_key)?)
            }
            _ => return Err(CliError(usage().into())),
        })
    })();
    result
        .map(|value| (command.clone(), value))
        .map_err(|error| (command, error.0))
}

fn command_name(args: &[String]) -> String {
    args.iter()
        .take(2)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(".")
        .chars()
        .take(64)
        .collect::<String>()
        .if_empty("help")
}

fn usage() -> &'static str {
    "usage: murmur-setup plan | diagnose [--data-dir PATH] | secret set|delete|status [NAME] | provider validate NAME | insertion check --confirm-target-ready [DATA_DIR] | model install [DATA_DIR] | model download URL SHA256 PATH | model validate PATH SHA256 | backup create DATA_DIR ZIP | backup restore ZIP DATA_DIR | startup enable EXE | updates check OWNER REPO CURRENT_VERSION | updates download OWNER REPO CURRENT_VERSION DIRECTORY | updates verify DIRECTORY PUBLIC_KEY. secret set reads the value from standard input"
}

fn progress(operation: &'static str) -> impl FnMut(u64, Option<u64>) {
    move |received, total| {
        eprintln!(
            "{}",
            serde_json::json!({
                "event": "progress",
                "operation": operation,
                "receivedBytes": received,
                "totalBytes": total
            })
        );
    }
}

fn install_local_model(data_dir: &Path) -> Result<serde_json::Value, CliError> {
    let model_dir = data_dir.join("models");
    let archive = model_dir.join(format!(
        "whisper-runtime-{LOCAL_WHISPER_RUNTIME_RELEASE}.zip"
    ));
    let runtime_download = download_checked_model(
        LOCAL_WHISPER_RUNTIME_URL,
        &archive,
        LOCAL_WHISPER_RUNTIME_SHA256,
        progress("model.runtime"),
    )?;
    let runtime = install_local_whisper_runtime_archive(&archive, &model_dir)?;
    let model = download_checked_model(
        LOCAL_WHISPER_MODEL_URL,
        &model_dir.join(LOCAL_WHISPER_MODEL_NAME),
        LOCAL_WHISPER_MODEL_SHA256,
        progress("model.weights"),
    )?;
    if !validate_local_whisper_install(&model_dir)? {
        return Err(CliError(
            "local whisper.cpp installation failed validation".into(),
        ));
    }
    Ok(serde_json::json!({
        "runtimeDownload": runtime_download,
        "runtime": runtime,
        "model": model,
        "valid": true
    }))
}

fn required_secret(name: &str) -> Result<String, CliError> {
    WindowsCredentialStore
        .get(name)?
        .ok_or_else(|| CliError(format!("{name} credential is not configured")))
}

fn insertion_check(data_dir: &Path) -> Result<serde_json::Value, CliError> {
    eprintln!(
        "{}",
        serde_json::json!({
            "event": "focusTarget",
            "operation": "insertion.check",
            "delaySeconds": 3
        })
    );
    std::thread::sleep(std::time::Duration::from_secs(3));
    const MARKER: &str = "MURMUR_INSERTION_CHECK";
    let insertion = WindowsTextInsertion::new();
    insertion.paste_retaining_clipboard(MARKER)?;
    if !insertion.focused_text_contains(MARKER)? {
        return Err(CliError(
            "Windows accepted the paste shortcut, but the marker did not appear in the focused field"
                .into(),
        ));
    }
    fs::create_dir_all(data_dir)?;
    fs::write(
        data_dir.join("insertion-check.json"),
        serde_json::to_vec(&serde_json::json!({ "passedAt": chrono::Utc::now() }))
            .map_err(murmur_core::error::CoreError::from)?,
    )?;
    Ok(serde_json::json!({ "inserted": true, "marker": MARKER }))
}

fn default_data_dir() -> Result<PathBuf, CliError> {
    let roaming = env::var_os("APPDATA").ok_or_else(|| CliError("APPDATA is not set".into()))?;
    Ok(PathBuf::from(roaming).join("com.bynhat.murmur"))
}

fn diagnose(data_dir: &Path) -> Result<murmur_core::diagnostics::DiagnosticReport, CliError> {
    let model_dir = data_dir.join("models");
    let microphone = match list_microphones() {
        Ok(devices) if !devices.is_empty() => Some(Ok(())),
        Ok(_) => Some(Err("Windows reports no active microphone endpoint".into())),
        Err(error) => Some(Err(error.to_string())),
    };
    Ok(run_diagnostics_with(
        data_dir,
        &WindowsCredentialStore,
        DiagnosticInputs {
            microphone,
            insertion: data_dir
                .join("insertion-check.json")
                .is_file()
                .then_some(Ok(())),
            startup_enabled: Some(startup_enabled()),
            whisper_install_directory: Some(&model_dir),
            updater_public_key_configured: Some(
                option_env!("MURMUR_UPDATER_PUBLIC_KEY").is_some_and(|key| !key.trim().is_empty()),
            ),
        },
    )?)
}

#[cfg(windows)]
fn startup_enabled() -> bool {
    Command::new("reg.exe")
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

#[cfg(windows)]
fn enable_startup(executable: &Path) -> Result<(), String> {
    if !executable.is_file() {
        return Err(format!(
            "startup executable does not exist: {}",
            executable.display()
        ));
    }
    let value = format!("\"{}\"", executable.display());
    let status = Command::new("reg.exe")
        .args([
            "add",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            "Murmur",
            "/t",
            "REG_SZ",
            "/d",
            &value,
            "/f",
        ])
        .status()
        .map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("reg.exe exited with {status}"))
    }
}

#[cfg(not(windows))]
fn enable_startup(_executable: &Path) -> Result<(), String> {
    Err("launch with Windows can only be configured on Windows".into())
}

fn json_value<T: Serialize>(value: T) -> serde_json::Value {
    serde_json::to_value(value)
        .unwrap_or_else(|error| serde_json::json!({ "serializationError": error.to_string() }))
}

fn print_json<T: Serialize>(value: &T) {
    match serde_json::to_string(value) {
        Ok(json) => println!("{json}"),
        Err(_) => println!(
            "{}",
            r#"{"schemaVersion":1,"ok":false,"command":"internal","error":"could not serialize output"}"#
        ),
    }
}

trait EmptyString {
    fn if_empty(self, fallback: &str) -> String;
}

impl EmptyString for String {
    fn if_empty(self, fallback: &str) -> String {
        if self.is_empty() {
            fallback.into()
        } else {
            self
        }
    }
}
