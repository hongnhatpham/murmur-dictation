# niri / Wayland integration notes

The MVP CLI works as a one-shot command first:

```bash
murmur dictate --duration 5 --paste
```

Bind that to a compositor shortcut for early testing.

True hold-to-talk requires press/release semantics. If the compositor binding layer can call separate commands on key press and release, bind this pair:

```bash
murmur start-recording --paste
murmur stop-recording
```

`start-recording` launches a tracked `pw-record` subprocess and writes a local session file under `~/.local/state/murmur/recording-session.json`. `stop-recording` interrupts that recorder, clears the session file, then runs the normal transcribe/transform/copy-or-paste pipeline. Use `murmur cancel-recording` for a cancel binding; it stops the recorder and deletes captured audio without insertion.

## Suggested early binding

Bind a key to:

```bash
bash -lc 'source "/mnt/storage/01 Projects/murmur-dictation/.venv/bin/activate" && murmur dictate --duration 5 --paste'
```

Keep the first binding copy-only if paste simulation is unreliable:

```bash
bash -lc 'source "/mnt/storage/01 Projects/murmur-dictation/.venv/bin/activate" && murmur dictate --duration 5'
```

## Known fragile points

- Some Wayland apps/compositors reject simulated paste events.
- `wtype` may require compositor support for the virtual keyboard protocol.
- `ydotool` may require extra uinput permissions and a running daemon.
- Clipboard mutation is intentional in the MVP; Murmur leaves text there as fallback.
