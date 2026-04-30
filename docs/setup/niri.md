# niri / Wayland integration notes

The MVP CLI works as a one-shot command first:

```bash
murmur dictate --duration 5 --paste
```

Bind that to a compositor shortcut for early testing.

True hold-to-talk requires press/release semantics. If the compositor binding layer can call separate commands on key press and release, the planned shape is:

```bash
murmur start-recording
murmur stop-recording --paste
```

Those commands are not implemented yet. Until then, use fixed-duration one-shot dictation or a shell wrapper.

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
