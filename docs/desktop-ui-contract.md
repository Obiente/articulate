# Desktop UI bridge

The `articulate` binary is a Tauri shell that runs the application controller on one Rust-owned thread. Tauri is the sole interface and requires no feature flag. The renderer never receives audio buffers, native handles, the Discord pairing key or arbitrary filesystem contents. Managed models expose only their file names.

## State sections

Renderer state is one flat object (snake_case keys) assembled from named **sections**. Every key belongs to exactly one section. Each section has a revision; the newest revision of any section is the state revision. A section only gets a new revision when its serialized content changes, and only changed sections are sent.

Sections are typed serde views in `src/app/desktop/view/`; the registry is `view::collect`. A test (`snapshot_sections_have_a_locked_shape`) locks the section names, their keys and the nested shapes below.

| Section | Keys | Cadence |
| --- | --- | --- |
| `controller` | `controller` | On change (published by the bridge) |
| `shell` | `ready`, `loading`, `busy`, `status`, `saving`, `history_loading`, `history_error`, `filing_status`, `filing_working`, `source_move_working`, `summary_working` | Live |
| `capture` | `recording`, `call_recording`, `note_recording`, `seconds`, `call_status`, `note_status`, `call_cues_enabled`, `call_cues_status`, `capture_feedback` | Live |
| `dictation` | `text`, `original`, `dictation_session`, `dictation_interruption` | Live |
| `live` | `call_session`, `note_session` | Live |
| `live_rows` | `call_rows`, `note_rows` | Live, sent as row splices |
| `document` | `selected` | Live |
| `notes` | `notes`, `selected_notes`, `call_notes`, `note_notes` | Live |
| `settings` | `settings`, `microphones`, `outputs` | Live |
| `models` | `model_status`, `downloading`, `managed_models`, `cleanup` | Background |
| `discord` | `discord` | Live |
| `meetings` | `meetings` | Live |
| `updater` | `updater` | Live |
| `overlay` | `overlay` | Live |
| `call_tools` | `audio_import`, `playback` | Live |
| `history` | `history` | Background |
| `document_body` | `selected_body` | Background |
| `capture_bodies` | `dictation_body`, `call_body`, `note_body` | Background |
| `vocabulary` | `dictionary`, `macros`, `assort` | Background |
| `insights` | `insights` | Background |
| `search` | `search` | Live |
| `storage` | `storage`, `library_notice` | Live |
| `recovery` | `recovery` | Background |
| `people` | `people`, `people_error` | Background |

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
// `selected`, `dictation_session`, `call_session` and `note_session`: metadata only.
type SessionMeta = {
  id: string; title: string; kind: string;
  audio_packets_lost: number; created_ms: number; updated_ms: number;
  topic_id: string | null; is_collection: boolean; can_receive_sources: boolean;
  sources: { id: string; title: string; kind: string; created_ms: number; preview: string }[];
  pinned: boolean;
  follow_ups: number;                    // generated action items still in the notes
  subtitles_unavailable: string | null;  // why SRT/WebVTT can't be exported
  interruptions: Interruption[];
  speaker_undo: string | null;      // what `speaker_undo` would revert
  audio: { duration_ms: number; bytes: number; channels: string[]; incomplete: boolean } | null;
  transcript_versions: { id: string; index: number; replaced_ms: number; model: string; preview: string }[];
  transcript_revision: string | null; // the current transcript, while versions exist
};
type Interruption = {
  start_ms: number; end_ms: number;
  source: "microphone" | "system_audio" | "participants" | "processing" | "unknown";
  cause: "device_lost" | "overflow" | "packet_loss" | "fell_behind" | "unknown";
  ongoing: boolean;
  message: string; // "Audio interrupted 12:03–12:07: microphone disconnected"
};
// Each body carries notes and transcript content, sent separately.
type SessionBody = { id: string; personal_notes: string; text: string; original: string; rows: Row[] };
// The renderer combines each session metadata value with its matching body by id.
type SessionView = SessionMeta & Omit<SessionBody, "id">;
type Row = {
  start_ms: number; end_ms: number; speaker: string; speaker_labels: string[];
  // Opaque, parallel to speaker_labels: "you", "s1".."s8", "other",
  // "uncertain", or "n:<digest>" for a named person. Valid for one snapshot.
  speaker_keys: string[];
  text: string; cues: { start_ms: number; end_ms: number; label: string }[];
  provisional: boolean;
};
type AudioImport = {
  state: "idle" | "ready" | "working" | "done" | "failed";
  dragging: boolean;              // audio files are over the window
  file_name: string | null;       // the offered file's name, never its path
  progress: number | null; stage: string; message: string;
  session_id: string | null;      // the finished import or re-transcription
  rebuilding: string | null;      // the session being re-transcribed
  speakers_available: boolean;
};
type Playback = { session_id: string | null; playing: boolean; position_ms: number; error: string | null };
type Person = {
  id: string; name: string; source: "discord" | "name";
  discord_name: string | null; avatar: string | null; // bytes://discord-avatar/... reference
  used_ms: number;
};
type Overlay = { visible: boolean; recording: boolean; finishing: boolean; message: string };
type Participant = {
  id: string; name: string; speaking: boolean; is_self: boolean;
  avatar: string | null; // "bytes://discord-avatar/{id}/{hash}.png", null while pictures are off
};
type Discord = {
  connected: boolean; listener_started: boolean;
  status: "ready" | "connecting" | "unavailable" | "disconnected";
  in_voice: boolean; audio_ready: boolean; audio_status: string | null; audio_message: string | null;
  participants: Participant[];
  pairing_state: "paired" | "waiting" | "automatic"; pairing_key_present: boolean;
  companion_protocol: "current" | "outdated" | "newer" | "unknown"; companion_protocol_message: string | null;
  flavor: "" | "stable" | "ptb" | "canary"; installed_flavors: ("stable" | "ptb" | "canary")[];
  debugger_port: number; debugger_open: boolean; debugger_notice: string | null;
  automatic_can_retry: boolean; error: string | null; relaunching: boolean;
  companion_selected: boolean; automatic_status: string;
  companion: {
    checked: boolean; installed: boolean;
    runtime_status: "current" | "restart_required" | "update_required" | "offline" | "unverified";
    source_found: boolean; active: boolean; busy: boolean; status: string; error: string | null;
    plugin_status: "missing" | "current" | "update_available" | "unknown";
    can_build: boolean; native_audio: boolean; installer_ready: boolean;
  };
};
type MeetingApp = { id: "teams" | "zoom" | "meet"; label: string; best_effort: boolean };
type Meetings = {
  enabled: boolean;
  settings: { teams: boolean; zoom: boolean; meet: boolean; start: "ask" | "automatic" };
  consent_reminder: string;
  active: MeetingApp[]; prompt: MeetingApp | null; owned: MeetingApp | null;
  status: string; probe_error: string | null;
  events: { app: string; label: string; kind: "started" | "ended"; at_unix_ms: number }[];
};
type Updater =
  | { state: "idle" | "checking" | "latest" }
  | { state: "available"; version: string; size: number }
  | { state: "downloading"; version: string; progress: number }
  | { state: "ready"; version: string }
  | { state: "error"; error: string };
```

`settings` contains `clean_speech`, `insert`, `live_insert`, `cpu`, `transcription_language`, `audio_context`, `learn_corrections`, `audio_feedback`, `microphone`, `output`, `hotkey`, `quick_note_hotkey`, `quick_note_hotkey_pending`, `quick_note_shortcut_error`, `hotkey_mode`, `discord_auto_connect`, `discord_auto_transcribe`, `discord_companion`, `vencord_auto_update`, `discord_flavor`, `discord_avatars`, `meetings_teams`, `meetings_zoom`, `meetings_meet`, `meetings_start` and `keep_call_audio`. `model_status` reports installation, progress and status for the speech, speaker, audio context, second speech (verifier) and cleanup models, plus `backend`, `model_name` (file name only), `speech_languages` and `model_custom`. `downloading` is the speech model download progress from 0 to 1. The `discord`, `meetings`, `capture_feedback` and notes status objects are produced by their feature modules and keep the shapes described above and below.

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
- `desktop_revision()` returns the per-section revision map. The renderer checks every section every two seconds and fetches updates if any revision differs.
- `desktop_health()` returns the `Controller` object. It also reports `unresponsive` after a close request times out, also available through the cached controller section.
- `desktop_discord_avatar({ uri })` returns the cached PNG bytes for a `Participant.avatar` identifier, or an empty body while the picture is not cached yet or pictures are turned off. Only identifiers of that exact form are accepted; the renderer never receives Discord URLs and the existing `img-src 'self' data: blob:` policy is enough because the bytes are shown through a blob URL. The renderer retries an empty answer a few times and then keeps initials.

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

- `dictation_start`, `dictation_stop`, `dictation_cancel`. While an import or re-transcription runs (`audio_import.state = "working"`), capture cannot start and `dictation_cancel` is refused; that job has its own `audio_import_cancel`.
- `call_start { source?: "system"|"discord", language_preference?: string|null, audio_context?: boolean, keep_audio?: boolean }`, `call_stop`. `keep_audio` defaults to the `keep_call_audio` preference.
- `note_capture_start`, `note_capture_stop`

Copy and export (`actions/output.rs`): `copy { text }`, `export { text, extension }` with extension `txt`, `md`, `srt` or `vtt`. Export runs outside the controller so a save dialog never pauses capture.

Vocabulary (`actions/vocabulary.rs`):

- `dictionary_save { heard, wanted, app?: string, cues?: string, index?: number, expected_heard?: string, expected_entry?: Entry }`, `dictionary_delete { index, expected_heard, expected_entry }`
- `macro_save { trigger, expansion, app?: string, index?: number, expected_trigger?: string, expected_macro?: VoiceMacro }`, `macro_delete { index, expected_trigger, expected_macro }`

Settings (`actions/settings.rs`): `settings_patch { cpu?, model_path?, transcription_language?, clean_speech?, insert?, live_insert?, learn_corrections?, audio_feedback?, audio_context?, microphone?, output?, hotkey_mode?, hotkey?, quick_note_hotkey?, discord_auto_connect?, discord_auto_transcribe?, vencord_auto_update?, discord_flavor?, discord_avatars?, meetings_teams?, meetings_zoom?, meetings_meet?, meetings_start?, keep_call_audio? }`. The whole patch is validated before anything changes; a failed save restores the previous values. Shortcut changes are registered only after the preferences are saved. `discord_flavor` must be empty or an installed edition id and cannot change while Discord runs with the debugger or a restart is in progress; `meetings_start` is `ask` or `automatic`. Turning `discord_avatars` off drops the picture cache and removes avatar identifiers from the next update. The debugger port is not patchable: it is chosen by `discord_relaunch` and cleared by `discord_relaunch_normally`.

Models (`actions/models.rs`): `model_download`, `model_load`, `summary_download`, `summary_download_cancel`, `cleanup_download`, `cleanup_download_cancel`, `context_download`, `speakers_download`, `verifier_download`.

Discord (`actions/discord.rs`): `discord_connect { companion: boolean }`, `discord_disconnect`, `discord_retry`, `discord_relaunch`, `discord_relaunch_normally`, `discord_rotate_key`, `discord_forget_pairing`, `companion_detect`, `companion_install`, `companion_open_installer`, `meeting_respond { accept: boolean }`.

- `discord_relaunch_normally` restarts the chosen edition without the debugger an earlier `discord_relaunch` opened; refused while recording or while another restart runs.
- `discord_rotate_key` replaces the companion pairing key, so every companion holding the old key stops working; the current companion re-reads the new key locally and reconnects. `discord_forget_pairing` removes the key and stops listening until the next `discord_connect`. Both are refused during a recording.
- `meeting_respond` answers the `meetings.prompt`; `accept: true` starts a transcript through the same path as automatic Discord capture, `false` leaves that meeting alone until it ends. It fails when no prompt is waiting.

Updates (`actions/updates.rs`): `update_check`, `update_download`, `update_install`.

Conversations and speakers (`actions/calls.rs`):

- `speaker_rename { id, speaker, name }`, `speaker_merge { id, from, into }`, `speaker_reassign { id, rows: {index, start_ms}[], speaker? | name? }`, `speaker_undo { id }`. `speaker`, `from` and `into` are `speaker_keys` from the current snapshot. Each row reference carries the `start_ms` the renderer saw; a changed transcript is rejected. They apply to the call being recorded (committed rows only; later turns follow merges and participant renames) or to the open saved conversation, and save through the normal history path.
- `person_save { id?, name }`, `person_delete { id }`, `people_clear { confirm: { count } }`: the known-people list. `count` is how many people the confirmation showed; a different count is refused. Clearing also removes damaged copies of the list.
- `audio_import_choose` opens the system file dialog outside the controller. A chosen or dropped file is offered as `audio_import.state = "ready"`; `audio_import_start { kind: "call"|"dictation", keep_audio? }` imports it on the speech worker, `audio_import_cancel` stops it and `audio_import_dismiss` clears the offer or result. The renderer never sends or receives file paths.
- `audio_play { id, from_ms? }`, `audio_stop`: native playback of kept audio. `audio_delete { id, confirm: { id, bytes } }` removes kept audio and keeps the transcript. `confirm` repeats the session and the `audio.bytes` the confirmation showed; anything else is refused.
- `transcript_rebuild { id }` re-transcribes kept audio with the loaded model and keeps the replaced transcript in `transcript_versions`; `transcript_restore { id, version, expected }` swaps a kept version back: `version` is a `transcript_versions[].id` (stable, unlike `index`) and `expected` the `transcript_revision` the renderer showed, so a repeated or stale request is refused. A re-transcription changes only the transcript of the latest copy of the session; title and notes edited meanwhile stay, and if the transcript itself changed meanwhile it is kept and the new one is added to `transcript_versions`. Installation requires capture and processing to finish, no active download, and pending document saves to complete before the verified installer launches.

Editing an existing vocabulary entry requires `index` and its original `expected_heard`; snippet edits require `index` and `expected_trigger`. Edits and deletions also require the complete original snapshot object in `expected_entry` or `expected_macro`. A stale row or changed scope is rejected without mutating data. Device preferences use an empty string to select the Windows default.

### Document locks

`source_move_working` locks open editors while a move between topics is in progress, or while a pending deletion affects an open document. A deletion affects the deleted document itself, the topic that contains it, and recordings filed in a topic that is being deleted. Edits to unrelated documents continue. `session_patch` reports which of these applies.

## Closing

1. A window close request sends `desktop-close-requested` with a numeric request id to the main window.
2. The renderer acknowledges at once with `desktop_close_ack { id }`, repeats that acknowledgement every 500 ms while saving its pending document edits, then invokes `desktop_close`. If it decides not to close (a document action is running, or saving failed) it sends `desktop_close_ack { id, cancelled: true }`.
3. Without an acknowledgement within three seconds, the native shell closes on its own: the controller saves everything it holds, then the app exits. Edits still only in an unresponsive renderer cannot be saved.
4. Another close request while the first is unanswered for over a second, or without an acknowledgement or saving heartbeat for over two seconds, escalates to the same native shutdown.
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

`notes`, `selected_notes`, `call_notes` and `note_notes` share one shape: `{ ready, acceleration_installed, working, status, progress, download_status, download_progress, downloading }`. `notes` follows the selected document, or the current call when nothing is selected; the others are scoped to the selected document, the current call and the current spoken note.

Discord snapshots distinguish `listener_started` from fresh `connected`
membership, with `in_voice`, `audio_ready`, `pairing_state`, `automatic_status`,
and `automatic_can_retry`. The `discord_retry` action clears an automatic
capture pause and retries model loading without replacing a healthy listener.
The companion discovers its authentication key locally; the desktop renderer
never receives it.

`companion_protocol` reports the local protocol version the connected companion speaks relative to this build; `outdated` asks for a companion update and `newer` for an app update, with `companion_protocol_message` holding the explanation. `debugger_open` and `debugger_notice` stay set while a relaunched Discord still exposes its debugger, until `discord_relaunch_normally` completes.

`meetings` is a separate section: `enabled` mirrors the per-app settings, `active` lists confirmed meetings, `prompt` is the meeting waiting for `meeting_respond` in ask mode, `owned` is the meeting whose transcript detection started (and will finish when the meeting ends), `status` and `probe_error` describe the detector, and `events` keeps the last sixteen confirmed starts and ends.

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
`audio_import` follows imports and re-transcription (one runs at a time on the speech worker). `playback` reports the playing session and position; only the native player touches audio. `people` lists known people, most recently used first; `people_error` explains a list that could not be read and is not overwritten.

Insights contain `words`, `sessions`, `words_per_minute`, `current_streak`, `longest_streak`, `active_days`, `dictionary_replacements`, `cleanup_edits`, `apps: [{app, words, sessions}]` and `days: [{date, words, sessions}]`.

The Tauri interface includes model installation, companion setup and application
updates and per-app writing-style editing. Manual Polish previews are not yet
exposed in the renderer. There is no legacy UI build.

## Model and review controls

`managed_models` is an array of `{id, installed, disk_size, path, in_use}`. IDs are `speech`, `verifier`, `speaker`, `sound_cues`, `polish`, `notes`. `path` contains only the file name, never a managed directory. `model_delete {id}` rejects leased models. `model_download_cancel {id}` cancels speech, verifier, speaker or sound-cue downloads. Notes and cleanup retain their dedicated cancellation actions.

`desktop_model_file()` opens a native GGUF file dialog outside the controller, returning a path or null. The renderer applies it with the existing validated `settings_patch {model_path}` action.

`settings_patch` additionally accepts `automatic_update_checks`, `close_to_tray`, `start_with_windows`. These booleans are included in `settings`; defaults are true, false, false. Explicit `update_check` remains independent of automatic checks.

`cleanup` is `{installed: boolean, working: boolean, status: string, progress: number|null, preview: null|{original: string, source: string, text: string, blocked: string|null, cleanup_only: boolean}}`. `polish_start`, `polish_apply` and `polish_cancel` use the existing current-utterance checks. Blocked edits cannot apply unless the preview is cleanup-only.

`assort` is `{working: boolean, status: string, candidates: null|Candidate[], scores: null|Score[]}`. A candidate contains `entry`, `proposed` and `context`; a score contains `candidate` (index) and `replace_score` (uncalibrated preference). `assort_start {app?: string}`, `assort_apply {index, saved_spelling}` and `assort_cancel` never apply automatically. Applying a choice ends that review; review again for another choice against the updated text.

The four notes status values are typed as `{ready, acceleration_installed, working, status, progress, download_status, download_progress, downloading}`. Existing field names and nullability are preserved; `download_progress` adds a nullable download fraction.

## Dictation input additions

`settings_patch` accepts `spoken_editing_commands: boolean`, `styles: {app, style}[]` and `input_profiles: {app, hotkey_mode?, insert?, live_insert?, line_break?, style?, spoken_editing_commands?}[]`. Nullable profile fields inherit defaults. Enums: Hold/Toggle, Enter/ShiftEnter, Verbatim/Clean/Chat. Lists are capped at 100; invalid basenames and duplicate profiles reject the entire patch.

Settings exposes those fields plus `recent_dictation_app`, `shortcut_error` and `microphone_check: {running, result, error}`. Result is null or `{device, peak, rms, clipping_ratio, silent}`. `microphone_check` starts a three-second worker and rejects overlapping tests or active recording. The reusable `MicrophoneCheck` component accepts `check`, `onTest` and `disabled` for call setup integration.

`dictation_interruption` is null or the reason capture ended early, retained until the next capture. `insert_text {text, line_breaks?}` runs outside the controller, waits three seconds for external focus, then arms and inserts through field identity guards. It accepts up to 16,000 bytes and an optional executable-to-line-break map (up to 100 entries), used to select the focused app profile. Browser preview simulates microphone results; it never inserts into external apps.

`desktop_revision` returns a map of section names to revisions. The fallback check compares every section, so receiving a newer unrelated event cannot hide a missed update. Unresponsive controller health is published in the cached `controller` section and recovered by that check.

Invalid title drafts stay local to the title input and revert on blur. Flush reports and discards client-invalid patches once; transport failures retain patches for retry. A successful action remains successful if its follow-up synchronization fails.
