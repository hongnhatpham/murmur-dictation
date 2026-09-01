use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration as StdDuration,
};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::{DateTime, Duration, Utc};
use minisign_verify::{PublicKey, Signature};
use reqwest::{
    blocking::{Client, Response},
    header::{ACCEPT, AUTHORIZATION, USER_AGENT},
};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{CoreError, CoreResult};

const GITHUB_API_VERSION: &str = "2022-11-28";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateStatus {
    Idle,
    Checking,
    Downloading { version: String },
    ReadyOnRestart { version: String },
    Failed { message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePolicy {
    pub last_checked_at: Option<DateTime<Utc>>,
    pub status: UpdateStatus,
}

impl Default for UpdatePolicy {
    fn default() -> Self {
        Self {
            last_checked_at: None,
            status: UpdateStatus::Idle,
        }
    }
}

impl UpdatePolicy {
    pub fn check_due(&self, now: DateTime<Utc>) -> bool {
        self.last_checked_at
            .map(|last| now - last >= Duration::days(1))
            .unwrap_or(true)
    }

    pub fn mark_check_started(&mut self, now: DateTime<Utc>) {
        self.last_checked_at = Some(now);
        self.status = UpdateStatus::Checking;
    }

    pub fn mark_downloading(&mut self, version: String) {
        self.status = UpdateStatus::Downloading { version };
    }

    pub fn mark_downloaded(&mut self, version: String) {
        self.status = UpdateStatus::ReadyOnRestart { version };
    }

    pub fn can_install_on_restart(&self, recording: bool, meeting_processing: bool) -> bool {
        matches!(self.status, UpdateStatus::ReadyOnRestart { .. })
            && !recording
            && !meeting_processing
    }
}

pub fn load_update_policy(path: &Path) -> CoreResult<UpdatePolicy> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(Into::into),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(UpdatePolicy::default()),
        Err(error) => Err(error.into()),
    }
}

pub fn save_update_policy(path: &Path, policy: &UpdatePolicy) -> CoreResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let partial = path.with_extension("json.part");
    fs::write(&partial, serde_json::to_vec_pretty(policy)?)?;
    replace_file(&partial, path)
}

#[derive(Debug, Clone)]
pub struct GitHubUpdateConfig {
    pub owner: String,
    pub repository: String,
    pub platform_key: String,
    pub api_base: String,
}

impl GitHubUpdateConfig {
    pub fn private_repository(owner: impl Into<String>, repository: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            repository: repository.into(),
            platform_key: "windows-x86_64".into(),
            api_base: "https://api.github.com".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableUpdate {
    pub version: String,
    pub notes: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub artifact_url: String,
    pub signature: String,
}

/// Exact signed bytes plus their untouched Tauri signature. The integration layer must pass this
/// handoff to Tauri's updater verification path. This module never installs or bypasses signature
/// verification itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedUpdateHandoff {
    pub version: String,
    pub artifact_path: PathBuf,
    pub signature: String,
    pub sha256: String,
    pub downloaded_at: DateTime<Utc>,
}

/// An inert plan for an artifact whose persisted hash and Tauri minisign signature were verified.
/// The installer integration must recheck `expected_sha256` immediately before consuming the file
/// to close the time-of-check/time-of-use window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedInstallPlan {
    pub version: String,
    pub artifact_path: PathBuf,
    pub artifact_size_bytes: u64,
    pub expected_sha256: String,
    pub verified_at: DateTime<Utc>,
}

pub struct ReadyUpdateLaunchGuard {
    ready_state: PathBuf,
    launching_state: PathBuf,
    committed: bool,
}

impl ReadyUpdateLaunchGuard {
    /// Call only after the installer process was spawned successfully.
    pub fn commit(mut self) -> CoreResult<()> {
        fs::remove_file(&self.launching_state)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for ReadyUpdateLaunchGuard {
    fn drop(&mut self) {
        if !self.committed && self.launching_state.is_file() && !self.ready_state.exists() {
            let _ = fs::rename(&self.launching_state, &self.ready_state);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "decision")]
pub enum RestartInstallDecision {
    NoReadyUpdate,
    DeferredForActiveWork { version: String },
    Ready { plan: VerifiedInstallPlan },
}

pub fn plan_restart_install(
    update_directory: &Path,
    configured_public_key: &str,
    recording: bool,
    meeting_processing: bool,
) -> CoreResult<RestartInstallDecision> {
    if !update_directory.join("ready-update.json").is_file() {
        return Ok(RestartInstallDecision::NoReadyUpdate);
    }
    let plan = verify_ready_update(update_directory, configured_public_key)?;
    if recording || meeting_processing {
        Ok(RestartInstallDecision::DeferredForActiveWork {
            version: plan.version,
        })
    } else {
        Ok(RestartInstallDecision::Ready { plan })
    }
}

/// Reload and authenticate the ready update without executing it.
///
/// `configured_public_key` is the base64 value stored in Tauri's updater configuration. The
/// signature in `ready-update.json` remains untouched after download and uses the same base64
/// encoding expected by Tauri's updater.
pub fn verify_ready_update(
    update_directory: &Path,
    configured_public_key: &str,
) -> CoreResult<VerifiedInstallPlan> {
    let directory = fs::canonicalize(update_directory)?;
    let state_path = directory.join("ready-update.json");
    let state_metadata = fs::metadata(&state_path)?;
    if !state_metadata.is_file() || state_metadata.len() > 1024 * 1024 {
        return Err(CoreError::InvalidInput(
            "ready update state is missing or unexpectedly large".into(),
        ));
    }
    let handoff: SignedUpdateHandoff = serde_json::from_slice(&fs::read(&state_path)?)?;
    parse_version(&handoff.version)?;
    if handoff.sha256.len() != 64
        || !handoff
            .sha256
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(CoreError::InvalidInput(
            "ready update SHA-256 is malformed".into(),
        ));
    }

    let artifact = fs::canonicalize(&handoff.artifact_path)?;
    if !artifact.starts_with(&directory) || artifact.parent() != Some(directory.as_path()) {
        return Err(CoreError::InvalidInput(
            "ready update artifact is outside the update directory".into(),
        ));
    }
    let metadata = fs::metadata(&artifact)?;
    if !metadata.is_file() {
        return Err(CoreError::InvalidInput(
            "ready update artifact is not a regular file".into(),
        ));
    }
    let bytes = fs::read(&artifact)?;
    let actual_sha256 = format!("{:x}", Sha256::digest(&bytes));
    if !actual_sha256.eq_ignore_ascii_case(&handoff.sha256) {
        return Err(CoreError::InvalidInput(
            "ready update artifact changed after download".into(),
        ));
    }

    let public_key_text = decode_tauri_base64(configured_public_key, "updater public key")?;
    let signature_text = decode_tauri_base64(&handoff.signature, "update signature")?;
    let public_key = PublicKey::decode(&public_key_text).map_err(minisign_error)?;
    let signature = Signature::decode(&signature_text).map_err(minisign_error)?;
    public_key
        .verify(&bytes, &signature, true)
        .map_err(minisign_error)?;

    Ok(VerifiedInstallPlan {
        version: handoff.version,
        artifact_path: artifact,
        artifact_size_bytes: metadata.len(),
        expected_sha256: actual_sha256,
        verified_at: Utc::now(),
    })
}

/// Recheck the exact artifact immediately before it is handed to an installer process.
pub fn recheck_install_plan(plan: &VerifiedInstallPlan) -> CoreResult<()> {
    let artifact = fs::canonicalize(&plan.artifact_path)?;
    if artifact != plan.artifact_path {
        return Err(CoreError::InvalidInput(
            "verified update artifact path changed before launch".into(),
        ));
    }
    let metadata = fs::metadata(&artifact)?;
    if !metadata.is_file() || metadata.len() != plan.artifact_size_bytes {
        return Err(CoreError::InvalidInput(
            "verified update artifact size changed before launch".into(),
        ));
    }
    let mut file = fs::File::open(artifact)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let actual = format!("{:x}", digest.finalize());
    if !actual.eq_ignore_ascii_case(&plan.expected_sha256) {
        return Err(CoreError::InvalidInput(
            "verified update artifact changed before launch".into(),
        ));
    }
    Ok(())
}

/// Lease the persisted ready state for one installer launch. Dropping the guard before `commit`
/// restores the ready state; committing after a successful spawn prevents restart loops.
pub fn prepare_ready_update_launch(
    update_directory: &Path,
    plan: &VerifiedInstallPlan,
) -> CoreResult<ReadyUpdateLaunchGuard> {
    recheck_install_plan(plan)?;
    let directory = fs::canonicalize(update_directory)?;
    if plan.artifact_path.parent() != Some(directory.as_path()) {
        return Err(CoreError::InvalidInput(
            "verified update artifact is outside the update directory".into(),
        ));
    }
    let ready_state = directory.join("ready-update.json");
    let launching_state = directory.join("launching-update.json");
    if launching_state.exists() {
        return Err(CoreError::Unavailable(
            "an update launch is already in progress".into(),
        ));
    }
    fs::rename(&ready_state, &launching_state)?;
    Ok(ReadyUpdateLaunchGuard {
        ready_state,
        launching_state,
        committed: false,
    })
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    body: Option<String>,
    published_at: Option<DateTime<Utc>>,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Deserialize)]
struct TauriUpdateManifest {
    version: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    pub_date: Option<DateTime<Utc>>,
    platforms: std::collections::HashMap<String, TauriPlatform>,
}

#[derive(Debug, Deserialize)]
struct TauriPlatform {
    signature: String,
    url: String,
}

pub struct GitHubUpdateClient {
    client: Client,
    token: String,
    config: GitHubUpdateConfig,
}

impl GitHubUpdateClient {
    pub fn new(token: impl Into<String>, config: GitHubUpdateConfig) -> CoreResult<Self> {
        let token = token.into();
        if token.trim().is_empty() {
            return Err(CoreError::InvalidInput(
                "private update credential is empty".into(),
            ));
        }
        Ok(Self {
            client: Client::builder()
                .connect_timeout(StdDuration::from_secs(5))
                .timeout(StdDuration::from_secs(300))
                .user_agent(concat!("Murmur/", env!("CARGO_PKG_VERSION")))
                .build()
                .map_err(network_error)?,
            token,
            config,
        })
    }

    pub fn check(&self, current_version: &str) -> CoreResult<Option<AvailableUpdate>> {
        let current = parse_version(current_version)?;
        let release_url = format!(
            "{}/repos/{}/{}/releases/latest",
            self.config.api_base, self.config.owner, self.config.repository
        );
        let release: GitHubRelease = self
            .checked(
                self.authorized(self.client.get(&release_url))
                    .send()
                    .map_err(network_error)?,
            )?
            .json()
            .map_err(network_error)?;
        let manifest_asset = release
            .assets
            .iter()
            .find(|asset| asset.name == "latest.json")
            .ok_or_else(|| CoreError::NotFound("release does not contain latest.json".into()))?;
        validate_download_url(&manifest_asset.browser_download_url)?;
        let manifest: TauriUpdateManifest = self
            .checked(
                self.authorized(self.client.get(&manifest_asset.browser_download_url))
                    .header(ACCEPT, "application/octet-stream")
                    .send()
                    .map_err(network_error)?,
            )?
            .json()
            .map_err(network_error)?;
        let release_version = parse_version(manifest.version.trim_start_matches('v'))?;
        let tag_version = parse_version(release.tag_name.trim_start_matches('v'))?;
        if release_version != tag_version {
            return Err(CoreError::InvalidInput(
                "release tag and Tauri manifest version do not match".into(),
            ));
        }
        if release_version <= current {
            return Ok(None);
        }
        let platform = manifest
            .platforms
            .get(&self.config.platform_key)
            .ok_or_else(|| {
                CoreError::NotFound(format!(
                    "latest.json has no {} artifact",
                    self.config.platform_key
                ))
            })?;
        if platform.signature.trim().is_empty() {
            return Err(CoreError::InvalidInput(
                "update manifest signature is empty".into(),
            ));
        }
        validate_download_url(&platform.url)?;
        Ok(Some(AvailableUpdate {
            version: release_version.to_string(),
            notes: manifest.notes.or(release.body),
            published_at: manifest.pub_date.or(release.published_at),
            artifact_url: platform.url.clone(),
            signature: platform.signature.clone(),
        }))
    }

    pub fn download(
        &self,
        update: &AvailableUpdate,
        directory: &Path,
        mut progress: impl FnMut(u64, Option<u64>),
    ) -> CoreResult<SignedUpdateHandoff> {
        validate_download_url(&update.artifact_url)?;
        if update.signature.trim().is_empty() {
            return Err(CoreError::InvalidInput("update signature is empty".into()));
        }
        parse_version(&update.version)?;
        fs::create_dir_all(directory)?;
        let directory = fs::canonicalize(directory)?;
        let file_name = url::Url::parse(&update.artifact_url)
            .ok()
            .and_then(|url| {
                url.path_segments()
                    .and_then(Iterator::last)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| format!("murmur-{}.updater", update.version));
        let file_name = if file_name == "." || file_name == ".." {
            format!("murmur-{}.updater", update.version)
        } else {
            file_name
        };
        let destination = directory.join(file_name);
        let partial = directory.join(format!(".update-{}.download", uuid::Uuid::new_v4()));
        let mut response = self.checked(
            self.authorized(self.client.get(&update.artifact_url))
                .header(ACCEPT, "application/octet-stream")
                .send()
                .map_err(network_error)?,
        )?;
        let total = response.content_length();
        let mut file = fs::File::create(&partial)?;
        let mut digest = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        let mut received = 0_u64;
        loop {
            let read = response.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read])?;
            digest.update(&buffer[..read]);
            received += read as u64;
            progress(received, total);
        }
        file.sync_all()?;
        replace_file(&partial, &destination)?;
        let handoff = SignedUpdateHandoff {
            version: update.version.clone(),
            artifact_path: destination,
            signature: update.signature.clone(),
            sha256: format!("{:x}", digest.finalize()),
            downloaded_at: Utc::now(),
        };
        let state_path = directory.join("ready-update.json");
        let state_partial = directory.join("ready-update.json.part");
        fs::write(&state_partial, serde_json::to_vec_pretty(&handoff)?)?;
        replace_file(&state_partial, &state_path)?;
        Ok(handoff)
    }

    fn authorized(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> reqwest::blocking::RequestBuilder {
        request
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .header(USER_AGENT, concat!("Murmur/", env!("CARGO_PKG_VERSION")))
    }

    fn checked(&self, response: Response) -> CoreResult<Response> {
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let message = match status.as_u16() {
            401 | 403 => "private update access was denied",
            404 => "private update release was not found",
            429 => "GitHub update quota was exhausted",
            _ => "private update request failed",
        };
        Err(CoreError::Unavailable(format!("{message}: HTTP {status}")))
    }
}

fn parse_version(value: &str) -> CoreResult<Version> {
    Version::parse(value)
        .map_err(|error| CoreError::InvalidInput(format!("invalid version {value}: {error}")))
}

fn validate_download_url(value: &str) -> CoreResult<()> {
    let url = url::Url::parse(value)
        .map_err(|error| CoreError::InvalidInput(format!("invalid update URL: {error}")))?;
    if url.scheme() != "https" && !cfg!(test) {
        return Err(CoreError::InvalidInput(
            "update downloads require HTTPS".into(),
        ));
    }
    if !cfg!(test)
        && !matches!(
            url.host_str(),
            Some("api.github.com")
                | Some("github.com")
                | Some("objects.githubusercontent.com")
                | Some("release-assets.githubusercontent.com")
        )
    {
        return Err(CoreError::InvalidInput(
            "private update URL is not hosted by GitHub".into(),
        ));
    }
    Ok(())
}

fn replace_file(source: &Path, destination: &Path) -> CoreResult<()> {
    if !destination.exists() {
        fs::rename(source, destination)?;
        return Ok(());
    }
    let previous = destination.with_extension(format!("previous-{}", uuid::Uuid::new_v4()));
    fs::rename(destination, &previous)?;
    if let Err(error) = fs::rename(source, destination) {
        let _ = fs::rename(&previous, destination);
        return Err(error.into());
    }
    fs::remove_file(previous)?;
    Ok(())
}

fn network_error(error: reqwest::Error) -> CoreError {
    CoreError::Unavailable(format!("private update request failed: {error}"))
}

fn decode_tauri_base64(value: &str, name: &str) -> CoreResult<String> {
    let bytes = BASE64
        .decode(value.trim())
        .map_err(|_| CoreError::InvalidInput(format!("{name} is not valid base64")))?;
    String::from_utf8(bytes)
        .map_err(|_| CoreError::InvalidInput(format!("{name} is not valid UTF-8")))
}

fn minisign_error(error: minisign_verify::Error) -> CoreError {
    CoreError::InvalidInput(format!("update signature verification failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader},
        net::TcpListener,
        sync::{Arc, Mutex},
        thread,
    };

    #[test]
    fn ready_update_waits_for_active_work() {
        let mut policy = UpdatePolicy::default();
        policy.mark_downloaded("0.2.0".into());
        assert!(!policy.can_install_on_restart(true, false));
        assert!(!policy.can_install_on_restart(false, true));
        assert!(policy.can_install_on_restart(false, false));
    }

    #[test]
    fn update_policy_round_trips_and_preserves_daily_check_time() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("update-policy.json");
        let mut policy = UpdatePolicy::default();
        let now = Utc::now();
        policy.mark_check_started(now);
        save_update_policy(&path, &policy).unwrap();
        let loaded = load_update_policy(&path).unwrap();
        assert_eq!(loaded.last_checked_at, Some(now));
        assert!(!loaded.check_due(now + Duration::hours(23)));
        assert!(loaded.check_due(now + Duration::hours(24)));
    }

    #[test]
    fn private_release_check_preserves_signature() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let base = format!("http://{address}");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let release = format!(
            r#"{{"tag_name":"v0.2.0","body":"Fix","published_at":"2026-08-31T00:00:00Z","assets":[{{"name":"latest.json","browser_download_url":"{base}/latest.json"}}]}}"#
        );
        let manifest = format!(
            r#"{{"version":"0.2.0","platforms":{{"windows-x86_64":{{"signature":"tauri-signature","url":"{base}/Murmur.msi.zip"}}}}}}"#
        );
        thread::spawn(move || {
            for body in [release, manifest] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut authorization = false;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header == "\r\n" {
                        break;
                    }
                    authorization |= header
                        .to_ascii_lowercase()
                        .starts_with("authorization: bearer ");
                }
                captured
                    .lock()
                    .unwrap()
                    .push(format!("{line}{authorization}"));
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        let client = GitHubUpdateClient::new(
            "not-a-real-token",
            GitHubUpdateConfig {
                owner: "owner".into(),
                repository: "repo".into(),
                platform_key: "windows-x86_64".into(),
                api_base: base,
            },
        )
        .unwrap();
        let update = client.check("0.1.0").unwrap().unwrap();
        assert_eq!(update.version, "0.2.0");
        assert_eq!(update.signature, "tauri-signature");
        assert!(requests
            .lock()
            .unwrap()
            .iter()
            .all(|request| request.ends_with("true")));
    }

    const TEST_PUBLIC_KEY: &str = "untrusted comment: minisign public key E7620F1842B4E81F\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    const TEST_SIGNATURE: &str = "untrusted comment: signature from minisign secret key\nRWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaVWuNrm73YiIiJbq71Wi+dP9eKL8OC351vwIasSSbXxwA=\ntrusted comment: timestamp:1555779966\tfile:test\nQtKMXWyYcwdpZAlPF7tE2ENJkRd1ujvKjlj1m9RtHTBnZPa5WKU5uWRs5GoP5M/VqE81QFuMKI5k/SfNQUaOAA==";

    fn ready_fixture(directory: &Path, bytes: &[u8]) {
        let artifact = directory.join("Murmur.msi.zip");
        fs::write(&artifact, bytes).unwrap();
        let handoff = SignedUpdateHandoff {
            version: "0.2.0".into(),
            artifact_path: artifact,
            signature: BASE64.encode(TEST_SIGNATURE),
            sha256: format!("{:x}", Sha256::digest(bytes)),
            downloaded_at: Utc::now(),
        };
        fs::write(
            directory.join("ready-update.json"),
            serde_json::to_vec(&handoff).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn ready_update_verification_returns_inert_plan() {
        let directory = tempfile::tempdir().unwrap();
        ready_fixture(directory.path(), b"test");
        let plan = verify_ready_update(directory.path(), &BASE64.encode(TEST_PUBLIC_KEY)).unwrap();
        assert_eq!(plan.version, "0.2.0");
        assert_eq!(plan.artifact_size_bytes, 4);
        assert_eq!(
            plan.artifact_path,
            fs::canonicalize(directory.path().join("Murmur.msi.zip")).unwrap()
        );
    }

    #[test]
    fn ready_update_verification_rejects_tampered_artifact() {
        let directory = tempfile::tempdir().unwrap();
        ready_fixture(directory.path(), b"test");
        fs::write(directory.path().join("Murmur.msi.zip"), b"tampered").unwrap();
        let error = verify_ready_update(directory.path(), &BASE64.encode(TEST_PUBLIC_KEY))
            .expect_err("tampering must fail");
        assert!(error.to_string().contains("changed after download"));
    }

    #[test]
    fn launch_guard_rechecks_and_consumes_ready_state_only_after_commit() {
        let directory = tempfile::tempdir().unwrap();
        ready_fixture(directory.path(), b"test");
        let plan = verify_ready_update(directory.path(), &BASE64.encode(TEST_PUBLIC_KEY)).unwrap();
        {
            let _guard = prepare_ready_update_launch(directory.path(), &plan).unwrap();
            assert!(!directory.path().join("ready-update.json").exists());
            assert!(directory.path().join("launching-update.json").exists());
        }
        assert!(directory.path().join("ready-update.json").exists());

        prepare_ready_update_launch(directory.path(), &plan)
            .unwrap()
            .commit()
            .unwrap();
        assert!(!directory.path().join("ready-update.json").exists());
        assert!(!directory.path().join("launching-update.json").exists());
    }

    #[test]
    fn launch_guard_rejects_changes_after_verification() {
        let directory = tempfile::tempdir().unwrap();
        ready_fixture(directory.path(), b"test");
        let plan = verify_ready_update(directory.path(), &BASE64.encode(TEST_PUBLIC_KEY)).unwrap();
        fs::write(directory.path().join("Murmur.msi.zip"), b"changed").unwrap();
        assert!(prepare_ready_update_launch(directory.path(), &plan).is_err());
        assert!(directory.path().join("ready-update.json").exists());
    }

    #[test]
    fn ready_update_verification_rejects_wrong_key() {
        let directory = tempfile::tempdir().unwrap();
        ready_fixture(directory.path(), b"test");
        let mut wrong_key = [0_u8; 42];
        wrong_key[0..2].copy_from_slice(b"Ed");
        wrong_key[2..10].copy_from_slice(b"wrongkey");
        let wrong_key_text = format!(
            "untrusted comment: another minisign key\n{}",
            BASE64.encode(wrong_key)
        );
        let error = verify_ready_update(directory.path(), &BASE64.encode(wrong_key_text))
            .expect_err("wrong public key must fail");
        assert!(error.to_string().contains("signature verification failed"));
    }

    #[test]
    fn restart_plan_verifies_before_deferring_active_work() {
        let directory = tempfile::tempdir().unwrap();
        ready_fixture(directory.path(), b"test");
        let decision = plan_restart_install(
            directory.path(),
            &BASE64.encode(TEST_PUBLIC_KEY),
            true,
            false,
        )
        .unwrap();
        assert_eq!(
            decision,
            RestartInstallDecision::DeferredForActiveWork {
                version: "0.2.0".into()
            }
        );
        fs::write(directory.path().join("Murmur.msi.zip"), b"tampered").unwrap();
        assert!(plan_restart_install(
            directory.path(),
            &BASE64.encode(TEST_PUBLIC_KEY),
            true,
            false
        )
        .is_err());
    }
}
