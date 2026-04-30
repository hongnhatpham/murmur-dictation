# ADR 0001: Continue the daemon/prototype in Python through the MVP

Date: 2026-04-30

## Status

Accepted for the current MVP. Revisit after daily use of hold-to-talk and insertion.

## Context

Murmur started as a Python CLI tracer bullet because the fastest learning loop was more important than final runtime shape. The working path now records audio, calls local STT, transforms text, copies/pastes, writes SQLite history, and emits status notifications.

The remaining near-term risk is product/desktop integration: Wayland hotkeys, paste reliability, status UX, local model latency, and recovery. A Rust rewrite would not remove those unknowns yet, and would slow iteration on provider and transform behavior.

## Decision

Keep the Murmur MVP implementation in Python, including the next foreground/service-like hold-to-talk slice. Keep module boundaries narrow (`audio`, `stt`, `transform`, `insertion`, `history`, `notify`) so a later Rust daemon can replace only the parts that need it.

Do not start a Rust rewrite until one of these is true:

- Python process startup or long-running reliability is the measured bottleneck.
- Hotkey/audio integration needs libraries or privilege boundaries that are materially better in Rust.
- Packaging a daily-driver daemon becomes harder than maintaining the Python prototype.

## Consequences

- Development speed stays high for local testing and UX iteration.
- Python packaging/systemd templates remain the supported install path for now.
- Rust remains a possible future daemon language, especially for a long-running hotkey/audio service, but not before the MVP validates latency and desktop behavior.
