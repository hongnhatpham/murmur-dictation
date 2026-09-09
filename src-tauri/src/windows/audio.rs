use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    slice,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use uuid::Uuid;
use windows::{
    core::{GUID, HSTRING, PCWSTR},
    Win32::{
        Foundation::PROPERTYKEY,
        Media::Audio::{
            eCapture, eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDevice,
            IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT,
            AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, DEVICE_STATE_ACTIVE,
            WAVEFORMATEX,
        },
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
            StructuredStorage::{PropVariantClear, PropVariantToStringAlloc},
            CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
        },
    },
};

use crate::{
    adapters::{AudioCapture, CaptureRequest},
    error::{CoreError, CoreResult},
};

const CHUNK_DURATION: Duration = Duration::from_secs(5);
const ACTIVITY_RECENCY: Duration = Duration::from_secs(2);
const PACKET_QUEUE_CAPACITY: usize = 8;
const SHARED_BUFFER_100NS: i64 = 1_000_000;
const PKEY_DEVICE_FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrophoneDevice {
    /// Stable Windows Core Audio endpoint ID. Store this value for an explicit override.
    pub id: String,
    pub label: String,
    pub is_default: bool,
}

/// Lists active Windows capture endpoints. The default endpoint sorts first, followed by label.
pub fn list_microphones() -> CoreResult<Vec<MicrophoneDevice>> {
    thread::Builder::new()
        .name("murmur-list-microphones".into())
        .spawn(list_microphones_mta)
        .map_err(CoreError::Io)?
        .join()
        .map_err(|_| CoreError::Unavailable("microphone discovery worker panicked".into()))?
}

fn list_microphones_mta() -> CoreResult<Vec<MicrophoneDevice>> {
    let _com = ComApartment::initialize()?;
    let enumerator: IMMDeviceEnumerator = unsafe {
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
            .map_err(|error| native_error("create audio device enumerator", error))?
    };
    let default_id = unsafe {
        enumerator
            .GetDefaultAudioEndpoint(eCapture, eConsole)
            .ok()
            .and_then(|device| device_id(&device).ok())
    };
    let collection = unsafe { enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) }
        .map_err(|error| native_error("enumerate active microphones", error))?;
    let count = unsafe { collection.GetCount() }
        .map_err(|error| native_error("count active microphones", error))?;
    let mut devices = Vec::with_capacity(count as usize);
    for index in 0..count {
        let device = unsafe { collection.Item(index) }
            .map_err(|error| native_error("read active microphone", error))?;
        let id = device_id(&device)?;
        devices.push(MicrophoneDevice {
            is_default: default_id.as_deref() == Some(id.as_str()),
            label: friendly_name(&device)?,
            id,
        });
    }
    sort_microphones(&mut devices);
    Ok(devices)
}

fn friendly_name(device: &IMMDevice) -> CoreResult<String> {
    unsafe {
        let store = device
            .OpenPropertyStore(STGM_READ)
            .map_err(|error| native_error("open microphone properties", error))?;
        let mut value = store
            .GetValue(&PKEY_DEVICE_FRIENDLY_NAME)
            .map_err(|error| native_error("read microphone friendly name", error))?;
        let label = match PropVariantToStringAlloc(&value) {
            Ok(label) => label,
            Err(error) => {
                let _ = PropVariantClear(&mut value);
                return Err(native_error("convert microphone friendly name", error));
            }
        };
        let decoded = PCWSTR(label.0)
            .to_string()
            .map_err(|error| native_error("decode microphone friendly name", error.into()));
        CoTaskMemFree(Some(label.0.cast()));
        let cleared = PropVariantClear(&mut value)
            .map_err(|error| native_error("release microphone friendly name", error));
        let decoded = decoded?;
        cleared?;
        if decoded.trim().is_empty() {
            return Err(CoreError::Unavailable(
                "microphone has an empty friendly name".into(),
            ));
        }
        Ok(decoded)
    }
}

fn sort_microphones(devices: &mut [MicrophoneDevice]) {
    devices.sort_by_cached_key(|device| {
        (
            !device.is_default,
            device.label.to_lowercase(),
            device.id.clone(),
        )
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AudioChannel {
    Microphone,
    System,
}

impl AudioChannel {
    fn file_label(self) -> &'static str {
        match self {
            Self::Microphone => "microphone",
            Self::System => "system",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureGap {
    pub channel: AudioChannel,
    pub started_millis: u64,
    pub ended_millis: Option<u64>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelCaptureReport {
    pub channel: AudioChannel,
    pub device_id: Option<String>,
    pub chunks: Vec<PathBuf>,
    pub gaps: Vec<CaptureGap>,
    pub fatal_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureReport {
    pub capture_id: String,
    pub channels: Vec<ChannelCaptureReport>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureActivity {
    pub microphone_active: bool,
    pub system_active: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MicrophoneLevel {
    /// Linear full-scale RMS amplitude in the range 0.0..=1.0.
    pub rms: f32,
    /// Linear full-scale peak amplitude in the range 0.0..=1.0.
    pub peak: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SampleEncoding {
    Pcm,
    Float,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WasapiFormat {
    /// Byte-for-byte WAVEFORMATEX or WAVEFORMATEXTENSIBLE returned by WASAPI.
    pub raw: Vec<u8>,
    pub format_tag: u16,
    pub channels: u16,
    pub sample_rate: u32,
    pub average_bytes_per_second: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
    pub encoding: SampleEncoding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioPacket {
    pub sequence: u64,
    /// Packets dropped for this subscriber since its previous delivered packet.
    pub dropped_before: u64,
    pub channel: AudioChannel,
    pub captured_millis: u64,
    pub frames: u32,
    pub flags: u32,
    pub device_position: u64,
    pub qpc_position: u64,
    pub format: WasapiFormat,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CaptureEvent {
    ChannelLost {
        channel: AudioChannel,
        at_millis: u64,
        reason: String,
    },
    ChannelRecovered {
        channel: AudioChannel,
        at_millis: u64,
    },
}

struct ActiveCapture {
    stop: Arc<AtomicBool>,
    workers: Vec<JoinHandle<ChannelCaptureReport>>,
    events: Arc<Mutex<Vec<CaptureEvent>>>,
    activity: Arc<Mutex<HashMap<AudioChannel, Instant>>>,
    packets: Arc<PacketHub>,
}

struct PacketSubscriber {
    sender: SyncSender<AudioPacket>,
    dropped: u64,
}

#[derive(Debug, Clone, Copy, Default)]
struct PacketStatistics {
    sum_squares: f64,
    sample_count: u64,
    peak: f32,
}

impl PacketStatistics {
    fn rms(self) -> f32 {
        if self.sample_count == 0 {
            0.0
        } else {
            (self.sum_squares / self.sample_count as f64).sqrt() as f32
        }
    }
}

#[derive(Default)]
struct LevelAccumulator {
    sum_squares: f64,
    sample_count: u64,
    peak: f32,
}

impl LevelAccumulator {
    fn push(&mut self, packet: PacketStatistics) {
        self.sum_squares += packet.sum_squares;
        self.sample_count = self.sample_count.saturating_add(packet.sample_count);
        self.peak = self.peak.max(packet.peak);
    }

    fn take(&mut self) -> Option<MicrophoneLevel> {
        if self.sample_count == 0 {
            return None;
        }
        let level = MicrophoneLevel {
            rms: (self.sum_squares / self.sample_count as f64)
                .sqrt()
                .clamp(0.0, 1.0) as f32,
            peak: self.peak.clamp(0.0, 1.0),
        };
        *self = Self::default();
        Some(level)
    }
}

#[derive(Default)]
struct PacketHub {
    subscribers: Mutex<Vec<PacketSubscriber>>,
    next_sequence: Mutex<u64>,
    level: Mutex<LevelAccumulator>,
}

impl PacketHub {
    fn subscribe(&self) -> CoreResult<Receiver<AudioPacket>> {
        let (sender, receiver) = mpsc::sync_channel(PACKET_QUEUE_CAPACITY);
        self.subscribers
            .lock()
            .map_err(|_| poisoned("audio packet subscribers"))?
            .push(PacketSubscriber { sender, dropped: 0 });
        Ok(receiver)
    }

    fn publish(&self, mut packet: AudioPacket, statistics: Option<PacketStatistics>) {
        if let (Some(statistics), Ok(mut level)) = (statistics, self.level.lock()) {
            level.push(statistics);
        }
        let sequence = if let Ok(mut next) = self.next_sequence.lock() {
            let sequence = *next;
            *next = next.saturating_add(1);
            sequence
        } else {
            return;
        };
        packet.sequence = sequence;
        let Ok(mut subscribers) = self.subscribers.lock() else {
            return;
        };
        subscribers.retain_mut(|subscriber| {
            let mut delivery = packet.clone();
            delivery.dropped_before = subscriber.dropped;
            match subscriber.sender.try_send(delivery) {
                Ok(()) => {
                    subscriber.dropped = 0;
                    true
                }
                Err(TrySendError::Full(_)) => {
                    subscriber.dropped = subscriber.dropped.saturating_add(1);
                    true
                }
                Err(TrySendError::Disconnected(_)) => false,
            }
        });
    }

    fn take_level(&self) -> CoreResult<Option<MicrophoneLevel>> {
        Ok(self
            .level
            .lock()
            .map_err(|_| poisoned("microphone level"))?
            .take())
    }
}

#[derive(Default)]
pub struct WindowsAudioCapture {
    active: Mutex<HashMap<String, ActiveCapture>>,
    completed: Mutex<HashMap<String, CaptureReport>>,
}

impl WindowsAudioCapture {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn report(&self, capture_id: &str) -> CoreResult<CaptureReport> {
        self.completed
            .lock()
            .map_err(|_| poisoned("completed capture"))?
            .get(capture_id)
            .cloned()
            .ok_or_else(|| CoreError::NotFound(format!("capture report {capture_id}")))
    }

    /// Drains device-loss and recovery events for immediate UI notification while capture runs.
    pub fn drain_events(&self, capture_id: &str) -> CoreResult<Vec<CaptureEvent>> {
        let events = self
            .active
            .lock()
            .map_err(|_| poisoned("active capture"))?
            .get(capture_id)
            .map(|capture| Arc::clone(&capture.events))
            .ok_or_else(|| CoreError::NotFound(format!("active capture {capture_id}")))?;
        let drained = events
            .lock()
            .map_err(|_| poisoned("capture events"))?
            .drain(..)
            .collect();
        Ok(drained)
    }

    /// Reports whether each channel contained non-silent samples during the last two seconds.
    pub fn channel_activity(&self, capture_id: &str) -> CoreResult<CaptureActivity> {
        let activity = self
            .active
            .lock()
            .map_err(|_| poisoned("active capture"))?
            .get(capture_id)
            .map(|capture| Arc::clone(&capture.activity))
            .ok_or_else(|| CoreError::NotFound(format!("active capture {capture_id}")))?;
        let activity = activity.lock().map_err(|_| poisoned("capture activity"))?;
        Ok(activity_from_timestamps(&activity, Instant::now()))
    }

    /// Subscribes to live microphone packets. The bounded receiver drops packets instead of
    /// blocking durable capture; `dropped_before` exposes any resulting streaming gap.
    pub fn subscribe_microphone(&self, capture_id: &str) -> CoreResult<Receiver<AudioPacket>> {
        let packets = self
            .active
            .lock()
            .map_err(|_| poisoned("active capture"))?
            .get(capture_id)
            .map(|capture| Arc::clone(&capture.packets))
            .ok_or_else(|| CoreError::NotFound(format!("active capture {capture_id}")))?;
        packets.subscribe()
    }

    /// Drains the microphone energy accumulated since the previous read.
    pub fn take_microphone_level(&self, capture_id: &str) -> CoreResult<Option<MicrophoneLevel>> {
        let packets = self
            .active
            .lock()
            .map_err(|_| poisoned("active capture"))?
            .get(capture_id)
            .map(|capture| Arc::clone(&capture.packets))
            .ok_or_else(|| CoreError::NotFound(format!("active capture {capture_id}")))?;
        packets.take_level()
    }

    fn finish(&self, capture_id: &str, delete: bool) -> CoreResult<Vec<PathBuf>> {
        let active = self
            .active
            .lock()
            .map_err(|_| poisoned("active capture"))?
            .remove(capture_id)
            .ok_or_else(|| CoreError::NotFound(format!("active capture {capture_id}")))?;
        active.stop.store(true, Ordering::Release);

        let mut channels = Vec::with_capacity(active.workers.len());
        for worker in active.workers {
            channels.push(worker.join().map_err(|_| {
                CoreError::Unavailable("Windows audio capture worker panicked".into())
            })?);
        }
        let files: Vec<_> = channels
            .iter()
            .flat_map(|channel| channel.chunks.iter().cloned())
            .collect();
        if delete {
            for path in &files {
                match fs::remove_file(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(CoreError::Io(error)),
                }
            }
            for channel in &mut channels {
                channel.chunks.clear();
            }
        }
        self.completed
            .lock()
            .map_err(|_| poisoned("completed capture"))?
            .insert(
                capture_id.to_string(),
                CaptureReport {
                    capture_id: capture_id.to_string(),
                    channels,
                },
            );
        Ok(if delete { Vec::new() } else { files })
    }
}

impl Drop for WindowsAudioCapture {
    fn drop(&mut self) {
        if let Ok(active) = self.active.get_mut() {
            for (_, capture) in active.drain() {
                capture.stop.store(true, Ordering::Release);
                for worker in capture.workers {
                    let _ = worker.join();
                }
            }
        }
    }
}

impl AudioCapture for WindowsAudioCapture {
    fn start(&self, request: CaptureRequest) -> CoreResult<String> {
        fs::create_dir_all(&request.output_directory)?;
        let capture_id = Uuid::new_v4().to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let events = Arc::new(Mutex::new(Vec::new()));
        let activity = Arc::new(Mutex::new(HashMap::new()));
        let packets = Arc::new(PacketHub::default());
        let mut workers = Vec::with_capacity(if request.include_system_audio { 2 } else { 1 });
        let (microphone, microphone_ready) = spawn_channel(
            capture_id.clone(),
            AudioChannel::Microphone,
            request.microphone_device,
            request.output_directory.clone(),
            Arc::clone(&stop),
            Arc::clone(&events),
            Arc::clone(&activity),
            Arc::clone(&packets),
        )?;
        workers.push(microphone);
        // Online calls can still be captured from the render loopback when the configured
        // microphone is unavailable. Keep the worker report so the meeting can explain the
        // missing microphone channel instead of failing before a session is persisted.
        let microphone_ready_result = microphone_ready.recv_timeout(Duration::from_secs(2));
        if !request.include_system_audio {
            match microphone_ready_result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    stop.store(true, Ordering::Release);
                    let _ = workers.pop().expect("microphone worker exists").join();
                    return Err(CoreError::Unavailable(error));
                }
                Err(_) => {
                    stop.store(true, Ordering::Release);
                    let _ = workers.pop().expect("microphone worker exists").join();
                    return Err(CoreError::Unavailable(
                        "microphone capture exited during startup".into(),
                    ));
                }
            }
        }
        if request.include_system_audio {
            let (system, system_ready) = spawn_channel(
                capture_id.clone(),
                AudioChannel::System,
                None,
                request.output_directory,
                Arc::clone(&stop),
                Arc::clone(&events),
                Arc::clone(&activity),
                Arc::clone(&packets),
            )?;
            workers.push(system);
            // A meeting may continue on the microphone if loopback is unavailable. The stopped
            // worker's report records the failed channel and explains the missing interval.
            let _ = system_ready.recv_timeout(Duration::from_secs(2));
        }
        self.active
            .lock()
            .map_err(|_| poisoned("active capture"))?
            .insert(
                capture_id.clone(),
                ActiveCapture {
                    stop,
                    workers,
                    events,
                    activity,
                    packets,
                },
            );
        Ok(capture_id)
    }

    fn stop(&self, capture_id: &str) -> CoreResult<Vec<PathBuf>> {
        self.finish(capture_id, false)
    }

    fn cancel(&self, capture_id: &str) -> CoreResult<()> {
        self.finish(capture_id, true).map(|_| ())
    }
}

fn spawn_channel(
    capture_id: String,
    channel: AudioChannel,
    requested_device_id: Option<String>,
    output_directory: PathBuf,
    stop: Arc<AtomicBool>,
    events: Arc<Mutex<Vec<CaptureEvent>>>,
    activity: Arc<Mutex<HashMap<AudioChannel, Instant>>>,
    packets: Arc<PacketHub>,
) -> CoreResult<(
    JoinHandle<ChannelCaptureReport>,
    mpsc::Receiver<Result<(), String>>,
)> {
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let worker = thread::Builder::new()
        .name(format!("murmur-audio-{}", channel.file_label()))
        .spawn(move || {
            capture_channel(
                &capture_id,
                channel,
                requested_device_id,
                &output_directory,
                &stop,
                ready_tx,
                &events,
                &activity,
                &packets,
            )
        })
        .map_err(CoreError::Io)?;
    Ok((worker, ready_rx))
}

fn capture_channel(
    capture_id: &str,
    channel: AudioChannel,
    requested_device_id: Option<String>,
    output_directory: &Path,
    stop: &AtomicBool,
    ready: mpsc::SyncSender<Result<(), String>>,
    events: &Mutex<Vec<CaptureEvent>>,
    activity: &Mutex<HashMap<AudioChannel, Instant>>,
    packets: &PacketHub,
) -> ChannelCaptureReport {
    let started = Instant::now();
    let mut report = ChannelCaptureReport {
        channel,
        device_id: None,
        chunks: Vec::new(),
        gaps: Vec::new(),
        fatal_error: None,
    };
    let com = match ComApartment::initialize() {
        Ok(com) => com,
        Err(error) => {
            report.fatal_error = Some(error.to_string());
            let _ = ready.send(Err(error.to_string()));
            return report;
        }
    };
    let _com = com;

    let enumerator: IMMDeviceEnumerator =
        match unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) } {
            Ok(enumerator) => enumerator,
            Err(error) => {
                report.fatal_error = Some(format!("create audio device enumerator: {error}"));
                let _ = ready.send(Err(format!("create audio device enumerator: {error}")));
                return report;
            }
        };

    let device = match resolve_device(&enumerator, channel, requested_device_id.as_deref()) {
        Ok(device) => device,
        Err(error) => {
            report.fatal_error = Some(error.to_string());
            let _ = ready.send(Err(error.to_string()));
            return report;
        }
    };
    let device_id = match device_id(&device) {
        Ok(value) => value,
        Err(error) => {
            report.fatal_error = Some(error.to_string());
            let _ = ready.send(Err(error.to_string()));
            return report;
        }
    };
    report.device_id = Some(device_id.clone());

    let mut chunk_index = 0_u32;
    let mut gap_index: Option<usize> = None;
    let mut ready = Some(ready);
    while !stop.load(Ordering::Acquire) {
        let device = match unsafe { enumerator.GetDevice(&HSTRING::from(&device_id)) } {
            Ok(device) => device,
            Err(error) => {
                open_gap(
                    &mut report,
                    &mut gap_index,
                    started,
                    error.to_string(),
                    events,
                );
                sleep_interruptibly(stop, Duration::from_secs(1));
                continue;
            }
        };
        match capture_client(
            capture_id,
            channel,
            &device,
            output_directory,
            &mut chunk_index,
            stop,
            &mut report.chunks,
            &mut report.gaps,
            &mut gap_index,
            started,
            events,
            &mut ready,
            activity,
            packets,
        ) {
            Ok(()) => {}
            Err(error) if !stop.load(Ordering::Acquire) => {
                open_gap(
                    &mut report,
                    &mut gap_index,
                    started,
                    error.to_string(),
                    events,
                );
                sleep_interruptibly(stop, Duration::from_secs(1));
            }
            Err(_) => break,
        }
    }
    if let Some(ready) = ready {
        let error = report
            .gaps
            .last()
            .map(|gap| gap.reason.clone())
            .unwrap_or_else(|| "audio capture stopped during startup".into());
        let _ = ready.send(Err(error));
    }
    report
}

fn resolve_device(
    enumerator: &IMMDeviceEnumerator,
    channel: AudioChannel,
    requested_device_id: Option<&str>,
) -> CoreResult<IMMDevice> {
    unsafe {
        if let Some(id) = requested_device_id {
            return enumerator
                .GetDevice(&HSTRING::from(id))
                .map_err(|error| native_error("open configured microphone", error));
        }
        let flow = match channel {
            AudioChannel::Microphone => eCapture,
            AudioChannel::System => eRender,
        };
        enumerator
            .GetDefaultAudioEndpoint(flow, eConsole)
            .map_err(|error| native_error("open current default audio endpoint", error))
    }
}

fn device_id(device: &IMMDevice) -> CoreResult<String> {
    unsafe {
        let id = device
            .GetId()
            .map_err(|error| native_error("read audio device ID", error))?;
        let value = PCWSTR(id.0)
            .to_string()
            .map_err(|error| native_error("decode audio device ID", error.into()));
        CoTaskMemFree(Some(id.0.cast()));
        value
    }
}

#[allow(clippy::too_many_arguments)]
fn capture_client(
    capture_id: &str,
    channel: AudioChannel,
    device: &IMMDevice,
    output_directory: &Path,
    chunk_index: &mut u32,
    stop: &AtomicBool,
    chunks: &mut Vec<PathBuf>,
    gaps: &mut [CaptureGap],
    gap_index: &mut Option<usize>,
    capture_started: Instant,
    events: &Mutex<Vec<CaptureEvent>>,
    ready: &mut Option<mpsc::SyncSender<Result<(), String>>>,
    activity: &Mutex<HashMap<AudioChannel, Instant>>,
    packets: &PacketHub,
) -> CoreResult<()> {
    unsafe {
        let client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|error| native_error("activate WASAPI endpoint", error))?;
        let format_ptr = client
            .GetMixFormat()
            .map_err(|error| native_error("read WASAPI mix format", error))?;
        let format = WaveFormat::copy_from(format_ptr);
        let flags = if channel == AudioChannel::System {
            AUDCLNT_STREAMFLAGS_LOOPBACK
        } else {
            0
        };
        let initialized = client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            flags,
            SHARED_BUFFER_100NS,
            0,
            format_ptr,
            None,
        );
        CoTaskMemFree(Some(format_ptr.cast()));
        initialized.map_err(|error| native_error("initialize WASAPI capture", error))?;
        let capture: IAudioCaptureClient = client
            .GetService()
            .map_err(|error| native_error("create WASAPI capture client", error))?;
        client
            .Start()
            .map_err(|error| native_error("start WASAPI capture", error))?;
        close_gap(channel, gaps, gap_index, capture_started, events);
        if let Some(ready) = ready.take() {
            let _ = ready.send(Ok(()));
        }
        let _guard = AudioClientGuard(&client);
        let mut writer: Option<WaveChunk> = None;
        let packet_format = format.as_wasapi_format();

        let capture_result = (|| -> CoreResult<()> {
            while !stop.load(Ordering::Acquire) {
                let mut packet_size = capture
                    .GetNextPacketSize()
                    .map_err(|error| native_error("query WASAPI packet", error))?;
                if packet_size == 0 {
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
                while packet_size > 0 {
                    let mut data = std::ptr::null_mut();
                    let mut frames = 0;
                    let mut packet_flags = 0;
                    let mut device_position = 0_u64;
                    let mut qpc_position = 0_u64;
                    capture
                        .GetBuffer(
                            &mut data,
                            &mut frames,
                            &mut packet_flags,
                            Some(&mut device_position),
                            Some(&mut qpc_position),
                        )
                        .map_err(|error| native_error("read WASAPI packet", error))?;
                    let captured_millis = millis(capture_started);
                    let bytes = frames as usize * format.base.nBlockAlign as usize;
                    let silent = packet_flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                    let packet = if silent {
                        vec![0; bytes]
                    } else {
                        slice::from_raw_parts(data, bytes).to_vec()
                    };
                    capture
                        .ReleaseBuffer(frames)
                        .map_err(|error| native_error("release WASAPI packet", error))?;
                    let statistics = if silent {
                        Some(PacketStatistics {
                            sample_count: frames as u64 * format.base.nChannels as u64,
                            ..Default::default()
                        })
                    } else {
                        packet_statistics(&packet, &packet_format)
                    };
                    if packet_has_signal(&packet, silent, &format, statistics) {
                        if let Ok(mut activity) = activity.lock() {
                            activity.insert(channel, Instant::now());
                        }
                    }

                    let mut offset = 0;
                    while offset < packet.len() {
                        if writer.is_none() {
                            let path = output_directory.join(format!(
                                "{capture_id}-{}-{chunk_index:05}.wav",
                                channel.file_label()
                            ));
                            *chunk_index += 1;
                            writer = Some(WaveChunk::create(path, format.clone())?);
                        }
                        let current = writer.as_mut().expect("wave chunk exists");
                        let count = current.remaining_capacity().min(packet.len() - offset);
                        current.write_all(&packet[offset..offset + count])?;
                        offset += count;
                        if current.remaining_capacity() == 0 {
                            let completed = writer.take().expect("wave chunk exists").finish()?;
                            chunks.push(completed);
                        }
                    }
                    // Durable WAV writing is authoritative. Live consumers receive a cloned packet
                    // only after the full packet has reached its current chunk or chunks.
                    if channel == AudioChannel::Microphone {
                        packets.publish(
                            AudioPacket {
                                sequence: 0,
                                dropped_before: 0,
                                channel,
                                captured_millis,
                                frames,
                                flags: packet_flags,
                                device_position,
                                qpc_position,
                                format: packet_format.clone(),
                                bytes: packet,
                            },
                            statistics,
                        );
                    }
                    packet_size = capture
                        .GetNextPacketSize()
                        .map_err(|error| native_error("query next WASAPI packet", error))?;
                }
            }
            Ok(())
        })();
        // Device invalidation must not discard the partial chunk captured before the gap.
        if let Some(writer) = writer {
            chunks.push(writer.finish()?);
        }
        capture_result
    }
}

struct AudioClientGuard<'a>(&'a IAudioClient);

impl Drop for AudioClientGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            let _ = self.0.Stop();
        }
    }
}

struct WaveChunk {
    path: PathBuf,
    file: File,
    format: WaveFormat,
    bytes: u32,
}

impl WaveChunk {
    fn create(path: PathBuf, format: WaveFormat) -> CoreResult<Self> {
        let mut file = File::create(&path)?;
        write_wave_header(&mut file, &format, 0)?;
        file.sync_data()?;
        Ok(Self {
            path,
            file,
            format,
            bytes: 0,
        })
    }

    fn write_all(&mut self, data: &[u8]) -> CoreResult<()> {
        self.file.write_all(data)?;
        self.bytes = self
            .bytes
            .checked_add(data.len() as u32)
            .ok_or_else(|| CoreError::Unavailable("audio chunk exceeds WAV size limit".into()))?;
        Ok(())
    }

    fn remaining_capacity(&self) -> usize {
        let bytes_per_second = self.format.base.nAvgBytesPerSec as usize;
        let block_align = self.format.base.nBlockAlign.max(1) as usize;
        let five_seconds = bytes_per_second * CHUNK_DURATION.as_secs() as usize;
        let capacity = five_seconds - (five_seconds % block_align);
        capacity.saturating_sub(self.bytes as usize)
    }

    fn finish(mut self) -> CoreResult<PathBuf> {
        self.file.seek(SeekFrom::Start(0))?;
        write_wave_header(&mut self.file, &self.format, self.bytes)?;
        self.file.flush()?;
        self.file.sync_all()?;
        Ok(self.path)
    }
}

#[derive(Clone)]
struct WaveFormat {
    base: WAVEFORMATEX,
    bytes: Vec<u8>,
}

impl WaveFormat {
    unsafe fn copy_from(format: *const WAVEFORMATEX) -> Self {
        let base = *format;
        let length = std::mem::size_of::<WAVEFORMATEX>() + base.cbSize as usize;
        Self {
            base,
            bytes: slice::from_raw_parts(format.cast::<u8>(), length).to_vec(),
        }
    }

    #[cfg(test)]
    fn from_base(base: WAVEFORMATEX) -> Self {
        unsafe { Self::copy_from(&base) }
    }

    fn as_wasapi_format(&self) -> WasapiFormat {
        let encoding_tag = if self.base.wFormatTag == 0xfffe && self.bytes.len() >= 28 {
            u32::from_le_bytes(
                self.bytes[24..28]
                    .try_into()
                    .expect("checked format length"),
            ) as u16
        } else {
            self.base.wFormatTag
        };
        let encoding = match encoding_tag {
            1 => SampleEncoding::Pcm,
            3 => SampleEncoding::Float,
            _ => SampleEncoding::Other,
        };
        WasapiFormat {
            raw: self.bytes.clone(),
            format_tag: self.base.wFormatTag,
            channels: self.base.nChannels,
            sample_rate: self.base.nSamplesPerSec,
            average_bytes_per_second: self.base.nAvgBytesPerSec,
            block_align: self.base.nBlockAlign,
            bits_per_sample: self.base.wBitsPerSample,
            encoding,
        }
    }
}

fn write_wave_header(file: &mut File, format: &WaveFormat, bytes: u32) -> CoreResult<()> {
    let format_size = format.bytes.len() as u32;
    let riff_size = 4 + 8 + format_size + 8 + bytes;
    file.write_all(b"RIFF")?;
    file.write_all(&riff_size.to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&format_size.to_le_bytes())?;
    file.write_all(&format.bytes)?;
    file.write_all(b"data")?;
    file.write_all(&bytes.to_le_bytes())?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WaveDataLayout {
    pub data_offset: usize,
    pub data_size_offset: usize,
}

/// Locates the RIFF `data` chunk without assuming a fixed WAVEFORMATEX header length.
pub(crate) fn wave_data_layout(wave: &[u8]) -> CoreResult<WaveDataLayout> {
    if wave.len() < 12 || &wave[..4] != b"RIFF" || &wave[8..12] != b"WAVE" {
        return Err(CoreError::InvalidInput("invalid RIFF/WAVE header".into()));
    }
    let mut offset = 12_usize;
    while offset.checked_add(8).is_some_and(|end| end <= wave.len()) {
        let size = u32::from_le_bytes(
            wave[offset + 4..offset + 8]
                .try_into()
                .expect("checked RIFF chunk header"),
        ) as usize;
        if &wave[offset..offset + 4] == b"data" {
            return Ok(WaveDataLayout {
                data_offset: offset + 8,
                data_size_offset: offset + 4,
            });
        }
        let padded_size = size
            .checked_add(size % 2)
            .ok_or_else(|| CoreError::InvalidInput("RIFF chunk size overflow".into()))?;
        offset = offset
            .checked_add(8)
            .and_then(|value| value.checked_add(padded_size))
            .filter(|value| *value <= wave.len())
            .ok_or_else(|| CoreError::InvalidInput("truncated RIFF chunk".into()))?;
    }
    Err(CoreError::InvalidInput(
        "WAVE file has no data chunk".into(),
    ))
}

fn open_gap(
    report: &mut ChannelCaptureReport,
    gap_index: &mut Option<usize>,
    started: Instant,
    reason: String,
    events: &Mutex<Vec<CaptureEvent>>,
) {
    if gap_index.is_none() {
        report.gaps.push(CaptureGap {
            channel: report.channel,
            started_millis: millis(started),
            ended_millis: None,
            reason,
        });
        if let Ok(mut events) = events.lock() {
            events.push(CaptureEvent::ChannelLost {
                channel: report.channel,
                at_millis: millis(started),
                reason: report.gaps.last().expect("gap exists").reason.clone(),
            });
        }
        *gap_index = Some(report.gaps.len() - 1);
    }
}

fn close_gap(
    channel: AudioChannel,
    gaps: &mut [CaptureGap],
    gap_index: &mut Option<usize>,
    started: Instant,
    events: &Mutex<Vec<CaptureEvent>>,
) {
    if let Some(index) = gap_index.take() {
        let at_millis = millis(started);
        gaps[index].ended_millis = Some(at_millis);
        if let Ok(mut events) = events.lock() {
            events.push(CaptureEvent::ChannelRecovered { channel, at_millis });
        }
    }
}

fn millis(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
}

fn activity_from_timestamps(
    timestamps: &HashMap<AudioChannel, Instant>,
    now: Instant,
) -> CaptureActivity {
    let active = |channel| {
        timestamps
            .get(&channel)
            .is_some_and(|last| now.saturating_duration_since(*last) <= ACTIVITY_RECENCY)
    };
    CaptureActivity {
        microphone_active: active(AudioChannel::Microphone),
        system_active: active(AudioChannel::System),
    }
}

fn packet_statistics(packet: &[u8], format: &WasapiFormat) -> Option<PacketStatistics> {
    match (format.encoding, format.bits_per_sample) {
        (SampleEncoding::Float, 32) => {
            statistics_from_samples(packet.chunks_exact(4).filter_map(|sample| {
                let value = f32::from_le_bytes(sample.try_into().expect("four-byte sample"));
                value.is_finite().then_some(value)
            }))
        }
        (SampleEncoding::Pcm, 16) => {
            statistics_from_samples(packet.chunks_exact(2).map(|sample| {
                i16::from_le_bytes(sample.try_into().expect("two-byte sample")) as f32 / 32768.0
            }))
        }
        _ => return None,
    }
}

fn statistics_from_samples(samples: impl Iterator<Item = f32>) -> Option<PacketStatistics> {
    let statistics = samples.fold(PacketStatistics::default(), |mut statistics, sample| {
        let sample = sample.clamp(-1.0, 1.0);
        statistics.sum_squares += f64::from(sample) * f64::from(sample);
        statistics.sample_count = statistics.sample_count.saturating_add(1);
        statistics.peak = statistics.peak.max(sample.abs());
        statistics
    });
    (statistics.sample_count > 0).then_some(statistics)
}

fn packet_has_signal(
    packet: &[u8],
    silent: bool,
    format: &WaveFormat,
    statistics: Option<PacketStatistics>,
) -> bool {
    if silent || packet.is_empty() {
        return false;
    }
    if let Some(statistics) = statistics {
        statistics.rms() >= 0.002
    } else {
        // Unknown shared-mode formats are uncommon. Treat only exact digital silence as inactive.
        let _ = format;
        packet.iter().any(|byte| *byte != 0)
    }
}

fn sleep_interruptibly(stop: &AtomicBool, duration: Duration) {
    let until = Instant::now() + duration;
    while !stop.load(Ordering::Acquire) && Instant::now() < until {
        thread::sleep(Duration::from_millis(25));
    }
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> CoreResult<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|error| native_error("initialize COM for audio capture", error))?;
        }
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}

fn poisoned(name: &str) -> CoreError {
    CoreError::Unavailable(format!("{name} lock is poisoned"))
}

fn native_error(action: &str, error: windows::core::Error) -> CoreError {
    CoreError::Unavailable(format!("{action}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio_packet(byte: u8) -> AudioPacket {
        AudioPacket {
            sequence: 0,
            dropped_before: 0,
            channel: AudioChannel::Microphone,
            captured_millis: 125,
            frames: 1,
            flags: 2,
            device_position: 3,
            qpc_position: 4,
            format: WaveFormat::from_base(pcm_format()).as_wasapi_format(),
            bytes: vec![byte, 0],
        }
    }

    fn pcm_format() -> WAVEFORMATEX {
        WAVEFORMATEX {
            wFormatTag: 1,
            nChannels: 1,
            nSamplesPerSec: 16_000,
            nAvgBytesPerSec: 32_000,
            nBlockAlign: 2,
            wBitsPerSample: 16,
            cbSize: 0,
        }
    }

    #[test]
    fn finalizes_a_valid_wave_header() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("chunk.wav");
        let mut chunk =
            WaveChunk::create(path.clone(), WaveFormat::from_base(pcm_format())).unwrap();
        chunk.write_all(&[1, 2, 3, 4]).unwrap();
        chunk.finish().unwrap();
        let bytes = fs::read(path).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes(bytes[42..46].try_into().unwrap()), 4);
        assert_eq!(&bytes[46..], &[1, 2, 3, 4]);
    }

    #[test]
    fn locates_pcm_and_extensible_wave_data_without_fixed_offsets() {
        let directory = tempfile::tempdir().unwrap();
        let pcm_path = directory.path().join("pcm.wav");
        WaveChunk::create(pcm_path.clone(), WaveFormat::from_base(pcm_format()))
            .unwrap()
            .finish()
            .unwrap();
        assert_eq!(
            wave_data_layout(&fs::read(pcm_path).unwrap())
                .unwrap()
                .data_offset,
            46
        );

        let mut base = pcm_format();
        base.wFormatTag = 0xfffe;
        base.cbSize = 22;
        let mut raw = WaveFormat::from_base(pcm_format()).bytes;
        raw[0..2].copy_from_slice(&base.wFormatTag.to_le_bytes());
        raw[16..18].copy_from_slice(&base.cbSize.to_le_bytes());
        raw.resize(40, 0);
        raw[24..28].copy_from_slice(&1_u32.to_le_bytes());
        let extensible_path = directory.path().join("extensible.wav");
        WaveChunk::create(extensible_path.clone(), WaveFormat { base, bytes: raw })
            .unwrap()
            .finish()
            .unwrap();
        let layout = wave_data_layout(&fs::read(extensible_path).unwrap()).unwrap();
        assert_eq!(layout.data_offset, 68);
        assert_eq!(layout.data_size_offset, 64);
    }

    #[test]
    fn audio_packet_serializes_exact_format_bytes_and_camel_case_fields() {
        let packet = audio_packet(7);
        let raw = packet.format.raw.clone();
        assert_eq!(
            serde_json::to_value(packet).unwrap(),
            serde_json::json!({
                "sequence": 0,
                "droppedBefore": 0,
                "channel": "microphone",
                "capturedMillis": 125,
                "frames": 1,
                "flags": 2,
                "devicePosition": 3,
                "qpcPosition": 4,
                "format": {
                    "raw": raw,
                    "formatTag": 1,
                    "channels": 1,
                    "sampleRate": 16_000,
                    "averageBytesPerSecond": 32_000,
                    "blockAlign": 2,
                    "bitsPerSample": 16,
                    "encoding": "pcm"
                },
                "bytes": [7, 0]
            })
        );
    }

    #[test]
    fn extensible_format_uses_subformat_encoding_and_preserves_raw_bytes() {
        let mut base = pcm_format();
        base.wFormatTag = 0xfffe;
        base.cbSize = 22;
        let mut raw = WaveFormat::from_base(pcm_format()).bytes;
        raw[0..2].copy_from_slice(&0xfffe_u16.to_le_bytes());
        raw[16..18].copy_from_slice(&22_u16.to_le_bytes());
        raw.resize(40, 0);
        raw[24..28].copy_from_slice(&3_u32.to_le_bytes());
        let format = WaveFormat {
            base,
            bytes: raw.clone(),
        }
        .as_wasapi_format();
        assert_eq!(format.raw, raw);
        assert_eq!(format.format_tag, 0xfffe);
        assert_eq!(format.encoding, SampleEncoding::Float);
    }

    #[test]
    fn saturated_subscriber_never_blocks_packet_publishing() {
        let hub = Arc::new(PacketHub::default());
        let _receiver = hub.subscribe().unwrap();
        let (finished_tx, finished_rx) = mpsc::sync_channel(1);
        let publishing_hub = Arc::clone(&hub);
        thread::spawn(move || {
            for byte in 0..=u8::MAX {
                publishing_hub.publish(audio_packet(byte), None);
            }
            let _ = finished_tx.send(());
        });
        finished_rx
            .recv_timeout(Duration::from_millis(250))
            .expect("bounded publishing must not wait for a slow subscriber");
    }

    #[test]
    fn disconnected_subscriber_is_removed_on_publish() {
        let hub = PacketHub::default();
        let receiver = hub.subscribe().unwrap();
        drop(receiver);
        hub.publish(audio_packet(1), None);
        assert!(hub.subscribers.lock().unwrap().is_empty());
    }

    #[test]
    fn first_delivery_after_saturation_reports_dropped_packets() {
        let hub = PacketHub::default();
        let receiver = hub.subscribe().unwrap();
        for byte in 0..PACKET_QUEUE_CAPACITY as u8 {
            hub.publish(audio_packet(byte), None);
        }
        hub.publish(audio_packet(100), None);
        assert_eq!(receiver.recv().unwrap().sequence, 0);
        hub.publish(audio_packet(101), None);

        let delivered = (0..PACKET_QUEUE_CAPACITY)
            .map(|_| receiver.recv().unwrap())
            .last()
            .unwrap();
        assert_eq!(delivered.sequence, (PACKET_QUEUE_CAPACITY + 1) as u64);
        assert_eq!(delivered.dropped_before, 1);
        assert_eq!(delivered.bytes, vec![101, 0]);
    }

    #[test]
    fn saturated_live_queue_does_not_prevent_durable_wave_finalization() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("durable.wav");
        let mut chunk =
            WaveChunk::create(path.clone(), WaveFormat::from_base(pcm_format())).unwrap();
        let hub = PacketHub::default();
        let _receiver = hub.subscribe().unwrap();
        for byte in 0..32 {
            chunk.write_all(&[byte, 0]).unwrap();
            hub.publish(audio_packet(byte), None);
        }
        chunk.finish().unwrap();

        let wave = fs::read(path).unwrap();
        assert_eq!(u32::from_le_bytes(wave[42..46].try_into().unwrap()), 64);
        assert_eq!(wave.len(), 46 + 64);
    }

    #[test]
    fn opens_only_one_gap_until_recovery() {
        let mut report = ChannelCaptureReport {
            channel: AudioChannel::Microphone,
            device_id: None,
            chunks: vec![],
            gaps: vec![],
            fatal_error: None,
        };
        let mut index = None;
        let started = Instant::now();
        let events = Mutex::new(Vec::new());
        open_gap(&mut report, &mut index, started, "lost".into(), &events);
        open_gap(
            &mut report,
            &mut index,
            started,
            "lost again".into(),
            &events,
        );
        assert_eq!(report.gaps.len(), 1);
        close_gap(
            report.channel,
            &mut report.gaps,
            &mut index,
            started,
            &events,
        );
        assert!(report.gaps[0].ended_millis.is_some());
        assert_eq!(events.lock().unwrap().len(), 2);
    }

    #[test]
    fn chunk_capacity_is_exactly_five_seconds_and_block_aligned() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("chunk.wav");
        let chunk = WaveChunk::create(path, WaveFormat::from_base(pcm_format())).unwrap();
        assert_eq!(chunk.remaining_capacity(), 160_000);
        assert_eq!(chunk.remaining_capacity() % 2, 0);
    }

    #[test]
    fn microphone_list_puts_default_first_then_sorts_stably() {
        let mut devices = vec![
            MicrophoneDevice {
                id: "z".into(),
                label: "USB microphone".into(),
                is_default: false,
            },
            MicrophoneDevice {
                id: "b".into(),
                label: "Built-in array".into(),
                is_default: true,
            },
            MicrophoneDevice {
                id: "a".into(),
                label: "Studio microphone".into(),
                is_default: false,
            },
        ];
        sort_microphones(&mut devices);
        assert_eq!(
            devices
                .iter()
                .map(|device| device.id.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "a", "z"]
        );
    }

    #[test]
    fn microphone_descriptor_uses_camel_case_for_tauri() {
        let device = MicrophoneDevice {
            id: "endpoint".into(),
            label: "Microphone".into(),
            is_default: true,
        };
        assert_eq!(
            serde_json::to_value(device).unwrap(),
            serde_json::json!({
                "id": "endpoint",
                "label": "Microphone",
                "isDefault": true
            })
        );
    }

    #[test]
    fn sample_energy_ignores_silence_and_detects_speech_level_pcm() {
        let format = WaveFormat::from_base(pcm_format());
        let packet_format = format.as_wasapi_format();
        let quiet = vec![0_u8; 320];
        let speech: Vec<u8> = (0..160)
            .flat_map(|index| {
                let sample: i16 = if index % 2 == 0 { 4_000 } else { -4_000 };
                sample.to_le_bytes()
            })
            .collect();
        assert!(!packet_has_signal(
            &quiet,
            false,
            &format,
            packet_statistics(&quiet, &packet_format)
        ));
        assert!(!packet_has_signal(
            &speech,
            true,
            &format,
            packet_statistics(&speech, &packet_format)
        ));
        assert!(packet_has_signal(
            &speech,
            false,
            &format,
            packet_statistics(&speech, &packet_format)
        ));
    }

    #[test]
    fn packet_statistics_reports_pcm16_rms_and_peak() {
        let format = WaveFormat::from_base(pcm_format()).as_wasapi_format();
        let packet: Vec<u8> = [-16_384_i16, 0, 16_384]
            .into_iter()
            .flat_map(i16::to_le_bytes)
            .collect();
        let statistics = packet_statistics(&packet, &format).unwrap();
        assert!((statistics.peak - 0.5).abs() < 0.0001);
        assert!((statistics.rms() - (0.5_f32 / 3.0).sqrt()).abs() < 0.0001);
    }

    #[test]
    fn packet_statistics_reports_float32_and_ignores_non_finite_samples() {
        let mut format = WaveFormat::from_base(pcm_format()).as_wasapi_format();
        format.encoding = SampleEncoding::Float;
        format.bits_per_sample = 32;
        let packet: Vec<u8> = [0.5_f32, -0.25, 2.0, f32::NAN]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        let statistics = packet_statistics(&packet, &format).unwrap();
        assert_eq!(statistics.sample_count, 3);
        assert_eq!(statistics.peak, 1.0);
        assert!((statistics.rms() - (1.3125_f32 / 3.0).sqrt()).abs() < 0.0001);
    }

    #[test]
    fn packet_statistics_rejects_unsupported_formats() {
        let mut format = WaveFormat::from_base(pcm_format()).as_wasapi_format();
        format.encoding = SampleEncoding::Other;
        assert!(packet_statistics(&[1, 2, 3, 4], &format).is_none());
    }

    #[test]
    fn level_accumulator_weights_packets_and_resets_after_take() {
        let mut levels = LevelAccumulator::default();
        levels.push(PacketStatistics {
            sum_squares: 1.0,
            sample_count: 1,
            peak: 1.0,
        });
        levels.push(PacketStatistics {
            sum_squares: 0.0,
            sample_count: 3,
            peak: 0.0,
        });
        let level = levels.take().unwrap();
        assert!((level.rms - 0.5).abs() < 0.0001);
        assert_eq!(level.peak, 1.0);
        assert_eq!(levels.take(), None);
    }

    #[test]
    fn meter_accumulates_even_when_live_subscriber_is_saturated() {
        let hub = PacketHub::default();
        let _receiver = hub.subscribe().unwrap();
        for byte in 0..32 {
            hub.publish(
                audio_packet(byte),
                Some(PacketStatistics {
                    sum_squares: 0.25,
                    sample_count: 1,
                    peak: 0.5,
                }),
            );
        }
        let level = hub.take_level().unwrap().unwrap();
        assert!((level.rms - 0.5).abs() < 0.0001);
        assert_eq!(level.peak, 0.5);
    }

    #[test]
    fn channel_activity_expires_after_two_seconds() {
        let now = Instant::now();
        let timestamps = HashMap::from([
            (AudioChannel::Microphone, now - Duration::from_secs(1)),
            (AudioChannel::System, now - Duration::from_secs(3)),
        ]);
        assert_eq!(
            activity_from_timestamps(&timestamps, now),
            CaptureActivity {
                microphone_active: true,
                system_active: false,
            }
        );
    }

    #[test]
    fn capture_events_have_stable_camel_case_shapes() {
        assert_eq!(
            serde_json::to_value(CaptureEvent::ChannelLost {
                channel: AudioChannel::System,
                at_millis: 1_250,
                reason: "device invalidated".into(),
            })
            .unwrap(),
            serde_json::json!({
                "kind": "channelLost",
                "channel": "system",
                "atMillis": 1250,
                "reason": "device invalidated"
            })
        );
        assert_eq!(
            serde_json::to_value(CaptureEvent::ChannelRecovered {
                channel: AudioChannel::Microphone,
                at_millis: 2_000,
            })
            .unwrap(),
            serde_json::json!({
                "kind": "channelRecovered",
                "channel": "microphone",
                "atMillis": 2000
            })
        );
    }
}
