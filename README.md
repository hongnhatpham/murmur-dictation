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
- Light or Medium dictation cleanup through Groq GPT-OSS 20B for every STT route, with raw insertion when hosted correction fails
- Personal Vocabulary and attributed-correction learning
- Consistent ZIP backup and staged restore without credentials
- Redacted rotating diagnostics, deterministic setup, startup registration, and signed GitHub-release update handling

On the current RTX 3060 laptop, the public ten-second JFK fixture completes direct local CUDA transcription in about 2.0 seconds. A fresh import reached the visible literal transcript in 3.8 seconds. Ordinary Settings loading and dictation use lightweight installation checks. Full model checksums run during installation, diagnostics, and the explicit offline speech check.

Dictation cleanup runs after every transcription route, hosted or local. Light (default) fixes punctuation, capitalization, and obvious grammar and keeps the spoken words, including self-corrections. Medium also keeps only the corrected version of a self-correction, drops fillers and false starts, and fixes clearly misheard words from context or Personal Vocabulary. Correction requests use `reasoning_effort: low`, `include_reasoning: false`, and a length-derived `max_completion_tokens`; measured p95 latency is about 1.6 s, so the correction timeout is 3 s and a timeout inserts the Raw Transcript marked `Raw`.

Hosted STT, correction, and Meeting Brief generation require user-supplied credentials. Updates from public GitHub releases do not require a token; a private release source does. Local dictation and literal meeting transcripts remain available without credentials. Provider validation uses each service's least-privileged speech or account endpoint, so setup does not require broader management permissions.

## Run

Requirements for development:

- Node.js 24 or newer and pnpm 10
- Rust stable
- Windows 11 and Visual Studio C++ Build Tools

```powershell
pnpm install
pnpm desktop:dev
```

Run checks:

```powershell
pnpm typecheck
pnpm test
pnpm build
& "$env:USERPROFILE\.cargo\bin\cargo.exe" test --manifest-path src-tauri\Cargo.toml --all-targets
```

Build the native app without an installer:

```powershell
pnpm exec tauri build --no-bundle --debug
```

Build the signed NSIS installer and updater artifacts after generating the local signing key:

```powershell
pnpm updater:key:init
pnpm desktop:build:signed
```

The signing key lives under `%APPDATA%\com.bynhat.murmur\signing`, outside Git and Murmur backups. Its password is protected with Windows DPAPI for the current user.

The signed build verifies the production frontend, embedded updater key, installer signature, and configured version. It then writes `latest.json` beside the installer in `src-tauri\target\release\bundle\nsis`.

## Install and release 0.3.0

Install the per-user NSIS package produced by the signed build:

```powershell
& .\src-tauri\target\release\bundle\nsis\Murmur_0.3.0_x64-setup.exe
```

Do not install Murmur by copying an executable produced by an arbitrary `cargo build`. The signed Tauri build embeds the production frontend and updater trust key, and the verifier rejects builds missing either one.

Publish `Murmur_0.3.0_x64-setup.exe`, `Murmur_0.3.0_x64-setup.exe.sig`, and `latest.json` as assets on the public GitHub release tagged `v0.3.0`. Murmur treats this as a minor update from 0.2.x.

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
