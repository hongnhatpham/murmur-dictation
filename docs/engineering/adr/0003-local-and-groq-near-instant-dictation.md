# ADR 0003: Target local and Groq paths for near-instant dictation

Date: 2026-06-02

## Status

Accepted for the next latency-focused slices.

## Context

Murmur should feel like a universal voice keyboard: hold a key, speak, release, and text appears in the focused app. The current working path is useful, but it is still a release-time pipeline: stop recording, finalize a WAV file, transcribe, transform, optionally correct, copy, and paste.

Recent daily-driver history showed a Groq `whisper-large-v3-turbo` dictation taking about 3.8 seconds from release to insertion for a 6.5 second utterance. That is usable, but not near-instant. The same history also showed Groq correction failures caused by strict JSON validation. Correction should not be on the critical path until it is reliable and measured.

ADR 0002 remains true: Murmur is local/free-first. The user now wants the near-instant effort to target two provider families only:

- local STT for private/offline daily use;
- Groq as an explicit turbo profile for low-latency request/response transcription.

Other cloud STT providers may be useful later, but adding more provider breadth now would dilute the latency work.

## Decision

Make near-instant dictation a two-profile effort:

1. **Local profile:** first-class, private/offline path using `faster-whisper` and/or `whisper.cpp`. This remains the default product posture.
2. **Groq profile:** explicit opt-in turbo path using Groq STT for fast request/response transcription. Groq may use local deterministic cleanup, but cloud correction is not on the hot path by default.

For both profiles, optimize the pipeline in this order:

1. Add stage-level timing so every dictation records recorder-stop, STT, transform, correction, clipboard, paste, and total release-to-insert latency.
2. Create fast profile defaults that skip AI correction on short dictations and keep deterministic cleanup in the hot path.
3. Add a benchmark harness for local models and Groq using the same utterance set and history schema.
4. Build a warmed Python daemon so config, provider setup, and model/client initialization do not happen per invocation.
5. Add streaming or incremental local STT so transcription can begin while the hotkey is still held. On release, paste the final transcript; interim overlay text is allowed, but live mutation of the focused app is not part of this slice.

Do not pursue OpenAI, ElevenLabs, Deepgram, AssemblyAI, or another cloud STT provider for the near-instant work until local and Groq have been measured and exhausted.

## Latency targets

- Acceptable daily-driver target: under 1200 ms from hotkey release to pasted text.
- Strong target: under 500 ms from hotkey release to pasted text.
- Perceived instant target: overlay/interim text visible while speaking, with final paste on release.

## Consequences

- Provider work stays narrow and measurable.
- Local remains the privacy/default path, while Groq is available when speed is worth a network dependency.
- AI correction moves out of the default hot path for latency-sensitive dictation.
- The daemon/streaming work becomes more important than model choice alone.
- Benchmarks must compare local and Groq on the same real phrases before changing defaults.
