# CLI tracer bullet

This prototype command runs one local dictation pipeline:

```bash
python -m murmur dictate --duration 5
# or, after editable install:
python -m pip install -e '.[stt]'
murmur dictate --duration 5
```

Pipeline: `pw-record` → free/local STT → light cleanup/action parsing → `wl-copy` → optional paste simulation → SQLite history.

Murmur records release-to-insert latency plus stage spans for STT, deterministic transform, correction, clipboard copy, paste, and residual overhead where available. Use `murmur metrics --limit 20` for transcript-free latency review.

## Required local tools

- `pw-record` from PipeWire tools, for recording the default microphone.
- `timeout` from GNU coreutils, used to stop the first fixed-duration recording.
- `wl-copy` and `wl-paste` from `wl-clipboard`, for Wayland clipboard input/output.
- Optional: `wtype` or `ydotool`, for paste/Enter simulation and opt-in selection copy.
- One local/free STT backend:
  - `faster-whisper` Python package, or
  - `whisper.cpp` executable plus a local ggml model file.

Check the machine:

```bash
murmur doctor
```

If `doctor` warns that the default source is muted, configure `[recording].target`
with a source name from `pactl list sources short`. This keeps Murmur on the
intended microphone even when the desktop default input changes.

## faster-whisper setup

```bash
python -m pip install -e '.[stt]'
murmur init
murmur doctor
murmur dictate --duration 5
```

Default config uses `faster-whisper` with `base.en`. The first run may download model files through the normal faster-whisper/Hugging Face cache path. To stay fully offline, pre-download the model and set `stt.model` in `~/.config/murmur/config.toml` to a local path/name.

## whisper.cpp setup

Edit `~/.config/murmur/config.toml`:

```toml
[stt]
provider = "whisper-cpp"
whisper_cpp_binary = "whisper-cli"
whisper_cpp_model = "/path/to/ggml-base.en.bin"
```

Then run:

```bash
murmur doctor
murmur dictate --duration 5
```

## Hold-to-talk command pair

For compositors that support separate press/release bindings, use the session commands instead of a fixed duration:

```bash
murmur start-recording --paste   # key press
murmur stop-recording            # key release; processes and inserts
murmur cancel-recording          # optional cancel binding
```

Only one session can be active at a time. Murmur uses a non-blocking lock file plus `~/.local/state/murmur/recording-session.json` to prevent overlapping recording/processing runs. If a command crashes, check that no `pw-record` or `murmur stop-recording` process is active before deleting a stale session or lock file.

## Copy vs paste

Copy-only is safest:

```bash
murmur dictate --duration 5
```

Try insertion into the focused app:

```bash
murmur dictate --duration 5 --paste
```

If no paste simulator is available, Murmur keeps the final text on the clipboard and records `copied-only`/fallback status.

## Held recording command pair

For release-aware hotkey helpers, use the command pair instead of fixed `--duration` recording:

```bash
murmur start-recording --paste
murmur stop-recording
murmur cancel-recording
```

`start-recording` refuses overlapping sessions. `stop-recording` runs the same transcription, transform, insertion, notification, and history path as `dictate`. `cancel-recording` aborts without insertion and deletes the captured audio.

## Selected-text command prototype

`murmur command` records a short spoken instruction and applies only conservative local transforms to selected or clipboard text. Supported transforms are currently `uppercase`, `lowercase`, and `concise`.

Safer clipboard-only flow:

```bash
murmur command --clipboard --instruction make this uppercase
```

Opt-in selected-text flow:

```bash
murmur command --selection --duration 3
```

With `--selection`, Murmur sends Ctrl+C through `wtype`/`ydotool`, reads the clipboard with `wl-paste`, records/transcribes the command, copies the transformed text, and tries to paste it back over the selection. If copy/paste simulation is unavailable, it falls back to the existing clipboard or leaves the transformed text copied. Unsupported commands are not guessed; Murmur prints a message and preserves the original text.

## History and privacy

History is stored in SQLite by default:

```txt
~/.local/state/murmur/history.sqlite3
```

Useful recovery commands:

```bash
murmur history
murmur metrics --limit 20
murmur recopy-last
murmur last-transcript
```

History records timestamp, mode, provider, raw transcript, final text, insertion status, error message, stage latencies, and failure stage when available. It does **not** store audio. Temporary WAV files are deleted unless `--keep-audio` is passed or `privacy.keep_debug_audio = true` is set.
