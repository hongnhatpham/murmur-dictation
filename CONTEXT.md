# Personal Voice Workspace

A private desktop workspace that turns spoken English and Vietnamese into dictation or durable meeting records.

## Language

**Recording Session**:
A bounded period of captured audio that becomes either a Dictation Session or a Meeting Session.
_Avoid_: Recording, capture

**Dictation Session**:
A short Recording Session whose outcome is text for the user's current task.
_Avoid_: Voice note, meeting

**Meeting Session**:
A long Recording Session or imported recording whose outcome is a transcript and Meeting Brief.
_Avoid_: Dictation, recording

**Raw Transcript**:
The immutable speech-recognition result that preserves what the provider heard.
_Avoid_: Final transcript, notes

**Edited Transcript**:
A human-corrected version of a Raw Transcript that preserves its source and timing.
_Avoid_: Raw transcript, meeting notes

**Dictation Result**:
The lightly corrected text produced by a Dictation Session for insertion or copying.
_Avoid_: Transcript, rewrite

**Meeting Brief**:
A cited account of a Meeting Session containing only transcript-supported summaries, decisions, action items, and open questions.
_Avoid_: Transcript, meeting transcript, minutes

**Personal Vocabulary**:
Names and terms supplied by the user or learned from attributed corrections, then used to correct recognized speech.
_Avoid_: Dictionary prompt, glossary

**Attributed Correction**:
A change observed within text inserted by a Dictation Session and associated with that session rather than unrelated document editing.
_Avoid_: Edit, surrounding context, rewrite

**Context Snapshot**:
Ephemeral text and application identity captured from the user's focused task to guide a Dictation Result.
_Avoid_: Clipboard history, document copy, stored context

**Pending Enhancement**:
Meeting transcription or Meeting Brief generation waiting to process preserved session data.
_Avoid_: Failed transcript, retry

**Meeting Prompt**:
A suggestion to start or stop a Meeting Session based on observed meeting activity.
_Avoid_: Automatic recording, meeting alert

**Processing Mode**:
The visible origin or waiting state of transcription and enhancement work: hosted, hosted fallback, local, or queued.
_Avoid_: Provider, model, backend
