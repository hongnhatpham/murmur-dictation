# Wayland/niri setup

This document is the MVP integration path for running Murmur from a Wayland/niri desktop. It intentionally uses ordinary compositor bindings, `systemd --user`, and `notify-send` before a Quickshell overlay exists.

## Required desktop tools

Install equivalents for your distro:

- `pipewire` / `wireplumber` and `pw-record` for microphone capture.
- `wl-clipboard` for `wl-copy` clipboard fallback.
- `wtype` or `ydotool` for simulated paste/Enter once insertion is enabled.
- `libnotify` for `notify-send` status messages.
- `systemd --user` for the optional background service.

Quick check:

```sh
command -v pw-record wl-copy notify-send systemctl
command -v wtype || command -v ydotool || true
```

## Recommended MVP binding

niri config bindings are press-triggered. They are good for launching a command, but they are not enough by themselves for a true “record while held, stop on release” workflow. For the first usable slice, bind a one-shot command where Murmur controls recording duration, sends status notifications, and then runs the existing CLI pipeline:

```kdl
binds {
    Mod+Space repeat=false { spawn "sh" "-lc" "murmur dictate --paste"; }
}
```

A copyable example lives at [`../../examples/niri.murmur.kdl`](../../examples/niri.murmur.kdl).

If the CLI is being run from a checkout instead of an installed package, replace `murmur` with the absolute venv binary, for example:

```kdl
Mod+Space repeat=false { spawn "sh" "-lc" "/path/to/murmur-dictation/.venv/bin/murmur dictate --paste"; }
```

Reload niri after editing its config.

## Future hold-to-talk path

For real hold-to-talk, Murmur needs either:

1. a long-running service that listens for global hotkey press/release events itself; or
2. an external keybinding layer that can run separate press and release commands.

When command-pair support exists, it should stay compatible with the same pipeline:

```sh
murmur record-start
murmur record-stop --paste
murmur cancel
```

The stop command should run `record -> transcribe -> transform -> copy -> insert -> history`; if paste simulation is unavailable, it should leave the text on the Wayland clipboard and notify `copied`.

## Minimal notifications

Until the overlay exists, use short focus-safe notifications:

```sh
scripts/murmur-notify recording
scripts/murmur-notify processing
scripts/murmur-notify inserted
scripts/murmur-notify copied "Paste manually if needed."
scripts/murmur-notify failed "Transcription failed; see history/logs."
```

The helper falls back to stderr when `notify-send` is unavailable. The current CLI calls `notify-send` through `murmur.notify`; shell scripts can use this helper for the same terse states. Avoid secrets or transcript content in error notifications.

## Service install

The service template is intentionally override-friendly while the daemon command is still settling.

```sh
# from the repo checkout
python -m venv .venv
. .venv/bin/activate
pip install -e '.[stt]'

packaging/systemd/install-user-service.sh "$(pwd)/.venv/bin/murmur"
systemctl --user daemon-reload
systemctl --user import-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP DBUS_SESSION_BUS_ADDRESS PATH
systemctl --user start murmur-dictate.service
journalctl --user -u murmur-dictate.service -f
```

The installer also writes `~/.config/systemd/user/murmur.service` as a future long-running daemon template. Do not enable it until the CLI exposes a service/daemon subcommand. If the daemon subcommand changes, edit the installed service:

```sh
systemctl --user edit --full murmur.service
systemctl --user daemon-reload
systemctl --user restart murmur.service
```

Disable or uninstall:

```sh
systemctl --user disable --now murmur.service murmur-dictate.service 2>/dev/null || true
rm ~/.config/systemd/user/murmur.service ~/.config/systemd/user/murmur-dictate.service
systemctl --user daemon-reload
```

## Logs and constraints

- One-shot service logs: `journalctl --user -u murmur-dictate.service -f`; daemon logs later: `journalctl --user -u murmur.service -f`.
- Runtime config/env: `~/.config/murmur/`.
- Local history/cache should remain under `~/.local/share/murmur` and `~/.cache/murmur`.
- Do not commit model files, audio recordings, API keys, personal env files, or history databases.
