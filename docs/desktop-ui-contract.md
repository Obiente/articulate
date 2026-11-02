# Desktop UI bridge

The `articulate` binary is a Tauri shell that runs the application controller on one Rust-owned thread. Tauri is the sole interface and requires no feature flag. The renderer never receives audio buffers, native handles, the Discord pairing key or the model folder.

## State sections

Renderer state is one flat object (snake_case keys) assembled from named **sections**. Every key belongs to exactly one section. Each section has a revision; the newest revision of any section is the state revision. A section only gets a new revision when its serialized content changes, and only changed sections are sent.

Sections are typed serde views in `src/app/desktop/view/`; the registry is `view::collect`. A test (`snapshot_sections_have_a_locked_shape`) locks the section names, their keys and the nested shapes below.

| Section | Keys | Cadence |
| --- | --- | --- |
| `controller` | `controller` | On change (published by the bridge) |
| `shell` | `ready`, `loading`, `busy`, `status`, `saving`, `history_loading`, `history_error`, `filing_status`, `filing_working`, `source_move_working`, `summary_working` | Live |
| `capture` | `recording`, `call_recording`, `note_recording`, `seconds`, `call_status`, `note_status`, `call_cues_enabled`, `call_cues_status`, `capture_feedback` | Live |
| `dictation` | `text`, `original`, `dictation_session` | Live |
| `live` | `call_session`, `note_session` | Live |
| `live_rows` | `call_rows`, `note_rows` | Live, sent as row splices |
| `document` | `selected` | Live |
| `notes` | `notes`, `selected_notes`, `call_notes`, `note_notes` | Live |
| `settings` | `settings`, `microphones`, `outputs` | Live |
| `models` | `model_status`, `downloading` | Live |
| `discord` | `discord` | Live |
| `updater` | `updater` | Live |
| `overlay` | `overlay` | Live |
| `history` | `history` | Background |
| `document_body` | `selected_body` | Background |
| `vocabulary` | `dictionary`, `macros` | Background |
| `insights` | `insights` | Background |
| `search` | `search` | Live |
| `storage` | `storage`, `library_notice` | Live |
| `recovery` | `recovery` | Background |

Live sections are compared every 250 ms. Background sections (large lists and document bodies) are compared about once a second, after every action, and in the same pass whenever `document` changes, so a newly opened document arrives with its body. Comparison serializes the borrowed view into a reused buffer and compares bytes; nothing is sent for unchanged sections.

```ts
type Controller = {
  state: "starting" | "running" | "failed" | "unresponsive";
  message: string | null;
  can_close_anyway: boolean; // closing without verified saves is allowed
};
type Summary = {
  id: string; title: string; kind: "call" | "dictation" | "note";
  created_ms: number; updated_ms: number; preview: string; duration_ms: number;
  topic_id: string | null; is_collection: boolean; can_receive_sources: boolean;
  pinned: boolean;
};
// `selected`, `call_session` and `note_session`: metadata and notes only.
type SessionMeta = {
  id: string; title: string; kind: string; personal_notes: string;
  audio_packets_lost: number; created_ms: number; updated_ms: number;
  topic_id: string | null; is_collection: boolean; can_receive_sources: boolean;
  sources: { id: string; title: string; kind: string; created_ms: number; preview: string }[];
  pinned: boolean;
  follow_ups: number;                    // generated action items still in the notes
  subtitles_unavailable: string | null;  // why SRT/WebVTT can't be exported
};
// `selected_body`: the open document's transcript, sent separately.
type SessionBody = { id: string; text: string; original: string; rows: Row[] };
// `dictation_session`: both together.
type SessionView = SessionMeta & Omit<SessionBody, "id">;
type Row = {
  start_ms: number; end_ms: number; speaker: string; speaker_labels: string[];
  text: string; cues: { start_ms: number; end_ms: number; label: string }[];
  provisional: boolean;
};
type Overlay = { visible: boolean; recording: boolean; finishing: boolean; message: string };
type Updater =
  | { state: "idle" | "checking" | "latest" }
  | { state: "available"; version: string; size: number }
  | { state: "downloading"; version: string; progress: number }
  | { state: "ready"; version: string }
  | { state: "error"; error: string };
```

`settings` contains `clean_speech`, `insert`, `live_insert`, `cpu`, `transcription_language`, `audio_context`, `learn_corrections`, `audio_feedback`, `microphone`, `output`, `hotkey`, `quick_note_hotkey`, `quick_note_hotkey_pending`, `quick_note_shortcut_error`, `hotkey_mode`, `discord_auto_connect`, `discord_auto_transcribe`, `discord_companion` and `vencord_auto_update`. `model_status` reports installation, progress and status for the speech, speaker, audio context, second speech (verifier) and cleanup models, plus `backend`, `model_name` (file name only), `speech_languages` and `model_custom`. `downloading` is the speech model download progress from 0 to 1. The `discord`, `capture_feedback` and notes status objects are produced by their feature modules and keep the shapes described below.

A live call's or spoken note's transcript is only in `call_rows` or `note_rows`; `call_session` and `note_session` carry no `text`, `original` or `rows`. The open document arrives as `selected` (metadata and notes) plus `selected_body` (transcript), so editing notes never resends a long transcript. The renderer joins them into `state.selected` when both have the same `id`; until the body arrives, the joined document has empty text and rows.

## Transport

Updates have one shape, used by the `desktop-state` event and the `desktop_snapshot` command:

```ts
type StateUpdate = { revision: number; sections: Record<string, Section> };
type Section = {
  revision: number;
  data?: Record<string, unknown>; // the complete section; keys merge into state
  base?: number;                  // row splices apply to this section revision
  splices?: Record<string, { start: number; rows: Row[] }>;
};
```

- The controller emits `desktop-state` to the main window whenever a pass changes any section. The event carries only changed sections.
- `live_rows` changes are sent as splices: keep rows `0..start` from the renderer's copy at revision `base`, then append `rows`. A renderer whose revision for the section is not `base` must resynchronize.
- `desktop_snapshot({ since?: Record<string, number> })` returns every section newer than `since`, or all sections without it. Row sections are returned as splices when the renderer is exactly one revision behind, otherwise as complete `data`.
- `desktop_revision()` returns the state revision. The renderer calls it every two seconds as a fallback for missed events and fetches newer sections when it differs.
- `desktop_health()` returns the `Controller` object. It also reports `unresponsive` after a close request times out, which the event stream cannot.

The renderer (`ui/src/state/store.js`) applies sections in order, ignores sections at or below its revision, and keeps the previous object identity for every unchanged value, including individual rows and history entries (matched by `id`).

## Actions

Invoke `desktop_action` with `{ action: { type: "...", ... } }`. The promise resolves after the controller accepts the action, or rejects with a readable error. The controller publishes the resulting state before replying, so fetching newer sections afterwards reflects the action. Every type is listed in `ui/src/actions.js` with its document and preview flags; a Rust test keeps that list identical to the controller's.

Library and documents (`actions/library.rs`):

- `history_refresh`, `history_open { id }`, `history_close`, `history_retry`, `note_new`, `insights_refresh`
- `session_patch { id, title?: string, personal_notes?: string }`: patches only the identified open or current session; never replaces transcript fields. Rejected while a move or deletion affects that document (see below).
- `session_delete { id }`, `notes_generate { id }`
- `dictation_edit { id, expected_text, text, correction?: { heard, wanted, cues? } }`
- `source_move { id, topic_id: string|null, expected_topic_id: string|null }`, `source_file { id }`: move one independent source or retry automatic topic filing.
- `history_search { query, filters?: { from_ms?, to_ms?, kind?: "call"|"dictation"|"note", speaker?, app? } }`: full-text search; replaces and cancels the previous search. An empty query clears the results. Dates are inclusive creation times. Query problems are reported in `search.error`, not as a rejected action.
- `session_pin { id, pinned }`: pinned items are kept by retention.
- `session_export { id, format: "text"|"markdown"|"srt"|"vtt" }`: export an open, captured or selected document. Formats the document can't produce (see `subtitles_unavailable`) are rejected before a dialog opens.
- `followups_export { id, format: "markdown"|"ics", reminder_ms?: number|null }`: export the document's `follow_ups` as a Markdown checklist or calendar tasks, optionally with a future reminder. Rejected when there are none.

Privacy and storage (`actions/library.rs`):

- `storage_encryption { enabled }`: the only way to change encryption at rest. Progress and the result are in `storage`.
- `retention_preview { policy }`, `retention_save { policy }` with `policy = { enabled, days, dictations, calls, notes }`. Saving an enabled policy requires a finished preview of exactly that policy; the previewed items are deleted at once and later ones once a day. Saving a disabled policy needs no preview.
- `quarantine_list`, `quarantine_restore { file_name }`, `quarantine_export { file_name }`, `quarantine_discard { file_name }`: review isolated files from `recovery.quarantine`.
- `backup_create { passphrase?: string|null }` (8 or more characters when present), `backup_restore { passphrase?, replace_older, include_settings }`, `history_export_markdown`.

Actions that ask for a file or folder (`session_export`, `followups_export`, `quarantine_export`, `backup_create`, `backup_restore`, `history_export_markdown`) resolve once the dialog is open. The dialog runs on its own thread so capture and autosave continue; only one can be open at a time (`storage.dialog_open`). Outcomes arrive as `library_notice` or in the matching `storage` result.

Capture (`actions/capture.rs`):

- `dictation_start`, `dictation_stop`, `dictation_cancel`
- `call_start { source?: "system"|"discord", language_preference?: string|null, audio_context?: boolean }`, `call_stop`
- `note_capture_start`, `note_capture_stop`

Copy and export (`actions/output.rs`): `copy { text }`, `export { text, extension }` with extension `txt`, `md`, `srt` or `vtt`. Export runs outside the controller so a save dialog never pauses capture.

Vocabulary (`actions/vocabulary.rs`):

- `dictionary_save { heard, wanted, app?: string, cues?: string, index?: number, expected_heard?: string, expected_entry?: Entry }`, `dictionary_delete { index, expected_heard, expected_entry }`
- `macro_save { trigger, expansion, app?: string, index?: number, expected_trigger?: string, expected_macro?: VoiceMacro }`, `macro_delete { index, expected_trigger, expected_macro }`

Settings (`actions/settings.rs`): `settings_patch { cpu?, model_path?, transcription_language?, clean_speech?, insert?, live_insert?, learn_corrections?, audio_feedback?, audio_context?, microphone?, output?, hotkey_mode?, hotkey?, quick_note_hotkey?, discord_auto_connect?, discord_auto_transcribe?, vencord_auto_update? }`. The whole patch is validated before anything changes; a failed save restores the previous values. Shortcut changes are registered only after the preferences are saved.

Models (`actions/models.rs`): `model_download`, `model_load`, `summary_download`, `summary_download_cancel`, `cleanup_download`, `cleanup_download_cancel`, `context_download`, `speakers_download`, `verifier_download`.

Discord (`actions/discord.rs`): `discord_connect { companion: boolean }`, `discord_disconnect`, `discord_retry`, `discord_relaunch`, `companion_detect`, `companion_install`, `companion_open_installer`.

Updates (`actions/updates.rs`): `update_check`, `update_download`, `update_install`. Installation requires capture and processing to finish, no active download, and pending document saves to complete before the verified installer launches.

Editing an existing vocabulary entry requires `index` and its original `expected_heard`; snippet edits require `index` and `expected_trigger`. Edits and deletions also require the complete original snapshot object in `expected_entry` or `expected_macro`. A stale row or changed scope is rejected without mutating data. Device preferences use an empty string to select the Windows default.

### Document locks

`source_move_working` locks open editors while a move between topics is in progress, or while a pending deletion affects an open document. A deletion affects the deleted document itself, the topic that contains it, and recordings filed in a topic that is being deleted. Edits to unrelated documents continue. `session_patch` reports which of these applies.

## Closing

1. A window close request sends `desktop-close-requested` with a numeric request id to the main window.
2. The renderer acknowledges at once with `desktop_close_ack { id }`, saves its pending document edits, then invokes `desktop_close`. If it decides not to close (a document action is running, or saving failed) it sends `desktop_close_ack { id, cancelled: true }`.
3. Without an acknowledgement within three seconds, the native shell closes on its own: the controller saves everything it holds, then the app exits. Edits still only in an unresponsive renderer cannot be saved.
4. Another close request while the first is unanswered for over a second, or acknowledged for over two seconds without closing, escalates to the same native shutdown.
5. Native shutdown and `desktop_close` refuse while recording or processing is active, while a model or update download is running, or while saves have not settled; the reason is shown. A rejected close keeps the window open and recoverable.
6. When the controller has failed or stopped answering, a rejected close offers **Close anyway**: `desktop_close { force: true }` in the renderer, or a confirmation dialog from native code. `force` is refused while the controller is healthy.

If the controller panics, it stops normal work, logs the panic, publishes `controller.state = "failed"` with the message, hides the indicator and rejects further actions. A close request still tries to save, so a clean close remains possible.

## Indicator window

The native dictation indicator sets `window.__ARTICULATE_OVERLAY__ = true` before loading the frontend. It may only listen for events. It receives `desktop-overlay` events with the `Overlay` object whenever it changes, and invokes `desktop_overlay` once on load and every few seconds as a fallback. It cannot invoke state-changing commands. Native code positions, shows and hides this nonfocusable, mouse-passthrough window when `visible` changes.

## Feature status objects

`capture_feedback` carries a monotonic `sequence`, `event` (`started`, `stopped`,
or `failed`), `kind` (`call` or `note`), optional `session_id`, and `status`.
It begins at sequence zero with no event. Frontends baseline their first
snapshot and deduplicate subsequent notifications. Native sounds and the
nonactivating overlay follow capture acknowledgements and stop boundaries,
independently of renderer updates. They never imply that final transcription
or persistence has completed.

`notes`, `selected_notes`, `call_notes` and `note_notes` share one shape: `{ ready, acceleration_installed, working, status, progress, download_status, downloading }`. `notes` follows the selected document, or the current call when nothing is selected; the others are scoped to the selected document, the current call and the current spoken note.

Discord snapshots distinguish `listener_started` from fresh `connected`
membership, with `in_voice`, `audio_ready`, `pairing_state`, `automatic_status`,
and `automatic_can_retry`. The `discord_retry` action clears an automatic
capture pause and retries model loading without replacing a healthy listener.
The companion discovers its authentication key locally; the desktop renderer
never receives it.

`companion.runtime_status` verifies the fresh companion fingerprint separately
from source/build receipts (`current`, `restart_required`, `update_required`,
`offline`, or `unverified`). The fingerprint covers the plugin and native audio
payload and is compiled into the companion. Optional `audio_status` and
`audio_message` explain preload or native attachment failures; old companions
remain compatible and report unknown status. Only allowlisted adapter states
cross the metadata protocol, not arbitrary logs.

Spoken notes expose `note_recording`, `note_session`, `note_rows`, `note_status`
and per-document `note_notes`. They do not set `call_recording`. Topic sessions
include `is_collection`, `topic_id`, `can_receive_sources` and a `sources` list;
source text remains in independently saved sessions. `filing_working` and
`filing_status` describe automatic organization. Pending filing is persisted for
retry after restart.

```ts
type Search = {
  query: string; working: boolean; error: string | null;
  filters: { from_ms: number|null; to_ms: number|null; kind: string|null; speaker: string|null; app: string|null };
  results: null | {
    query: string; indexed: boolean; partial: boolean; more: boolean;
    scanned_sessions: number; unreadable_sessions: number;
    hits: { session_id: string; title: string; kind: string; created_ms: number;
      field: "title"|"text"|"original"|"personal_notes"|"generated_notes"|"transcript"|"speaker_names";
      excerpt: string; matches: [number, number][]; // UTF-8 byte ranges within `excerpt`
      speaker: string|null; start_ms: number|null; end_ms: number|null }[];
  };
};
type Progress = "idle" | "choosing" | "working";
type Storage = {
  encrypt_at_rest: boolean; encryption_working: boolean;
  encryption_result: null | { error: string|null; protect: boolean; changed: number; failed: number };
  retention: Policy; retention_preview_working: boolean;
  retention_preview: null | { policy: Policy|null; error: string|null; total: number;
    items: { id: string; title: string; kind: string; updated_ms: number }[] }; // first 100
  backup: Progress; backup_result: null | { error: string|null; path: string|null; sessions: number; encrypted: boolean; skipped: number };
  restore: Progress; restore_result: null | { error: string|null; sessions_in_archive: number; added: number;
    updated: number; kept_local: number; newer_in_backup: number; deleted_locally: number; failed: number;
    settings_restored: boolean; shortcuts_changed: boolean; model_reload: boolean };
  markdown: Progress; markdown_result: null | { error: string|null; folder: string|null; files: number; skipped: number };
  dialog_open: boolean;
};
type LibraryNotice = null | { sequence: number; error: boolean; message: string };
type Recovery = {
  warnings: { id: string; kind: "damaged"|"unavailable"|"newer_version"; message: string }[];
  quarantined: number; listed: boolean;
  quarantine: { file_name: string; id: string; bytes: number; modified_ms: number;
    restorable: boolean; title: string|null; reason: string }[];
};
```

`search` holds at most 100 hits; `more` reports that the index returned others. `indexed: false` means the full-text index was still being built and a bounded scan of recent files answered instead. `library_notice.sequence` increases with every notice; renderers baseline the first value they see and show each later notice once. A restore that includes preferences keeps this device's encryption and retention choices, registers changed shortcuts again and sets `model_reload` when the speech model's language or processor changed.

Insights contain `words`, `sessions`, `words_per_minute`, `current_streak`, `longest_streak`, `active_days`, `dictionary_replacements`, `cleanup_edits`, `apps: [{app, words, sessions}]` and `days: [{date, words, sessions}]`.

The Tauri interface includes model installation, companion setup and application
updates. Per-app writing-style editing and manual Polish previews are not yet
exposed in the renderer. There is no legacy UI build.
