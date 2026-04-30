# Local MVP setup

Murmur currently targets a free/local speech-to-text stack.

## Install dev package

```bash
cd "/mnt/storage/01 Projects/murmur-dictation"
python -m venv .venv
source .venv/bin/activate
pip install -e '.[dev,stt]'
```

## System tools

Install the equivalent packages for your distro:

- `pw-record` from PipeWire tools, for microphone capture;
- `wl-copy` from `wl-clipboard`, for clipboard output;
- `wtype` for Wayland paste/Enter simulation;
- `notify-send` from libnotify, optional status notifications.

## Initialize config

```bash
murmur init
murmur doctor
```

Default config path:

```txt
~/.config/murmur/config.toml
```

Default state paths:

```txt
~/.local/share/murmur/murmur.sqlite3
~/.cache/murmur/audio/
```

## First dictation test

Copy-only, safest first test:

```bash
murmur dictate --duration 5
```

Paste into the focused app after transcription:

```bash
murmur dictate --duration 5 --paste
```

Use an existing audio file instead of recording:

```bash
murmur dictate --audio sample.wav
```

## History and recovery

```bash
murmur history
murmur recopy-last
murmur last-transcript
```

## Free/local model note

The default STT provider is `faster-whisper` with `base.en`. The first run may download the model through the normal Hugging Face/faster-whisper cache path. Do not commit model files to the repo.

If `faster-whisper` is too slow or too large, test `tiny.en`, `base.en`, and `small.en`, then document the winner in an ADR.
