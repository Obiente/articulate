import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export async function onCloseRequested(handler) {
  if (!native) return () => {};
  return listen("desktop-close-requested", handler);
}
export async function closeDesktop() {
  if (native) await invoke("desktop_close");
}
export async function overlaySnapshot() {
  return invoke("desktop_overlay");
}

export const native = isTauri();
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
const key = "articulate-ui-sample-workspace-v1";
const now = Date.now();
const rows = [
  {
    start_ms: 12000,
    end_ms: 32000,
    speaker: "Casey",
    text: "The new onboarding feels much clearer. I think we should keep the first step focused on getting a good microphone level.",
    provisional: false,
  },
  {
    start_ms: 33000,
    end_ms: 55000,
    speaker: "Jordan",
    cues: [
      { start_ms: 33000, end_ms: 38000, label: "Laughing" },
    ],
    text: "Agreed. Once someone finishes their first recording, we can show them where their transcript is saved.",
    provisional: false,
  },
  {
    start_ms: 56000,
    end_ms: 84000,
    speaker: "You",
    text: "I will update the checklist and share it before tomorrow. Let us keep the download step short and explain what happens next.",
    provisional: false,
  },
  {
    start_ms: 87000,
    end_ms: 104000,
    speaker: "Casey",
    text: "Sounds good. We will try the revised flow with the team on Friday and use their feedback to decide what to change.",
    provisional: false,
  },
];
const notes =
  "A simpler first recording\n\nKeep onboarding focused on choosing a microphone and making a first recording. Show where the transcript is saved immediately afterward.\n\nNext steps\n\nUpdate and share the onboarding checklist tomorrow. Keep the model download explanation short and clear.\n\nTry the revised flow with the team on Friday, then review the feedback together.";
const fixtures = [
  {
    id: "sample-topic-release",
    title: "Project release",
    kind: "note",
    is_collection: true,
    can_receive_sources: true,
    topic_id: null,
    created_ms: now - 600000,
    updated_ms: now - 600000,
    duration_ms: 0,
    text: "",
    original: "",
    personal_notes:
      "Keep the release focused on a clear first experience.\n\nPersonal reminder: leave time for accessibility testing.",
    rows: [],
  },
  {
    id: "sample-source-checklist",
    title: "Release checklist",
    kind: "note",
    topic_id: "sample-topic-release",
    created_ms: now - 800000,
    updated_ms: now - 800000,
    duration_ms: 18000,
    text: "For the project release, Casey will review the installation checklist on Thursday. Leave time for testing before we publish.",
    original: "",
    personal_notes: "",
    rows: [],
  },
  {
    id: "sample-source-help",
    title: "Help for new users",
    kind: "dictation",
    topic_id: "sample-topic-release",
    created_ms: now - 700000,
    updated_ms: now - 700000,
    duration_ms: 12000,
    text: "One more thought for the release: put the microphone setup and keyboard shortcut on the same page so new users can get started quickly.",
    original: "",
    personal_notes: "",
    rows: [],
  },
  {
    id: "sample-call-1",
    title: "A clearer first recording",
    kind: "call",
    created_ms: now - 3600000,
    updated_ms: now - 3600000,
    duration_ms: 384000,
    text: rows.map((r) => r.text).join("\n\n"),
    original: "",
    personal_notes: notes,
    rows,
  },
  {
    id: "sample-call-2",
    title: "Planning the next release",
    kind: "call",
    created_ms: now - 86400000,
    updated_ms: now - 86400000,
    duration_ms: 1680000,
    text: "The team agreed to finish the setup flow before the next release. Casey will test the Windows installer and Jordan will review the documentation.",
    original: "",
    personal_notes:
      "Next release\n\nFinish the setup flow before release. Casey will test the Windows installer. Jordan will review the documentation.",
    rows: [],
  },
  {
    id: "sample-note-1",
    title: "Ideas worth coming back to",
    kind: "note",
    created_ms: now - 90000000,
    updated_ms: now - 90000000,
    duration_ms: 0,
    text: "",
    original: "",
    personal_notes:
      "Make the small moments feel effortless.\n\nA clear recording cue. A place for every conversation. Notes that are easy to come back to.",
    rows: [],
  },
  {
    id: "sample-dictation-1",
    title: "A quick follow-up",
    kind: "dictation",
    created_ms: now - 7200000,
    updated_ms: now - 7200000,
    duration_ms: 19000,
    text: "Thanks for the conversation earlier. I will send the updated plan tomorrow morning so we can review it before Friday.",
    original: "",
    personal_notes: "",
    rows: [],
  },
];
let saved;
try {
  saved = JSON.parse(localStorage.getItem(key));
} catch {
  /* A fresh sample workspace is safe. */
}
let records = Array.isArray(saved?.records) ? saved.records : fixtures;
// Add new authored examples to an existing preview without replacing edits.
if (Array.isArray(saved?.records) && !saved.topic_examples_added) {
  records = [
    ...records,
    ...fixtures
      .filter(
        (item) =>
          item.id.startsWith("sample-topic-") ||
          item.id.startsWith("sample-source-"),
      )
      .filter((item) => !records.some((existing) => existing.id === item.id)),
  ];
}
let selected = null;
let sample = {
  preview: true,
  ready: true,
  loading: false,
  busy: false,
  recording: false,
  call_recording: false,
  note_recording: false,
  note_session: null,
  note_notes: null,
  note_status: "Ready for your thoughts",
  note_seconds: 0,
  status: "Ready when you are",
  call_status: "Ready to capture a conversation",
  seconds: 0,
  text: "",
  original: "",
  call_rows: [],
  call_session: null,
  history_loading: false,
  history_error: null,
  saving: false,
  dictionary: saved?.dictionary ?? [
    {
      heard: "articulate",
      wanted: "Articulate",
      app: null,
      cues: [],
      enabled: true,
    },
    {
      heard: "q wen",
      wanted: "Qwen",
      app: null,
      cues: ["model"],
      enabled: true,
    },
  ],
  macros: saved?.macros ?? [
    {
      trigger: "signature",
      expansion: "Thanks,\nCasey",
      app: null,
      enabled: true,
    },
  ],
  settings: {
    clean_speech: true,
    insert: true,
    live_insert: true,
    learn_corrections: true,
    audio_feedback: true,
    microphone: null,
    output: null,
    hotkey: { ctrl: true, alt: true, shift: false, win: false, key: "Space" },
    hotkey_mode: "Hold",
    quick_note_hotkey: {
      ctrl: true,
      alt: true,
      shift: false,
      win: false,
      key: "N",
    },
    discord_auto_connect: true,
    discord_auto_transcribe: false,
    discord_companion: true,
    vencord_auto_update: false,
    cpu: false,
    ...saved?.settings,
  },
  microphones: ["Default microphone"],
  outputs: ["Default speakers"],
  insights: null,
  notes: {
    ready: false,
    working: false,
    status: "",
    progress: null,
    downloading: false,
    download_status: "",
  },
};
function persist() {
  localStorage.setItem(
    key,
    JSON.stringify({
      records,
      dictionary: sample.dictionary,
      macros: sample.macros,
      settings: sample.settings,
      topic_examples_added: true,
    }),
  );
}
function sameRecord(left, right) {
  if (left === right) return true;
  if (!left || !right || typeof left !== "object" || typeof right !== "object")
    return false;
  const keys = Object.keys(left);
  return (
    keys.length === Object.keys(right).length &&
    keys.every(
      (key) => Object.hasOwn(right, key) && sameRecord(left[key], right[key]),
    )
  );
}
function summary(session) {
  return {
    ...session,
    can_receive_sources:
      session.is_collection ||
      (session.kind === "note" &&
        !session.topic_id &&
        !session.text?.trim() &&
        !session.rows?.length),
    preview: (session.personal_notes || session.text)
      .replaceAll("\n", " ")
      .slice(0, 160),
  };
}
function previewSession(session) {
  if (!session?.is_collection) return session;
  const sources = records.filter((source) => source.topic_id === session.id);
  return {
    ...session,
    text: sources.map((source) => source.text).join("\n\n"),
    sources: sources.map(summary),
  };
}
export async function snapshot() {
  if (native) return invoke("desktop_snapshot");
  // A renderer snapshot must not retain writable references to saved documents.
  return structuredClone({
    ...sample,
    history: records
      .map((item) => summary(previewSession(item)))
      .sort((a, b) => b.created_ms - a.created_ms),
    selected: previewSession(records.find((s) => s.id === selected)) ?? null,
  });
}
export async function action(action) {
  if (native) return invoke("desktop_action", { action });
  const before = structuredClone({ records, sample, selected });
  try {
    switch (action.type) {
      case "history_open":
        if (!records.some((s) => s.id === action.id))
          throw Error("This note is no longer available.");
        selected = action.id;
        break;
      case "history_close":
        selected = null;
        break;
      case "history_refresh":
      case "history_retry":
        break;
      case "source_move": {
        sourceMoveAction(action.id, action.topic_id, action.expected_topic_id);
        const source = records.find((item) => item.id === action.id);
        if (!source || source.is_collection || !source.text?.trim())
          throw Error("Choose an original dictation to move.");
        if ((source.topic_id ?? null) !== action.expected_topic_id)
          throw Error(
            "This dictation was already moved. Reopen it and try again.",
          );
        const target =
          action.topic_id === null
            ? null
            : records.find((item) => item.id === action.topic_id);
        if (
          action.topic_id !== null &&
          (!target || !summary(target).can_receive_sources)
        )
          throw Error("Choose an existing topic or personal note.");
        if (target) target.is_collection = true;
        source.topic_id = action.topic_id;
        source.updated_ms = Date.now();
        sample.filing_status = target
          ? `Filed in ${target.title}.`
          : "This dictation is now kept separately.";
        break;
      }
      case "note_new": {
        const id = crypto.randomUUID();
        selected = id;
        records = [
          {
            id,
            title: "Untitled note",
            kind: "note",
            created_ms: Date.now(),
            updated_ms: Date.now(),
            duration_ms: 0,
            text: "",
            original: "",
            personal_notes: "",
            rows: [],
          },
          ...records,
        ];
        break;
      }
      case "session_patch": {
        const record = records.find((s) => s.id === action.id);
        if (!record) throw Error("This note is no longer available.");
        if (action.title !== undefined) {
          const title = action.title.trim();
          if (
            !title ||
            [...title].length > 160 ||
            /[\u0000-\u001f\u007f-\u009f]/u.test(title)
          )
            throw Error("Use a title from 1 to 160 characters.");
          record.title = title;
        }
        if (action.personal_notes !== undefined)
          record.personal_notes = action.personal_notes;
        record.updated_ms = Date.now();
        break;
      }
      case "dictation_edit": {
        const record = records.find((s) => s.id === action.id);
        if (!record || record.kind !== "dictation" || record.rows?.length)
          throw Error("Choose a finished dictation to edit.");
        if (record.text !== action.expected_text)
          throw Error(
            "This dictation changed. Reopen the editor to use its latest text.",
          );
        if (!action.text.trim() || action.text.length > 8 * 1024 * 1024)
          throw Error("Enter some text, up to 8 MB.");
        if (action.correction) {
          const { heard, wanted, cues = "" } = action.correction;
          if (
            !heard.trim() ||
            !wanted.trim() ||
            heard === wanted ||
            !record.text.includes(heard) ||
            !action.text.includes(wanted)
          )
            throw Error(
              "The spelling must appear in the original and corrected text.",
            );
          const existing = sample.dictionary.find(
            (e) => e.heard === heard && e.wanted === wanted && !e.app,
          );
          const context = cues
            .split(",")
            .map((s) => s.trim())
            .filter(Boolean);
          if (existing?.enabled)
            existing.cues = [
              ...new Set([...(existing.cues || []), ...context]),
            ];
          else if (!existing)
            sample.dictionary.push({
              heard,
              wanted,
              cues: context,
              enabled: true,
              app: null,
            });
        }
        record.text = action.text;
        record.updated_ms = Date.now();
        break;
      }
      case "session_delete":
        records = records.filter((s) => s.id !== action.id);
        if (selected === action.id) selected = null;
        break;
      case "dictionary_save": {
        if (!action.heard?.trim() || !action.wanted?.trim())
          throw Error("Enter the spoken phrase and its spelling.");
        const original =
          action.index === undefined ? null : sample.dictionary[action.index];
        if (
          action.index !== undefined &&
          (!original || !sameRecord(original, action.expected_entry))
        )
          throw Error("Vocabulary changed. Refresh and try again.");
        const existing = sample.dictionary.findIndex(
          (e) =>
            e.heard === action.heard &&
            e.wanted === action.wanted &&
            e.app === (action.app || null),
        );
        const entry = {
          ...original,
          heard: action.heard,
          wanted: action.wanted,
          app:
            action.app === undefined
              ? (original?.app ?? null)
              : action.app || null,
          cues:
            action.cues === undefined
              ? (original?.cues ?? [])
              : action.cues
                  .split(",")
                  .map((s) => s.trim())
                  .filter(Boolean),
          enabled: original?.enabled ?? true,
        };
        if (action.index !== undefined)
          sample.dictionary = sample.dictionary.map((e, i) =>
            i === action.index ? entry : e,
          );
        else if (existing >= 0)
          sample.dictionary[existing] = {
            ...entry,
            cues: [
              ...new Set([...sample.dictionary[existing].cues, ...entry.cues]),
            ],
          };
        else sample.dictionary = [...sample.dictionary, entry];
        break;
      }
      case "dictionary_delete":
        if (
          !sample.dictionary[action.index] ||
          !sameRecord(sample.dictionary[action.index], action.expected_entry)
        )
          throw Error("Vocabulary changed. Refresh and try again.");
        sample.dictionary = sample.dictionary.filter(
          (_, i) => i !== action.index,
        );
        break;
      case "macro_save": {
        if (!action.trigger?.trim() || !action.expansion?.trim())
          throw Error("Enter a trigger and the text to insert.");
        const original =
          action.index === undefined
            ? sample.macros.find(
                (e) =>
                  e.trigger === action.trigger &&
                  e.app === (action.app || null),
              )
            : sample.macros[action.index];
        if (
          action.index !== undefined &&
          (!original || !sameRecord(original, action.expected_macro))
        )
          throw Error("Snippets changed. Refresh and try again.");
        const entry = {
          trigger: action.trigger,
          expansion: action.expansion,
          app: action.app || null,
          enabled: original?.enabled ?? true,
        };
        if (
          action.index !== undefined &&
          sample.macros.some(
            (e, i) =>
              i !== action.index &&
              e.trigger === entry.trigger &&
              e.app === entry.app,
          )
        )
          throw Error("A snippet already uses this trigger in that app.");
        if (action.index !== undefined)
          sample.macros = sample.macros.map((e, i) =>
            i === action.index ? entry : e,
          );
        else
          sample.macros = [
            ...sample.macros.filter(
              (e) => e.trigger !== entry.trigger || e.app !== entry.app,
            ),
            entry,
          ];
        break;
      }
      case "macro_delete":
        if (
          !sample.macros[action.index] ||
          !sameRecord(sample.macros[action.index], action.expected_macro)
        )
          throw Error("Snippets changed. Refresh and try again.");
        sample.macros = sample.macros.filter((_, i) => i !== action.index);
        break;
      case "settings_patch": {
        const { type, ...patch } = action;
        sample.settings = { ...sample.settings, ...patch };
        break;
      }
      case "insights_refresh":
        break;
      case "copy":
        await navigator.clipboard.writeText(action.text);
        return;
      case "export": {
        const url = URL.createObjectURL(
          new Blob([action.text], { type: "text/plain;charset=utf-8" }),
        );
        const link = document.createElement("a");
        link.href = url;
        link.download = `Articulate.${action.extension || "txt"}`;
        link.click();
        setTimeout(() => URL.revokeObjectURL(url), 1000);
        return;
      }
      default:
        throw Error("Open the desktop app to use recording and local models.");
    }
    persist();
  } catch (error) {
    // Failed persistence must never appear successful in the next poll.
    if (!["copy", "export"].includes(action.type))
      ({ records, sample, selected } = before);
    throw error;
  }
}
