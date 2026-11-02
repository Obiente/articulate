// Every desktop action type, shared by the app shell and the preview
// workspace. A Rust test (src/app/desktop/tests.rs) keeps this list in sync
// with the controller's feature actions.
//
// document: flush pending document edits first and lock document controls
//           until the action finishes.
// preview:  the browser preview workspace implements the action. Other
//           actions need the desktop app.
export const actionCatalog = Object.freeze({
  // Library and documents
  history_refresh: { preview: true },
  history_open: { document: true, preview: true },
  history_close: { document: true, preview: true },
  history_retry: { preview: true },
  note_new: { document: true, preview: true },
  source_move: { document: true, preview: true },
  source_file: { document: true },
  session_patch: { preview: true },
  session_delete: { document: true, preview: true },
  dictation_edit: { document: true, preview: true },
  notes_generate: { document: true },
  insights_refresh: { preview: true },
  history_search: { preview: true },
  session_pin: { preview: true },
  session_export: { document: true, preview: true },
  followups_export: { document: true, preview: true },
  // Privacy and storage
  storage_encryption: { preview: true },
  retention_preview: { preview: true },
  retention_save: { preview: true },
  quarantine_list: { preview: true },
  quarantine_restore: { preview: true },
  quarantine_export: { preview: true },
  quarantine_discard: { preview: true },
  backup_create: { preview: true },
  backup_restore: { preview: true },
  history_export_markdown: { preview: true },
  // Capture
  dictation_start: { document: true },
  dictation_stop: {},
  dictation_cancel: {},
  call_start: { document: true },
  call_stop: { document: true },
  note_capture_start: { document: true },
  note_capture_stop: { document: true },
  // Copy and export
  copy: { preview: true },
  export: { preview: true },
  // Vocabulary and snippets
  dictionary_save: { preview: true },
  dictionary_delete: { preview: true },
  macro_save: { preview: true },
  macro_delete: { preview: true },
  // Settings
  settings_patch: { preview: true },
  // Models
  model_download: {},
  model_load: {},
  summary_download: {},
  summary_download_cancel: {},
  cleanup_download: {},
  cleanup_download_cancel: {},
  context_download: {},
  speakers_download: {},
  verifier_download: {},
  // Discord
  discord_connect: {},
  discord_disconnect: {},
  discord_retry: {},
  discord_relaunch: {},
  companion_detect: {},
  companion_install: {},
  companion_open_installer: {},
  // Updates
  update_check: {},
  update_download: {},
  update_install: { document: true },
});

export const changesDocument = (type) => Boolean(actionCatalog[type]?.document);

export const previewSupports = (type) => Boolean(actionCatalog[type]?.preview);

const utf8Length = (text) => new TextEncoder().encode(text).length;
const MiB = 1024 * 1024;

export function validTitle(title) {
  const trimmed = title.trim();
  return (
    Boolean(trimmed) &&
    [...trimmed].length <= 160 &&
    !/[\u0000-\u001f\u007f-\u009f]/u.test(trimmed)
  );
}

// The value a saved document reports after a session patch.
export const savedPatchValue = (key, value) =>
  key === "title" ? value.trim() : value;

export function sourceMoveAction(id, topicId, expectedTopicId) {
  if (typeof id !== "string" || !id.trim())
    throw Error("Choose the dictation you want to move.");
  for (const value of [topicId, expectedTopicId]) {
    if (value !== null && (typeof value !== "string" || !value.trim()))
      throw Error("Refresh this dictation before moving it.");
  }
  if (id === topicId) throw Error("A dictation cannot be its own topic.");
  return {
    type: "source_move",
    id,
    topic_id: topicId,
    expected_topic_id: expectedTopicId,
  };
}

// Checks shared by the desktop and preview transports. Messages match the
// native controller's.
const validators = {
  session_patch(action) {
    if (action.title !== undefined && !validTitle(action.title))
      throw Error("Use a title from 1 to 160 characters.");
  },
  dictation_edit(action) {
    const text = action.text ?? "";
    if (!text.trim() || utf8Length(text) > 8 * MiB || text.includes("\0"))
      throw Error("Enter some text, up to 8 MB.");
  },
  source_move(action) {
    sourceMoveAction(action.id, action.topic_id, action.expected_topic_id);
  },
  copy(action) {
    if (utf8Length(action.text ?? "") > 8 * MiB)
      throw Error("Text is too long.");
  },
  export(action) {
    if (
      utf8Length(action.text ?? "") > 16 * MiB ||
      !["txt", "md", "srt", "vtt"].includes(action.extension)
    )
      throw Error("Unsupported export.");
  },
  dictionary_save(action) {
    if (!action.heard?.trim() || !action.wanted?.trim())
      throw Error("Fill in both words or phrases");
  },
};

// Validate an action before it reaches either transport.
export function prepareAction(action) {
  if (!action || !Object.hasOwn(actionCatalog, action.type))
    throw Error("This action is not available.");
  validators[action.type]?.(action);
  return action;
}
