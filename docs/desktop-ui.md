# Tauri desktop interface

The interface uses React and Mantine inside Tauri. Rust continues to own transcription, call audio, model inference, hotkeys, insertion and local storage. Tauri is the only application interface.

## Build and run

Install Node.js 22 LTS or newer, Rust, GitHub CLI and Visual Studio C++ Build Tools, then run the build script. It builds the frontend, prepares native runtimes and embeds the UI in the executable:

```powershell
.\scripts\build-windows.ps1
```

Open `Start.cmd` or `target/release/articulate.exe`. Keep only one Articulate instance open. Use `build-windows.ps1 -Debug` to build `target/debug/articulate.exe` instead. `CARGO_TARGET_DIR` is respected by both the script and launcher. Required native runtime DLLs must remain beside the executable, as with the original application.

The frontend is embedded in the executable. It does not require a development server. Build the frontend again before rebuilding the native executable after UI changes.

For a browser-only interface preview:

```powershell
npm --prefix ui run dev
```

Open `http://127.0.0.1:4173`. This preview uses synthetic documents stored in the browser and cannot record audio or access desktop transcripts. Native connection errors never fall back to sample documents.

## Available screens

- Dictation with recording state, recent text and a separate non-activating indicator.
- Notetaker for personal notes, Conversations for meetings and calls, and Dictation for recent dictations. Saved documents include transcript search and participant labels.
- Vocabulary and snippets with searchable lists and accessible editing dialogs.
- Insights calculated from retained local dictations.
- Settings for shortcuts, hold or toggle behavior, audio devices, automatic cleanup, Discord auto-connect and capture, and local speech and notes models.

Document edits save after a short delay. Navigation, note generation and window close drain pending edits first. A failed save keeps changes available for retry. Manual note edits remain protected from automatic generation by the existing Rust controller.

On launch, the desktop interface checks the public release feed for app updates and checks the configured Vencord companion locally. App checks repeat once per day while the interface remains open. Available updates, ready-to-install builds, companion restart requirements and update errors appear as one-time toasts and in the persistent notification menu on every workspace. The menu opens the relevant Settings section. Downloading, installing and restarting remain explicit actions.

Interface motion uses Motion for React for the shared navigation selection, Mantine transitions for dialogs and menus, and short CSS transitions for page entry, card feedback and recording status. The first six library cards have a bounded entrance stagger. Transcript rows and edited document text stay stable during background updates. System reduced-motion preferences disable movement, pulses and CSS transitions. See [Motion accessibility](https://motion.dev/docs/react-accessibility).

## Migration boundaries

Per-app writing styles and manual Polish previews are not yet exposed in the React interface. The old UI has been removed; these remaining controls must be ported before claiming feature parity. Build and packaging scripts target the single Tauri executable; no release is published by building locally.

## Validation

```powershell
npm --prefix ui test
npm --prefix ui run format:check
npm --prefix ui run build
cargo test --locked --bin articulate
cargo clippy --locked --bin articulate -- -D warnings
```

See the [IPC contract](desktop-ui-contract.md) for command and snapshot schemas. Local captures and QA reports belong in ignored local directories, not published source.
