# Wispr Flow research notes

Research date: 2026-04-30

Sources reviewed:

- Wispr Flow product site: <https://wisprflow.ai/>
- Wispr Flow Help Center: <https://docs.wisprflow.ai/>
- “What is Flow?” help article
- “How to use Command Mode” help article
- Shortcut and privacy help pages discovered through the Help Center

## What Wispr Flow is

Wispr Flow is an AI voice dictation app for Mac, Windows, iOS, and Android beta. It positions itself around **dictating anywhere you can type**, claiming roughly 4x faster input than typing.

The core workflow:

1. Focus any text field.
2. Press/hold a hotkey.
3. Speak naturally.
4. Flow transcribes in real time.
5. Clean text appears where the cursor is.

This is best understood as a **universal AI-assisted keyboard**, not primarily a full desktop agent.

## Core product behaviors

### Universal insertion

Flow works in normal text fields across apps and websites: chat apps, email, docs, browsers, editors, and other desktop/mobile surfaces. The important part is that the user does not open a separate dictation workspace; they use Flow inside the app they are already using.

### Hotkey-first interaction

Documented defaults:

- Desktop dictation:
  - Mac: `fn`, or `Ctrl + Option` backup.
  - Windows: `Ctrl + Win`.
- Command Mode:
  - Mac: `fn + Ctrl`, or `Cmd + Ctrl + Option` backup.
  - Windows: `Ctrl + Win + Alt`.
- `Esc` cancels command mode / processing where supported.

Shortcuts can be customized, with constraints around reserved OS shortcuts. Mouse buttons can be bound for Command Mode.

### Speech cleanup

Flow handles natural speech rather than literal transcription only:

- automatic punctuation;
- filler word removal;
- self-correction handling;
- formatting cleanup;
- vocabulary adaptation for names, technical terms, and repeated user-specific words;
- 100+ language support per public marketing/help content.

### Command Mode

Command Mode is a paid/experimental feature in Flow.

With selected text:

1. Highlight text in any app.
2. Hold the Command Mode shortcut.
3. Speak an instruction such as “make this more concise,” “translate to Polish,” or “turn this outline into an essay.”
4. Flow replaces the highlighted text with the transformed result.

Without selected text:

- Desktop opens Perplexity with the spoken query.
- iOS supports voice triggers for destinations such as Perplexity, Google, ChatGPT, and Claude.

Behavioral constraints:

- Selected text must be between 1 and 1,000 words.
- If Flow cannot paste into the current field, it saves the result to clipboard and notifies the user.
- If a transform makes no change, Flow shows a “Your text looks good!” notification.
- Command Mode cannot start while a previous transcription is processing.

### “Press enter” voice command

If the user says “press enter” at the end of dictation, Flow removes the phrase from the output and sends an Enter key after insertion. If “press enter” is the only phrase, it presses Enter without inserting text.

This is useful for chat/prompt workflows where Enter submits the current message. Flow introduces the feature with a first-use notification and lets the user disable it.

### Tone retransform

After transforming dictated text, Flow supports rewriting with tone presets:

- Professional
- Casual
- Gen Z
- Partner
- custom instructions

This suggests a two-step UX: insert a reasonable default quickly, then optionally refine.

### Automatic background transform

For longer dictations, Flow can transform text in the background and show a “Transform ready” notification. The user can reveal a diff, then accept or dismiss suggested edits.

### Dictionary, snippets, styles

Flow includes a Hub for:

- transcription history;
- dictionary/custom words;
- snippets;
- personalization styles;
- dictation stats and streaks.

On Android, personalization styles apply by app category such as Personal, Work, Email, and Other. Snippets are text expansion shortcuts that sync across devices.

### Flow Fetch

Desktop early-access feature. Flow tracks recently copied URLs locally for up to 14 days and exposes them from the Flow bar. Privacy-sensitive URLs, such as auth-token links or password-manager URLs, are filtered out. Links are not uploaded or synced.

This is adjacent to dictation, but not core to the MVP.

## Privacy and data behavior

Important public/help claims:

- Flow requires an internet connection for transcription.
- Transcription history is stored locally per device and does not sync.
- Audio recordings are automatically cleaned up after 14 days; text is retained.
- Privacy Mode disables transcription history.
- Flow Fetch links are stored locally only and retained up to 14 days.
- The company claims SOC 2 Type II, ISO 27001, and HIPAA compliance.

Product implication: Flow chooses cloud-quality transcription plus compliance/privacy controls, not fully offline operation.

## Lessons for Murmur

1. The primary product is **fast universal text insertion**, not broad autonomous control.
2. A hold-to-talk hotkey is the right default mental model.
3. Cleanup should be conservative: preserve intent, remove speech artifacts, add punctuation.
4. Personal vocabulary is not optional; it is a core accuracy feature.
5. Selected-text rewrite is the most valuable “agentic” feature after base dictation.
6. “Press enter” is small but high-leverage for chat and prompt workflows.
7. Clipboard fallback and history are safety features, not nice-to-haves.
8. Linux support is an opening, especially for Wayland users.
9. The UI should feel like an input method: quick overlay/status, not chatbot chrome.
