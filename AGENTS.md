# AGENTS.md

- Recording-source debugging: before blaming STT for blank or stock-phrase transcripts, check `pactl get-default-source`, `wpctl get-volume @DEFAULT_AUDIO_SOURCE@`, and a one-second `pw-record` RMS probe. A muted default source can produce non-empty WAV files of digital silence; use `[recording].target` when multiple inputs exist.
