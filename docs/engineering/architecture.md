# Architecture plan

## High-level shape

```text
Global hotkey
  -> audio recorder
  -> speech-to-text provider
  -> cleanup/transform provider
  -> insertion engine
  -> overlay + history
```

The first implementation should be a thin vertical slice, not a platform. Prove the latency and insertion loop before building complex UI.

## Proposed components

### 1. Daemon

Long-running background process responsible for:

- registering global hotkeys;
- starting/stopping audio capture;
- coordinating STT and cleanup;
- inserting text or falling back to clipboard;
- writing history;
- exposing local status events to the overlay.

Preferred long-term language: Rust.

Reasoning:

- good system integration;
- safe long-running daemon;
- strong process management;
- suitable for Wayland/Linux integration;
- easy to package later.

Prototype can be Python if faster for validating STT and audio.

### 2. Audio capture

Linux audio stack target: PipeWire/PulseAudio compatibility.

Initial options:

- `pw-record` subprocess for prototype simplicity;
- Rust `cpal` for native capture;
- GStreamer if pipeline control becomes useful.

MVP requirement: record while hotkey is held and produce a WAV/PCM buffer for STT.

### 3. Speech-to-text providers

Design as provider adapters behind a stable interface.

Candidate free/local providers:

- `faster-whisper` with a free Whisper-family model, preferably GPU-backed if available;
- `whisper.cpp` with a quantized free Whisper-family model for a simple local baseline;
- Vosk only if a low-resource/offline fallback is needed.

Cloud STT can remain an optional future adapter, but it is not the MVP default because the project should start with a free model.

Recommended validation path:

1. Start with a free local STT backend, likely `faster-whisper` or `whisper.cpp`.
2. Test model sizes for the best latency/accuracy tradeoff on this machine.
3. Keep provider switching configurable so a cloud adapter can be added later without changing the pipeline.

Provider interface:

```text
transcribe(audio, language_hint?, dictionary?) -> transcript + confidence/timing metadata
```

### 4. Cleanup/transform layer

Transforms transcript into final inserted text.

MVP cleanup responsibilities:

- remove filler words where safe;
- fix punctuation/capitalization;
- handle common self-correction phrases;
- detect trailing “press enter”;
- preserve meaning and avoid tone-heavy rewriting.

This can start as an LLM call with a strict prompt, then become hybrid:

- deterministic post-processing for simple commands;
- LLM cleanup for natural speech;
- app/mode-specific prompts later.

Transform interface:

```text
transform(transcript, mode, app_context?, selection?, dictionary?) -> final_text + actions
```

Where actions may include:

- insert text;
- press enter after insert;
- copy to clipboard;
- replace selection;
- show review diff later.

### 5. Insertion engine

Wayland makes universal text injection hard. Start simple and robust.

MVP strategy:

1. Save current clipboard contents if feasible.
2. Set clipboard to final text.
3. Simulate paste hotkey.
4. Keep final text in history.
5. If paste fails or cannot be triggered, leave text on clipboard and notify.

Potential tools/approaches:

- `wl-copy` for clipboard;
- `wtype` for simulated key events/text;
- `ydotool` where permitted;
- compositor-specific bindings for niri if useful;
- editor/browser extensions later for direct insertion.

Open issue: restoring the previous clipboard may conflict with user expectations. For MVP, prefer reliability and make clipboard mutation visible in history/overlay.

### 6. Focus/app context

MVP can avoid context. Later versions should detect:

- active app/window title;
- whether focused app is terminal/editor/browser/chat;
- selected text, if command mode is active.

Possible sources:

- compositor IPC if available;
- accessibility APIs where available;
- xdg-desktop-portal limitations;
- app-specific integrations for editors/browsers.

### 7. Overlay

Purpose: status only, not a full app.

States:

- idle/hidden;
- recording;
- processing;
- inserted;
- copied fallback;
- failed;
- command-mode active.

Implementation options:

- Quickshell/QML overlay for best desktop fit;
- Tauri/egui if portability matters;
- simple notification-first MVP.

Recommendation:

- Prototype with notifications/stdout/logs.
- Build Quickshell overlay after the daemon loop works.

### 8. Storage

Use SQLite for local state:

- dictation history;
- transcript/final text;
- timestamps;
- mode/provider;
- target app if known;
- fallback/error status;
- dictionary entries;
- snippets;
- settings.

Privacy defaults:

- local-only history;
- easy history disable/private mode;
- audio files deleted immediately or after a short configurable period;
- never store API keys in project files/logs.

## Configuration

Candidate config format: TOML.

Example:

```toml
[hotkeys]
dictate = "SUPER+SPACE"
raw = "SUPER+ALT+SPACE"
command = "SUPER+SHIFT+SPACE"

[stt]
provider = "faster-whisper"
model = "base.en"
fallback = "whisper-cpp"
language = "auto"

[cleanup]
provider = "llm"
default_mode = "clean"

[insertion]
method = "clipboard-paste"
press_enter_phrase = true

[privacy]
history = true
keep_audio_days = 0
```

## Development milestones

### Spike 1: CLI loop

- Record a fixed-duration audio clip.
- Transcribe it.
- Clean it.
- Copy to clipboard.

### Spike 2: Hotkey + paste

- Hold hotkey to record.
- Release to process.
- Paste into focused app.

### Spike 3: History + recovery

- Store transcript and final text.
- Add command to recopy last result.
- Add undo/recover path.

### Spike 4: Overlay

- Show recording/processing/result states.

### Spike 5: Personal dictionary

- Add dictionary file/table.
- Feed terms to STT/cleanup where supported.

## Main risks

- Wayland hotkeys and insertion reliability.
- End-to-end latency above the threshold where dictation feels worse than typing.
- STT quality for names, tools, code terms, and mixed-language speech.
- Cleanup overreach changing the user's intended meaning.
- Clipboard side effects.
- Local STT latency if the selected free model is too large or not GPU-accelerated.

## Recommended first tracer bullet

A single command that:

1. records audio from the default microphone;
2. sends it to one STT backend;
3. runs minimal cleanup;
4. copies the result to clipboard;
5. pastes it with a simulated shortcut;
6. logs transcript/final text locally.

If this loop feels good, everything else is worth building.
