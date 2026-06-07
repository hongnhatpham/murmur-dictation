# AGENTS.md

- Recording-source debugging: before blaming STT for blank or stock-phrase transcripts, check `pactl get-default-source`, `wpctl get-volume @DEFAULT_AUDIO_SOURCE@`, and a one-second `pw-record` RMS probe. A muted default source can produce non-empty WAV files of digital silence. Leave `[recording].target` unset to respect the desktop-selected input source; use it only for an intentional Murmur-specific override.
- STT vocabulary hallucination: if Groq returns a comma-separated list of personal dictionary terms, check for dictionary prompt leakage before changing the dictionary or transform layer. Groq STT should not receive personal dictionary terms as prompt text.
