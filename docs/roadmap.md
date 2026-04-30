# Roadmap

## Phase 0 — Planning

Status: current.

Deliverables:

- Research notes on Wispr Flow.
- Product requirements.
- Architecture plan.
- Initial milestone roadmap.

Exit criteria:

- The first implementation slice is clear enough to start without re-litigating product direction.

## Phase 1 — CLI tracer bullet

Goal: prove the core audio -> text -> clipboard loop.

Tasks:

- Choose prototype language: Python for speed or Rust if starting final daemon immediately.
- Capture microphone audio into a temporary WAV file.
- Send audio to one free/local STT backend.
- Run basic cleanup.
- Copy output to clipboard.
- Store transcript/final text in a local history file or SQLite.

Acceptance criteria:

- User can run one command, speak, and get cleaned text on clipboard.
- Audio files are deleted after processing unless debug mode is enabled.
- Errors are visible and do not lose transcript text if available.

## Phase 2 — Hotkey and insertion

Goal: make it feel like an input method.

Tasks:

- Register global hold-to-talk hotkey.
- Record while held; process on release.
- Paste final text into focused app using clipboard + paste simulation.
- Add clipboard fallback notification.
- Detect and strip trailing “press enter,” then send Enter after paste.

Acceptance criteria:

- User can dictate into at least browser text fields, chat input, and an editor.
- Median release-to-insert latency is acceptable for daily testing.
- Failed insertion leaves text on clipboard.

## Phase 3 — Minimal daemon

Goal: turn prototype into a background service.

Tasks:

- Create long-running daemon.
- Add config file for hotkeys/providers/modes.
- Add SQLite history.
- Add last-result recopy command.
- Add private mode/history disable.

Acceptance criteria:

- Daemon survives repeated dictations.
- User can inspect recent dictations locally.
- No secrets are written to logs/history.

## Phase 4 — Overlay

Goal: reduce uncertainty while dictating.

Tasks:

- Add small overlay or notifications for recording, processing, inserted, copied, failed.
- Show mode indicator.
- Add cancel gesture/key.
- Keep UI terse and non-chatty.

Acceptance criteria:

- User always knows whether Murmur is listening or processing.
- Overlay does not steal focus.
- Overlay disappears automatically after completion/failure.

## Phase 5 — Personal vocabulary

Goal: improve accuracy on real daily language.

Tasks:

- Add dictionary storage.
- Add quick command/config to add terms.
- Include terms in cleanup prompt and STT hints where supported.
- Track likely misses for review.

Acceptance criteria:

- User can add terms like project names, people, tools, acronyms.
- Subsequent dictations preserve those terms more reliably.

## Phase 6 — Command mode

Goal: support selected-text transformations.

Tasks:

- Add separate command-mode hotkey.
- Capture selected text where possible.
- Transform selection with spoken instruction.
- Replace selected text via clipboard paste.
- Support undo/recovery.

Acceptance criteria:

- User can select text and say “make this shorter” or “translate to …”.
- Long selections are rejected with a clear local notification.
- Replacement can be undone through app undo or Murmur recovery.

## Phase 7 — App-aware styles and snippets

Goal: make output fit the surface.

Tasks:

- Detect active app/window where feasible.
- Define app/category style presets: chat, email, code, terminal, docs.
- Add snippets/text expansion.
- Add raw/clean/code hotkey modes.

Acceptance criteria:

- Chat output is casual by default; email/docs are cleaner; terminal/code are less over-written.
- Snippets expand predictably without requiring a separate UI.

## Backlog

- Long dictation background transform with accept/dismiss diff.
- Browser extension for direct web text insertion and selected-text capture.
- Editor extension for code-aware insertion.
- Local whisper.cpp/faster-whisper optimization and model-size bakeoff.
- Search handoff to Perplexity/Google/ChatGPT/Claude.
- Niri/Quickshell-specific integration polish.
- Voice profile calibration.
- Import/export settings.
- Packaging/systemd user service.
