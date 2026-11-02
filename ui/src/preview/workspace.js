// The browser preview workspace: synthetic documents stored in the browser.
// It implements the preview actions in ../actions.js; validation shared with
// the desktop transport runs before these handlers.
import { previewSupports, savedPatchValue } from "../actions.js";
import {
  sampleDictionary,
  sampleMacros,
  sampleRecords,
  sampleSettings,
} from "./fixtures.js";

const storageKey = "articulate-ui-sample-workspace-v1";

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

const cueList = (cues) =>
  cues
    .split(",")
    .map((cue) => cue.trim())
    .filter(Boolean);

export function createPreviewWorkspace(storage) {
  let saved;
  try {
    saved = JSON.parse(storage?.getItem(storageKey));
  } catch {
    /* A fresh sample workspace is safe. */
  }
  // Each workspace owns its copy; fixtures are shared module data.
  const fixtures = structuredClone(sampleRecords);
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
    cleanup: { installed: true, working: false, status: "", preview: null },
    assort: { working: false, status: "", candidates: null, scores: null },
    managed_models: [
      "speech",
      "verifier",
      "speaker",
      "sound_cues",
      "polish",
      "notes",
    ].map((id) => ({
      id,
      installed: true,
      in_use: id === "speech",
      disk_size: 250000000,
      path: `models/${id}.gguf`,
    })),
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
    text: "The captured part of this thought is still available.",
    original: "The captured part of this thought is still available.",
    dictation_interruption: "The sample microphone disconnected.",
    call_rows: [],
    note_rows: [],
    call_session: null,
    history_loading: false,
    history_error: null,
    saving: false,
    dictionary: saved?.dictionary ?? structuredClone(sampleDictionary),
    macros: saved?.macros ?? structuredClone(sampleMacros),
    settings: { ...structuredClone(sampleSettings), ...saved?.settings },
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
  let revision = 1;
  const listeners = new Set();

  function persist() {
    storage.setItem(
      storageKey,
      JSON.stringify({
        records,
        dictionary: sample.dictionary,
        macros: sample.macros,
        settings: sample.settings,
        topic_examples_added: true,
      }),
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

  function withSources(session) {
    if (!session?.is_collection) return session;
    const sources = records.filter((source) => source.topic_id === session.id);
    return {
      ...session,
      text: sources.map((source) => source.text).join("\n\n"),
      sources: sources.map(summary),
    };
  }

  // A renderer snapshot must not retain writable references to saved documents.
  function snapshot() {
    return structuredClone({
      ...sample,
      history: records
        .map((item) => summary(withSources(item)))
        .sort((a, b) => b.created_ms - a.created_ms),
      selected: withSources(records.find((s) => s.id === selected)) ?? null,
    });
  }

  function update() {
    return {
      revision,
      sections: { preview: { revision, data: snapshot() } },
    };
  }

  const record = (id) => records.find((s) => s.id === id);

  const handlers = {
    polish_start() {
      sample.cleanup.preview = {
        original: sample.text,
        source: sample.text,
        text: sample.text + ".",
        blocked: null,
        cleanup_only: false,
      };
    },
    polish_apply() {
      if (sample.cleanup.preview) sample.text = sample.cleanup.preview.text;
      sample.cleanup.preview = null;
    },
    polish_cancel() {
      sample.cleanup.preview = null;
    },
    assort_start() {
      sample.assort = {
        working: false,
        status: "Review suggestions before applying",
        candidates: [
          {
            context: "Please ask casey to review the plan.",
            entry: { heard: "casey", wanted: "Casey" },
          },
        ],
        scores: [{ candidate: 0, replace_score: 0.82 }],
      };
    },
    assort_apply() {
      sample.assort = {
        working: false,
        status: "Choice saved. Review again for the next choice.",
        candidates: null,
        scores: null,
      };
    },
    assort_cancel() {
      sample.assort = {
        working: false,
        status: "Cancelled",
        candidates: null,
        scores: null,
      };
    },
    model_delete({ id }) {
      const model = sample.managed_models.find((m) => m.id === id);
      if (model.in_use) throw Error("This model is in use");
      model.installed = false;
      model.disk_size = 0;
    },

    history_open(action) {
      if (!record(action.id)) throw Error("This note is no longer available.");
      selected = action.id;
    },
    history_close() {
      selected = null;
    },
    history_refresh() {},
    history_retry() {},
    insights_refresh() {},
    source_move(action) {
      const source = record(action.id);
      if (!source || source.is_collection || !source.text?.trim())
        throw Error("Choose an original dictation to move.");
      if ((source.topic_id ?? null) !== action.expected_topic_id)
        throw Error(
          "This dictation was already moved. Reopen it and try again.",
        );
      const target = action.topic_id === null ? null : record(action.topic_id);
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
    },
    note_new() {
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
    },
    session_patch(action) {
      const document = record(action.id);
      if (!document) throw Error("This note is no longer available.");
      for (const key of ["title", "personal_notes"])
        if (action[key] !== undefined)
          document[key] = savedPatchValue(key, action[key]);
      document.updated_ms = Date.now();
    },
    dictation_edit(action) {
      const document = record(action.id);
      if (!document || document.kind !== "dictation" || document.rows?.length)
        throw Error("Choose a finished dictation to edit.");
      if (document.text !== action.expected_text)
        throw Error(
          "This dictation changed. Reopen the editor to use its latest text.",
        );
      if (action.correction) {
        const { heard, wanted, cues = "" } = action.correction;
        if (
          !heard.trim() ||
          !wanted.trim() ||
          heard === wanted ||
          !document.text.includes(heard) ||
          !action.text.includes(wanted)
        )
          throw Error(
            "The spelling must appear in the original and corrected text.",
          );
        const existing = sample.dictionary.find(
          (e) => e.heard === heard && e.wanted === wanted && !e.app,
        );
        const context = cueList(cues);
        if (existing?.enabled)
          existing.cues = [...new Set([...(existing.cues || []), ...context])];
        else if (!existing)
          sample.dictionary.push({
            heard,
            wanted,
            cues: context,
            enabled: true,
            app: null,
          });
      }
      document.text = action.text;
      document.updated_ms = Date.now();
    },
    session_delete(action) {
      records = records.filter((s) => s.id !== action.id);
      if (selected === action.id) selected = null;
    },
    dictionary_save(action) {
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
            : cueList(action.cues),
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
    },
    dictionary_delete(action) {
      if (
        !sample.dictionary[action.index] ||
        !sameRecord(sample.dictionary[action.index], action.expected_entry)
      )
        throw Error("Vocabulary changed. Refresh and try again.");
      sample.dictionary = sample.dictionary.filter(
        (_, i) => i !== action.index,
      );
    },
    macro_save(action) {
      if (!action.trigger?.trim() || !action.expansion?.trim())
        throw Error("Enter a trigger and the text to insert.");
      const original =
        action.index === undefined
          ? sample.macros.find(
              (e) =>
                e.trigger === action.trigger && e.app === (action.app || null),
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
    },
    macro_delete(action) {
      if (
        !sample.macros[action.index] ||
        !sameRecord(sample.macros[action.index], action.expected_macro)
      )
        throw Error("Snippets changed. Refresh and try again.");
      sample.macros = sample.macros.filter((_, i) => i !== action.index);
    },
    microphone_check() {
      sample.settings.microphone_check = {
        running: false,
        result: {
          device: "Sample microphone",
          peak: 0.64,
          rms: 0.18,
          clipping_ratio: 0,
          silent: false,
        },
      };
    },
    settings_patch(action) {
      const { type, ...patch } = action;
      sample.settings = { ...sample.settings, ...patch };
    },
  };

  // Clipboard and download requests do not change the workspace.
  const effects = {
    async copy(action) {
      await navigator.clipboard.writeText(action.text);
    },
    export(action) {
      const url = URL.createObjectURL(
        new Blob([action.text], { type: "text/plain;charset=utf-8" }),
      );
      const link = document.createElement("a");
      link.href = url;
      link.download = `Articulate.${action.extension}`;
      link.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    },
  };

  function publish() {
    revision += 1;
    const next = update();
    for (const listener of listeners) listener(next);
  }

  return {
    snapshot,
    revision: () => revision,
    state(since) {
      return since?.preview === revision
        ? { revision, sections: {} }
        : update();
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    async action(action) {
      if (!previewSupports(action.type))
        throw Error("Open the desktop app to use recording and local models.");
      if (effects[action.type]) return effects[action.type](action);
      const before = structuredClone({ records, sample, selected });
      try {
        handlers[action.type](action);
        persist();
      } catch (error) {
        // Failed persistence must never appear successful in the next update.
        ({ records, sample, selected } = before);
        throw error;
      }
      publish();
    },
  };
}
