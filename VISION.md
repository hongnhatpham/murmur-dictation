# Murmur

Murmur is a personal Windows voice workspace for fast dictation and durable meeting records. It turns spoken English, Vietnamese, and mixed-language speech into useful text without making the user manage recordings, providers, or recovery paths during ordinary use.

## Product

Murmur has two modes:

- A Dictation Session captures a short utterance, transcribes it, lightly corrects it using focused application context and Personal Vocabulary, then inserts the result into the active application.
- A Meeting Session records or imports a longer conversation, produces an editable timestamped transcript, and derives a cited Meeting Brief.

The two modes share capture, transcription, storage, provider fallback, and history. They keep separate completion rules because dictation must finish immediately while meeting work may wait for connectivity.

## Principles

- Keep the immediate interaction fast. Recovery and provider complexity stay out of the user's way.
- Preserve the Raw Transcript before correction or summarization.
- Link Meeting Brief claims to transcript evidence and audio timestamps.
- Keep recordings, transcripts, settings, and learned vocabulary local by default.
- Use free hosted allowances before local STT, without farming accounts or hiding provider changes.
- Prefer a working narrow feature over broad platform or format support.

## V1 boundary

V1 supports Windows 11 only. It includes English and Vietnamese dictation, hosted STT rotation, local Whisper fallback, hosted correction, context-aware insertion, silent vocabulary learning, imported and recorded meetings, post-meeting transcripts, cited Meeting Briefs, local search, retention, backup, deterministic setup, and private signed updates.

V1 excludes code dictation, cloud sync, accounts, billing, collaboration, live meeting transcripts, custom meeting templates, semantic search, local correction models, remote telemetry, and non-Windows platforms.
