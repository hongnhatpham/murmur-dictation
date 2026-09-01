use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{
    domain::RawTranscript,
    error::{CoreError, CoreResult},
    routing::SttProvider,
};

#[cfg(windows)]
#[path = "windows/mod.rs"]
pub mod windows;

#[cfg(windows)]
pub use windows::{WindowsAudioCapture, WindowsTextInsertion};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureRequest {
    pub microphone_device: Option<String>,
    pub include_system_audio: bool,
    pub output_directory: PathBuf,
}

pub trait AudioCapture: Send + Sync {
    fn start(&self, request: CaptureRequest) -> CoreResult<String>;
    fn stop(&self, capture_id: &str) -> CoreResult<Vec<PathBuf>>;
    fn cancel(&self, capture_id: &str) -> CoreResult<()>;
}

pub trait TextInsertion: Send + Sync {
    fn insert_accessibly(&self, text: &str) -> CoreResult<()>;
    fn paste_retaining_clipboard(&self, text: &str) -> CoreResult<()>;
}

pub trait SpeechRecognition: Send + Sync {
    fn transcribe(&self, provider: SttProvider, audio: &[PathBuf]) -> CoreResult<RawTranscript>;
}

/// Explicit fallback for non-Windows builds and tests which do not own native resources.
pub struct UnavailableNativeAdapter;

impl AudioCapture for UnavailableNativeAdapter {
    fn start(&self, _request: CaptureRequest) -> CoreResult<String> {
        unavailable("native audio capture")
    }
    fn stop(&self, _capture_id: &str) -> CoreResult<Vec<PathBuf>> {
        unavailable("native audio capture")
    }
    fn cancel(&self, _capture_id: &str) -> CoreResult<()> {
        unavailable("native audio capture")
    }
}

impl TextInsertion for UnavailableNativeAdapter {
    fn insert_accessibly(&self, _text: &str) -> CoreResult<()> {
        unavailable("Windows accessibility insertion")
    }
    fn paste_retaining_clipboard(&self, _text: &str) -> CoreResult<()> {
        unavailable("clipboard-retaining insertion")
    }
}

pub struct UnavailableProviderAdapter;

impl SpeechRecognition for UnavailableProviderAdapter {
    fn transcribe(&self, _provider: SttProvider, _audio: &[PathBuf]) -> CoreResult<RawTranscript> {
        unavailable("provider network adapter")
    }
}

fn unavailable<T>(feature: &str) -> CoreResult<T> {
    Err(CoreError::Unavailable(format!(
        "{feature} is not configured in this build"
    )))
}
