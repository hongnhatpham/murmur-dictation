# Murmur PRD

## Summary

Murmur is a Linux-native, Wispr Flow-inspired dictation app that lets the user input text faster anywhere on the computer. It behaves like a universal voice keyboard: hold a hotkey, speak naturally, release, and clean text appears in the currently focused app.

Murmur is intentionally not a broad voice-control agent at first. It should optimize for fast, reliable, recoverable text input.

## Target user

Primary target: a power user on Linux/Wayland who writes across many surfaces:

- terminal and code editors;
- browsers and AI chat apps;
- Discord/Slack/Signal-style messaging;
- email and docs;
- notes and project planning.

The user wants to reduce typing friction without switching apps or trusting an always-listening assistant.

## Problem

Typing is a bottleneck for thinking, messaging, prompting, documenting, and coding. Existing dictation tools are often:

- not Linux/Wayland-native;
- too literal about speech;
- poor at personal vocabulary;
- awkward outside their own text box;
- hard to recover from when they paste into the wrong place;
- either too cloud-dependent or too slow locally.

## Product thesis

The winning experience is not “talk to an AI agent.” It is:

> press a key, say the messy thing, get usable text exactly where the cursor already is.

## Goals

### MVP goals

- Start/stop dictation from a global hold-to-talk hotkey.
- Capture microphone audio reliably.
- Transcribe spoken audio.
- Lightly clean speech into usable text.
- Insert text into the currently focused app.
- Provide clipboard fallback when insertion fails.
- Show minimal status UI: idle, recording, processing, inserted, failed.
- Store local dictation history.
- Support undo/recovery for the last insertion.

### V1 goals

- Custom dictionary for names, project terms, commands, and slang.
- Raw vs clean dictation modes.
- App-aware style presets.
- “Press enter” command at the end of dictation.
- Selected-text rewrite command mode.
- Snippets/text expansion.
- Configurable hotkeys and providers.

### V2 goals

- Background transform for long dictations with review/diff.
- Voice commands for search/destination routing.
- Local-first transcription mode with cloud turbo fallback.
- Deeper editor/browser integrations where needed.
- Personal style memory.

## Non-goals initially

- Always-listening wake word.
- Full desktop automation.
- Sending messages without explicit user intent.
- Running shell commands by voice.
- Browser automation beyond text insertion/search handoff.
- Multi-user/team product features.
- Mobile support.

## Core user journeys

### 1. Basic dictation

1. User focuses a text field.
2. User holds the dictation hotkey.
3. User speaks naturally.
4. User releases the hotkey.
5. Murmur transcribes, cleans, and inserts text.
6. A compact overlay confirms insertion.

Success: the user keeps their flow and does not need to edit much.

### 2. Chat/prompt sending

1. User focuses a chat input.
2. User dictates a message ending with “press enter.”
3. Murmur inserts the message without the phrase “press enter.”
4. Murmur sends Enter.

Success: this feels as fast as speaking a message.

### 3. Clipboard fallback

1. User dictates into an app where paste injection fails.
2. Murmur copies the final text to clipboard.
3. Overlay says the text is copied and can be pasted manually.

Success: no work is lost.

### 4. Recover last dictation

1. Murmur inserts text into the wrong field or output is wrong.
2. User triggers undo/recover.
3. Murmur either sends normal undo or exposes the previous dictation from history.

Success: errors are low-cost.

### 5. Rewrite selected text

1. User selects text.
2. User holds command-mode hotkey.
3. User says “make this shorter and more direct.”
4. Murmur replaces the selected text or copies the replacement to clipboard.

Success: the selected text changes predictably and can be undone.

## UX principles

- **Input method, not app window:** UI should be minimal and transient.
- **Hold-to-talk first:** no accidental recording.
- **Fast visual feedback:** recording and processing must be obvious.
- **Do not over-clean:** preserve meaning; remove speech artifacts.
- **Never lose output:** history and clipboard fallback are mandatory.
- **No generic assistant microcopy:** keep labels short and functional.

## Modes

### Clean mode

Default. Removes fillers, fixes punctuation, handles self-corrections, and lightly improves grammar. It should not rewrite tone aggressively.

### Raw mode

Literal transcript with punctuation only. Useful for exact notes, commands, quoted speech, and debugging.

### Command mode

Transforms selected text according to spoken instruction. If there is no selection, V1 may route to search or copy a generated answer, but MVP can skip this.

### Code mode

Later mode for editor contexts. More literal with code terms and punctuation, less prose smoothing.

## Success metrics

Personal/local metrics are more useful than SaaS analytics at first:

- median time from hotkey release to inserted text;
- percentage of dictations accepted without manual edit;
- clipboard fallback frequency;
- failed insertion frequency;
- words dictated per day;
- user-triggered undo/recovery frequency;
- top dictionary misses.

## Open questions

- Which STT provider gives the best speed/quality tradeoff on this machine?
- Can Wayland insertion be reliable enough with clipboard + paste simulation?
- Should cleanup happen via local LLM, cloud LLM, or simple deterministic post-processing first?
- How much context should be captured from the focused app, if any?
- What is the right overlay implementation: Quickshell, Tauri, or native GTK/Qt?
