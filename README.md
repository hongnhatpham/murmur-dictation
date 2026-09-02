# Murmur

Murmur is a personal Windows 11 voice workspace for English, Vietnamese, and mixed-language dictation and meeting records.

The app uses Tauri 2, React, TypeScript, Rust, SQLite, WASAPI, Windows UI Automation, and Windows Credential Manager. Dictation streams through Deepgram Flux while the user speaks, with AssemblyAI as the live fallback. Finalized audio tries Deepgram, Groq Whisper, AssemblyAI, then local `whisper.cpp`. Meeting Sessions record microphone and system audio separately, transcribe after recording, preserve an immutable Raw Transcript, and create a cited Meeting Brief when Groq is configured.

## Current operation

The Windows build includes:

- Global hold-to-talk and toggle shortcuts, with `Ctrl+Win` as the default hold shortcut
- A non-activating dictation overlay with a live speech signal and clear processing states, Escape cancellation, target-verified undo-friendly insertion, and every completed result retained on the clipboard
- Default or selected microphone capture with five-second durable WAV chunks
- Hosted streaming and batch STT rotation with local CUDA Whisper fallback
- English, Vietnamese, and mixed-language detection for local Whisper
- Context exclusions for sensitive fields and applications
- Imported, online, and in-person Meeting Sessions
- Editable timestamped transcripts, speaker labels, cited Meeting Briefs, search, pinning, and Markdown export
- Seven-day Dictation Session and 30-day Meeting Session retention defaults
- Personal Vocabulary and attributed-correction learning
- Consistent ZIP backup and staged restore without credentials
- Redacted rotating diagnostics, deterministic setup, startup registration, and signed private-update handling

On the current RTX 3060 laptop, the public ten-second JFK fixture completes direct local CUDA transcription in about 2.0 seconds. A fresh import reached the visible literal transcript in 3.8 seconds. Ordinary Settings loading and dictation use lightweight installation checks. Full model checksums run during installation, diagnostics, and the explicit offline speech check.

Hosted STT, correction, Meeting Brief generation, and private updates require user-supplied credentials. Local dictation and literal meeting transcripts remain available without them. Provider validation uses each service's least-privileged speech or account endpoint, so setup does not require broader management permissions.

## Run

Requirements for development:

- Node.js 24 or newer with Corepack
- Rust stable
- Windows 11 and Visual Studio C++ Build Tools

```powershell
corepack pnpm install
corepack pnpm desktop:dev
```

Run checks:

```powershell
corepack pnpm typecheck
corepack pnpm test
corepack pnpm build
& "$env:USERPROFILE\.cargo\bin\cargo.exe" test --manifest-path src-tauri\Cargo.toml --all-targets
```

Build the native app without an installer:

```powershell
corepack pnpm exec tauri build --no-bundle --debug
```

Build the signed NSIS installer and updater artifacts after generating the local signing key:

```powershell
corepack pnpm updater:key:init
corepack pnpm desktop:build:signed
```

The signing key lives under `%APPDATA%\com.bynhat.murmur\signing`, outside Git and Murmur backups. Its password is protected with Windows DPAPI for the current user.

## Setup

The installed application includes a structured setup CLI. Commands return JSON. `secret set` reads the value from standard input so secrets do not enter command arguments, files, logs, or Git.

```powershell
Murmur.exe --setup plan
Murmur.exe --setup diagnose
Murmur.exe --setup secret status
Murmur.exe --setup secret set deepgram
Murmur.exe --setup provider validate deepgram
Murmur.exe --setup model install
Murmur.exe --setup insertion check --confirm-target-ready
```

The UI can also store and validate provider credentials, install the offline model, create or restore backups, export diagnostics, and check for updates.

Offline files live under `%APPDATA%\com.bynhat.murmur\models`. The setup process installs and verifies the pinned whisper.cpp CUDA 12.4 runtime and `ggml-large-v3-turbo-q5_0` model.

## Product contract

- [Vision](VISION.md)
- [V1 product definition](docs/product/v1.md)
- [Domain language](CONTEXT.md)
- [Architecture decisions](docs/adr)
