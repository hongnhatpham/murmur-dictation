# Local setup for Murmur MVP

Murmur is local-first. Do not commit personal config, model binaries, audio captures, API keys, or history databases.

## Repo command

From the repository:

```sh
python -m murmur doctor
python -m murmur init
python -m murmur history --limit 10
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
python -m murmur dictionary add "ARIA-03" --note "assistant profile name"
python -m murmur dictionary list
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

It does not store audio or secrets. Captured audio lives under the debug audio cache only while needed for transcription, unless `--keep-audio` or `[privacy].keep_debug_audio = true` is enabled. Use `keep_audio_days` or `murmur cleanup-audio --keep-days N` to prune retained audio.

Commands:

```sh
python -m murmur history --limit 20
python -m murmur recopy-last
python -m murmur last-transcript
python -m murmur cleanup-audio --keep-days 0
```

Disable history in private mode:

```toml
[privacy]
history = false
```
