# Prototype Instructions

The user wants polished animations, with React Bits or another suitable animation source as inspiration.

Conversation transcripts should clearly distinguish speakers with consistent colors, readable speaker turns, and controls for focusing on one voice. The Nemotron diarization demo is an interaction reference; preserve Articulate's own visual language.

Keep the workspace navigation task-specific: Notetaker contains only notes, Conversations contains meetings and calls, and Dictation contains dictation history. Starting a conversation must support meeting audio from any app; Discord participant tracks are an optional source when connected.

Conversation setup exposes a language preference for that recording. Keep recognition automatic so speakers can switch languages within a call; use the preference only to resolve ambiguous short words.
Automatic conversation capture inherits the saved language preference and sound cue setting without requiring the setup dialog.

Run the local server yourself and open the preview in the browser available to this environment. Do not give the user server-start instructions when you can run it.

Before making substantial visual changes, use the Product Design plugin's `get-context` skill when the visual source is unclear or no longer matches the current goal. When the user gives durable prototype-specific design feedback, preferences, or decisions, record them in `AGENTS.md`.

When implementing from a selected generated mock, treat that image as the source of truth for layout, component anatomy, density, spacing, color, typography, visible content, and hierarchy.

Build app UI in `src/`. Keep `.openai/hosting.json`, `worker/index.js`, `scripts/prepare-sites-build.mjs`, and `tests/sites-worker.test.mjs` intact so the same local prototype can be handed to Sites. Before a Sites handoff, run `npm run build` and `npm run test:sites`; the build must leave `dist/client/index.html`, `dist/server/index.js`, and `dist/.openai/hosting.json`.
