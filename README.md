# Murmur Dictation

A Linux-native, Wispr Flow-inspired dictation layer for faster input everywhere on this computer.

## Thesis

Murmur is not a general voice agent first. It is a **universal voice keyboard**: hold a hotkey, speak naturally, and get clean text inserted into the app you are already using.

The agentic part lives mostly in the text transformation layer: punctuation, filler removal, self-correction handling, personal vocabulary, app-aware style, selected-text rewrites, and small explicit commands like “press enter.”

## Current status

Planning repository. No implementation yet.

## Documents

- [`docs/research/wispr-flow.md`](docs/research/wispr-flow.md) — research notes on Wispr Flow behavior and product model.
- [`docs/product/prd.md`](docs/product/prd.md) — product requirements for the first versions.
- [`docs/engineering/architecture.md`](docs/engineering/architecture.md) — proposed Linux/Wayland architecture.
- [`docs/roadmap.md`](docs/roadmap.md) — phased implementation plan.

## Product principles

1. **Fast enough to trust** — perceived latency matters more than feature breadth.
2. **Works anywhere text can go** — current-app insertion is the product.
3. **Speech is messy** — cleanup should preserve intent without over-writing the user.
4. **Reversible by default** — history, clipboard fallback, and undo are core UX.
5. **Personal vocabulary matters** — project names, people, tools, and slang should improve over time.
6. **No chatbot ceremony** — this should feel like an input method, not a conversation.
7. **Explicit for risky actions** — sending, deleting, running shell commands, or submitting forms require clear intent/confirmation.

## MVP in one sentence

Hold a global hotkey, speak, release, transcribe + lightly clean the speech, and paste the result into the focused app with a small status overlay and recoverable history.
