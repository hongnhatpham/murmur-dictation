# Local setup for Murmur MVP

Murmur is local-first. Do not commit personal config, model binaries, audio captures, API keys, or history databases.

## Repo command

From the repository:

```sh
python -m murmur doctor
python -m murmur init
python -m murmur history --limit 10
python -m murmur metrics --limit 20
python -m murmur recopy-last
python -m murmur last-transcript
python -m murmur start-recording --paste
python -m murmur stop-recording
python -m murmur cancel-recording
```

After editable install, the same commands are available as `murmur ...`:

```sh
python -m pip install -e '.[dev]'
murmur doctor
```

## Config defaults

Murmur reads TOML config from:

- `${XDG_CONFIG_HOME}/murmur/config.toml`, or
- `~/.config/murmur/config.toml` when `XDG_CONFIG_HOME` is unset.

Missing config is safe: the CLI uses local/free defaults.

Default state paths:

- state directory: `${XDG_STATE_HOME}/murmur` or `~/.local/state/murmur`
- history DB: `<state directory>/history.sqlite3`
- personal dictionary/snippets DB: `<state directory>/personal.sqlite3`
- model cache: `${XDG_CACHE_HOME}/murmur/models` or `~/.cache/murmur/models`
- debug audio: `${XDG_CACHE_HOME}/murmur/audio` or `~/.cache/murmur/audio`

Default behavior:

- recording source: PipeWire default source, unless `[recording].target` is set
- STT provider: `faster-whisper`
- STT model: `base.en`
- language: `auto`
- cleanup mode: `clean`
- AI correction: disabled by default; optional local Ollama-compatible correction can be enabled under `[correction]`
- clipboard tool: `wl-copy`
- paste simulator preference: `wtype`, then `ydotool`
- history: enabled
- debug audio retention: disabled; successfully transcribed temporary audio is removed automatically, and retained debug audio is pruned after 1 day by default

Generate a documented config with:

```sh
python -m murmur init
python -m murmur init --overwrite
```

## Recording source

Murmur records from PipeWire's default source unless a target is configured.
That means desktop input selection is the normal control surface. If the default
source is a muted onboard microphone, capture can produce a valid non-empty WAV
of digital silence and STT may hallucinate text. `murmur doctor` warns when the
default source reports muted.

Inspect sources:

```sh
pactl get-default-source
wpctl get-volume @DEFAULT_AUDIO_SOURCE@
pactl list sources short
```

Pin Murmur to a known microphone only when you intentionally want Murmur to
ignore the desktop-selected default:

```toml
[recording]
target = "alsa_input.usb-Focusrite_Scarlett_2i2_USB_Y87GQFB9923514-00.HiFi__Mic1__source"
```

Both `murmur dictate` and the `start-recording`/`stop-recording` hotkey path use
this target. Remove the `[recording]` section to return to the desktop-selected
default source.

## Near-instant provider profiles

The latency track targets two profiles only:

- `local`: local/private `faster-whisper`, deterministic cleanup, AI correction disabled.
- `groq`: Groq `whisper-large-v3-turbo`, deterministic cleanup, cloud correction disabled.

Switch profiles with:

```sh
python -m murmur provider-profile local
python -m murmur provider-profile groq
```

`python -m murmur provider-profile cloud` remains as a compatibility alias for `groq`; it does not mean broad cloud-provider support. Run `python -m murmur doctor` after switching. Doctor reports whether the selected local package/model or Groq key is available without printing API key values.

## Local/Groq benchmark harness

Benchmark existing audio files without pasting into the focused app:

```sh
python -m murmur benchmark sample1.wav sample2.wav --output tmp/murmur-benchmark.jsonl
```

Default benchmark profiles:

- local `faster-whisper` `tiny.en`
- local `faster-whisper` `base.en`
- Groq `whisper-large-v3-turbo` or the configured Groq STT model

Results print provider, model, total latency, STT latency, `insertion=skipped`, status, and transcript. Saving to `tmp/` keeps benchmark JSONL output under the repo's ignored scratch path; do not commit personal audio or transcript outputs.

Suggested HITL phrase set:

- project/app names: `Murmur`, `Niri`, `Quickshell`, `Codeberg`, `Groq`
- code-ish terms: `faster-whisper`, `whisper.cpp`, `JSONL`, `SQLite`, `provider-profile`
- casual message: a short chat reply with contractions and slang
- docs/email sentence: a complete sentence with punctuation
- command phrase: a message ending with `press enter`

Compare speed first with `total_ms` and `stt_ms`, then judge transcripts manually for names, tool casing, punctuation, and whether `press enter` is recognized cleanly enough for the transform layer.

## Required local tools

`murmur doctor` checks:

- `pw-record` for PipeWire audio capture
- `wl-copy` for Wayland clipboard writes
- `wtype` or `ydotool` for paste/key simulation; if neither is installed, clipboard fallback can still work
- selected local STT backend availability
- config, model, and history paths

Typical package names vary by distribution, but the tools usually come from PipeWire, wl-clipboard, wtype, and ydotool packages.

## Local STT backends

Preferred MVP backend:

```toml
[stt]
provider = "faster-whisper"
model = "base.en"
language = "auto"
```

Install the Python package in your environment if you want this backend to pass doctor:

```sh
python -m pip install faster-whisper
```

Alternative local backend:

```toml
[stt]
provider = "whisper-cpp"
whisper_cpp_binary = "whisper-cli"
whisper_cpp_model = "~/.cache/murmur/models/ggml-base.en.bin"
```

Download model files manually into the local model cache. Model binaries are ignored by git and should not be committed.

## Optional local AI correction

Murmur's deterministic cleanup remains the default. To try a local Ollama-compatible polish pass after STT cleanup:

```toml
[correction]
enabled = true
provider = "ollama"
model = "llama3.2:3b"
endpoint = "http://127.0.0.1:11434/api/generate"
timeout_seconds = 2.5
```

If correction times out or fails, Murmur falls back to the deterministic cleaned text instead of losing the dictation. Raw mode bypasses AI correction unless `[correction].raw_mode = true`.

Per-app categories can also apply Flow-style presets before the optional AI pass:

```toml
[styles.presets.personal_message]
trailing_period = false
rewrite_aggressiveness = "light"
formality = "casual"

[styles.presets.email]
trailing_period = true
rewrite_aggressiveness = "medium"
formality = "formal"
```

## Personal dictionary and snippets

Personal vocabulary and snippets are stored locally in SQLite at `<state directory>/personal.sqlite3` unless `paths.personal_db` is set in config.

Dictionary terms are used as local STT hints where supported (`faster-whisper` initial prompt and `whisper.cpp --prompt`) and as deterministic cleanup context to restore preferred casing/spelling when the recognized words already match.

Examples:

```sh
python -m murmur dictionary add "Niri"
python -m murmur dictionary add "a r e a three" --replacement "ARIA-03" --category docs --note "assistant profile name"
python -m murmur dictionary list
python -m murmur dictionary misses --limit 50
python -m murmur dictionary remove "Niri"

python -m murmur snippets add ";sig" "Regards, Murmur"
python -m murmur snippets list
python -m murmur snippets expand ";sig"
python -m murmur snippets remove ";sig"
```

`copy`, `paste`, `dictate`, and command-mode STT load this store at runtime. Snippet expansion is deterministic text expansion; it does not execute desktop actions. Command mode refuses selections larger than `[command].max_selection_chars` to avoid accidentally sending huge/private buffers through the transform path.

## History and recovery

History is SQLite and stores text only:

- timestamp
- mode
- provider
- transcript
- final text
- insertion status
- error message
- release-to-insert latency and stage latencies for STT, deterministic transform, correction, clipboard copy, paste, overhead, and failure stage when available

It does not store audio or secrets. Captured audio lives under the debug audio cache only while needed for transcription, unless `--keep-audio` or `[privacy].keep_debug_audio = true` is enabled. Use `keep_audio_days` or `murmur cleanup-audio --keep-days N` to prune retained audio.

Commands:

```sh
python -m murmur history --limit 20
python -m murmur metrics --limit 20
python -m murmur recopy-last
python -m murmur last-transcript
python -m murmur cleanup-audio --keep-days 0
```

`murmur metrics` prints recent latency summaries without transcript or final-text content, so it is safe for quick performance review and provider comparison.

Disable history in private mode:

```toml
[privacy]
history = false
```
