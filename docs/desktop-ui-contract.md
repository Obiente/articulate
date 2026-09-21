# Desktop UI bridge

The `articulate` binary is a Tauri shell that runs the application controller on one Rust-owned thread. Tauri is the sole interface and requires no feature flag. The renderer never receives audio buffers, native handles or the Discord pairing key.

Invoke `desktop_snapshot` without arguments every 500 ms. It returns this shape (snake_case):

```ts
type Snapshot = {
  ready: boolean; loading: boolean; busy: boolean; status: string;
  recording: boolean; call_recording: boolean; call_status: string;
  seconds: number; text: string; original: string; call_rows: Row[];
  history: Summary[]; selected: SessionView | null; call_session: SessionView | null;
  notes: { ready: boolean; working: boolean; status: string; progress: number | null; downloading: boolean; download_status: string }; downloading: number | null;
  history_loading: boolean; history_error: string | null; saving: boolean;
  dictionary: Entry[]; macros: VoiceMacro[];
  settings: {
    clean_speech: boolean; insert: boolean; live_insert: boolean;
    learn_corrections: boolean; audio_feedback: boolean;
    microphone: string | null; output: string | null;
    hotkey: { ctrl: boolean; alt: boolean; shift: boolean; win: boolean; key: string };
    hotkey_mode: 'Hold' | 'Toggle'; discord_auto_connect: boolean;
    discord_auto_transcribe: boolean; discord_companion: boolean;
  };
  microphones: string[]; outputs: string[]; insights: object | null;
};
type Summary = { id: string; title: string; kind: 'call'|'dictation'|'note'; created_ms: number; updated_ms: number; preview: string; duration_ms: number };
type SessionView = { id: string; title: string; kind: string; text: string; original: string; personal_notes: string; rows: Row[]; created_ms: number; updated_ms: number };
type Row = { start_ms: number; end_ms: number; speaker: string; text: string; provisional: boolean };
```

Invoke `desktop_action` with `{ action: { type: "...", ... } }`. The promise resolves after the controller accepts the action, or rejects with a readable error. Supported actions:

- `history_refresh`, `history_open { id }`, `history_close`, `note_new`
- `session_patch { id, title?: string, personal_notes?: string }`: patches only the identified open/current session; never replaces transcript fields.
- `session_delete { id }`, `history_retry`
- `dictation_start`, `dictation_stop`, `dictation_cancel`
- `call_start`, `call_stop`, `note_capture_start`, `note_capture_stop`
- `source_move { id, topic_id: string|null, expected_topic_id: string|null }`, `source_file { id }`: move one independent source or retry automatic topic filing.
- `copy { text }`, `export { text, extension }` with extension `txt`, `md`, `srt` or `vtt`
- `dictionary_save { heard, wanted, app?: string, cues?: string, index?: number, expected_heard?: string, expected_entry?: Entry }`, `dictionary_delete { index, expected_heard, expected_entry }`
- `macro_save { trigger, expansion, app?: string, index?: number, expected_trigger?: string, expected_macro?: VoiceMacro }`, `macro_delete { index, expected_trigger, expected_macro }`
- `settings_patch { cpu?, model_path?, clean_speech?, insert?, live_insert?, learn_corrections?, audio_feedback?, microphone?, output?, hotkey_mode?, hotkey?, quick_note_hotkey?, discord_auto_connect?, discord_auto_transcribe?, vencord_auto_update? }`
- `insights_refresh`, `model_download`, `model_load`, `summary_download`, `notes_generate { id }`
- `summary_download_cancel`, `cleanup_download`, `cleanup_download_cancel`, `speakers_download`
- `discord_connect { companion: boolean }`, `discord_disconnect`, `discord_relaunch`
- `companion_detect`, `companion_install`, `companion_open_installer`
- `update_check`, `update_download`, `update_install`: installation requires capture to finish and pending document saves to complete before the verified installer launches.

Closing the window requests native shutdown. Capture must finish first; pending local saves are drained, and save errors keep the window open. UI polling is only presentation: Rust continues capture, transcription, automatic notes, hotkey handling and persistence independently.

`capture_feedback` carries a monotonic `sequence`, `event` (`started`, `stopped`,
or `failed`), `kind` (`call` or `note`), optional `session_id`, and `status`.
It begins at sequence zero with no event. Frontends baseline their first
snapshot and deduplicate subsequent notifications. Native sounds and the
nonactivating overlay follow capture acknowledgements and stop boundaries,
independently of renderer polling. They never imply that final transcription
or persistence has completed.

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

The renderer listens for `desktop-close-requested`, flushes its pending document edits, then invokes `desktop_close`. It must display a rejected close request so recording or unsaved work remains recoverable. The native window stays open until this handshake completes.

`selected_notes` and `call_notes` use the same status shape as `notes`, scoped to the selected document and current call respectively. Use the matching field in each reader; `notes` remains available for shared model setup status.

The native dictation indicator sets `window.__ARTICULATE_OVERLAY__ = true` before loading the frontend. This window invokes `desktop_overlay`, which returns `{ visible, recording, finishing, message }`, and cannot invoke state-changing commands. Native code positions, shows and hides this nonfocusable, mouse-passthrough window.

The Tauri interface includes model installation, companion setup and application
updates. Per-app writing-style editing and manual Polish previews are not yet
exposed in the renderer. There is no legacy UI build.

Spoken notes expose `note_recording`, `note_session`, `note_rows`, `note_status`
and per-document `note_notes`. They do not set `call_recording`. Topic sessions
include `is_collection`, `topic_id`, `can_receive_sources` and a `sources` list;
source text remains in independently saved sessions. `source_move_working`
temporarily locks editors during membership changes or source deletion.
`filing_working` and `filing_status` describe automatic organization. Pending
filing is persisted for retry after restart.

Editing an existing vocabulary entry requires `index` and its original `expected_heard`; snippet edits require `index` and `expected_trigger`. Device preferences use an empty string to select the Windows default. Insights contain `words`, `sessions`, `words_per_minute`, `current_streak`, `longest_streak`, `active_days`, `dictionary_replacements`, `cleanup_edits`, `apps: [{app, words, sessions}]` and `days: [{date, words, sessions}]`.

Edits and deletions require the complete original snapshot object in `expected_entry` or `expected_macro`, in addition to the original phrase/trigger. A stale row or changed scope is rejected without mutating data.
