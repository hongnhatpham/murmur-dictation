# PRD: Wispr Flow-style AI Correction

## Summary

Murmur should evolve from local STT plus deterministic cleanup into a context-aware voice keyboard that turns messy speech into text that fits the current app, cursor position, and user style. The goal is not broad assistant behavior; it is low-friction dictation that needs fewer manual edits.

This PRD adapts the useful parts of Wispr Flow's correction model for a Linux/Wayland/local-first Murmur implementation.

## Source model

Wispr Flow's public docs describe five relevant layers:

1. raw speech-to-text;
2. context awareness from active app and nearby text;
3. per-app/category style settings;
4. personal dictionary and replacement rules;
5. a polish/correction pass that removes filler, restores punctuation, lightly fixes grammar, and preserves meaning.

Murmur should copy the product shape, not the cloud architecture. The default path should remain local/free where practical, with optional local LLM correction before any cloud adapter is considered.

## Goals

- Reduce manual edits after dictation.
- Preserve speaker intent and avoid surprising rewrites.
- Adapt output to the active app: terminal/code, chat, email, docs, and generic fields.
- Use nearby text to fix spacing, casing, names, and continuation behavior.
- Make personal vocabulary and preferred writing style first-class local data.
- Keep latency acceptable for short dictations.
- Keep correction optional and observable so raw STT remains debuggable.

## Non-goals

- Always-listening assistant behavior.
- Sending messages or running commands without explicit spoken action.
- Cloud-only correction as the default.
- Full browser/editor automation.
- Rewriting the user's meaning for style.

## Users and contexts

Primary user: a Linux/Wayland power user dictating into terminals, code editors, AI chat, messaging apps, email, docs, and notes.

Important contexts:

- **Terminal/code:** literal text, conservative punctuation, no aggressive prose cleanup.
- **AI chat/docs:** polished prose, good sentence spacing, clear punctuation.
- **Work messages:** concise, casual-professional, usually no excessive punctuation.
- **Personal messages:** casual, potentially no trailing period.
- **Email:** more formal capitalization and sentence boundaries.

## User stories

- As a user, I can dictate a rough sentence and get clean text with punctuation and filler removed.
- As a user, I can dictate after existing text and Murmur inserts the right leading/trailing spacing.
- As a user, I can dictate into a terminal without Murmur rewriting shell/code-like text aggressively.
- As a user, I can add names/project terms once and have Murmur preserve spelling/casing.
- As a user, I can choose different style defaults for chat, email, docs, terminal, and code contexts.
- As a user, I can see whether a correction was deterministic, local AI, or skipped.
- As a user, I can recover the raw transcript if the correction is wrong.

## Functional requirements

### 1. Raw STT layer

- Continue producing and storing raw transcripts.
- Preserve provider/model metadata in history.
- Keep raw mode available and fast.

### 2. Context awareness

- Detect focused app and map it to a Murmur category.
- Read safe nearby context where possible.
- Capture cursor-adjacent text when available or infer from clipboard/current insertion fallback when not.
- Use context to decide leading space, lowercase continuation, proper noun casing, and punctuation behavior.
- Never read password fields when detectable; allow context awareness to be disabled.

### 3. App/category styles

- Add configurable style presets by category:
  - terminal/code;
  - AI chat/docs;
  - work messaging;
  - personal messaging;
  - email;
  - other.
- Style controls:
  - formality;
  - punctuation density;
  - trailing period policy;
  - capitalization/continuation behavior;
  - rewrite aggressiveness.

### 4. Personal dictionary

- Extend the existing dictionary to support:
  - preferred casing;
  - replacement rules;
  - pronunciation hints where useful;
  - category-specific terms;
  - likely-miss review from history.
- Feed dictionary terms into STT hints and correction prompts.

### 5. AI polish/correction

- Add an optional correction provider after deterministic cleanup.
- First target: local Ollama or llama.cpp-compatible endpoint.
- Prompt must preserve meaning and avoid adding facts.
- Prompt receives:
  - raw transcript;
  - deterministic cleaned text;
  - app/category style;
  - nearby context summary;
  - dictionary terms;
  - snippets/replacements;
  - explicit actions like `press_enter_after_insert` outside the text body.
- Correction output must be plain insertion text plus structured metadata, not free-form assistant commentary.

## UX requirements

- Correction should not make short dictations feel sluggish.
- If correction times out, Murmur should insert deterministic cleanup rather than fail.
- History should show raw transcript, deterministic text, final corrected text, provider, latency, and fallback reason.
- The Quickshell overlay can show concise states: recording, processing, correcting, inserted, copied, failed.
- No generic assistant copy.

## Privacy and safety

- Local-first default.
- Context awareness and AI correction independently configurable.
- Do not log sensitive nearby context by default.
- History stores final text and transcript as today, but any extra context should be either omitted or summarized unless debug mode is enabled.
- Cloud correction, if ever added, must be opt-in.

## Success metrics

- Lower manual edit rate after dictation.
- Median release-to-insert latency remains acceptable for short dictations.
- Fewer personal dictionary misses over time.
- User can recover raw transcript after any correction.
- Terminal/code contexts avoid destructive rewrites.

## Open questions

- Which local LLM is fast enough for short dictation cleanup on this machine?
- How much context can be captured reliably on Wayland/niri without app-specific plugins?
- Should Murmur apply AI correction synchronously before insertion or insert deterministic text first and optionally refine later?
- How should style samples be stored locally without turning Murmur into a general writing assistant?
