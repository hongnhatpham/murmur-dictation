//! Hardware harness for the meeting pipeline without the Tauri AppHandle.
//!
//! Both tests are `#[ignore]` because they need Windows audio endpoints, the local whisper.cpp
//! install in `%APPDATA%\com.bynhat.murmur\models`, the Groq key in Credential Manager, and
//! network access. They play Windows TTS through the default speakers so WASAPI loopback has
//! real speech to capture. Nothing is written to the user's app data.
//!
//! Run: `cargo test --test meeting_end_to_end -- --ignored --nocapture`
#![cfg(windows)]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use murmur_core::{
    adapters::{windows::WindowsAudioCapture, AudioCapture, CaptureRequest},
    commands::merge_audio_chunks,
    domain::SessionKind,
    providers::{
        normalize_meeting_audio_chunks, validate_meeting_brief, AudioRequest, LocalWhisperConfig,
        MeetingBriefRequest, SpeechProviders, TranscriptionOutcome,
    },
    secrets::{provider_credentials, WindowsCredentialStore},
};

const STAND_UP: &str = "Good morning everyone, this is our Tuesday stand-up. Yesterday we finished the import screen and the retry queue. We decided to ship the importer on Friday. Sam will fix the two capture bugs before the release. Priya is still waiting on the design review for the settings page, so that is an open question. That is all for today, thanks.";

/// Records ~20 s of TTS, checks both channels, transcribes the system channel with the real
/// provider chain, and asks Groq for a brief. Prints every provider attempt.
#[test]
#[ignore]
fn records_transcribes_and_briefs_a_short_meeting() {
    let session = record_meeting(1);
    let providers = providers();
    let transcript = transcribe(&session.system_wav, "system", "audio/wav", &providers);
    let TranscriptionOutcome::Completed { transcript, .. } = transcript else {
        panic!("system channel transcription did not complete");
    };
    let request = MeetingBriefRequest {
        title: Some("Tuesday stand-up".into()),
        segments: transcript.segments.clone(),
    };
    match providers.create_meeting_brief(&request) {
        Ok(brief) => {
            let accepted = validate_meeting_brief(&brief.markdown, &request.segments);
            println!(
                "brief provider: {} ({} chunk notes)",
                brief.provider, brief.chunk_count
            );
            println!("validate_meeting_brief: {accepted:?}");
            println!(
                "--- brief markdown ---\n{}\n--- end brief ---",
                brief.markdown
            );
            assert!(accepted.is_ok(), "brief failed validation: {accepted:?}");
        }
        Err(error) => panic!(
            "create_meeting_brief failed: {:?} {}",
            error.failure, error.message
        ),
    }
}

/// Records ~75 s (TTS looped) so the merged 48 kHz stereo float WAV exceeds Groq's 25 MB upload
/// limit, then reports exactly what the provider chain returns.
#[test]
#[ignore]
fn long_meeting_exceeds_groq_upload_limit() {
    let session = record_meeting(4);
    let size = fs::metadata(&session.system_wav).unwrap().len();
    println!(
        "merged system WAV: {} bytes ({:.1} MB)",
        size,
        size as f64 / 1_048_576.0
    );
    assert!(size > 25 * 1_048_576, "expected merged WAV above 25 MB");
    let directory = session.system_wav.parent().unwrap();
    let chunks = normalize_meeting_audio_chunks(&session.system_wav, directory, "system-harness")
        .expect("normalize long meeting");
    assert!(!chunks.is_empty());
    for chunk in &chunks {
        let normalized_size = fs::metadata(&chunk.path).unwrap().len();
        assert!(
            normalized_size < 25 * 1_048_576,
            "normalized chunk is still too large: {normalized_size}"
        );
        let outcome = transcribe(&chunk.path, "normalized system", "audio/wav", &providers());
        assert_eq!(
            completed_provider(&outcome),
            Some("groq_whisper_large_v3_turbo"),
            "normalized hosted transcription did not complete: {outcome:?}"
        );
    }
}

/// Sends an oversized upload straight to Groq, once exactly like `transcribe_groq` and once with
/// `Expect: 100-continue` so the server's verdict arrives before the body. Prints the HTTP status
/// or the transport error chain that the product flattens into one message. The key is never
/// printed.
fn probe_groq_upload(path: &Path) {
    use std::error::Error;
    let key = provider_credentials(&WindowsCredentialStore)
        .unwrap()
        .groq
        .expect("groq key");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .unwrap();
    let bytes = fs::read(path).unwrap();
    for expect_continue in [false, true] {
        let file = reqwest::blocking::multipart::Part::bytes(bytes.clone())
            .file_name("recording.wav")
            .mime_str("audio/wav")
            .unwrap();
        let form = reqwest::blocking::multipart::Form::new()
            .part("file", file)
            .text("model", "whisper-large-v3-turbo");
        let mut request = client
            .post("https://api.groq.com/openai/v1/audio/transcriptions")
            .bearer_auth(&key)
            .multipart(form);
        if expect_continue {
            request = request.header("Expect", "100-continue");
        }
        let label = if expect_continue {
            "probe (Expect: 100-continue)"
        } else {
            "probe (as transcribe_groq)"
        };
        match request.send() {
            Ok(response) => {
                let status = response.status();
                let body = response.text().unwrap_or_default();
                println!(
                    "{label}: HTTP {status}; body: {}",
                    &body[..body.len().min(400)]
                );
            }
            Err(error) => {
                println!("{label}: transport error: {error}");
                let mut source = error.source();
                while let Some(inner) = source {
                    println!("{label}:   caused by: {inner}");
                    source = inner.source();
                }
            }
        }
    }
}

/// Same probe without recording: 70 s of silent 48 kHz stereo float (26.9 MB) is enough to cross
/// Groq's 25 MB free-tier limit.
#[test]
#[ignore]
fn groq_upload_limit_probe() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("oversized.wav");
    fs::write(&path, silent_float_wav(48_000, 2, 70)).unwrap();
    println!(
        "oversized WAV: {:.1} MB",
        fs::metadata(&path).unwrap().len() as f64 / 1_048_576.0
    );
    probe_groq_upload(&path);
}

/// Mirrors `import_recording`: a single non-WAV file is sent as-is with its extension's MIME
/// type. Checks Groq with an MP3 and an M4A, then the local-only fallback with the M4A.
#[test]
#[ignore]
fn imported_non_wav_recordings() {
    let directory = tempfile::tempdir().unwrap();
    let wav = directory.path().join("source.wav");
    let script = format!(
        "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; $s.SetOutputToWaveFile('{}'); $s.Speak('{STAND_UP}'); $s.Dispose()",
        wav.display()
    );
    let status = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()
        .unwrap();
    assert!(status.success(), "TTS to file failed");
    let mp3 = directory.path().join("imported.mp3");
    let m4a = directory.path().join("imported.m4a");
    for target in [&mp3, &m4a] {
        let status = Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-i"])
            .arg(&wav)
            .arg(target)
            .status()
            .expect("ffmpeg on PATH");
        assert!(status.success(), "ffmpeg failed for {}", target.display());
    }
    let groq = providers();
    let mp3_outcome = transcribe(&mp3, "mp3", "audio/mpeg", &groq);
    let m4a_outcome = transcribe(&m4a, "m4a", "audio/mp4", &groq);
    let local_only = SpeechProviders::new(Default::default()).unwrap();
    let m4a_local = transcribe(&m4a, "m4a local-only", "audio/mp4", &local_only);
    println!(
        "summary: mp3 via {:?}, m4a via {:?}, m4a local-only completed: {}",
        completed_provider(&mp3_outcome),
        completed_provider(&m4a_outcome),
        completed_provider(&m4a_local).is_some()
    );
}

/// An online call with a muted microphone yields a silent `-microphone-` group. Shows what the
/// provider chain returns for 20 s of digital silence in the capture format, with Groq and with
/// local whisper only, because `process_meeting_result` fails the whole meeting when any group
/// ends `PendingEnhancement`.
#[test]
#[ignore]
fn silent_microphone_group_outcome() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("silent.wav");
    fs::write(&path, silent_float_wav(48_000, 2, 20)).unwrap();
    let outcome = transcribe(&path, "silent groq", "audio/wav", &providers());
    println!("silent outcome via groq: {outcome:?}");
    let local_only = SpeechProviders::new(Default::default()).unwrap();
    let outcome = transcribe(&path, "silent local", "audio/wav", &local_only);
    println!("silent outcome via local whisper: {outcome:?}");
}

/// Loopback delivers no packets while the render engine is idle. Records 8 s of silence, then
/// ~6 s of TTS, and compares the system channel's duration against the microphone's wall clock.
#[test]
#[ignore]
fn loopback_duration_while_nothing_plays() {
    let directory = tempfile::tempdir().unwrap();
    let audio = WindowsAudioCapture::new();
    let capture_id = audio
        .start(CaptureRequest {
            microphone_device: None,
            include_system_audio: true,
            output_directory: directory.path().to_path_buf(),
        })
        .unwrap();
    let started = Instant::now();
    thread::sleep(Duration::from_secs(8));
    println!(
        "activity before speech: {:?}",
        audio.channel_activity(&capture_id).unwrap()
    );
    speak("Loopback check, one two three four five.", 1)
        .wait()
        .unwrap();
    println!(
        "activity after speech: {:?}",
        audio.channel_activity(&capture_id).unwrap()
    );
    let files = audio.stop(&capture_id).unwrap();
    let wall = started.elapsed().as_secs_f64();
    for (label, keep) in [("microphone", "-microphone-"), ("system", "-system-")] {
        let chunks = group(&files, |name| name.contains(keep));
        let merged =
            merge_audio_chunks(&chunks, &directory.path().join(format!("{label}.wav"))).unwrap();
        let wav = fs::read(&merged).unwrap();
        let header = parse_wav(&wav);
        println!(
            "{label}: {:.1} s of audio for {wall:.1} s of wall clock ({} chunks)",
            header.seconds(wav.len()),
            chunks.len()
        );
    }
}

/// Tauri runs sync commands such as `start_meeting` on the STA main thread. Repeats the start /
/// stop handshake from an STA-initialized thread to check it behaves like the MTA test thread.
#[test]
#[ignore]
fn capture_starts_from_an_sta_thread() {
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    let directory = tempfile::tempdir().unwrap();
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().unwrap() };
    let audio = WindowsAudioCapture::new();
    let started = Instant::now();
    let capture_id = audio
        .start(CaptureRequest {
            microphone_device: None,
            include_system_audio: true,
            output_directory: directory.path().to_path_buf(),
        })
        .unwrap();
    println!("STA start took {:?}", started.elapsed());
    thread::sleep(Duration::from_secs(3));
    let files = audio.stop(&capture_id).unwrap();
    let report = audio.report(&capture_id).unwrap();
    for channel in &report.channels {
        println!(
            "channel {:?}: chunks={} gaps={:?} fatal={:?}",
            channel.channel,
            channel.chunks.len(),
            channel.gaps,
            channel.fatal_error
        );
    }
    assert!(files
        .iter()
        .any(|p| p.to_string_lossy().contains("-system-")));
    unsafe { CoUninitialize() };
}

/// 32-bit float stereo WAV (plain WAVEFORMATEX, tag 3) of digital silence.
fn silent_float_wav(rate: u32, channels: u16, seconds: u32) -> Vec<u8> {
    let block_align = channels * 4;
    let data_len = rate * u32::from(block_align) * seconds;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&3_u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&rate.to_le_bytes());
    wav.extend_from_slice(&(rate * u32::from(block_align)).to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&32_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.resize(44 + data_len as usize, 0);
    wav
}

fn completed_provider(outcome: &TranscriptionOutcome) -> Option<&str> {
    match outcome {
        TranscriptionOutcome::Completed { transcript, .. } => Some(transcript.provider.as_str()),
        _ => None,
    }
}

struct RecordedMeeting {
    _directory: tempfile::TempDir,
    system_wav: PathBuf,
}

/// Starts capture like `start_meeting` (microphone + loopback), plays TTS `repeats` times,
/// stops, then groups and merges chunks exactly like `process_meeting_result`.
fn record_meeting(repeats: usize) -> RecordedMeeting {
    let directory = tempfile::tempdir().unwrap();
    let audio = WindowsAudioCapture::new();
    let started = Instant::now();
    let capture_id = audio
        .start(CaptureRequest {
            microphone_device: None,
            include_system_audio: true,
            output_directory: directory.path().to_path_buf(),
        })
        .expect("start capture");
    println!("capture started in {:?}", started.elapsed());

    let mut speech = speak(STAND_UP, repeats);
    let speaking = Instant::now();
    speech.wait().expect("TTS process");
    println!("TTS finished after {:?}", speaking.elapsed());
    thread::sleep(Duration::from_secs(1));

    let files = audio.stop(&capture_id).expect("stop capture");
    let report = audio.report(&capture_id).expect("capture report");
    for channel in &report.channels {
        println!(
            "channel {:?}: device={:?} chunks={} gaps={:?} fatal={:?}",
            channel.channel,
            channel.device_id,
            channel.chunks.len(),
            channel.gaps,
            channel.fatal_error
        );
    }

    let microphone = group(&files, |name| !name.contains("-system-"));
    let system = group(&files, |name| name.contains("-system-"));
    assert!(
        microphone
            .iter()
            .any(|p| p.to_string_lossy().contains("-microphone-")),
        "no -microphone- chunks: {files:?}"
    );
    assert!(!system.is_empty(), "no -system- chunks: {files:?}");
    for path in microphone.iter().chain(&system).take(2) {
        println!(
            "{}: {}",
            path.file_name().unwrap().to_string_lossy(),
            describe_wav(path)
        );
    }

    let microphone_wav =
        merge_audio_chunks(&microphone, &directory.path().join("microphone.wav")).unwrap();
    let system_wav = merge_audio_chunks(&system, &directory.path().join("system.wav")).unwrap();
    for (label, path) in [("microphone", &microphone_wav), ("system", &system_wav)] {
        let wav = fs::read(path).unwrap();
        let header = parse_wav(&wav);
        let rms = rms(&wav, &header);
        println!(
            "merged {label}: {} bytes, {:.1} s, {} Hz, {} ch, {} bit, tag 0x{:x}, rms {:.4}",
            wav.len(),
            header.seconds(wav.len()),
            header.sample_rate,
            header.channels,
            header.bits,
            header.format_tag,
            rms
        );
        if label == "system" {
            assert!(rms > 0.002, "loopback captured silence (rms {rms})");
        }
    }
    RecordedMeeting {
        _directory: directory,
        system_wav,
    }
}

fn group(files: &[PathBuf], keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    files
        .iter()
        .filter(|path| keep(&path.to_string_lossy()))
        .cloned()
        .collect()
}

/// Runs the real provider chain (Groq first when configured, then local whisper.cpp) and prints
/// every attempt the way the meeting pipeline would see it.
fn transcribe(
    path: &Path,
    label: &str,
    mime: &str,
    providers: &SpeechProviders,
) -> TranscriptionOutcome {
    let local = local_whisper();
    println!("local whisper configured: {}", local.is_some());
    let request = AudioRequest::from_file(path, mime).unwrap();
    let started = Instant::now();
    let outcome = providers.transcribe_with_fallback_observed(
        SessionKind::Meeting,
        &request,
        local.as_ref(),
        |attempt| {
            println!(
                "[{label}] attempt {} via {} ({:?}) at {:?}",
                attempt.attempt,
                attempt.provider,
                attempt.mode,
                started.elapsed()
            );
        },
    );
    let attempts = match &outcome {
        TranscriptionOutcome::Completed { attempts, .. }
        | TranscriptionOutcome::PendingEnhancement { attempts }
        | TranscriptionOutcome::Failed { attempts } => attempts,
    };
    for failure in attempts {
        println!(
            "[{label}] FAILED provider={} kind={:?} message={}",
            failure.provider, failure.failure, failure.message
        );
    }
    if let TranscriptionOutcome::Completed { transcript, .. } = &outcome {
        println!(
            "[{label}] transcribed by {} in {:?}, language {:?}, {} segments",
            transcript.provider,
            started.elapsed(),
            transcript.language,
            transcript.segments.len()
        );
        println!("[{label}] text: {}", transcript.text);
    } else {
        println!("[{label}] outcome: {outcome:?}");
    }
    outcome
}

fn providers() -> SpeechProviders {
    let credentials = provider_credentials(&WindowsCredentialStore).expect("credential store");
    println!(
        "groq credential configured: {}",
        credentials.groq.as_deref().is_some_and(|k| !k.is_empty())
    );
    SpeechProviders::new(credentials).unwrap()
}

/// Mirrors `commands::local_whisper` against the user's real models directory (read-only).
fn local_whisper() -> Option<LocalWhisperConfig> {
    let directory = PathBuf::from(std::env::var_os("APPDATA")?)
        .join("com.bynhat.murmur")
        .join("models");
    let executable = directory.join("whisper-cli.exe");
    let model = directory.join("ggml-large-v3-turbo-q5_0.bin");
    (executable.is_file() && model.is_file()).then_some(LocalWhisperConfig { executable, model })
}

/// Plays `text` through the default render device with Windows TTS, `repeats` times in a row.
fn speak(text: &str, repeats: usize) -> Child {
    let script = format!(
        "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; for ($i = 0; $i -lt {repeats}; $i++) {{ $s.Speak('{text}') }}"
    );
    Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn PowerShell TTS")
}

struct WavHeader {
    format_tag: u16,
    channels: u16,
    sample_rate: u32,
    bits: u16,
    block_align: u16,
    is_float: bool,
    data_offset: usize,
    data_len: usize,
}

impl WavHeader {
    fn seconds(&self, file_len: usize) -> f64 {
        let bytes = self.data_len.min(file_len.saturating_sub(self.data_offset));
        bytes as f64 / (self.sample_rate as f64 * self.block_align as f64)
    }
}

fn describe_wav(path: &Path) -> String {
    let wav = fs::read(path).unwrap();
    let h = parse_wav(&wav);
    format!(
        "{} bytes, {} Hz, {} ch, {} bit, tag 0x{:x}{}",
        wav.len(),
        h.sample_rate,
        h.channels,
        h.bits,
        h.format_tag,
        if h.is_float { " float" } else { "" }
    )
}

/// Minimal RIFF walker: reads `fmt ` (including WAVEFORMATEXTENSIBLE sub-format) and locates `data`.
fn parse_wav(wav: &[u8]) -> WavHeader {
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    let u16_at = |o: usize| u16::from_le_bytes([wav[o], wav[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes(wav[o..o + 4].try_into().unwrap());
    let mut offset = 12;
    let mut fmt = None;
    while offset + 8 <= wav.len() {
        let id = &wav[offset..offset + 4];
        let size = u32_at(offset + 4) as usize;
        let body = offset + 8;
        if id == b"fmt " {
            let format_tag = u16_at(body);
            let sub_tag = if format_tag == 0xfffe && size >= 26 {
                u16_at(body + 24)
            } else {
                format_tag
            };
            fmt = Some((
                format_tag,
                u16_at(body + 2),
                u32_at(body + 4),
                u16_at(body + 12),
                u16_at(body + 14),
                sub_tag == 3,
            ));
        } else if id == b"data" {
            let (format_tag, channels, sample_rate, block_align, bits, is_float) =
                fmt.expect("fmt before data");
            return WavHeader {
                format_tag,
                channels,
                sample_rate,
                bits,
                block_align,
                is_float,
                data_offset: body,
                data_len: size,
            };
        }
        offset = body + size + (size % 2);
    }
    panic!("no data chunk");
}

/// Full-scale RMS across all channels for float32 or PCM16 payloads.
fn rms(wav: &[u8], header: &WavHeader) -> f64 {
    let end = (header.data_offset + header.data_len).min(wav.len());
    let data = &wav[header.data_offset..end];
    let (sum, count) = match (header.is_float, header.bits) {
        (true, 32) => data.chunks_exact(4).fold((0.0_f64, 0_u64), |(s, n), b| {
            let v = f32::from_le_bytes(b.try_into().unwrap()) as f64;
            (s + v * v, n + 1)
        }),
        (false, 16) => data.chunks_exact(2).fold((0.0_f64, 0_u64), |(s, n), b| {
            let v = i16::from_le_bytes(b.try_into().unwrap()) as f64 / 32768.0;
            (s + v * v, n + 1)
        }),
        other => panic!("unsupported sample layout {other:?}"),
    };
    if count == 0 {
        0.0
    } else {
        (sum / count as f64).sqrt()
    }
}
