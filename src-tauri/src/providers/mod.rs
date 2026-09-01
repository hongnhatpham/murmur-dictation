//! Hosted and local speech processing.
//!
//! STT requests intentionally have no vocabulary or prompt field. Personal Vocabulary belongs
//! only in [`CorrectionRequest`], which prevents dictionary text from leaking into provider
//! recognition prompts.

use std::{
    ffi::OsString,
    fs,
    io::{Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use reqwest::{
    blocking::{multipart, Client, Response},
    header::{AUTHORIZATION, CONTENT_TYPE},
    StatusCode,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tungstenite::{
    client::IntoClientRequest, http::HeaderValue, stream::MaybeTlsStream, Message, WebSocket,
};
use uuid::Uuid;
use zip::ZipArchive;

use crate::{
    domain::{ProcessingMode, SessionKind},
    error::{CoreError, CoreResult},
    routing::{ProviderFailure, SttProvider},
};

pub const GROQ_WHISPER_MODEL: &str = "whisper-large-v3-turbo";
pub const GROQ_CORRECTION_MODEL: &str = "openai/gpt-oss-20b";
pub const GROQ_MEETING_MODEL: &str = "openai/gpt-oss-120b";
const MEETING_GROUNDING_RULES: &str = "Treat the meeting title as organizational metadata only, never as evidence. Use only the supplied timestamped transcript or notes as evidence. Do not infer speaker identity, quote attribution or source, or facts from outside knowledge.";
const DICTATION_HOSTED_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(5);
const DICTATION_ASSEMBLY_DEADLINE: Duration = Duration::from_secs(10);
const DICTATION_CORRECTION_TIMEOUT: Duration = Duration::from_secs(1);
pub const LOCAL_WHISPER_MODEL_NAME: &str = "ggml-large-v3-turbo-q5_0.bin";
pub const LOCAL_WHISPER_MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-large-v3-turbo-q5_0.bin";
pub const LOCAL_WHISPER_MODEL_SHA256: &str =
    "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2";
pub const LOCAL_WHISPER_RUNTIME_RELEASE: &str = "b4938";
pub const LOCAL_WHISPER_RUNTIME_URL: &str =
    "https://github.com/ggml-org/whisper.cpp/releases/download/b4938/whisper-cublas-12.4.0-bin-x64.zip";
pub const LOCAL_WHISPER_RUNTIME_SHA256: &str =
    "c1b17166e1e31a91cc8e9c1f910d3785e3ce757bb2958bf9dce13fdb4880005f";
const LOCAL_WHISPER_RUNTIME_FILES: &[&str] = &[
    "whisper-cli.exe",
    "whisper.dll",
    "ggml.dll",
    "ggml-base.dll",
    "ggml-cpu-alderlake.dll",
    "ggml-cpu-cannonlake.dll",
    "ggml-cpu-cascadelake.dll",
    "ggml-cpu-haswell.dll",
    "ggml-cpu-icelake.dll",
    "ggml-cpu-sandybridge.dll",
    "ggml-cpu-skylakex.dll",
    "ggml-cpu-sse42.dll",
    "ggml-cpu-x64.dll",
    "ggml-cuda.dll",
    "cublas64_12.dll",
    "cublasLt64_12.dll",
    "cudart64_12.dll",
    "nvblas64_12.dll",
    "nvrtc-builtins64_124.dll",
    "nvrtc64_120_0.dll",
];

#[derive(Debug, Clone)]
pub struct ProviderEndpoints {
    pub deepgram: String,
    pub assemblyai: String,
    pub groq: String,
}

impl Default for ProviderEndpoints {
    fn default() -> Self {
        Self {
            deepgram: "https://api.deepgram.com".into(),
            assemblyai: "https://api.assemblyai.com".into(),
            groq: "https://api.groq.com".into(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProviderCredentials {
    pub deepgram: Option<String>,
    pub assemblyai: Option<String>,
    pub groq: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AudioRequest {
    pub bytes: Vec<u8>,
    pub mime_type: String,
    pub source_path: Option<PathBuf>,
}

impl AudioRequest {
    pub fn from_file(path: impl Into<PathBuf>, mime_type: impl Into<String>) -> CoreResult<Self> {
        let source_path = path.into();
        Ok(Self {
            bytes: fs::read(&source_path)?,
            mime_type: mime_type.into(),
            source_path: Some(source_path),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptSegment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub speaker: Option<String>,
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionResult {
    pub text: String,
    pub language: Option<String>,
    pub segments: Vec<TranscriptSegment>,
    pub provider: String,
    pub diarization: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAttemptFailure {
    pub provider: String,
    pub failure: ProviderFailure,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "outcome")]
pub enum TranscriptionOutcome {
    Completed {
        transcript: TranscriptionResult,
        attempts: Vec<ProviderAttemptFailure>,
    },
    PendingEnhancement {
        attempts: Vec<ProviderAttemptFailure>,
    },
    Failed {
        attempts: Vec<ProviderAttemptFailure>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAttemptStarted {
    pub provider: String,
    pub mode: ProcessingMode,
    pub attempt: u8,
}

#[derive(Debug, Clone)]
pub struct LocalWhisperConfig {
    pub executable: PathBuf,
    pub model: PathBuf,
}

#[derive(Debug, Clone)]
pub enum LiveAudioSamples {
    Float32(Vec<f32>),
    Pcm16(Vec<i16>),
}

#[derive(Debug, Clone)]
pub struct LiveAudioPacket {
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub samples: LiveAudioSamples,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StreamingSttProvider {
    DeepgramFlux,
    AssemblyAi,
}

impl StreamingSttProvider {
    pub fn id(self) -> &'static str {
        match self {
            Self::DeepgramFlux => "deepgram_flux_streaming",
            Self::AssemblyAi => "assemblyai_streaming",
        }
    }
}

type ProviderSocket = WebSocket<MaybeTlsStream<TcpStream>>;

pub struct StreamingDictationSession {
    provider: StreamingSttProvider,
    socket: ProviderSocket,
    normalizer: Pcm16Normalizer,
    segments: Vec<TranscriptSegment>,
    language: Option<String>,
}

impl StreamingDictationSession {
    pub fn provider(&self) -> StreamingSttProvider {
        self.provider
    }

    /// Normalize one live capture packet to mono PCM16 at 16 kHz and send it immediately.
    pub fn push_audio(&mut self, packet: LiveAudioPacket) -> Result<(), ProviderError> {
        let pcm = self.normalizer.normalize(packet)?;
        if pcm.is_empty() {
            return Ok(());
        }
        let bytes = pcm
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect::<Vec<_>>();
        self.socket
            .send(Message::Binary(bytes.into()))
            .map_err(stream_error)
    }

    /// Finalize the live stream after key release and return the provider's completed turns.
    pub fn finish(mut self) -> Result<TranscriptionResult, ProviderError> {
        let control = match self.provider {
            StreamingSttProvider::DeepgramFlux => json!({"type":"Finalize"}),
            StreamingSttProvider::AssemblyAi => json!({"type":"Terminate"}),
        };
        self.socket
            .send(Message::Text(control.to_string().into()))
            .map_err(stream_error)?;
        set_socket_timeout(&mut self.socket, Duration::from_secs(3))?;
        loop {
            match self.socket.read() {
                Ok(Message::Text(text)) => {
                    let terminal = self.consume_message(&text)?;
                    if terminal {
                        break;
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(tungstenite::Error::Io(error))
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    break
                }
                Err(error) => return Err(stream_error(error)),
            }
        }
        let _ = self.socket.close(None);
        if self.segments.is_empty() {
            return Err(ProviderError::new(
                ProviderFailure::Service,
                format!("{} returned no finalized transcript", self.provider.id()),
            ));
        }
        let text = self
            .segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        Ok(TranscriptionResult {
            text,
            language: self.language,
            segments: self.segments,
            provider: self.provider.id().into(),
            diarization: false,
        })
    }

    fn consume_message(&mut self, text: &str) -> Result<bool, ProviderError> {
        let value: Value = serde_json::from_str(text).map_err(|error| {
            ProviderError::new(
                ProviderFailure::Service,
                format!("streaming provider returned invalid JSON: {error}"),
            )
        })?;
        match self.provider {
            StreamingSttProvider::DeepgramFlux => {
                self.language = self
                    .language
                    .take()
                    .or_else(|| value["language"].as_str().map(str::to_owned));
                if value["type"] == "TurnInfo"
                    && matches!(
                        value["event"].as_str(),
                        Some("EndOfTurn") | Some("EagerEndOfTurn")
                    )
                {
                    push_stream_segment(
                        &mut self.segments,
                        value["transcript"].as_str(),
                        seconds_to_ms(value["start"].as_f64()),
                        seconds_to_ms(
                            value["start"]
                                .as_f64()
                                .zip(value["duration"].as_f64())
                                .map(|(start, duration)| start + duration),
                        ),
                    );
                }
                Ok(value["type"] == "Metadata" || value["type"] == "CloseStream")
            }
            StreamingSttProvider::AssemblyAi => {
                if value["type"] == "Turn" && value["end_of_turn"].as_bool() == Some(true) {
                    let words = value["words"].as_array();
                    let start = words
                        .and_then(|words| words.first())
                        .and_then(|word| word["start"].as_u64())
                        .unwrap_or(0);
                    let end = words
                        .and_then(|words| words.last())
                        .and_then(|word| word["end"].as_u64())
                        .unwrap_or(start);
                    push_stream_segment(
                        &mut self.segments,
                        value["transcript"].as_str(),
                        start,
                        end,
                    );
                }
                Ok(value["type"] == "Termination")
            }
        }
    }
}

struct Pcm16Normalizer {
    sample_rate_hz: u32,
    channels: u16,
    previous: Option<f32>,
    input_index: u64,
    next_output_position: f64,
}

impl Pcm16Normalizer {
    fn new(sample_rate_hz: u32, channels: u16) -> Result<Self, ProviderError> {
        if sample_rate_hz == 0 || channels == 0 {
            return Err(ProviderError::new(
                ProviderFailure::Unavailable,
                "live audio sample rate and channel count must be non-zero",
            ));
        }
        Ok(Self {
            sample_rate_hz,
            channels,
            previous: None,
            input_index: 0,
            next_output_position: 0.0,
        })
    }

    fn normalize(&mut self, packet: LiveAudioPacket) -> Result<Vec<i16>, ProviderError> {
        if packet.sample_rate_hz != self.sample_rate_hz || packet.channels != self.channels {
            return Err(ProviderError::new(
                ProviderFailure::Unavailable,
                "live audio format changed during streaming dictation",
            ));
        }
        let samples = match packet.samples {
            LiveAudioSamples::Float32(samples) => samples,
            LiveAudioSamples::Pcm16(samples) => samples
                .into_iter()
                .map(|sample| sample as f32 / i16::MAX as f32)
                .collect(),
        };
        if samples.len() % self.channels as usize != 0 {
            return Err(ProviderError::new(
                ProviderFailure::Unavailable,
                "live audio packet is not channel aligned",
            ));
        }
        let mono = samples
            .chunks_exact(self.channels as usize)
            .map(|frame| frame.iter().copied().sum::<f32>() / self.channels as f32)
            .collect::<Vec<_>>();
        let mut output = Vec::new();
        let step = self.sample_rate_hz as f64 / 16_000.0;
        for sample in mono {
            let index = self.input_index as f64;
            if let Some(previous) = self.previous {
                while self.next_output_position <= index {
                    let fraction = (self.next_output_position - (index - 1.0)).clamp(0.0, 1.0);
                    let interpolated = previous + (sample - previous) * fraction as f32;
                    output.push(float_to_pcm16(interpolated));
                    self.next_output_position += step;
                }
            } else {
                output.push(float_to_pcm16(sample));
                self.next_output_position += step;
            }
            self.previous = Some(sample);
            self.input_index += 1;
        }
        Ok(output)
    }
}

#[derive(Debug)]
pub struct ProviderError {
    pub failure: ProviderFailure,
    pub message: String,
}

impl ProviderError {
    fn new(failure: ProviderFailure, message: impl Into<String>) -> Self {
        Self {
            failure,
            message: message.into(),
        }
    }
}

/// A blocking client keeps provider work outside Tauri's UI thread without requiring a second
/// runtime. Callers should run these methods in a worker thread or `spawn_blocking`.
pub struct SpeechProviders {
    client: Client,
    endpoints: ProviderEndpoints,
    credentials: ProviderCredentials,
    assembly_poll_interval: Duration,
    assembly_deadline: Duration,
    dictation_request_timeout: Duration,
    dictation_assembly_deadline: Duration,
    dictation_correction_timeout: Duration,
}

impl SpeechProviders {
    pub fn new(credentials: ProviderCredentials) -> CoreResult<Self> {
        Self::with_endpoints(credentials, ProviderEndpoints::default())
    }

    pub fn with_endpoints(
        credentials: ProviderCredentials,
        endpoints: ProviderEndpoints,
    ) -> CoreResult<Self> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(45))
            .user_agent(concat!("Murmur/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(network_core_error)?;
        Ok(Self {
            client,
            endpoints,
            credentials,
            assembly_poll_interval: Duration::from_secs(1),
            assembly_deadline: Duration::from_secs(600),
            dictation_request_timeout: DICTATION_HOSTED_ATTEMPT_TIMEOUT,
            dictation_assembly_deadline: DICTATION_ASSEMBLY_DEADLINE,
            dictation_correction_timeout: DICTATION_CORRECTION_TIMEOUT,
        })
    }

    #[cfg(test)]
    fn with_polling(mut self, interval: Duration, deadline: Duration) -> Self {
        self.assembly_poll_interval = interval;
        self.assembly_deadline = deadline;
        self
    }

    #[cfg(test)]
    fn with_dictation_timeouts(
        mut self,
        request: Duration,
        assembly_deadline: Duration,
        correction: Duration,
    ) -> Self {
        self.dictation_request_timeout = request;
        self.dictation_assembly_deadline = assembly_deadline;
        self.dictation_correction_timeout = correction;
        self
    }

    fn request_timeout(&self, kind: SessionKind) -> Option<Duration> {
        (kind == SessionKind::Dictation).then_some(self.dictation_request_timeout)
    }

    fn assembly_timeout(&self, kind: SessionKind) -> Duration {
        match kind {
            SessionKind::Dictation => self.dictation_assembly_deadline,
            SessionKind::Meeting => self.assembly_deadline,
        }
    }

    /// Run the documented provider order. Operational failures rotate immediately, except a
    /// timeout, which gets one retry on the same provider. Transcript quality never triggers a
    /// rotation here.
    pub fn transcribe_with_fallback(
        &self,
        kind: SessionKind,
        audio: &AudioRequest,
        local: Option<&LocalWhisperConfig>,
    ) -> TranscriptionOutcome {
        self.transcribe_with_fallback_observed(kind, audio, local, |_| {})
    }

    /// The callback runs synchronously immediately before every hosted request or local process
    /// attempt. UI callers can publish the Processing Mode before local STT begins rather than
    /// discovering the switch after transcription completes.
    pub fn transcribe_with_fallback_observed(
        &self,
        kind: SessionKind,
        audio: &AudioRequest,
        local: Option<&LocalWhisperConfig>,
        mut on_attempt: impl FnMut(ProviderAttemptStarted),
    ) -> TranscriptionOutcome {
        let mut candidates = match kind {
            SessionKind::Dictation => vec![
                SttProvider::DeepgramFlux,
                SttProvider::GroqWhisper,
                SttProvider::AssemblyAi,
            ],
            SessionKind::Meeting => vec![
                SttProvider::DeepgramNova3,
                SttProvider::GroqWhisper,
                SttProvider::AssemblyAi,
            ],
        };
        candidates.retain(|provider| self.provider_configured(*provider));
        if local.is_some() {
            candidates.push(SttProvider::LocalWhisper);
        }

        let mut attempts = Vec::new();
        for (provider_index, provider) in candidates.into_iter().enumerate() {
            let tries = if provider.is_local() { 1 } else { 2 };
            for attempt in 0..tries {
                let mode = if provider.is_local() {
                    ProcessingMode::Local
                } else if provider_index == 0 {
                    ProcessingMode::Hosted
                } else {
                    ProcessingMode::HostedFallback
                };
                on_attempt(ProviderAttemptStarted {
                    provider: provider.id().into(),
                    mode,
                    attempt: attempt + 1,
                });
                let result = match provider {
                    SttProvider::DeepgramFlux | SttProvider::DeepgramNova3 => {
                        self.transcribe_deepgram(provider, audio, kind)
                    }
                    SttProvider::AssemblyAi => self.transcribe_assemblyai(audio, kind),
                    SttProvider::GroqWhisper => self.transcribe_groq(audio, kind),
                    SttProvider::LocalWhisper => local
                        .ok_or_else(|| {
                            ProviderError::new(
                                ProviderFailure::Unavailable,
                                "local Whisper is not configured",
                            )
                        })
                        .and_then(|config| transcribe_local_whisper(config, audio)),
                };
                match result {
                    Ok(transcript) => {
                        return TranscriptionOutcome::Completed {
                            transcript,
                            attempts,
                        }
                    }
                    Err(error) => {
                        let retry_timeout = error.failure == ProviderFailure::Timeout
                            && attempt == 0
                            && !provider.is_local();
                        attempts.push(ProviderAttemptFailure {
                            provider: provider.id().into(),
                            failure: error.failure,
                            message: error.message,
                        });
                        if retry_timeout {
                            continue;
                        }
                        break;
                    }
                }
            }
        }

        if kind == SessionKind::Meeting {
            TranscriptionOutcome::PendingEnhancement { attempts }
        } else {
            TranscriptionOutcome::Failed { attempts }
        }
    }

    fn provider_configured(&self, provider: SttProvider) -> bool {
        match provider {
            SttProvider::DeepgramFlux | SttProvider::DeepgramNova3 => self
                .credentials
                .deepgram
                .as_deref()
                .is_some_and(|key| !key.trim().is_empty()),
            SttProvider::AssemblyAi => self
                .credentials
                .assemblyai
                .as_deref()
                .is_some_and(|key| !key.trim().is_empty()),
            SttProvider::GroqWhisper => self
                .credentials
                .groq
                .as_deref()
                .is_some_and(|key| !key.trim().is_empty()),
            SttProvider::LocalWhisper => true,
        }
    }

    /// Open the first configured live dictation provider before capture begins. If the first
    /// handshake fails, AssemblyAI is tried before the caller falls back to finalized audio.
    pub fn start_streaming_dictation(
        &self,
        sample_rate_hz: u32,
        channels: u16,
    ) -> Result<StreamingDictationSession, ProviderError> {
        let normalizer = Pcm16Normalizer::new(sample_rate_hz, channels)?;
        let mut failures = Vec::new();
        if let Some(key) = self.credentials.deepgram.as_deref() {
            match connect_provider_stream(StreamingSttProvider::DeepgramFlux, &self.endpoints, key)
            {
                Ok(socket) => {
                    return Ok(StreamingDictationSession {
                        provider: StreamingSttProvider::DeepgramFlux,
                        socket,
                        normalizer,
                        segments: Vec::new(),
                        language: None,
                    })
                }
                Err(error) => failures.push(error.message),
            }
        }
        if let Some(key) = self.credentials.assemblyai.as_deref() {
            match connect_provider_stream(StreamingSttProvider::AssemblyAi, &self.endpoints, key) {
                Ok(socket) => {
                    return Ok(StreamingDictationSession {
                        provider: StreamingSttProvider::AssemblyAi,
                        socket,
                        normalizer,
                        segments: Vec::new(),
                        language: None,
                    })
                }
                Err(error) => failures.push(error.message),
            }
        }
        Err(ProviderError::new(
            ProviderFailure::Unavailable,
            if failures.is_empty() {
                "no streaming dictation provider is configured".into()
            } else {
                format!(
                    "streaming provider handshakes failed: {}",
                    failures.join("; ")
                )
            },
        ))
    }

    pub fn validate_provider(&self, provider: &str) -> Result<(), ProviderError> {
        let (url, request) = match provider {
            "deepgram" => {
                let key = required_key(&self.credentials.deepgram, "Deepgram")?;
                let url = format!(
                    "{}/v1/listen?model=nova-3&punctuate=true",
                    self.endpoints.deepgram
                );
                let request = self
                    .client
                    .post(&url)
                    .header(AUTHORIZATION, format!("Token {key}"))
                    .header(CONTENT_TYPE, "audio/wav")
                    .body(deepgram_validation_audio());
                (url, request)
            }
            "assemblyai" => {
                let key = required_key(&self.credentials.assemblyai, "AssemblyAI")?;
                let url = format!("{}/v2/account", self.endpoints.assemblyai);
                let request = self.client.get(&url).header(AUTHORIZATION, key);
                (url, request)
            }
            "groq" => {
                let key = required_key(&self.credentials.groq, "Groq")?;
                let url = format!("{}/openai/v1/models", self.endpoints.groq);
                let request = self
                    .client
                    .get(&url)
                    .header(AUTHORIZATION, format!("Bearer {key}"));
                (url, request)
            }
            _ => {
                return Err(ProviderError::new(
                    ProviderFailure::Unavailable,
                    format!("unknown provider: {provider}"),
                ))
            }
        };
        let response = request.send().map_err(classify_reqwest)?;
        checked_response(response, &url).map(|_| ())
    }

    fn transcribe_deepgram(
        &self,
        provider: SttProvider,
        audio: &AudioRequest,
        kind: SessionKind,
    ) -> Result<TranscriptionResult, ProviderError> {
        let diarize = kind == SessionKind::Meeting;
        let key = required_key(&self.credentials.deepgram, "Deepgram")?;
        let model = match provider {
            SttProvider::DeepgramFlux => "flux-general-en",
            SttProvider::DeepgramNova3 => "nova-3",
            _ => unreachable!(),
        };
        // Deepgram accepts complete audio at /v1/listen. The capture pipeline may use the same
        // credentials with /v2/listen for incremental Flux results, but the finalized Raw
        // Transcript always comes through this deterministic complete-audio request.
        let url = format!(
            "{}/v1/listen?model={model}&detect_language=true&punctuate=true&utterances=true&diarize={diarize}",
            self.endpoints.deepgram
        );
        let mut request = self
            .client
            .post(&url)
            .header(AUTHORIZATION, format!("Token {key}"))
            .header(CONTENT_TYPE, &audio.mime_type)
            .body(audio.bytes.clone());
        if let Some(timeout) = self.request_timeout(kind) {
            request = request.timeout(timeout);
        }
        let response = request.send().map_err(classify_reqwest)?;
        let value: Value = checked_response(response, &url)?
            .json()
            .map_err(parse_provider_response)?;
        parse_deepgram(value, provider.id())
    }

    fn transcribe_assemblyai(
        &self,
        audio: &AudioRequest,
        kind: SessionKind,
    ) -> Result<TranscriptionResult, ProviderError> {
        let started = Instant::now();
        let deadline = self.assembly_timeout(kind);
        let diarize = kind == SessionKind::Meeting;
        let key = required_key(&self.credentials.assemblyai, "AssemblyAI")?;
        let upload_url = format!("{}/v2/upload", self.endpoints.assemblyai);
        let mut upload_request = self
            .client
            .post(&upload_url)
            .header(AUTHORIZATION, key)
            .header(CONTENT_TYPE, "application/octet-stream")
            .body(audio.bytes.clone());
        if let Some(timeout) = self.assembly_request_timeout(kind, started, deadline)? {
            upload_request = upload_request.timeout(timeout);
        }
        let upload: Value = checked_response(
            upload_request.send().map_err(classify_reqwest)?,
            &upload_url,
        )?
        .json()
        .map_err(parse_provider_response)?;
        let audio_url = upload["upload_url"].as_str().ok_or_else(|| {
            ProviderError::new(
                ProviderFailure::Service,
                "AssemblyAI upload response omitted upload_url",
            )
        })?;
        let transcript_url = format!("{}/v2/transcript", self.endpoints.assemblyai);
        let body = json!({
            "audio_url": audio_url,
            "language_detection": true,
            "speaker_labels": diarize,
            "speech_models": ["universal-3-pro", "universal-2"]
        });
        let mut create_request = self
            .client
            .post(&transcript_url)
            .header(AUTHORIZATION, key)
            .json(&body);
        if let Some(timeout) = self.assembly_request_timeout(kind, started, deadline)? {
            create_request = create_request.timeout(timeout);
        }
        let created: Value = checked_response(
            create_request.send().map_err(classify_reqwest)?,
            &transcript_url,
        )?
        .json()
        .map_err(parse_provider_response)?;
        let id = created["id"].as_str().ok_or_else(|| {
            ProviderError::new(
                ProviderFailure::Service,
                "AssemblyAI response omitted transcript id",
            )
        })?;
        let poll_url = format!("{}/v2/transcript/{id}", self.endpoints.assemblyai);
        loop {
            if started.elapsed() >= deadline {
                return Err(assembly_timeout_error());
            }
            let mut poll_request = self.client.get(&poll_url).header(AUTHORIZATION, key);
            if let Some(timeout) = self.assembly_request_timeout(kind, started, deadline)? {
                poll_request = poll_request.timeout(timeout);
            }
            let value: Value =
                checked_response(poll_request.send().map_err(classify_reqwest)?, &poll_url)?
                    .json()
                    .map_err(parse_provider_response)?;
            match value["status"].as_str() {
                Some("completed") => return parse_assemblyai(value),
                Some("error") => {
                    return Err(ProviderError::new(
                        ProviderFailure::Service,
                        value["error"]
                            .as_str()
                            .unwrap_or("AssemblyAI transcription failed"),
                    ))
                }
                _ => {
                    let sleep = if kind == SessionKind::Dictation {
                        deadline
                            .saturating_sub(started.elapsed())
                            .min(self.assembly_poll_interval)
                    } else {
                        self.assembly_poll_interval
                    };
                    if sleep.is_zero() {
                        return Err(assembly_timeout_error());
                    }
                    thread::sleep(sleep);
                }
            }
        }
    }

    fn assembly_request_timeout(
        &self,
        kind: SessionKind,
        started: Instant,
        deadline: Duration,
    ) -> Result<Option<Duration>, ProviderError> {
        if kind != SessionKind::Dictation {
            return Ok(self.request_timeout(kind));
        }
        deadline
            .checked_sub(started.elapsed())
            .filter(|remaining| !remaining.is_zero())
            .map(Some)
            .ok_or_else(assembly_timeout_error)
    }

    fn transcribe_groq(
        &self,
        audio: &AudioRequest,
        kind: SessionKind,
    ) -> Result<TranscriptionResult, ProviderError> {
        let key = required_key(&self.credentials.groq, "Groq")?;
        let url = format!("{}/openai/v1/audio/transcriptions", self.endpoints.groq);
        let file = multipart::Part::bytes(audio.bytes.clone())
            .file_name("recording.wav")
            .mime_str(&audio.mime_type)
            .map_err(|error| ProviderError::new(ProviderFailure::Service, error.to_string()))?;
        let form = multipart::Form::new()
            .part("file", file)
            .text("model", GROQ_WHISPER_MODEL)
            .text("response_format", "verbose_json");
        let mut request = self
            .client
            .post(&url)
            .header(AUTHORIZATION, format!("Bearer {key}"))
            .multipart(form);
        if let Some(timeout) = self.request_timeout(kind) {
            request = request.timeout(timeout);
        }
        let value: Value = checked_response(request.send().map_err(classify_reqwest)?, &url)?
            .json()
            .map_err(parse_provider_response)?;
        parse_groq(value)
    }

    pub fn correct_dictation(&self, request: &CorrectionRequest) -> CorrectionOutcome {
        match self.complete_groq_chat_with_timeout(
            GROQ_CORRECTION_MODEL,
            correction_messages(request),
            Some(self.dictation_correction_timeout),
        ) {
            Ok(text) if !text.trim().is_empty() => CorrectionOutcome {
                text,
                corrected: true,
                provider: Some(GROQ_CORRECTION_MODEL.into()),
                error: None,
            },
            Ok(_) => CorrectionOutcome::raw(&request.raw_transcript, "empty correction response"),
            Err(error) => CorrectionOutcome::raw(&request.raw_transcript, &error.message),
        }
    }

    pub fn transform_text(
        &self,
        input: &str,
        action: ExplicitTransformAction,
    ) -> Result<String, ProviderError> {
        if input.trim().is_empty() {
            return Err(ProviderError::new(
                ProviderFailure::Unavailable,
                "text transform input is empty",
            ));
        }
        self.complete_groq_chat(GROQ_CORRECTION_MODEL, transform_messages(input, action))
            .and_then(|text| {
                if text.trim().is_empty() {
                    Err(ProviderError::new(
                        ProviderFailure::Service,
                        "text transform response was empty",
                    ))
                } else {
                    Ok(text)
                }
            })
    }

    /// Generate timestamp-preserving chunk notes, then synthesize the fixed brief. Failure is
    /// returned to the queue owner; meetings never silently lose the pending enhancement.
    pub fn create_meeting_brief(
        &self,
        request: &MeetingBriefRequest,
    ) -> Result<GeneratedMeetingBrief, ProviderError> {
        if request.segments.is_empty() {
            return Err(ProviderError::new(
                ProviderFailure::Unavailable,
                "meeting transcript has no segments",
            ));
        }
        let chunks = timestamped_chunks(&request.segments, 24_000);
        let mut notes = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            let messages = meeting_chunk_messages(chunk);
            notes.push(self.complete_groq_chat(GROQ_MEETING_MODEL, messages)?);
        }
        let notes = self.collapse_meeting_notes(notes)?;
        let mut messages = meeting_synthesis_messages(request.title.as_deref(), &notes);
        let mut markdown = self.complete_groq_chat(GROQ_MEETING_MODEL, messages.clone())?;
        ensure_optional_meeting_brief_sections(&mut markdown);
        if let Err(reason) = validate_meeting_brief(&markdown, &request.segments) {
            messages.push(json!({"role":"assistant","content":markdown}));
            messages.push(json!({"role":"user","content":format!("Repair the brief without adding facts. {MEETING_GROUNDING_RULES} {reason}. Return the complete fixed brief only.")}));
            markdown = self.complete_groq_chat(GROQ_MEETING_MODEL, messages)?;
            ensure_optional_meeting_brief_sections(&mut markdown);
            validate_meeting_brief(&markdown, &request.segments).map_err(|reason| {
                ProviderError::new(
                    ProviderFailure::Service,
                    format!("meeting brief failed citation validation after repair: {reason}"),
                )
            })?;
        }
        Ok(GeneratedMeetingBrief {
            markdown,
            provider: GROQ_MEETING_MODEL.into(),
            chunk_count: notes.len(),
        })
    }

    fn collapse_meeting_notes(&self, mut notes: Vec<String>) -> Result<Vec<String>, ProviderError> {
        const FINAL_NOTES_LIMIT: usize = 72_000;
        while joined_length(&notes) > FINAL_NOTES_LIMIT {
            let previous_len = notes.len();
            let batches = text_batches(notes, 24_000);
            let mut reduced = Vec::with_capacity(batches.len());
            for batch in batches {
                reduced.push(
                    self.complete_groq_chat(GROQ_MEETING_MODEL, meeting_reduction_messages(batch))?,
                );
            }
            if reduced.len() >= previous_len {
                return Err(ProviderError::new(
                    ProviderFailure::Service,
                    "meeting notes could not be reduced within the synthesis context limit",
                ));
            }
            notes = reduced;
        }
        Ok(notes)
    }

    fn complete_groq_chat(
        &self,
        model: &str,
        messages: Vec<Value>,
    ) -> Result<String, ProviderError> {
        self.complete_groq_chat_with_timeout(model, messages, None)
    }

    fn complete_groq_chat_with_timeout(
        &self,
        model: &str,
        messages: Vec<Value>,
        timeout: Option<Duration>,
    ) -> Result<String, ProviderError> {
        let key = required_key(&self.credentials.groq, "Groq")?;
        let url = format!("{}/openai/v1/chat/completions", self.endpoints.groq);
        let mut request = self
            .client
            .post(&url)
            .header(AUTHORIZATION, format!("Bearer {key}"))
            .json(&json!({
                "model": model,
                "messages": messages,
                "temperature": 0.1
            }));
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        let response = checked_response(request.send().map_err(classify_reqwest)?, &url)?;
        let value: Value = response.json().map_err(parse_provider_response)?;
        value["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| {
                ProviderError::new(
                    ProviderFailure::Service,
                    "Groq response omitted message content",
                )
            })
    }
}

#[derive(Debug, Clone)]
pub struct CorrectionRequest {
    pub raw_transcript: String,
    pub application: Option<String>,
    pub window_title: Option<String>,
    pub selected_text: Option<String>,
    pub surrounding_text: Option<String>,
    pub personal_vocabulary: Vec<VocabularyReplacement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VocabularyReplacement {
    pub heard: String,
    pub replacement: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExplicitTransformAction {
    Rewrite,
    TranslateEnglish,
    TranslateVietnamese,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorrectionOutcome {
    pub text: String,
    pub corrected: bool,
    pub provider: Option<String>,
    pub error: Option<String>,
}

impl CorrectionOutcome {
    fn raw(raw: &str, error: &str) -> Self {
        Self {
            text: raw.into(),
            corrected: false,
            provider: None,
            error: Some(error.into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MeetingBriefRequest {
    pub title: Option<String>,
    pub segments: Vec<TranscriptSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedMeetingBrief {
    pub markdown: String,
    pub provider: String,
    pub chunk_count: usize,
}

fn correction_messages(request: &CorrectionRequest) -> Vec<Value> {
    let vocabulary = request
        .personal_vocabulary
        .iter()
        .map(|item| format!("{} => {}", item.heard, item.replacement))
        .collect::<Vec<_>>()
        .join("\n");
    let context = format!(
        "Application: {}\nWindow: {}\nSelected text: {}\nSurrounding text: {}\nPersonal Vocabulary replacements:\n{}",
        request.application.as_deref().unwrap_or("unknown"),
        request.window_title.as_deref().unwrap_or("unknown"),
        request.selected_text.as_deref().unwrap_or(""),
        request.surrounding_text.as_deref().unwrap_or(""),
        vocabulary
    );
    vec![
        json!({"role":"system","content":"Lightly correct the dictation for punctuation, filler words, obvious grammar, and the supplied Personal Vocabulary. Preserve meaning and preserve English, Vietnamese, and mixed-language phrases. Do not translate or aggressively rewrite. Return only the corrected dictation."}),
        json!({"role":"user","content":format!("Context:\n{context}\n\nRaw Transcript:\n{}", request.raw_transcript)}),
    ]
}

fn transform_messages(input: &str, action: ExplicitTransformAction) -> Vec<Value> {
    let instruction = match action {
        ExplicitTransformAction::Rewrite => {
            "Rewrite the supplied text for clarity while preserving its facts, intent, names, and language. Return only the rewritten text."
        }
        ExplicitTransformAction::TranslateEnglish => {
            "Translate the supplied text into English. Preserve names, facts, formatting, and meaning. Return only the translation."
        }
        ExplicitTransformAction::TranslateVietnamese => {
            "Translate the supplied text into Vietnamese. Preserve names, facts, formatting, and meaning. Return only the translation."
        }
    };
    vec![
        json!({"role":"system","content":instruction}),
        json!({"role":"user","content":input}),
    ]
}

fn meeting_chunk_messages(chunk: String) -> Vec<Value> {
    vec![
        json!({"role":"system","content":format!("Extract transcript-supported facts from this timestamped meeting chunk. Preserve timestamps. Do not infer names, decisions, or commitments. {MEETING_GROUNDING_RULES} Return concise markdown notes.")}),
        json!({"role":"user","content":chunk}),
    ]
}

fn meeting_reduction_messages(notes: Vec<String>) -> Vec<Value> {
    vec![
        json!({"role":"system","content":format!("Merge these timestamped meeting notes without adding facts. Preserve every decision, action, uncertainty, explicit speaker label, and supporting timestamp. {MEETING_GROUNDING_RULES} Return concise markdown notes.")}),
        json!({"role":"user","content":notes.join("\n\n---\n\n")}),
    ]
}

fn meeting_synthesis_messages(title: Option<&str>, notes: &[String]) -> Vec<Value> {
    let prompt = format!(
        "Organizational metadata, not evidence:\nMeeting title: {}\n\nTimestamped chunk notes, the only evidence:\n\n{}",
        title.unwrap_or("Untitled meeting"),
        notes.join("\n\n---\n\n")
    );
    vec![
        json!({"role":"system","content":format!("Create a fixed Meeting Brief in markdown with these headings: Overview, Detailed topics, Decisions, Action items, Open questions, Notable cited moments, Possible follow-ups. State only transcript-supported facts. Every decision and action item must contain a supporting [HH:MM:SS] timestamp. Put uncertain interpretations only under Possible follow-ups. Do not translate English or Vietnamese speech. {MEETING_GROUNDING_RULES}")}),
        json!({"role":"user","content":prompt}),
    ]
}

fn timestamped_chunks(segments: &[TranscriptSegment], max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for segment in segments {
        let speaker = segment.speaker.as_deref().unwrap_or("Speaker");
        let line = format!(
            "[{}] {speaker}: {}\n",
            format_timestamp(segment.start_ms),
            segment.text
        );
        if !current.is_empty() && current.len() + line.len() > max_chars {
            chunks.push(current);
            current = String::new();
        }
        current.push_str(&line);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn text_batches(items: Vec<String>, max_chars: usize) -> Vec<Vec<String>> {
    let mut batches = Vec::new();
    let mut batch = Vec::new();
    let mut length = 0;
    for item in items {
        let additional = item.len() + usize::from(!batch.is_empty()) * 5;
        if !batch.is_empty() && length + additional > max_chars {
            batches.push(batch);
            batch = Vec::new();
            length = 0;
        }
        length += item.len() + usize::from(!batch.is_empty()) * 5;
        batch.push(item);
    }
    if !batch.is_empty() {
        batches.push(batch);
    }
    batches
}

fn joined_length(items: &[String]) -> usize {
    items.iter().map(String::len).sum::<usize>() + items.len().saturating_sub(1) * 5
}

fn ensure_optional_meeting_brief_sections(markdown: &mut String) {
    let has_follow_ups = markdown.lines().any(|line| {
        line.trim_start_matches('#')
            .trim()
            .eq_ignore_ascii_case("possible follow-ups")
    });
    if has_follow_ups {
        return;
    }
    if !markdown.ends_with('\n') {
        markdown.push('\n');
    }
    markdown.push_str("\n# Possible follow-ups\n");
}

pub(crate) fn is_empty_brief_entry(line: &str) -> bool {
    let normalized = line
        .trim_start_matches(['-', '*'])
        .trim()
        .trim_matches('*')
        .trim()
        .to_ascii_lowercase();
    normalized == "none"
        || normalized == "none."
        || normalized.starts_with("no decision")
        || normalized.starts_with("no explicit decision")
        || normalized.starts_with("no action item")
        || normalized.starts_with("no explicit action item")
}

fn validate_meeting_brief(
    markdown: &str,
    transcript_segments: &[TranscriptSegment],
) -> Result<(), String> {
    let required = [
        "overview",
        "detailed topics",
        "decisions",
        "action items",
        "open questions",
        "notable cited moments",
        "possible follow-ups",
    ];
    let mut sections = std::collections::HashMap::<String, Vec<&str>>::new();
    let mut current = None;
    for line in markdown.lines() {
        let candidate = line.trim_start_matches('#').trim().to_ascii_lowercase();
        if required.contains(&candidate.as_str()) {
            sections.entry(candidate.clone()).or_default();
            current = Some(candidate);
            continue;
        }
        if let Some(section) = &current {
            if !line.trim().is_empty() {
                sections
                    .entry(section.clone())
                    .or_default()
                    .push(line.trim());
            }
        }
    }
    for name in required {
        if !sections.contains_key(name) {
            return Err(format!("missing {name} section"));
        }
    }
    for name in ["decisions", "action items"] {
        for line in &sections[name] {
            if is_empty_brief_entry(line) {
                continue;
            }
            if !contains_timestamp(line) {
                return Err(format!("{name} entry lacks a [HH:MM:SS] citation"));
            }
            for timestamp in timestamps(line) {
                if !transcript_segments
                    .iter()
                    .any(|segment| timestamp >= segment.start_ms && timestamp <= segment.end_ms)
                {
                    return Err(format!(
                        "{name} entry cites a timestamp outside the transcript"
                    ));
                }
            }
        }
    }
    Ok(())
}

fn timestamps(value: &str) -> Vec<u64> {
    value
        .as_bytes()
        .windows(10)
        .filter_map(|window| {
            (window[0] == b'['
                && window[3] == b':'
                && window[6] == b':'
                && window[9] == b']'
                && [1, 2, 4, 5, 7, 8]
                    .iter()
                    .all(|index| window[*index].is_ascii_digit()))
            .then(|| {
                let hours = ((window[1] - b'0') * 10 + window[2] - b'0') as u64;
                let minutes = ((window[4] - b'0') * 10 + window[5] - b'0') as u64;
                let seconds = ((window[7] - b'0') * 10 + window[8] - b'0') as u64;
                (hours * 3_600 + minutes * 60 + seconds) * 1_000
            })
        })
        .collect()
}

fn contains_timestamp(value: &str) -> bool {
    value.as_bytes().windows(10).any(|window| {
        window[0] == b'['
            && window[3] == b':'
            && window[6] == b':'
            && window[9] == b']'
            && [1, 2, 4, 5, 7, 8]
                .iter()
                .all(|index| window[*index].is_ascii_digit())
    })
}

fn format_timestamp(milliseconds: u64) -> String {
    let seconds = milliseconds / 1_000;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3_600,
        (seconds / 60) % 60,
        seconds % 60
    )
}

fn parse_deepgram(value: Value, provider: &str) -> Result<TranscriptionResult, ProviderError> {
    let alternative = &value["results"]["channels"][0]["alternatives"][0];
    let text = alternative["transcript"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let words = alternative["words"].as_array().cloned().unwrap_or_default();
    let segments = words
        .into_iter()
        .filter_map(|word| {
            let text = word["punctuated_word"]
                .as_str()
                .or_else(|| word["word"].as_str())?;
            Some(TranscriptSegment {
                start_ms: seconds_to_ms(word["start"].as_f64()),
                end_ms: seconds_to_ms(word["end"].as_f64()),
                text: text.into(),
                speaker: word["speaker"]
                    .as_u64()
                    .map(|id| format!("Speaker {}", id + 1)),
                confidence: word["confidence"].as_f64().map(|value| value as f32),
            })
        })
        .collect::<Vec<_>>();
    if text.trim().is_empty() && segments.is_empty() {
        return Err(ProviderError::new(
            ProviderFailure::Service,
            "Deepgram response contained no transcript",
        ));
    }
    Ok(TranscriptionResult {
        text,
        language: value["results"]["channels"][0]["detected_language"]
            .as_str()
            .map(str::to_owned),
        diarization: segments.iter().any(|segment| segment.speaker.is_some()),
        segments,
        provider: provider.into(),
    })
}

fn parse_assemblyai(value: Value) -> Result<TranscriptionResult, ProviderError> {
    let text = value["text"].as_str().unwrap_or_default().to_owned();
    let utterances = value["utterances"].as_array().cloned().unwrap_or_default();
    let segments = utterances
        .into_iter()
        .filter_map(|utterance| {
            Some(TranscriptSegment {
                start_ms: utterance["start"].as_u64()?,
                end_ms: utterance["end"].as_u64()?,
                text: utterance["text"].as_str()?.into(),
                speaker: utterance["speaker"]
                    .as_str()
                    .map(|speaker| format!("Speaker {speaker}")),
                confidence: utterance["confidence"].as_f64().map(|value| value as f32),
            })
        })
        .collect::<Vec<_>>();
    if text.trim().is_empty() && segments.is_empty() {
        return Err(ProviderError::new(
            ProviderFailure::Service,
            "AssemblyAI response contained no transcript",
        ));
    }
    Ok(TranscriptionResult {
        text,
        language: value["language_code"].as_str().map(str::to_owned),
        diarization: !segments.is_empty(),
        segments,
        provider: "assemblyai".into(),
    })
}

fn parse_groq(value: Value) -> Result<TranscriptionResult, ProviderError> {
    let text = value["text"].as_str().unwrap_or_default().to_owned();
    let segments = value["segments"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|segment| {
            Some(TranscriptSegment {
                start_ms: seconds_to_ms(segment["start"].as_f64()),
                end_ms: seconds_to_ms(segment["end"].as_f64()),
                text: segment["text"].as_str()?.trim().into(),
                speaker: None,
                confidence: None,
            })
        })
        .collect::<Vec<_>>();
    if text.trim().is_empty() && segments.is_empty() {
        return Err(ProviderError::new(
            ProviderFailure::Service,
            "Groq response contained no transcript",
        ));
    }
    Ok(TranscriptionResult {
        text,
        language: value["language"].as_str().map(str::to_owned),
        segments,
        provider: "groq_whisper_large_v3_turbo".into(),
        diarization: false,
    })
}

fn seconds_to_ms(value: Option<f64>) -> u64 {
    value.map(|value| (value * 1_000.0) as u64).unwrap_or(0)
}

fn connect_provider_stream(
    provider: StreamingSttProvider,
    endpoints: &ProviderEndpoints,
    key: &str,
) -> Result<ProviderSocket, ProviderError> {
    let url = streaming_url(provider, endpoints)?;
    let mut request = url.into_client_request().map_err(|error| {
        ProviderError::new(
            ProviderFailure::Unavailable,
            format!("invalid streaming provider URL: {error}"),
        )
    })?;
    let authorization = match provider {
        StreamingSttProvider::DeepgramFlux => format!("Token {key}"),
        StreamingSttProvider::AssemblyAi => key.to_owned(),
    };
    request.headers_mut().insert(
        tungstenite::http::header::AUTHORIZATION,
        HeaderValue::from_str(&authorization).map_err(|_| {
            ProviderError::new(
                ProviderFailure::Authentication,
                "streaming provider credential contains invalid header characters",
            )
        })?,
    );
    let (mut socket, _) = tungstenite::connect(request).map_err(stream_error)?;
    set_socket_timeout(&mut socket, Duration::from_secs(5))?;
    Ok(socket)
}

fn streaming_url(
    provider: StreamingSttProvider,
    endpoints: &ProviderEndpoints,
) -> Result<String, ProviderError> {
    let base = match provider {
        StreamingSttProvider::DeepgramFlux => endpoints.deepgram.as_str(),
        StreamingSttProvider::AssemblyAi
            if endpoints.assemblyai == "https://api.assemblyai.com" =>
        {
            "https://streaming.assemblyai.com"
        }
        StreamingSttProvider::AssemblyAi => endpoints.assemblyai.as_str(),
    };
    let mut url = url::Url::parse(base).map_err(|error| {
        ProviderError::new(
            ProviderFailure::Unavailable,
            format!("invalid streaming provider base URL: {error}"),
        )
    })?;
    url.set_scheme(match url.scheme() {
        "https" => "wss",
        "http" if cfg!(test) => "ws",
        _ => {
            return Err(ProviderError::new(
                ProviderFailure::Unavailable,
                "streaming providers require WSS",
            ))
        }
    })
    .map_err(|_| ProviderError::new(ProviderFailure::Unavailable, "invalid WebSocket scheme"))?;
    match provider {
        StreamingSttProvider::DeepgramFlux => {
            url.set_path("/v2/listen");
            url.set_query(Some(
                "model=flux-general-en&encoding=linear16&sample_rate=16000&channels=1",
            ));
        }
        StreamingSttProvider::AssemblyAi => {
            url.set_path("/v3/ws");
            url.set_query(Some(
                "sample_rate=16000&speech_model=universal-streaming-multilingual&format_turns=true",
            ));
        }
    }
    Ok(url.into())
}

fn set_socket_timeout(socket: &mut ProviderSocket, timeout: Duration) -> Result<(), ProviderError> {
    let result = match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => stream.set_read_timeout(Some(timeout)),
        MaybeTlsStream::Rustls(stream) => stream.sock.set_read_timeout(Some(timeout)),
        _ => Ok(()),
    };
    result.map_err(|error| ProviderError::new(ProviderFailure::Timeout, error.to_string()))
}

fn stream_error(error: tungstenite::Error) -> ProviderError {
    let failure = match &error {
        tungstenite::Error::Http(response) if matches!(response.status().as_u16(), 401 | 403) => {
            ProviderFailure::Authentication
        }
        tungstenite::Error::Http(response) if response.status().as_u16() == 429 => {
            ProviderFailure::QuotaExhausted
        }
        tungstenite::Error::Io(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) =>
        {
            ProviderFailure::Timeout
        }
        _ => ProviderFailure::Service,
    };
    ProviderError::new(failure, format!("streaming provider failed: {error}"))
}

fn push_stream_segment(
    segments: &mut Vec<TranscriptSegment>,
    text: Option<&str>,
    start_ms: u64,
    end_ms: u64,
) {
    let Some(text) = text.map(str::trim).filter(|text| !text.is_empty()) else {
        return;
    };
    segments.push(TranscriptSegment {
        start_ms,
        end_ms: end_ms.max(start_ms),
        text: text.into(),
        speaker: None,
        confidence: None,
    });
}

fn float_to_pcm16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
}

fn transcribe_local_whisper(
    config: &LocalWhisperConfig,
    audio: &AudioRequest,
) -> Result<TranscriptionResult, ProviderError> {
    if !config.executable.is_file() || !config.model.is_file() {
        return Err(ProviderError::new(
            ProviderFailure::Unavailable,
            "local whisper.cpp executable or model is missing",
        ));
    }
    let source = audio.source_path.as_ref().ok_or_else(|| {
        ProviderError::new(
            ProviderFailure::Unavailable,
            "local Whisper requires an audio file path",
        )
    })?;
    let output_base = std::env::temp_dir().join(format!("murmur-whisper-{}", Uuid::new_v4()));
    let status = Command::new(&config.executable)
        .args(local_whisper_arguments(config, source, &output_base))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|error| ProviderError::new(ProviderFailure::Unavailable, error.to_string()))?;
    if !status.success() {
        return Err(ProviderError::new(
            ProviderFailure::Unavailable,
            format!("whisper.cpp exited with {status}"),
        ));
    }
    let json_path = output_base.with_extension("json");
    let parsed = fs::read_to_string(&json_path)
        .map_err(|error| ProviderError::new(ProviderFailure::Unavailable, error.to_string()))
        .and_then(|text| {
            serde_json::from_str::<Value>(&text)
                .map_err(|error| ProviderError::new(ProviderFailure::Service, error.to_string()))
        });
    let _ = fs::remove_file(json_path);
    let value = parsed?;
    let transcription = value["transcription"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let segments = transcription
        .into_iter()
        .filter_map(|segment| {
            Some(TranscriptSegment {
                start_ms: segment["offsets"]["from"].as_u64()?,
                end_ms: segment["offsets"]["to"].as_u64()?,
                text: segment["text"].as_str()?.trim().into(),
                speaker: None,
                confidence: None,
            })
        })
        .collect::<Vec<_>>();
    let text = segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    if text.is_empty() {
        return Err(ProviderError::new(
            ProviderFailure::Service,
            "local Whisper response contained no transcript",
        ));
    }
    Ok(TranscriptionResult {
        text,
        language: value["result"]["language"].as_str().map(str::to_owned),
        segments,
        provider: "local_whisper_large_v3_turbo_q5_0".into(),
        diarization: false,
    })
}

fn local_whisper_arguments(
    config: &LocalWhisperConfig,
    source: &Path,
    output_base: &Path,
) -> Vec<OsString> {
    [
        OsString::from("-m"),
        config.model.as_os_str().to_owned(),
        OsString::from("-f"),
        source.as_os_str().to_owned(),
        OsString::from("-l"),
        OsString::from("auto"),
        OsString::from("-np"),
        OsString::from("-oj"),
        OsString::from("-of"),
        output_base.as_os_str().to_owned(),
    ]
    .into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDownloadResult {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalWhisperRuntimeInstallResult {
    pub directory: PathBuf,
    pub release: String,
    pub files: Vec<String>,
}

/// Extract the pinned whisper.cpp CUDA runtime. The archive is checksummed by the caller; this
/// function still rejects links, traversal, nested executable paths, and oversized payloads.
pub fn install_local_whisper_runtime_archive(
    archive_path: &Path,
    destination: &Path,
) -> CoreResult<LocalWhisperRuntimeInstallResult> {
    let file = fs::File::open(archive_path)?;
    let mut archive = ZipArchive::new(file).map_err(runtime_archive_error)?;
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(runtime_archive_error)?;
        let path = entry.enclosed_name().ok_or_else(|| {
            CoreError::InvalidInput("whisper.cpp archive contains an unsafe path".into())
        })?;
        let parts = path
            .components()
            .map(|part| part.as_os_str())
            .collect::<Vec<_>>();
        if entry.is_symlink()
            || (!entry.is_dir()
                && (parts.len() != 2 || parts[0] != "Release" || parts[1].is_empty()))
        {
            return Err(CoreError::InvalidInput(
                "whisper.cpp archive contains an unexpected file path".into(),
            ));
        }
    }

    fs::create_dir_all(destination)?;
    let mut installed = Vec::with_capacity(LOCAL_WHISPER_RUNTIME_FILES.len());
    for name in LOCAL_WHISPER_RUNTIME_FILES {
        let archive_name = format!("Release/{name}");
        let mut entry = archive.by_name(&archive_name).map_err(|_| {
            CoreError::InvalidInput(format!(
                "whisper.cpp archive is missing required runtime file {name}"
            ))
        })?;
        if entry.is_dir() || entry.size() == 0 || entry.size() > 16 * 1024 * 1024 {
            return Err(CoreError::InvalidInput(format!(
                "whisper.cpp runtime file {name} has an invalid size"
            )));
        }
        let path = destination.join(name);
        let mut output = fs::File::create(&path)?;
        let copied = std::io::copy(&mut entry.by_ref().take(16 * 1024 * 1024 + 1), &mut output)?;
        if copied != entry.size() || copied > 16 * 1024 * 1024 {
            return Err(CoreError::InvalidInput(format!(
                "whisper.cpp runtime file {name} exceeded its declared size"
            )));
        }
        output.sync_all()?;
        installed.push((*name).to_owned());
    }
    Ok(LocalWhisperRuntimeInstallResult {
        directory: destination.to_path_buf(),
        release: LOCAL_WHISPER_RUNTIME_RELEASE.into(),
        files: installed,
    })
}

pub fn validate_local_whisper_install(directory: &Path) -> CoreResult<bool> {
    validate_local_whisper_install_with_model_hash(directory, LOCAL_WHISPER_MODEL_SHA256)
}

/// Check whether the pinned local runtime and model files are available without hashing the
/// multi-gigabyte model. Use this for ordinary UI status; diagnostics and installation still
/// perform the full checksum validation.
pub fn local_whisper_install_is_present(directory: &Path) -> bool {
    LOCAL_WHISPER_RUNTIME_FILES
        .iter()
        .all(|name| directory.join(name).is_file())
        && directory.join(LOCAL_WHISPER_MODEL_NAME).is_file()
}

fn validate_local_whisper_install_with_model_hash(
    directory: &Path,
    model_sha256: &str,
) -> CoreResult<bool> {
    if !local_whisper_install_is_present(directory) {
        return Ok(false);
    }
    validate_local_model(&directory.join(LOCAL_WHISPER_MODEL_NAME), model_sha256)
}

fn runtime_archive_error(error: zip::result::ZipError) -> CoreError {
    CoreError::InvalidInput(format!("invalid whisper.cpp runtime archive: {error}"))
}

pub fn download_checked_model(
    url: &str,
    destination: &Path,
    expected_sha256: &str,
    mut progress: impl FnMut(u64, Option<u64>),
) -> CoreResult<ModelDownloadResult> {
    if expected_sha256.len() != 64
        || !expected_sha256
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(CoreError::InvalidInput(
            "expected SHA-256 must contain exactly 64 hexadecimal characters".into(),
        ));
    }
    let parsed = url::Url::parse(url)
        .map_err(|error| CoreError::InvalidInput(format!("invalid model URL: {error}")))?;
    if parsed.scheme() != "https" && !cfg!(test) {
        return Err(CoreError::InvalidInput(
            "model downloads require HTTPS".into(),
        ));
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let partial = destination.with_extension("download");
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60 * 30))
        .build()
        .map_err(network_core_error)?;
    let mut response = client.get(url).send().map_err(network_core_error)?;
    if !response.status().is_success() {
        return Err(CoreError::Unavailable(format!(
            "model download failed with HTTP {}",
            response.status()
        )));
    }
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
    let actual = format!("{:x}", digest.finalize());
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        let _ = fs::remove_file(&partial);
        return Err(CoreError::InvalidInput(format!(
            "model checksum mismatch: expected {}, got {actual}",
            expected_sha256.to_ascii_lowercase()
        )));
    }
    fs::rename(&partial, destination)?;
    Ok(ModelDownloadResult {
        path: destination.to_path_buf(),
        size_bytes: received,
        sha256: actual,
    })
}

pub fn validate_local_model(path: &Path, expected_sha256: &str) -> CoreResult<bool> {
    if !path.is_file() {
        return Ok(false);
    }
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
    Ok(format!("{:x}", digest.finalize()).eq_ignore_ascii_case(expected_sha256))
}

fn required_key<'a>(key: &'a Option<String>, provider: &str) -> Result<&'a str, ProviderError> {
    key.as_deref()
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| {
            ProviderError::new(
                ProviderFailure::Unavailable,
                format!("{provider} credential is not configured"),
            )
        })
}

fn checked_response(response: Response, endpoint: &str) -> Result<Response, ProviderError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let failure = match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => ProviderFailure::Authentication,
        StatusCode::TOO_MANY_REQUESTS | StatusCode::PAYMENT_REQUIRED => {
            ProviderFailure::QuotaExhausted
        }
        _ if status.is_server_error() => ProviderFailure::Service,
        _ => ProviderFailure::Service,
    };
    // Do not include response bodies. Providers sometimes echo request details, and diagnostics
    // must never acquire credential or transcript content.
    Err(ProviderError::new(
        failure,
        format!("provider request to {endpoint} failed with HTTP {status}"),
    ))
}

fn assembly_timeout_error() -> ProviderError {
    ProviderError::new(
        ProviderFailure::Timeout,
        "AssemblyAI dictation attempt timed out",
    )
}

/// A tenth of a second of mono PCM silence verifies speech access without sending user audio.
fn deepgram_validation_audio() -> Vec<u8> {
    const SAMPLE_RATE: u32 = 16_000;
    const DATA_BYTES: u32 = SAMPLE_RATE / 10 * 2;
    let mut wave = Vec::with_capacity(44 + DATA_BYTES as usize);
    wave.extend_from_slice(b"RIFF");
    wave.extend_from_slice(&(36 + DATA_BYTES).to_le_bytes());
    wave.extend_from_slice(b"WAVEfmt ");
    wave.extend_from_slice(&16_u32.to_le_bytes());
    wave.extend_from_slice(&1_u16.to_le_bytes());
    wave.extend_from_slice(&1_u16.to_le_bytes());
    wave.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wave.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wave.extend_from_slice(&2_u16.to_le_bytes());
    wave.extend_from_slice(&16_u16.to_le_bytes());
    wave.extend_from_slice(b"data");
    wave.extend_from_slice(&DATA_BYTES.to_le_bytes());
    wave.resize(44 + DATA_BYTES as usize, 0);
    wave
}

fn classify_reqwest(error: reqwest::Error) -> ProviderError {
    let failure = if error.is_timeout() {
        ProviderFailure::Timeout
    } else {
        ProviderFailure::Service
    };
    ProviderError::new(failure, format!("provider request failed: {error}"))
}

fn parse_provider_response(error: reqwest::Error) -> ProviderError {
    ProviderError::new(
        ProviderFailure::Service,
        format!("provider returned invalid JSON: {error}"),
    )
}

fn network_core_error(error: reqwest::Error) -> CoreError {
    CoreError::Unavailable(format!("network request failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader},
        net::TcpListener,
        sync::{Arc, Mutex},
    };

    fn mock_websocket(handler: impl FnOnce(WebSocket<TcpStream>) + Send + 'static) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handler(tungstenite::accept(stream).unwrap());
        });
        format!("http://{address}")
    }

    fn mock_server(responses: Vec<(u16, &'static str)>) -> (String, Arc<Mutex<Vec<String>>>) {
        mock_server_with_delays(
            responses
                .into_iter()
                .map(|(status, body)| (Duration::ZERO, status, body))
                .collect(),
        )
    }

    fn mock_server_with_delays(
        responses: Vec<(Duration, u16, &'static str)>,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        thread::spawn(move || {
            for (delay, status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                reader.read_line(&mut first).unwrap();
                captured.lock().unwrap().push(first);
                let mut content_length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        content_length = value.trim().parse().unwrap_or(0);
                    }
                }
                let mut discard = vec![0; content_length];
                reader.read_exact(&mut discard).unwrap();
                thread::sleep(delay);
                write!(
                    stream,
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        });
        (format!("http://{address}"), requests)
    }

    fn delayed_server(delay: Duration, requests: usize) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            for _ in 0..requests {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                thread::sleep(delay);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}"
                );
            }
        });
        format!("http://{address}")
    }

    #[test]
    fn deepgram_parser_preserves_vietnamese_and_timestamps() {
        let value = json!({"results":{"channels":[{"detected_language":"vi","alternatives":[{"transcript":"Xin chào Hong","words":[{"punctuated_word":"Xin","start":0.0,"end":0.2,"confidence":0.9},{"punctuated_word":"chào","start":0.2,"end":0.5,"confidence":0.9}]}]}]}});
        let result = parse_deepgram(value, "deepgram_nova_3").unwrap();
        assert_eq!(result.text, "Xin ch\u{e0}o Hong");
        assert_eq!(result.language.as_deref(), Some("vi"));
        assert_eq!(result.segments[1].start_ms, 200);
    }

    #[test]
    fn deepgram_validation_uses_speech_access_instead_of_project_access() {
        let (endpoint, requests) = mock_server(vec![(200, r#"{"results":{}}"#)]);
        let providers = SpeechProviders::with_endpoints(
            ProviderCredentials {
                deepgram: Some("speech-only-key".into()),
                assemblyai: None,
                groq: None,
            },
            ProviderEndpoints {
                deepgram: endpoint,
                ..ProviderEndpoints::default()
            },
        )
        .unwrap();

        providers.validate_provider("deepgram").unwrap();

        let requests = requests.lock().unwrap();
        assert_eq!(
            requests.as_slice(),
            ["POST /v1/listen?model=nova-3&punctuate=true HTTP/1.1\r\n"]
        );
    }

    #[test]
    fn deepgram_flux_stream_sends_pcm_and_returns_final_turn() {
        let endpoint = mock_websocket(|mut socket| {
            assert!(matches!(socket.read().unwrap(), Message::Binary(bytes) if !bytes.is_empty()));
            let finalize = socket.read().unwrap().into_text().unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&finalize).unwrap()["type"],
                "Finalize"
            );
            socket
                .send(Message::Text(
                    json!({
                        "type": "TurnInfo",
                        "event": "EndOfTurn",
                        "transcript": "Xin chào Hong",
                        "start": 0.0,
                        "duration": 1.0,
                        "language": "vi"
                    })
                    .to_string()
                    .into(),
                ))
                .unwrap();
            socket
                .send(Message::Text(
                    json!({"type": "Metadata"}).to_string().into(),
                ))
                .unwrap();
        });
        let providers = SpeechProviders::with_endpoints(
            ProviderCredentials {
                deepgram: Some("test-key".into()),
                assemblyai: None,
                groq: None,
            },
            ProviderEndpoints {
                deepgram: endpoint,
                ..ProviderEndpoints::default()
            },
        )
        .unwrap();
        let mut session = providers.start_streaming_dictation(48_000, 2).unwrap();
        assert_eq!(session.provider(), StreamingSttProvider::DeepgramFlux);
        session
            .push_audio(LiveAudioPacket {
                sample_rate_hz: 48_000,
                channels: 2,
                samples: LiveAudioSamples::Float32(vec![0.25; 960]),
            })
            .unwrap();
        let transcript = session.finish().unwrap();
        assert_eq!(transcript.text, "Xin chào Hong");
        assert_eq!(transcript.language.as_deref(), Some("vi"));
        assert_eq!(transcript.provider, "deepgram_flux_streaming");
        assert_eq!(transcript.segments[0].end_ms, 1_000);
    }

    #[test]
    fn assemblyai_stream_is_used_when_deepgram_is_not_configured() {
        let endpoint = mock_websocket(|mut socket| {
            assert!(matches!(socket.read().unwrap(), Message::Binary(bytes) if !bytes.is_empty()));
            let terminate = socket.read().unwrap().into_text().unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&terminate).unwrap()["type"],
                "Terminate"
            );
            socket
                .send(Message::Text(
                    json!({
                        "type": "Turn",
                        "end_of_turn": true,
                        "transcript": "Meeting ready",
                        "words": [{"start": 20, "end": 300}]
                    })
                    .to_string()
                    .into(),
                ))
                .unwrap();
            socket
                .send(Message::Text(
                    json!({"type": "Termination"}).to_string().into(),
                ))
                .unwrap();
        });
        let providers = SpeechProviders::with_endpoints(
            ProviderCredentials {
                deepgram: None,
                assemblyai: Some("test-key".into()),
                groq: None,
            },
            ProviderEndpoints {
                assemblyai: endpoint,
                ..ProviderEndpoints::default()
            },
        )
        .unwrap();
        let mut session = providers.start_streaming_dictation(16_000, 1).unwrap();
        assert_eq!(session.provider(), StreamingSttProvider::AssemblyAi);
        session
            .push_audio(LiveAudioPacket {
                sample_rate_hz: 16_000,
                channels: 1,
                samples: LiveAudioSamples::Pcm16(vec![100; 160]),
            })
            .unwrap();
        let transcript = session.finish().unwrap();
        assert_eq!(transcript.text, "Meeting ready");
        assert_eq!(transcript.provider, "assemblyai_streaming");
        assert_eq!(transcript.segments[0].start_ms, 20);
        assert_eq!(transcript.segments[0].end_ms, 300);
    }

    #[test]
    fn normalizer_downmixes_resamples_clips_and_keeps_packet_continuity() {
        let stereo = (0..480)
            .flat_map(|index| {
                let sample = if index == 0 {
                    2.0
                } else {
                    (index as f32 / 20.0).sin()
                };
                [sample, sample]
            })
            .collect::<Vec<_>>();
        let mut whole = Pcm16Normalizer::new(48_000, 2).unwrap();
        let expected = whole
            .normalize(LiveAudioPacket {
                sample_rate_hz: 48_000,
                channels: 2,
                samples: LiveAudioSamples::Float32(stereo.clone()),
            })
            .unwrap();
        assert_eq!(expected.len(), 160);
        assert_eq!(expected[0], i16::MAX);

        let mut chunked = Pcm16Normalizer::new(48_000, 2).unwrap();
        let mut actual = chunked
            .normalize(LiveAudioPacket {
                sample_rate_hz: 48_000,
                channels: 2,
                samples: LiveAudioSamples::Float32(stereo[..318].to_vec()),
            })
            .unwrap();
        actual.extend(
            chunked
                .normalize(LiveAudioPacket {
                    sample_rate_hz: 48_000,
                    channels: 2,
                    samples: LiveAudioSamples::Float32(stereo[318..].to_vec()),
                })
                .unwrap(),
        );
        assert_eq!(actual, expected);
    }

    #[test]
    fn runtime_archive_extracts_only_required_files_and_validates_complete_install() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("runtime.zip");
        let mut archive = zip::ZipWriter::new(fs::File::create(&archive_path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for name in LOCAL_WHISPER_RUNTIME_FILES {
            archive
                .start_file(format!("Release/{name}"), options)
                .unwrap();
            archive
                .write_all(format!("fixture-{name}").as_bytes())
                .unwrap();
        }
        archive.start_file("Release/bench.exe", options).unwrap();
        archive.write_all(b"not installed").unwrap();
        archive.finish().unwrap();

        let install = directory.path().join("models");
        let result = install_local_whisper_runtime_archive(&archive_path, &install).unwrap();
        assert_eq!(result.files.len(), LOCAL_WHISPER_RUNTIME_FILES.len());
        assert!(install.join("ggml-cuda.dll").is_file());
        assert!(install.join("cublas64_12.dll").is_file());
        assert!(!install.join("bench.exe").exists());
        let model_bytes = b"fixture-model";
        fs::write(install.join(LOCAL_WHISPER_MODEL_NAME), model_bytes).unwrap();
        let model_hash = format!("{:x}", Sha256::digest(model_bytes));
        assert!(validate_local_whisper_install_with_model_hash(&install, &model_hash).unwrap());
    }

    #[test]
    fn runtime_archive_rejects_traversal() {
        let directory = tempfile::tempdir().unwrap();
        let archive_path = directory.path().join("unsafe.zip");
        let mut archive = zip::ZipWriter::new(fs::File::create(&archive_path).unwrap());
        archive
            .start_file(
                "../whisper-cli.exe",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(b"unsafe").unwrap();
        archive.finish().unwrap();
        let error =
            install_local_whisper_runtime_archive(&archive_path, &directory.path().join("models"))
                .unwrap_err();
        assert!(error.to_string().contains("unsafe path"));
    }

    #[test]
    fn lightweight_install_status_does_not_hash_the_model() {
        let directory = tempfile::tempdir().unwrap();
        let install = directory.path().join("models");
        fs::create_dir_all(&install).unwrap();
        for name in LOCAL_WHISPER_RUNTIME_FILES {
            fs::write(install.join(name), b"runtime").unwrap();
        }
        fs::write(install.join(LOCAL_WHISPER_MODEL_NAME), b"model").unwrap();

        assert!(local_whisper_install_is_present(&install));
        assert!(
            !validate_local_whisper_install_with_model_hash(&install, &"0".repeat(64)).unwrap()
        );
    }

    #[test]
    fn timeout_is_retried_but_authentication_rotates() {
        let (deepgram, requests) = mock_server(vec![
            (401, "{}"),
            (200, r#"{"upload_url":"audio"}"#),
            (200, r#"{"id":"one"}"#),
            (
                200,
                r#"{"status":"completed","text":"xin ch\u00e0o","language_code":"vi"}"#,
            ),
        ]);
        let endpoints = ProviderEndpoints {
            deepgram: deepgram.clone(),
            assemblyai: deepgram.clone(),
            groq: deepgram,
        };
        let providers = SpeechProviders::with_endpoints(
            ProviderCredentials {
                deepgram: Some("not-a-real-key".into()),
                assemblyai: Some("not-a-real-key".into()),
                groq: None,
            },
            endpoints,
        )
        .unwrap()
        .with_polling(Duration::ZERO, Duration::from_secs(1));
        let outcome = providers.transcribe_with_fallback(
            SessionKind::Dictation,
            &AudioRequest {
                bytes: vec![1, 2, 3],
                mime_type: "audio/wav".into(),
                source_path: None,
            },
            None,
        );
        match outcome {
            TranscriptionOutcome::Completed {
                transcript,
                attempts,
            } => {
                assert_eq!(transcript.text, "xin ch\u{e0}o");
                assert_eq!(attempts.len(), 1);
                assert_eq!(attempts[0].failure, ProviderFailure::Authentication);
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
        assert_eq!(requests.lock().unwrap().len(), 4);
    }

    #[test]
    fn dictation_timeouts_do_not_shorten_meeting_operations() {
        let providers = SpeechProviders::new(ProviderCredentials::default())
            .unwrap()
            .with_polling(Duration::ZERO, Duration::from_secs(321))
            .with_dictation_timeouts(
                Duration::from_millis(31),
                Duration::from_millis(47),
                Duration::from_millis(59),
            );

        assert_eq!(
            providers.request_timeout(SessionKind::Dictation),
            Some(Duration::from_millis(31))
        );
        assert_eq!(providers.request_timeout(SessionKind::Meeting), None);
        assert_eq!(
            providers.assembly_timeout(SessionKind::Dictation),
            Duration::from_millis(47)
        );
        assert_eq!(
            providers.assembly_timeout(SessionKind::Meeting),
            Duration::from_secs(321)
        );
        assert_eq!(
            providers
                .assembly_request_timeout(SessionKind::Meeting, Instant::now(), Duration::ZERO)
                .unwrap(),
            None
        );
        assert_eq!(
            providers
                .assembly_request_timeout(SessionKind::Dictation, Instant::now(), Duration::ZERO)
                .unwrap_err()
                .failure,
            ProviderFailure::Timeout
        );
    }

    #[test]
    fn default_dictation_timeouts_allow_normal_hosted_latency() {
        let providers = SpeechProviders::new(ProviderCredentials::default()).unwrap();

        assert_eq!(
            providers.request_timeout(SessionKind::Dictation),
            Some(Duration::from_secs(5))
        );
        assert_eq!(
            providers.assembly_timeout(SessionKind::Dictation),
            Duration::from_secs(10)
        );
        assert_eq!(
            providers.dictation_correction_timeout,
            Duration::from_secs(1)
        );
    }

    #[test]
    fn default_dictation_timeout_accepts_a_normal_latency_deepgram_response() {
        let response = r#"{"results":{"channels":[{"alternatives":[{"transcript":"ready","words":[{"punctuated_word":"ready","start":0.0,"end":0.4,"confidence":0.9}]}]}]}}"#;
        let (deepgram, _) = mock_server_with_delays(vec![
            (Duration::from_millis(1_200), 200, response),
            (Duration::from_millis(1_200), 200, response),
        ]);
        let providers = SpeechProviders::with_endpoints(
            ProviderCredentials {
                deepgram: Some("test".into()),
                assemblyai: None,
                groq: None,
            },
            ProviderEndpoints {
                deepgram,
                ..ProviderEndpoints::default()
            },
        )
        .unwrap();

        let outcome = providers.transcribe_with_fallback(
            SessionKind::Dictation,
            &AudioRequest {
                bytes: vec![1],
                mime_type: "audio/wav".into(),
                source_path: None,
            },
            None,
        );

        let TranscriptionOutcome::Completed {
            transcript,
            attempts,
        } = outcome
        else {
            panic!("expected delayed Deepgram response to complete");
        };
        assert_eq!(transcript.text, "ready");
        assert!(attempts.is_empty());
    }

    #[test]
    fn dictation_stt_timeout_retries_once_before_rotating() {
        let deepgram = delayed_server(Duration::from_millis(150), 2);
        let providers = SpeechProviders::with_endpoints(
            ProviderCredentials {
                deepgram: Some("test".into()),
                assemblyai: None,
                groq: None,
            },
            ProviderEndpoints {
                deepgram: deepgram.clone(),
                assemblyai: deepgram.clone(),
                groq: deepgram,
            },
        )
        .unwrap()
        .with_dictation_timeouts(
            Duration::from_millis(30),
            Duration::from_millis(30),
            Duration::from_millis(30),
        );

        let outcome = providers.transcribe_with_fallback(
            SessionKind::Dictation,
            &AudioRequest {
                bytes: vec![1],
                mime_type: "audio/wav".into(),
                source_path: None,
            },
            None,
        );
        let TranscriptionOutcome::Failed { attempts } = outcome else {
            panic!("expected failed dictation");
        };
        assert_eq!(attempts.len(), 2);
        assert!(attempts
            .iter()
            .all(|attempt| attempt.failure == ProviderFailure::Timeout));
        assert!(attempts
            .iter()
            .all(|attempt| attempt.provider == "deepgram_flux"));
    }

    #[test]
    fn correction_timeout_returns_raw_dictation_without_waiting_for_client_timeout() {
        let groq = delayed_server(Duration::from_millis(250), 1);
        let providers = SpeechProviders::with_endpoints(
            ProviderCredentials {
                deepgram: None,
                assemblyai: None,
                groq: Some("test".into()),
            },
            ProviderEndpoints {
                deepgram: groq.clone(),
                assemblyai: groq.clone(),
                groq,
            },
        )
        .unwrap()
        .with_dictation_timeouts(
            Duration::from_millis(30),
            Duration::from_millis(30),
            Duration::from_millis(40),
        );
        let request = CorrectionRequest {
            raw_transcript: "xin chao Nhat".into(),
            application: None,
            window_title: None,
            selected_text: None,
            surrounding_text: None,
            personal_vocabulary: Vec::new(),
        };

        let started = Instant::now();
        let outcome = providers.correct_dictation(&request);

        assert!(!outcome.corrected);
        assert_eq!(outcome.text, request.raw_transcript);
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn correction_prompt_is_separate_from_stt_request() {
        let request = CorrectionRequest {
            raw_transcript: "hello nhat".into(),
            application: None,
            window_title: None,
            selected_text: None,
            surrounding_text: None,
            personal_vocabulary: vec![VocabularyReplacement {
                heard: "nhat".into(),
                replacement: "Nhat".into(),
            }],
        };
        let serialized_audio = serde_json::to_string(&AudioRequestFixture {
            bytes: vec![1],
            mime_type: "audio/wav".into(),
        })
        .unwrap();
        assert!(!serialized_audio.contains("Nhat"));
        assert!(correction_messages(&request)[1]["content"]
            .as_str()
            .unwrap()
            .contains("nhat => Nhat"));
    }

    #[test]
    fn explicit_transforms_are_typed_and_keep_input_separate() {
        let messages =
            transform_messages("Xin chào Hong", ExplicitTransformAction::TranslateEnglish);
        assert!(messages[0]["content"].as_str().unwrap().contains("English"));
        assert_eq!(messages[1]["content"], "Xin chào Hong");
    }

    #[derive(Serialize)]
    struct AudioRequestFixture {
        bytes: Vec<u8>,
        mime_type: String,
    }

    #[test]
    fn timestamp_chunks_keep_unicode_and_boundaries() {
        let chunks = timestamped_chunks(
            &[
                TranscriptSegment {
                    start_ms: 65_000,
                    end_ms: 66_000,
                    text: "Chúng ta đồng ý".into(),
                    speaker: Some("You".into()),
                    confidence: None,
                },
                TranscriptSegment {
                    start_ms: 67_000,
                    end_ms: 68_000,
                    text: "Ship it".into(),
                    speaker: Some("Others".into()),
                    confidence: None,
                },
            ],
            45,
        );
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].contains("[00:01:05]"));
        assert!(chunks[0].contains("Chúng ta đồng ý"));
    }

    #[test]
    fn every_meeting_brief_stage_forbids_ungrounded_inference() {
        let stages = [
            meeting_chunk_messages("[00:00:01] Speaker: hello".into()),
            meeting_reduction_messages(vec!["[00:00:01] hello".into()]),
            meeting_synthesis_messages(Some("Project Apollo"), &["[00:00:01] hello".into()]),
        ];

        for messages in stages {
            let instruction = messages[0]["content"].as_str().unwrap();
            assert!(instruction.contains("metadata only, never as evidence"));
            assert!(instruction.contains("Do not infer speaker identity"));
            assert!(instruction.contains("quote attribution or source"));
            assert!(instruction.contains("outside knowledge"));
        }
    }

    #[test]
    fn meeting_title_is_labeled_as_metadata_not_transcript_evidence() {
        let messages = meeting_synthesis_messages(
            Some("Nhat approved launch"),
            &["[00:00:01] Speaker: We should review it.".into()],
        );
        let prompt = messages[1]["content"].as_str().unwrap();

        assert!(prompt.starts_with("Organizational metadata, not evidence:"));
        assert!(prompt.contains("Meeting title: Nhat approved launch"));
        assert!(prompt.contains("Timestamped chunk notes, the only evidence:"));
    }

    #[test]
    fn brief_validation_requires_cited_decisions_and_actions() {
        let valid = "# Overview\nDone\n# Detailed topics\nTopic\n# Decisions\n- Ship [00:01:05]\n# Action items\nNone\n# Open questions\nNone\n# Notable cited moments\nMoment [00:01:05]\n# Possible follow-ups\nNone";
        let segments = vec![TranscriptSegment {
            start_ms: 60_000,
            end_ms: 70_000,
            text: "Ship".into(),
            speaker: None,
            confidence: None,
        }];
        assert!(validate_meeting_brief(valid, &segments).is_ok());
        assert!(
            validate_meeting_brief(&valid.replace("Ship [00:01:05]", "Ship"), &segments).is_err()
        );
        assert!(validate_meeting_brief(
            &valid.replace("Ship [00:01:05]", "Ship [00:10:00]"),
            &segments
        )
        .is_err());
    }

    #[test]
    fn brief_validation_allows_no_possible_follow_ups() {
        let brief = "# Overview\nA factual update.\n# Detailed topics\nStatus\n# Decisions\nNone\n# Action items\nNone\n# Open questions\nNone\n# Notable cited moments\nNone\n# Possible follow-ups\n";

        assert!(validate_meeting_brief(brief, &[]).is_ok());
    }

    #[test]
    fn missing_optional_follow_up_heading_is_added_without_content() {
        let mut brief = "# Overview\nA factual update.\n# Detailed topics\nStatus\n# Decisions\nNone\n# Action items\nNone\n# Open questions\nNone\n# Notable cited moments\nNone".to_string();

        ensure_optional_meeting_brief_sections(&mut brief);

        assert!(brief.ends_with("\n# Possible follow-ups\n"));
        assert!(validate_meeting_brief(&brief, &[]).is_ok());
    }

    #[test]
    fn prose_absence_markers_are_not_treated_as_uncited_items() {
        assert!(is_empty_brief_entry(
            "- *No decisions were explicitly recorded in the transcript.*【00:00:00】"
        ));
        assert!(is_empty_brief_entry(
            "- No action items were assigned during this meeting."
        ));
        assert!(!is_empty_brief_entry("- Ship the release"));
    }

    #[test]
    fn attempt_callback_precedes_each_provider_in_visible_mode_order() {
        let providers = SpeechProviders::with_endpoints(
            ProviderCredentials {
                deepgram: Some("test".into()),
                assemblyai: Some("test".into()),
                groq: Some("test".into()),
            },
            ProviderEndpoints {
                deepgram: "http://127.0.0.1:9".into(),
                assemblyai: "http://127.0.0.1:9".into(),
                groq: "http://127.0.0.1:9".into(),
            },
        )
        .unwrap()
        .with_dictation_timeouts(
            Duration::from_millis(10),
            Duration::from_millis(10),
            Duration::from_millis(10),
        );
        let local = LocalWhisperConfig {
            executable: PathBuf::from("missing-whisper-cli"),
            model: PathBuf::from("missing-model.bin"),
        };
        let mut observed = Vec::new();
        let outcome = providers.transcribe_with_fallback_observed(
            SessionKind::Dictation,
            &AudioRequest {
                bytes: vec![1],
                mime_type: "audio/wav".into(),
                source_path: None,
            },
            Some(&local),
            |attempt| observed.push(attempt),
        );
        assert!(matches!(outcome, TranscriptionOutcome::Failed { .. }));
        assert_eq!(
            observed,
            vec![
                ProviderAttemptStarted {
                    provider: "deepgram_flux".into(),
                    mode: ProcessingMode::Hosted,
                    attempt: 1,
                },
                ProviderAttemptStarted {
                    provider: "deepgram_flux".into(),
                    mode: ProcessingMode::Hosted,
                    attempt: 2,
                },
                ProviderAttemptStarted {
                    provider: "groq_whisper_large_v3_turbo".into(),
                    mode: ProcessingMode::HostedFallback,
                    attempt: 1,
                },
                ProviderAttemptStarted {
                    provider: "groq_whisper_large_v3_turbo".into(),
                    mode: ProcessingMode::HostedFallback,
                    attempt: 2,
                },
                ProviderAttemptStarted {
                    provider: "assemblyai".into(),
                    mode: ProcessingMode::HostedFallback,
                    attempt: 1,
                },
                ProviderAttemptStarted {
                    provider: "assemblyai".into(),
                    mode: ProcessingMode::HostedFallback,
                    attempt: 2,
                },
                ProviderAttemptStarted {
                    provider: "local_whisper_large_v3_turbo_q5_0".into(),
                    mode: ProcessingMode::Local,
                    attempt: 1,
                },
            ]
        );
    }

    #[test]
    fn unconfigured_hosted_providers_are_skipped() {
        let providers = SpeechProviders::new(ProviderCredentials::default()).unwrap();
        let mut observed = Vec::new();
        let outcome = providers.transcribe_with_fallback_observed(
            SessionKind::Meeting,
            &AudioRequest {
                bytes: vec![1],
                mime_type: "audio/wav".into(),
                source_path: None,
            },
            None,
            |attempt| observed.push(attempt),
        );
        assert!(matches!(
            outcome,
            TranscriptionOutcome::PendingEnhancement { .. }
        ));
        assert!(observed.is_empty());
    }

    #[test]
    fn long_note_batches_preserve_order_and_make_progress() {
        let batches = text_batches(vec!["a".repeat(10), "b".repeat(10), "c".repeat(10)], 21);
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0][0], "a".repeat(10));
        assert_eq!(batches[2][0], "c".repeat(10));
    }

    #[test]
    fn local_whisper_arguments_enable_english_vietnamese_detection() {
        let config = LocalWhisperConfig {
            executable: PathBuf::from("whisper-cli.exe"),
            model: PathBuf::from("model.bin"),
        };
        let arguments =
            local_whisper_arguments(&config, Path::new("speech.wav"), Path::new("transcript"))
                .into_iter()
                .map(|value| value.to_string_lossy().into_owned())
                .collect::<Vec<_>>();

        assert!(arguments.windows(2).any(|pair| pair == ["-l", "auto"]));
        assert!(arguments.iter().any(|argument| argument == "-np"));
        assert!(arguments.iter().any(|argument| argument == "-oj"));
        assert!(arguments
            .windows(2)
            .any(|pair| pair == ["-of", "transcript"]));
    }
}
