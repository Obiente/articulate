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
- Settings for shortcuts, hold or toggle behavior, audio devices, automatic cleanup, Discord edition, pairing, pictures, auto-connect and capture, meeting detection for Teams, Zoom and Google Meet, and local speech and notes models.

Document edits save after a short delay. Navigation, note generation and window close drain pending edits first. A failed save keeps changes available for retry. Manual note edits remain protected from automatic generation by the existing Rust controller.

The controller pushes only the state sections that changed, and live transcript rows as appended or revised rows, so idle windows do no polling beyond a cheap revision check every two seconds. Long transcripts render only the rows near the visible area. Closing waits for the window to save its edits; if the window does not answer within three seconds, or the user closes again while it seems stuck, the app saves from native code and closes. If the controller stops working, the window shows the error and offers **Close anyway** after explaining that unsaved work could not be verified.

On launch, the desktop interface checks the public release feed for app updates and checks the configured Vencord companion locally. App checks repeat once per day while the interface remains open. Available updates, ready-to-install builds, companion restart requirements and update errors appear as one-time toasts and in the persistent notification menu on every workspace. The menu opens the relevant Settings section. Downloading, installing and restarting remain explicit actions.

Interface motion uses Motion for React for the shared navigation selection, Mantine transitions for dialogs and menus, and short CSS transitions for page entry, card feedback and recording status. The first six library cards have a bounded entrance stagger. Transcript rows and edited document text stay stable during background updates. System reduced-motion preferences disable movement, pulses and CSS transitions. See [Motion accessibility](https://motion.dev/docs/react-accessibility).

## Migration boundaries

Per-app writing styles and manual Polish previews are not yet exposed in the React interface. The old UI has been removed; these remaining controls must be ported before claiming feature parity. Build and packaging scripts target the single Tauri executable; no release is published by building locally.

## Structure

Rust (`src/app/desktop/`):

| Module | Owns |
| --- | --- |
| `desktop.rs` | Module root and re-exports for `main.rs`. |
| `bridge.rs` | The controller thread, panic isolation, controller health and the close handshake. |
| `publish.rs` | Section revisions, byte-level change detection, row splices and the shared published state. |
| `view.rs` | The section registry (`collect`). |
| `view/*.rs` | Typed views by area: `shell`, `capture` (dictation, calls, spoken notes, live rows), `library` (history, open document, insights), `settings` (preferences, vocabulary), `models`, `discord` (connection, participants, companion), `meetings` (meeting detection), `updates`, `rows`. |
| `actions.rs` | Routes a renderer action to the feature that declares its `type`. |
| `actions/*.rs` | One action enum and `App` handler per feature: `library`, `capture`, `output`, `vocabulary`, `settings`, `models`, `discord` (including pairing and meeting prompts), `updates`. |
| `lifecycle.rs` | Shutdown, capture readiness, download guards and document edit locks. |
| `tests.rs` | Contract tests: section shapes, key ownership and the action list. |

`src/main.rs` registers the IPC commands (including `desktop_discord_avatar`, which serves cached participant pictures to the main window), forwards published sections as Tauri events, runs the close handshake and the indicator window. `src/diagnostics.rs` owns logging.

Frontend (`ui/src/`):

| Path | Owns |
| --- | --- |
| `App.jsx` | Shell: navigation, top bar, banners and page routing. |
| `bridge.js` | Transport to the controller, or to the preview workspace in a browser. |
| `actions.js` | The action catalog (document and preview flags) and validation shared by both transports. |
| `state/store.js`, `state/desktop-store.js` | Applying section updates with stable object identity; subscription, catch-up and the fallback revision check. |
| `preview/` | Synthetic preview workspace and fixtures. |
| `hooks/` | Document edit autosave, the close handshake and the list virtualizer. |
| `components/` | Shared page, dialog and settings-row components. |
| `lib/format.js` | Dates, durations, shortcuts and language names. |
| `features/<area>/` | `dictation`, `library`, `notes` (document reader), `calls` (transcript rows, speaker timeline, call setup), `capture`, `vocabulary`, `insights`, `settings`, `discord` (settings, participant list, the reusable `ParticipantAvatar`), `meetings` (settings section, detection prompt and indicator), `models`, `updates`, `overlay`. |

### Adding an action

1. Add a variant to the feature's enum in `src/app/desktop/actions/<feature>.rs` and handle it in that file's `desktop_<feature>_action`. For a new feature, create the module, add it to the `Action` enum, `FEATURES` and `App::desktop_action` in `actions.rs` (one line each).
2. Add the type to `actionCatalog` in `ui/src/actions.js`. Set `document: true` when it can replace or remove the open document, and `preview: true` with a handler in `ui/src/preview/workspace.js` when the browser preview can support it. Add shared validation to `validators` when the renderer can reject bad input early with the controller's message.
3. Call it with `act({ type: "...", ... })` from the feature's components.
4. Document it in [desktop-ui-contract.md](desktop-ui-contract.md). `renderer_action_types_match_the_frontend_catalog` fails until both lists agree.

### Adding a state section

1. Write a borrowed serde view and a `fn <name>(app: &App) -> View<'_>` in the matching `src/app/desktop/view/<area>.rs` (or a new module). Its fields are new top-level state keys; never reuse a key from another section.
2. Add one line to `view::collect`. Use `Cadence::Background` for large or rarely changing values; they are compared about once a second and after every action.
3. Add the section and its keys to `snapshot_sections_have_a_locked_shape` and to the contract table.
4. Read the keys from `state` in the renderer. Unchanged values keep their identity, so `memo` components and `useMemo` dependencies skip work.

## Diagnostics

The desktop app writes a local log to `%LOCALAPPDATA%\TranscribeLocal\logs\articulate.log`. It records startup, controller failures, panics with their source location, close requests and errors. It never records transcripts, notes or other document text. Each file is limited to 1 MB; the three previous files are kept as `articulate.1.log` (newest) to `articulate.3.log`. Set `ARTICULATE_LOG` to `error`, `warn`, `info` (default), `debug` or `trace` to change the level. Messages from libraries are recorded only at warning level or above.

Command-line runs (any argument, such as `--devices`) attach to the console that started them, so release builds print their output there, and report warnings on standard error. Windows returns to the prompt before a GUI-subsystem program finishes, so output can appear after the prompt; pipe or redirect it, for example `.\articulate.exe --devices | Out-String`, to read it in order.

## Validation

```powershell
npm --prefix ui test
npm --prefix ui run format:check
npm --prefix ui run build
cargo test --locked --bin articulate
cargo clippy --locked --bin articulate -- -D warnings
```

See the [IPC contract](desktop-ui-contract.md) for command and state schemas. Local captures and QA reports belong in ignored local directories, not published source.
