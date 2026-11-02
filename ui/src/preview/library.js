// Preview versions of library search, pins, exports and storage actions.
// Everything stays in the browser; "files" are downloads.

const encoder = new TextEncoder();
const bytes = (text) => encoder.encode(text).length;
const DAY_MS = 86400000;

export const emptySearch = () => ({
  query: "",
  filters: { from_ms: null, to_ms: null, kind: null, speaker: null, app: null },
  working: false,
  error: null,
  results: null,
});

export const emptyStorage = (saved) => ({
  encrypt_at_rest: Boolean(saved?.encrypt_at_rest),
  encryption_working: false,
  encryption_result: null,
  retention: saved?.retention ?? {
    enabled: false,
    days: 90,
    dictations: true,
    calls: true,
    notes: true,
  },
  retention_preview_working: false,
  retention_preview: null,
  backup: "idle",
  backup_result: null,
  restore: "idle",
  restore_result: null,
  markdown: "idle",
  markdown_result: null,
  dialog_open: false,
});

export function download(text, name, type = "text/plain;charset=utf-8") {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

// Action items still present as paragraphs of the notes.
export function followUps(record) {
  const blocks = (record?.personal_notes || "")
    .split("\n\n")
    .map((block) => block.trim());
  return (record?.follow_ups || []).filter((item) =>
    blocks.includes(item.trim()),
  );
}

export function subtitlesUnavailable(record) {
  const rows = record?.rows || [];
  if (!rows.length) {
    if (!record?.text?.trim())
      return "There is no transcript to turn into subtitles.";
    return record.duration_ms > 0 ? null : "This item has no recorded timing.";
  }
  return rows.some((row) => row.end_ms > row.start_ms && row.text?.trim())
    ? null
    : "This transcript has no timed sections.";
}

// Every occurrence of `needle` in `text`, as UTF-8 byte ranges of `excerpt`.
function excerptAround(text, needle) {
  const lower = text.toLowerCase();
  const first = lower.indexOf(needle);
  if (first < 0) return null;
  let start = Math.max(0, first - 60);
  let end = Math.min(text.length, first + needle.length + 100);
  if (start > 0) start = text.indexOf(" ", start) + 1 || start;
  if (end < text.length) end = text.lastIndexOf(" ", end) || end;
  const excerpt = text.slice(start, end);
  const lowered = excerpt.toLowerCase();
  const matches = [];
  for (let at = lowered.indexOf(needle); at >= 0; ) {
    const from = bytes(excerpt.slice(0, at));
    matches.push([from, from + bytes(excerpt.slice(at, at + needle.length))]);
    at = lowered.indexOf(needle, at + needle.length);
  }
  return { excerpt, matches };
}

export function searchRecords(records, query, filters = {}) {
  const needle = query.replace(/^"(.*)"$/, "$1").trim().toLowerCase();
  const hits = [];
  for (const record of [...records].sort(
    (a, b) => b.updated_ms - a.updated_ms,
  )) {
    if (filters.kind && record.kind !== filters.kind) continue;
    if (filters.from_ms != null && record.created_ms < filters.from_ms)
      continue;
    if (filters.to_ms != null && record.created_ms > filters.to_ms) continue;
    if (filters.app && (record.app || "").toLowerCase() !== filters.app)
      continue;
    const speaker = filters.speaker?.toLowerCase();
    const speakers = (record.rows || []).map((row) => row.speaker || "");
    if (speaker && !speakers.some((s) => s.toLowerCase().includes(speaker)))
      continue;
    const base = {
      session_id: record.id,
      title: record.title,
      kind: record.kind,
      created_ms: record.created_ms,
      speaker: null,
      start_ms: null,
      end_ms: null,
    };
    const fields = [
      ["title", record.title],
      ["personal_notes", record.personal_notes],
      ...(record.rows?.length ? [] : [["text", record.text]]),
    ];
    for (const [field, text] of fields) {
      const found = text && excerptAround(text, needle);
      if (found) hits.push({ ...base, field, ...found });
    }
    (record.rows || []).forEach((row) => {
      if (speaker && !row.speaker?.toLowerCase().includes(speaker)) return;
      const found = excerptAround(row.text || "", needle);
      if (found)
        hits.push({
          ...base,
          field: "transcript",
          speaker: row.speaker,
          start_ms: row.start_ms,
          end_ms: row.end_ms,
          ...found,
        });
    });
  }
  return hits;
}

function sessionText(record, format) {
  const rows = record.rows || [];
  const stamp = (ms, comma) => {
    const date = new Date(ms).toISOString().slice(11, 23);
    return comma ? date.replace(".", ",") : date;
  };
  if (format === "srt" || format === "vtt") {
    const cues = rows.length
      ? rows
      : [{ start_ms: 0, end_ms: record.duration_ms, text: record.text }];
    const body = cues
      .map(
        (row, index) =>
          `${format === "srt" ? `${index + 1}\n` : ""}${stamp(row.start_ms, format === "srt")} --> ${stamp(row.end_ms, format === "srt")}\n${row.speaker ? `${row.speaker}: ` : ""}${row.text}\n`,
      )
      .join("\n");
    return format === "vtt" ? `WEBVTT\n\n${body}` : body;
  }
  const heading = format === "markdown" ? "# " : "";
  const section = format === "markdown" ? "## " : "";
  const parts = [`${heading}${record.title}`];
  if (record.personal_notes?.trim())
    parts.push(`${section}My notes\n\n${record.personal_notes.trim()}`);
  if (rows.length)
    parts.push(
      `${section}Transcript\n\n${rows.map((row) => `${row.speaker}: ${row.text}`).join("\n\n")}`,
    );
  else if (record.text?.trim()) parts.push(`${section}Text\n\n${record.text}`);
  return `${parts.join("\n\n")}\n`;
}

const formatLabels = {
  text: ["text", "txt"],
  markdown: ["Markdown", "md"],
  srt: ["SRT subtitles", "srt"],
  vtt: ["WebVTT subtitles", "vtt"],
};

const icsStamp = (ms) =>
  new Date(ms).toISOString().replace(/[-:]/g, "").replace(/\.\d+/, "");

// Handlers for the preview workspace. `context` gives access to its state.
export function libraryHandlers(context) {
  const { record, records, notify, later } = context;
  const policyItems = (policy) => {
    const cutoff = Date.now() - policy.days * DAY_MS;
    const kinds = {
      dictation: policy.dictations,
      call: policy.calls,
      note: policy.notes,
    };
    return records()
      .filter(
        (item) =>
          kinds[item.kind] &&
          !item.pinned &&
          !item.is_collection &&
          !item.topic_id &&
          item.id !== context.selected() &&
          item.updated_ms < cutoff,
      )
      .map(({ id, title, kind, updated_ms }) => ({
        id,
        title,
        kind,
        updated_ms,
      }));
  };
  const validPolicy = (policy) => {
    if (!Number.isInteger(policy?.days) || policy.days < 1 || policy.days > 36500)
      throw Error("Choose a retention period between 1 and 36500 days.");
  };
  return {
    history_search(action) {
      const query = (action.query || "").trim();
      const filters = { ...emptySearch().filters, ...action.filters };
      context.sample.search = { ...emptySearch(), query, filters };
      if (!query) return;
      if ([...query].length > 160) {
        context.sample.search.error =
          "Search for a word or phrase of up to 160 characters.";
        return;
      }
      const hits = searchRecords(records(), query, filters);
      context.sample.search.results = {
        query,
        hits: hits.slice(0, 100),
        more: hits.length > 100,
        indexed: true,
        partial: false,
        scanned_sessions: records().length,
        unreadable_sessions: context.sample.recovery.quarantined,
      };
    },
    session_pin(action) {
      const item = record(action.id);
      if (!item) throw Error("This item is no longer available.");
      item.pinned = Boolean(action.pinned);
    },
    session_export(action) {
      const item = record(action.id);
      if (!item) throw Error("Open the document before exporting it.");
      const [label, extension] = formatLabels[action.format] || [];
      if (!label) throw Error("Unsupported export.");
      const blocked =
        (action.format === "srt" || action.format === "vtt") &&
        subtitlesUnavailable(item);
      if (blocked) throw Error(blocked);
      download(sessionText(item, action.format), `${item.title}.${extension}`);
      notify(`Exported as ${label}`);
    },
    followups_export(action) {
      const item = record(action.id);
      const items = followUps(item);
      if (!items.length)
        throw Error(
          "There are no follow-ups in this document's notes. Update notes, then keep the action items you want to export.",
        );
      if (action.reminder_ms != null && action.reminder_ms <= Date.now())
        throw Error("Choose a reminder time in the future.");
      if (action.format === "ics") {
        const now = icsStamp(Date.now());
        const due =
          action.reminder_ms != null ? icsStamp(action.reminder_ms) : null;
        const todos = items.map(
          (text, index) =>
            `BEGIN:VTODO\r\nUID:${item.id}-${index}@articulate\r\nDTSTAMP:${now}\r\nSUMMARY:${text.replace(/([,;\\])/g, "\\$1")}\r\nSTATUS:NEEDS-ACTION\r\n${due ? `DUE:${due}\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nDESCRIPTION:Meeting follow-up\r\nTRIGGER;VALUE=DATE-TIME:${due}\r\nEND:VALARM\r\n` : ""}END:VTODO\r\n`,
        );
        download(
          `BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Articulate//Follow-ups//EN\r\n${todos.join("")}END:VCALENDAR\r\n`,
          `${item.title} follow-ups.ics`,
          "text/calendar",
        );
      } else
        download(
          items.map((text) => `- [ ] ${text}\n`).join(""),
          `${item.title} follow-ups.md`,
        );
      notify(
        `Exported ${items.length} ${items.length === 1 ? "follow-up" : "follow-ups"}`,
      );
    },
    storage_encryption(action) {
      if (context.sample.storage.encryption_working)
        throw Error("Wait for the current encryption change to finish.");
      context.sample.storage.encrypt_at_rest = Boolean(action.enabled);
      context.sample.storage.encryption_working = true;
      context.sample.storage.encryption_result = null;
      later(1400, () => {
        context.sample.storage.encryption_working = false;
        context.sample.storage.encryption_result = {
          error: null,
          protect: context.sample.storage.encrypt_at_rest,
          changed: records().length,
          failed: 0,
        };
      });
    },
    retention_preview(action) {
      validPolicy(action.policy);
      const items = policyItems(action.policy);
      context.sample.storage.retention_preview = {
        policy: { ...action.policy },
        error: null,
        total: items.length,
        items: items.slice(0, 100),
      };
    },
    retention_save(action) {
      const policy = action.policy;
      validPolicy(policy);
      const preview = context.sample.storage.retention_preview;
      const same = (a, b) =>
        ["enabled", "days", "dictations", "calls", "notes"].every(
          (key) => a?.[key] === b?.[key],
        );
      if (policy.enabled && !same(preview?.policy, policy))
        throw Error("Review what this policy removes before turning it on.");
      const removed = policy.enabled ? preview.items.map((item) => item.id) : [];
      context.remove(removed);
      context.sample.storage.retention = { ...policy };
      context.sample.storage.retention_preview = null;
      if (removed.length)
        notify(
          `Removed ${removed.length} ${removed.length === 1 ? "item" : "items"} older than your retention period`,
        );
    },
    quarantine_list() {
      context.sample.recovery.listed = true;
      context.sample.recovery.quarantine = context.isolated().map(
        ({ record: _, ...file }) => file,
      );
    },
    quarantine_restore(action) {
      const file = context
        .isolated()
        .find((item) => item.file_name === action.file_name);
      if (!file) throw Error("This file is no longer isolated. Refresh the list.");
      if (!file.restorable)
        throw Error("This file doesn't hold a complete item.");
      context.restore(file);
      notify(`Restored “${file.title}”`);
    },
    quarantine_export(action) {
      const file = context
        .isolated()
        .find((item) => item.file_name === action.file_name);
      if (!file) throw Error("This file is no longer isolated. Refresh the list.");
      download(
        JSON.stringify(file.record ?? { id: file.id }, null, 2),
        `${file.file_name}.json`,
        "application/json",
      );
      notify(`Exported ${file.file_name}`);
    },
    quarantine_discard(action) {
      context.discard(action.file_name);
      notify("Discarded the damaged file");
    },
    backup_create(action) {
      if (action.passphrase && [...action.passphrase].length < 8)
        throw Error("Use a passphrase of at least 8 characters.");
      context.sample.storage.backup = "working";
      context.sample.storage.backup_result = null;
      later(900, () => {
        context.sample.storage.backup = "idle";
        context.sample.storage.backup_result = {
          error: null,
          path: "Documents\\Articulate backup.articulate-backup",
          sessions: records().length,
          encrypted: Boolean(action.passphrase),
          skipped: 0,
        };
      });
    },
    backup_restore(action) {
      context.sample.storage.restore = "working";
      context.sample.storage.restore_result = null;
      later(1100, () => {
        context.sample.storage.restore = "idle";
        context.sample.storage.restore_result = {
          error: null,
          sessions_in_archive: records().length,
          added: 0,
          updated: action.replace_older ? 1 : 0,
          kept_local: records().length - (action.replace_older ? 1 : 0),
          newer_in_backup: action.replace_older ? 0 : 1,
          deleted_locally: 0,
          failed: 0,
          settings_restored: Boolean(action.include_settings),
          shortcuts_changed: false,
          model_reload: Boolean(action.include_settings),
        };
      });
    },
    history_export_markdown() {
      context.sample.storage.markdown = "working";
      context.sample.storage.markdown_result = null;
      later(800, () => {
        context.sample.storage.markdown = "idle";
        context.sample.storage.markdown_result = {
          error: null,
          folder: "Documents\\Articulate",
          files: records().length,
          skipped: 0,
        };
      });
    },
  };
}
