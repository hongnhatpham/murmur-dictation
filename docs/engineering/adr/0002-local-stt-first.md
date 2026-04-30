# ADR 0002: Use local/free STT for the MVP

Date: 2026-04-30

## Status

Accepted.

## Context

Murmur is intended to be a universal voice keyboard. The MVP needs to be cheap to run, private by default, and usable without committing API keys or routing daily dictation through paid services.

The CLI already supports a local provider boundary. The practical candidates are `faster-whisper` and `whisper.cpp`; both can run with locally cached models. Cloud STT may be useful later as an optional adapter, but it should not be required for the first daily-driver loop.

## Decision

Default to `faster-whisper` with `base.en` for the MVP config, with `whisper.cpp` supported as the alternate local backend. Keep model files in user cache paths, not in the repository. Keep provider credentials out of committed config.

Run the human latency/accuracy bakeoff before changing the default model size. Candidate first tests: `tiny.en`, `base.en`, and `small.en` on the user's real vocabulary.

## Consequences

- The project remains local/free-first and avoids hidden per-use costs.
- Setup docs must explain local packages, model cache behavior, and the fact that first run may download model weights.
- Latency and accuracy still require HITL validation on the target machine before declaring the default model final.
