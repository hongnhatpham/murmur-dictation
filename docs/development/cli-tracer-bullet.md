# CLI tracer bullet

This prototype command runs one local dictation pipeline:

```bash
python -m murmur dictate --duration 5
# or, after editable install:
python -m pip install -e '.[stt]'
murmur dictate --duration 5
```

Pipeline: `pw-record` → free/local STT → light cleanup/action parsing → `wl-copy` → optional paste simulation → SQLite history.

## Required local tools

- `pw-record` from PipeWire tools, for recording the default microphone.
- `timeout` from GNU coreutils, used to stop the first fixed-duration recording.
- `wl-copy` from `wl-clipboard`, for Wayland clipboard output.
- Optional: `wtype` or `ydotool`, for paste/Enter simulation.
- One local/free STT backend:
  - `faster-whisper` Python package, or
  - `whisper.cpp` executable plus a local ggml model file.

Check the machine:

```bash
murmur doctor
```

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

## History and privacy

History is stored in SQLite by default:

```txt
~/.local/state/murmur/history.sqlite3
```

Useful recovery commands:

```bash
murmur history
murmur recopy-last
murmur last-transcript
```

History records timestamp, mode, provider, raw transcript, final text, insertion status, and error message. It does **not** store audio. Temporary WAV files are deleted unless `--keep-audio` is passed or `privacy.keep_debug_audio = true` is set.
