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
- model cache: `${XDG_CACHE_HOME}/murmur/models` or `~/.cache/murmur/models`
- debug audio: `${XDG_CACHE_HOME}/murmur/audio` or `~/.cache/murmur/audio`

Default behavior:

- STT provider: `faster-whisper`
- STT model: `base.en`
- language: `auto`
- cleanup mode: `clean`
- clipboard tool: `wl-copy`
- paste simulator preference: `wtype`, then `ydotool`
- history: enabled
- debug audio retention: disabled

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

## History and recovery

History is SQLite and stores text only:

- timestamp
- mode
- provider
- transcript
- final text
- insertion status
- error message

It does not store audio or secrets.

Commands:

```sh
python -m murmur history --limit 20
python -m murmur recopy-last
python -m murmur last-transcript
```

Disable history in private mode:

```toml
[privacy]
history = false
```
