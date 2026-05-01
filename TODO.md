# TODO

Source: `docs/product/prd.md`, `docs/engineering/architecture.md`, `docs/roadmap.md`, conversation research on Wispr Flow

## Doing

- [ ] Manual verification: paste into one browser field and one editor/text area; HITL local STT latency/accuracy bakeoff.

## Backlog

### 1. Build the CLI dictation tracer bullet

- Type: AFK
- Blocked by: None
- User stories covered: Basic dictation; clipboard fallback; local recovery foundation

#### What to build

Create the smallest end-to-end Murmur path: one command records audio, transcribes it, lightly cleans the transcript, copies the result to the clipboard, and writes a local history record.

This should favor a fast prototype over final architecture. Python is acceptable for this slice if it gets the loop working sooner; Rust can replace it later if the prototype validates the UX.

#### Acceptance criteria

- [x] A repo command can record from the default microphone into a temporary audio file.
- [x] The audio is sent to one configured free/local STT backend.
- [x] The transcript is transformed into clean text with punctuation/capitalization cleanup.
- [x] The final text is copied to the Wayland clipboard.
- [x] Transcript, final text, timestamp, mode, and error state are stored locally.
- [x] Temporary audio is deleted after processing unless debug mode is enabled.
- [x] Failure after transcription still preserves the transcript/final text in history when available.
- [x] A README/dev note explains required local tools, model download/setup, and environment variables without storing secrets.

#### Implementation notes

- Keep provider boundaries simple: `record -> transcribe -> transform -> copy -> history`.
- Use `pw-record` or another PipeWire-compatible CLI tool for the first recorder.
- Use `wl-copy` for clipboard output.
- Use a free local STT model first; prefer `faster-whisper` or `whisper.cpp` over paid cloud APIs.
- Keep any optional API keys in environment variables or local ignored config only.
- Prefer JSONL or SQLite for the first history store; SQLite is the intended long-term shape.

### 2. Add deterministic cleanup actions to the tracer bullet

- Type: AFK
- Blocked by: 1. Build the CLI dictation tracer bullet
- User stories covered: Basic dictation; chat/prompt sending

#### What to build

Extend the transform layer so it returns both final text and simple actions. The first action is Wispr-style trailing “press enter” detection.

#### Acceptance criteria

- [x] The transform output includes `final_text` and `actions`.
- [x] If dictated speech ends with “press enter,” that phrase is removed from final text.
- [x] A `press_enter_after_insert` action is emitted when the phrase is detected.
- [x] If the only spoken content is “press enter,” the action is emitted with empty final text.
- [x] Unit tests cover case-insensitive phrase detection and punctuation variants.
- [x] Raw mode can preserve literal text without stripping the phrase.

#### Implementation notes

- Do this deterministically before/after LLM cleanup rather than relying only on a prompt.
- Keep action execution separate from action detection so insertion can be added later.

### 3. Paste into the focused app with clipboard fallback

- Type: AFK
- Blocked by: 1. Build the CLI dictation tracer bullet; 2. Add deterministic cleanup actions to the tracer bullet
- User stories covered: Basic dictation; chat/prompt sending; clipboard fallback

#### What to build

Turn clipboard output into current-app insertion: set clipboard to final text, simulate paste, optionally send Enter if the transform action requested it, and report fallback clearly when paste cannot be executed.

#### Acceptance criteria

- [x] A command copies final text and triggers paste into the focused app.
- [ ] The paste path works in at least one browser text field and one editor/text area during manual verification.
- [x] `press_enter_after_insert` sends Enter only after text paste is attempted.
- [x] If paste simulation is unavailable, the text remains on clipboard and the command exits with a clear message.
- [x] History records whether the result was pasted, copied-only, or failed.
- [x] The implementation is isolated behind an insertion interface for later Wayland/niri-specific strategies.

#### Implementation notes

- Try `wtype` or `ydotool` for paste simulation.
- Prefer reliability over restoring the old clipboard in the first version.
- Do not send Enter unless the phrase/action was explicit.

### 4. Add hold-to-talk hotkey recording

- Type: AFK
- Blocked by: 3. Paste into the focused app with clipboard fallback
- User stories covered: Basic dictation; input-method UX

#### What to build

Add a foreground or daemon-like command that records while a global hotkey is held and processes on release. This is the first version that should feel like Murmur rather than a script.

#### Acceptance criteria

- [x] An interim compositor-bindable command pair starts recording on press and stops on release.
- [x] Releasing the hotkey runs the existing transcribe/transform/insert pipeline.
- [x] Pressing cancel during recording aborts without insertion.
- [x] The command prevents overlapping dictations while one is recording or processing.
- [x] Hotkey setup and known compositor limitations are documented.

#### Implementation notes

- Start with the path that works best on the current Wayland/niri setup.
- If a true global hold binding is too fragile, provide an interim command pair: start-recording and stop-recording, bindable by compositor config.

### 5. Create the local history and recovery CLI

- Type: AFK
- Blocked by: 1. Build the CLI dictation tracer bullet
- User stories covered: Recover last dictation; clipboard fallback; success metrics foundation

#### What to build

Make history useful enough for daily testing: list recent dictations, recopy the last result, and expose enough metadata to debug failed insertions and cleanup mistakes.

#### Acceptance criteria

- [x] History persists transcript, final text, timestamp, mode, provider, insertion status, and error message.
- [x] A CLI command lists recent dictations.
- [x] A CLI command copies the last final text back to clipboard.
- [x] A CLI command can print the last raw transcript.
- [x] Private mode can disable history writes.
- [x] No audio files or secrets are stored in history.

#### Implementation notes

- SQLite is preferred here if the first slice used JSONL.
- Use this data later for local metrics: latency, failure rate, fallback rate.

### 6. Add minimal status notifications

- Type: AFK
- Blocked by: 4. Add hold-to-talk hotkey recording
- User stories covered: Basic dictation; clipboard fallback; recoverability

#### What to build

Add terse status feedback before building a full Quickshell overlay. The user should always know whether Murmur is recording, processing, inserted, copied-only, failed, or canceled.

#### Acceptance criteria

- [x] Recording start shows a visible status.
- [x] Processing shows a visible status.
- [x] Successful paste shows a short confirmation.
- [x] Clipboard fallback shows a short confirmation.
- [x] Failure shows a useful error without exposing secrets.
- [x] Notifications/status messages do not steal focus.

#### Implementation notes

- Use `notify-send` or equivalent first.
- Keep copy short and functional; avoid generic assistant microcopy.
- This slice can later be replaced by Quickshell without changing the pipeline.

### 7. Add configurable providers and modes

- Type: AFK
- Blocked by: 1. Build the CLI dictation tracer bullet; 5. Create the local history and recovery CLI
- User stories covered: Raw mode; clean mode; configurable hotkeys/providers

#### What to build

Introduce a real config file for STT provider, cleanup provider, default mode, privacy/history, and insertion behavior. Add raw and clean modes end-to-end.

#### Acceptance criteria

- [x] Murmur loads config from a documented local path.
- [x] Config supports STT provider selection, local model paths/names, and optional provider-specific environment variable names.
- [x] Config supports `raw` and `clean` modes.
- [x] Raw mode avoids aggressive cleanup and preserves literal wording more closely.
- [x] Clean mode remains the default.
- [x] Config supports disabling history.
- [x] Missing config produces safe defaults or a clear setup error.

#### Implementation notes

- Candidate format: TOML.
- Do not commit personal config or credentials.
- Keep mode behavior visible in history records.

### 8. Add personal dictionary support

- Type: AFK
- Blocked by: 7. Add configurable providers and modes
- User stories covered: Personal vocabulary; better names/project/tool accuracy

#### What to build

Add a local dictionary for names, project terms, acronyms, tools, and slang. Feed it into cleanup prompts and STT hints where supported.

#### Acceptance criteria

- [x] CLI can add, list, and remove dictionary terms.
- [x] Dictionary terms are included in transform context.
- [x] STT hints are used when the selected provider supports them.
- [x] History can mark likely vocabulary misses for later review.
- [x] Documentation includes examples like project names and technical terms.

#### Implementation notes

- Store dictionary in SQLite or local config, not in prompts/logs beyond necessary runtime use.
- This is a core accuracy feature, not a polish feature.

### 9. Build selected-text command mode

- Type: AFK
- Blocked by: 3. Paste into the focused app with clipboard fallback; 7. Add configurable providers and modes
- User stories covered: Rewrite selected text

#### What to build

Add a command-mode path for transforming selected text using a spoken instruction: select text, hold command hotkey, say “make this shorter,” and replace the selection with transformed text.

#### Acceptance criteria

- [x] Command mode uses a separate invocation/hotkey from normal dictation.
- [x] The spoken instruction is transcribed and passed with the selected text to the transform provider.
- [x] Selection length is limited with a clear error for oversized input.
- [x] Replacement uses the same insertion/fallback safety model as dictation.
- [x] The previous selected text and replacement are stored in history unless private mode is enabled.
- [ ] App undo can restore the prior selection in common text fields during manual verification.

#### Implementation notes

- Capturing selected text is platform-sensitive; first fallback can use clipboard copy of selection if safe and documented.
- Keep command mode explicitly separate from general desktop automation.

### 10. Build a tiny Quickshell status overlay

- Type: AFK
- Blocked by: 6. Add minimal status notifications
- User stories covered: Basic dictation; input-method UX; minimal status UI

#### What to build

Replace or augment notifications with a compact Quickshell overlay showing Murmur states without stealing focus.

#### Acceptance criteria

- [x] Overlay shows recording, processing, inserted, copied fallback, failed, and command mode states.
- [x] Overlay does not steal focus from the target app.
- [x] Overlay auto-hides after completion or failure.
- [x] Overlay can receive state events from the daemon/prototype process.
- [x] The visual language is terse and utilitarian.

#### Implementation notes

- Keep this independent from core dictation logic.
- Do not build a full settings UI in this slice.

### 11. Package Murmur as a user service

- Type: AFK
- Blocked by: 4. Add hold-to-talk hotkey recording; 7. Add configurable providers and modes
- User stories covered: Daily-driver startup; background service reliability

#### What to build

Make Murmur easy to start, stop, restart, and run on login as a user-level service.

#### Acceptance criteria

- [x] A documented install/dev command creates or updates systemd user service templates for the current one-shot pipeline and future daemon.
- [x] The one-shot service starts `murmur dictate --paste` with the configured environment; the daemon service remains intentionally disabled until a service/daemon subcommand exists.
- [x] Logs are accessible through `journalctl --user` or an equivalent documented path.
- [x] Service restart does not lose config or history because both live under user XDG config/state paths.
- [x] Uninstall/disable instructions are documented.

#### Implementation notes

- Keep service files templated so paths under `/mnt/storage/01 Projects/murmur-dictation` do not have to be hard-coded forever.

### 12. Run a human free-model latency/provider bakeoff

- Type: HITL
- Blocked by: 1. Build the CLI dictation tracer bullet; 7. Add configurable providers and modes
- User stories covered: Basic dictation quality; free/local model choice; latency success metric

#### What to build

Compare candidate free/local STT models using real spoken samples from the user’s daily vocabulary. Decide the default MVP model, model size, and fallback strategy.

#### Acceptance criteria

- [ ] At least two free/local STT options are tested on the same sample phrases.
- [ ] Latency from hotkey release to final text is measured.
- [ ] Accuracy is checked against names, project terms, code terms, and mixed casual speech.
- [x] A default free model decision is documented in the repo (see `docs/engineering/adr/0002-local-stt-first.md`; HITL bakeoff still pending before changing it).
- [x] Required local model download/setup steps are documented without committing model binaries unless explicitly intended.

#### Implementation notes

- This is HITL because it requires real voice quality judgment and model downloads.
- Start with `faster-whisper` and/or `whisper.cpp`; only consider cloud STT later as an optional adapter, not the MVP default.
- Good default decision likely beats endless provider abstraction.

### 13. Decide and document the long-term implementation language

- Type: HITL
- Blocked by: 1. Build the CLI dictation tracer bullet
- User stories covered: Architecture stability; maintainability

#### What to build

Make an explicit architecture decision on whether Murmur’s daemon should continue in the prototype language or move to Rust for the long-running service.

#### Acceptance criteria

- [x] A short ADR is added under `docs/engineering/adr/`.
- [x] The ADR compares Python prototype continuation vs Rust daemon rewrite.
- [x] The decision accounts for hotkeys, audio capture, packaging, reliability, and development speed.
- [x] Follow-up TODO items are adjusted if the decision changes sequencing.

#### Implementation notes

- Decision recorded in `docs/engineering/adr/0001-continue-python-through-mvp.md`: continue Python through MVP, revisit Rust only after measured daily-driver friction.

### 14. Add app-aware styles and snippets

- Type: AFK
- Blocked by: 7. Add configurable providers and modes; 8. Add personal dictionary support
- User stories covered: App-aware style presets; snippets/text expansion

#### What to build

Add app/category-aware dictation behavior and text snippets after the core loop is stable.

#### Acceptance criteria

- [x] Murmur can detect or accept the active app/category for transform context.
- [x] Config supports style presets for chat, email, docs, code, and terminal-like contexts.
- [x] Snippets can be added, listed, removed, and expanded.
- [x] Snippets and styles are stored locally.
- [x] Clean mode output changes appropriately by category without becoming unpredictable.

#### Implementation notes

- Avoid overfitting before there is enough daily history.
- Terminal/code contexts should be less aggressively rewritten.

### 15. Add long-dictation review/diff flow

- Type: AFK
- Blocked by: 9. Build selected-text command mode; 10. Build a tiny Quickshell status overlay
- User stories covered: Background transform; review before accepting longer edits

#### What to build

For longer dictations, allow a conservative initial insertion and an optional background cleanup with review/diff before replacing text.

#### Acceptance criteria

- [ ] Long dictations can trigger a background transform threshold.
- [ ] User is notified when a refined transform is ready.
- [ ] User can inspect a diff between original insertion and refined output.
- [ ] User can accept or dismiss the refined output.
- [ ] Dismissal keeps the original text unchanged.

#### Implementation notes

- This is V2. Do not build until the core insertion and overlay are stable.

### 16. Add correction trace fields to history

- Type: AFK
- Blocked by: 7. Add configurable providers and modes
- User stories covered: Recover raw transcript; understand whether correction helped or hurt

#### What to build

Extend the dictation pipeline and history records so every insertion can show the raw transcript, deterministic cleaned text, final corrected text, correction provider, correction latency, and fallback reason.

#### Acceptance criteria

- [x] History distinguishes raw transcript, deterministic cleaned text, and final insertion text.
- [x] History records `correction_provider`, `correction_latency_ms`, and `correction_status`.
- [x] `murmur history` and recovery commands remain useful and concise.
- [x] Failed correction never loses raw transcript or deterministic text.

#### Implementation notes

- Source: `docs/product/ai-correction-prd.md`.
- This is the tracer bullet for making AI correction observable before making it smart.

### 17. Add focused-app category detection for correction context

- Type: AFK
- Blocked by: 16. Add correction trace fields to history
- User stories covered: App-aware correction; terminal/code safety

#### What to build

Map the focused app/window to a Murmur category such as terminal/code, AI chat/docs, work messaging, personal messaging, email, or other. Feed this category into deterministic cleanup and future correction prompts.

#### Acceptance criteria

- [x] Murmur detects focused niri app ID where available.
- [x] Config maps app IDs or URL/app hints to categories.
- [x] Terminal/code contexts are classified conservatively.
- [x] History records the detected app ID and category.
- [x] Missing app data falls back to `other` without failing dictation.

#### Implementation notes

- Reuse the existing focused-window detection added for terminal insertion.
- Keep browser URL/context as a later extension.

### 18. Implement Wispr-style spacing and continuation context

- Type: AFK
- Blocked by: 17. Add focused-app category detection for correction context
- User stories covered: Dictating subsequent sentences; context-aware formatting

#### What to build

Improve cursor-adjacent formatting: leading spaces, lowercase continuation, punctuation joining, and trailing period policy based on app category and nearby text.

#### Acceptance criteria

- [x] Subsequent dictations insert a leading space when continuing prose.
- [x] Mid-sentence continuation can lowercase the first word when appropriate.
- [x] Punctuation-leading text does not receive an extra leading space.
- [x] Messaging categories can omit trailing periods when configured.
- [x] Unit tests cover continuation, punctuation, empty context, and terminal/code contexts.

#### Implementation notes

- The current clipboard-based leading-space behavior is a first draft; harden it into a context module.

### 19. Extend personal dictionary into correction vocabulary rules

- Type: AFK
- Blocked by: 16. Add correction trace fields to history; 8. Add personal dictionary support
- User stories covered: Personal vocabulary; project names; replacement rules

#### What to build

Upgrade the dictionary from simple term storage to correction-aware vocabulary: preferred casing, replacement rules, category-specific terms, and likely-miss review from history.

#### Acceptance criteria

- [x] Dictionary entries can include preferred casing and optional replacement text.
- [x] Correction prompts and deterministic cleanup receive relevant dictionary entries.
- [x] History can surface likely misses for review.
- [x] CLI supports add/list/remove for the expanded fields.
- [x] Existing dictionary data migrates or remains compatible.

#### Implementation notes

- Keep data local in SQLite.
- Avoid logging private dictionary notes beyond what is needed for debugging.

### 20. Add local AI correction provider

- Type: AFK
- Blocked by: 16. Add correction trace fields to history; 17. Add focused-app category detection for correction context; 19. Extend personal dictionary into correction vocabulary rules
- User stories covered: AI correction; filler removal; punctuation restoration; preserve meaning

#### What to build

Add an optional local LLM correction pass after deterministic cleanup. Start with an Ollama-compatible HTTP provider and strict JSON/plain-text output contract.

#### Acceptance criteria

- [x] Config can enable/disable AI correction independently of STT.
- [x] Correction receives raw transcript, deterministic text, category/style, and dictionary terms.
- [x] Prompt instructs the model to preserve meaning, avoid adding facts, and return only insertion text plus metadata.
- [x] Correction has a timeout and falls back to deterministic text on failure.
- [x] Raw mode bypasses AI correction unless explicitly requested.
- [x] Unit tests cover timeout/fallback and response parsing.

#### Implementation notes

- Default remains off until latency/quality is proven.
- Prefer local Ollama first; cloud adapters are opt-in future work.

### 21. Add configurable Flow-style app styles

- Type: AFK
- Blocked by: 17. Add focused-app category detection for correction context; 20. Add local AI correction provider
- User stories covered: Email formal style; chat casual style; terminal/code literal style

#### What to build

Add per-category style presets that shape deterministic cleanup and AI correction: formal, casual, very casual, code/terminal literal, and custom.

#### Acceptance criteria

- [x] Config supports style presets by category.
- [x] Style controls punctuation density, trailing period policy, casing, and rewrite aggressiveness.
- [x] Terminal/code style disables aggressive prose rewriting.
- [x] Email/doc style favors complete sentences and formal punctuation.
- [x] History records which style was applied.

#### Implementation notes

- Match Wispr Flow's product shape but keep the implementation local-first and transparent.
