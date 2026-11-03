// The browser preview workspace: synthetic documents stored in the browser.
// It implements the preview actions in ../actions.js; validation shared with
// the desktop transport runs before these handlers.
import { previewSupports, savedPatchValue } from "../actions.js";
import {
  sampleAvatarUri,
  sampleDictionary,
  sampleDiscord,
  sampleMacros,
  sampleMeetings,
  sampleQuarantine,
  samplePeople,
  sampleRecords,
  sampleSettings,
  sampleWarnings,
} from "./fixtures.js";
import {
  emptySearch,
  emptyStorage,
  followUps,
  libraryHandlers,
  subtitlesUnavailable,
} from "./library.js";

const meetingApps = {
  teams: { id: "teams", label: "Microsoft Teams", best_effort: false },
  zoom: { id: "zoom", label: "Zoom", best_effort: false },
  meet: { id: "meet", label: "Google Meet", best_effort: true },
};

const storageKey = "articulate-ui-sample-workspace-v1";

// Speaker labels the way the desktop app derives them.
function defaultLabel(key, names = {}) {
  if (names[key]) return names[key];
  if (key === "you") return "You";
  if (key === "other") return "Other audio";
  if (key === "uncertain") return "Uncertain speaker";
  const voice = /^s([1-8])$/.exec(key);
  return voice ? `Speaker ${voice[1]}` : key;
}

function rowKeys(row) {
  return row.speaker_keys?.length ? row.speaker_keys : [row.speaker];
}

function labelRows(record) {
  const names = record.speaker_names || {};
  return (record.rows || []).map((row) => {
    const keys = rowKeys(row);
    const labels = keys.map((key) => defaultLabel(key, names));
    return {
      cues: [],
      provisional: false,
      ...row,
      speaker_keys: keys,
      speaker_labels: labels,
      speaker: labels.length > 1 ? `Overlap: ${labels.join(" + ")}` : labels[0],
    };
  });
}

// Stand-ins for the controller's transcript revision and version identity.
function textRevision(text = "") {
  let hash = 5381;
  for (let index = 0; index < text.length; index++)
    hash = (hash * 33 + text.charCodeAt(index)) >>> 0;
  return hash.toString(16);
}
const versionId = (version) =>
  `${version.replaced_ms}-${textRevision(version.text)}`;

function presented(record) {
  if (!record) return record;
  const versions = record.transcript_versions ?? [];
  return {
    ...record,
    rows: labelRows(record),
    speaker_undo: record.speaker_history?.at(-1)?.summary ?? null,
    interruptions: record.interruptions ?? [],
    transcript_versions: versions.map((version, index) => ({
      ...version,
      id: versionId(version),
      index,
    })),
    transcript_revision: versions.length ? textRevision(record.text) : null,
    audio: record.audio ?? null,
  };
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
  let isolated = structuredClone(sampleQuarantine);
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
    vocabulary_sound_alike: saved?.vocabulary_sound_alike ?? true,
    dictionary: saved?.dictionary ?? structuredClone(sampleDictionary),
    macros: saved?.macros ?? structuredClone(sampleMacros),
    settings: { ...structuredClone(sampleSettings), ...saved?.settings },
    microphones: ["Default microphone"],
    outputs: ["Default speakers"],
    discord: structuredClone(sampleDiscord),
    meetings: structuredClone(sampleMeetings),
    insights: null,
    search: emptySearch(),
    storage: emptyStorage(saved?.storage),
    recovery: {
      warnings: structuredClone(sampleWarnings),
      quarantined: isolated.length,
      quarantine: [],
      listed: false,
    },
    library_notice: null,
    people: saved?.people ?? structuredClone(samplePeople),
    people_error: null,
    audio_import: {
      state: "idle",
      dragging: false,
      file_name: null,
      progress: null,
      stage: "",
      message: "",
      session_id: null,
      rebuilding: null,
      speakers_available: true,
    },
    playback: {
      session_id: null,
      playing: false,
      position_ms: 0,
      error: null,
    },
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
        vocabulary_sound_alike: sample.vocabulary_sound_alike,
        macros: sample.macros,
        settings: sample.settings,
        storage: {
          encrypt_at_rest: sample.storage.encrypt_at_rest,
          retention: sample.storage.retention,
        },
        people: sample.people,
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

  function documentView(session) {
    if (!session) return null;
    return {
      ...session,
      pinned: Boolean(session.pinned),
      follow_ups: followUps(session).length,
      subtitles_unavailable: subtitlesUnavailable(session),
    };
  }

  // A renderer snapshot must not retain writable references to saved documents.
  function snapshot() {
    return structuredClone({
      ...sample,
      history: records
        .map((item) => summary(withSources(item)))
        .sort((a, b) => b.created_ms - a.created_ms),
      selected: documentView(
        presented(withSources(records.find((s) => s.id === selected))),
      ),
    });
  }

  function update() {
    return {
      revision,
      sections: { preview: { revision, data: snapshot() } },
    };
  }

  const record = (id) => records.find((s) => s.id === id);

  function notify(message, error = false) {
    sample.library_notice = {
      sequence: (sample.library_notice?.sequence ?? 0) + 1,
      error,
      message,
    };
  }

  const library = libraryHandlers({
    get sample() {
      return sample;
    },
    record,
    records: () => records,
    selected: () => selected,
    isolated: () => isolated,
    notify,
    // Finish simulated background work, as the controller would.
    later(delay, finish) {
      setTimeout(() => {
        finish();
        persist();
        publish();
      }, delay);
    },
    remove(ids) {
      records = records.filter((item) => !ids.includes(item.id));
    },
    restore(file) {
      if (!record(file.record.id)) records = [file.record, ...records];
      isolated = isolated.filter((item) => item.file_name !== file.file_name);
      sample.recovery.quarantined = isolated.length;
      sample.recovery.quarantine = sample.recovery.quarantine.filter(
        (item) => item.file_name !== file.file_name,
      );
    },
    discard(fileName) {
      isolated = isolated.filter((item) => item.file_name !== fileName);
      sample.recovery.quarantined = isolated.length;
      sample.recovery.quarantine = sample.recovery.quarantine.filter(
        (item) => item.file_name !== fileName,
      );
    },
  });

  // Speaker corrections on a sample conversation, with an undo stack.
  function correct(action, summary, change) {
    const document = record(action.id);
    if (!document || document.kind !== "call" || !document.rows?.length)
      throw Error("Only conversation transcripts have speakers to correct.");
    const before = {
      summary,
      rows: structuredClone(document.rows),
      speaker_names: { ...document.speaker_names },
    };
    change(document);
    document.speaker_history = [
      ...(document.speaker_history || []),
      before,
    ].slice(-20);
    document.updated_ms = Date.now();
  }
  const labelOf = (document, key) =>
    defaultLabel(key, document.speaker_names || {});
  const rememberName = (name) => {
    if (
      !sample.people.some(
        (person) => person.name.toLowerCase() === name.toLowerCase(),
      )
    )
      sample.people = [
        {
          id: crypto.randomUUID(),
          name,
          source: "name",
          discord_name: null,
          avatar: null,
          used_ms: Date.now(),
        },
        ...sample.people,
      ];
  };
  let playbackTimer = null;
  let importTimer = null;
  const stopPlayback = () => {
    clearInterval(playbackTimer);
    playbackTimer = null;
    sample.playback = { ...sample.playback, playing: false, session_id: null };
  };

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
            heard: "casey",
            match_kind: "exact",
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

    ...library,
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
    dictionary_sound_alike({ enabled }) {
      sample.vocabulary_sound_alike = enabled;
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
        sound_alike: action.sound_alike ?? original?.sound_alike ?? true,
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
      if (patch.discord_flavor !== undefined)
        sample.discord.flavor = patch.discord_flavor;
      if (patch.discord_avatars === false)
        for (const participant of sample.discord.participants)
          participant.avatar = null;
      else if (patch.discord_avatars === true && sample.discord.participants[0])
        sample.discord.participants[0].avatar = sampleAvatarUri;
      syncMeetings();
    },
    meeting_respond(action) {
      const prompt = sample.meetings.prompt;
      if (!prompt) throw Error("No meeting is waiting for an answer.");
      sample.meetings.prompt = null;
      if (action.accept) {
        sample.meetings.owned = prompt;
        sample.meetings.status = `Transcribing the ${prompt.label} meeting.`;
      } else {
        sample.meetings.status = `Transcription is paused for this ${prompt.label} meeting.`;
      }
    },
    discord_relaunch_normally() {
      sample.discord.debugger_open = false;
      sample.discord.debugger_port = 0;
      sample.discord.debugger_notice = null;
    },
    discord_rotate_key() {
      sample.discord.pairing_key_present = true;
    },
    discord_forget_pairing() {
      sample.discord.pairing_key_present = false;
      sample.discord.listener_started = false;
      sample.discord.connected = false;
      sample.discord.in_voice = false;
      sample.discord.status = "disconnected";
      sample.discord.pairing_state = "automatic";
      sample.discord.participants = [];
      sample.settings.discord_auto_connect = false;
    },
    speaker_rename(action) {
      const document = record(action.id);
      const key = action.speaker;
      if (["you", "other", "uncertain"].includes(key))
        throw Error("This speaker cannot be renamed.");
      const name = action.name.trim();
      correct(
        action,
        `Renamed ${labelOf(document, key)} to ${name}`,
        (target) => {
          target.speaker_names = { ...target.speaker_names, [key]: name };
        },
      );
      rememberName(name);
    },
    speaker_merge(action) {
      const document = record(action.id);
      if (action.from === action.into)
        throw Error("Choose two different speakers to merge.");
      correct(
        action,
        `Merged ${labelOf(document, action.from)} into ${labelOf(document, action.into)}`,
        (target) => {
          for (const row of target.rows)
            row.speaker_keys = [
              ...new Set(
                rowKeys(row).map((key) =>
                  key === action.from ? action.into : key,
                ),
              ),
            ];
        },
      );
    },
    speaker_reassign(action) {
      const document = record(action.id);
      for (const { index, start_ms } of action.rows)
        if (document?.rows?.[index]?.start_ms !== start_ms)
          throw Error("The transcript changed. Select the turns again.");
      let key = action.speaker;
      if (action.name !== undefined) {
        const name = action.name.trim();
        const names = document.speaker_names || {};
        key =
          Object.keys(names).find(
            (existing) => names[existing].toLowerCase() === name.toLowerCase(),
          ) ?? `n:${crypto.randomUUID().slice(0, 8)}`;
        document.speaker_names = { ...names, [key]: name };
        rememberName(name);
      }
      const count = action.rows.length;
      correct(
        action,
        `Assigned ${count} ${count === 1 ? "turn" : "turns"} to ${labelOf(document, key)}`,
        (target) => {
          for (const { index } of action.rows)
            target.rows[index].speaker_keys = [key];
        },
      );
    },
    speaker_undo(action) {
      const document = record(action.id);
      const last = document?.speaker_history?.pop();
      if (!last) throw Error("There is nothing to undo.");
      document.rows = last.rows;
      document.speaker_names = last.speaker_names;
    },
    person_save(action) {
      const name = action.name.trim();
      if (action.id) {
        const person = sample.people.find((p) => p.id === action.id);
        if (!person) throw Error("This person is no longer in the list.");
        person.name = name;
      } else {
        if (
          sample.people.some((p) => p.name.toLowerCase() === name.toLowerCase())
        )
          throw Error("That name is already in the list.");
        rememberName(name);
      }
    },
    person_delete(action) {
      sample.people = sample.people.filter((p) => p.id !== action.id);
    },
    people_clear(action) {
      if (action.confirm?.count !== sample.people.length)
        throw Error("The list of people changed. Review it and confirm again.");
      sample.people = [];
    },
    audio_import_choose() {
      sample.audio_import = {
        ...sample.audio_import,
        state: "ready",
        file_name: "Team sync.m4a",
        message: "",
        session_id: null,
      };
    },
    audio_import_start(action) {
      if (sample.audio_import.state !== "ready")
        throw Error("Choose an audio file to import.");
      // Simulate the desktop import: progress, then a new saved document.
      const id = crypto.randomUUID();
      const name = sample.audio_import.file_name.replace(/\.[^.]+$/, "");
      sample.audio_import = {
        ...sample.audio_import,
        state: "working",
        stage: "Transcribing",
        progress: 0,
      };
      clearInterval(importTimer);
      importTimer = setInterval(() => {
        const progress = Math.min(1, sample.audio_import.progress + 0.2);
        if (progress < 1) {
          sample.audio_import = { ...sample.audio_import, progress };
        } else {
          clearInterval(importTimer);
          const call = action.kind === "call";
          records = [
            {
              id,
              title: name,
              kind: action.kind,
              created_ms: Date.now(),
              updated_ms: Date.now(),
              duration_ms: 118000,
              text: call
                ? ""
                : "An imported thought about the release checklist.",
              original: "",
              personal_notes: "",
              rows: call
                ? structuredClone(
                    sampleRecords.find((r) => r.id === "sample-call-1")?.rows ??
                      [],
                  )
                : [],
              speaker_names: {},
              audio: action.keep_audio
                ? {
                    duration_ms: 118000,
                    bytes: 1_900_000,
                    channels: ["recording"],
                    incomplete: false,
                  }
                : null,
            },
            ...records,
          ];
          persist();
          sample.audio_import = {
            ...sample.audio_import,
            state: "done",
            progress: null,
            session_id: id,
            message: "Import finished",
          };
        }
        publish();
      }, 400);
    },
    audio_import_cancel() {
      clearInterval(importTimer);
      sample.audio_import = {
        ...sample.audio_import,
        state: "failed",
        progress: null,
        message: "Import cancelled",
      };
    },
    audio_import_dismiss() {
      sample.audio_import = {
        ...sample.audio_import,
        state: "idle",
        file_name: null,
        message: "",
        session_id: null,
      };
    },
    audio_play(action) {
      const document = record(action.id);
      if (!document?.audio) throw Error("This recording has no kept audio.");
      clearInterval(playbackTimer);
      sample.playback = {
        session_id: action.id,
        playing: true,
        position_ms: action.from_ms ?? 0,
        error: null,
      };
      // Advance the sample position like a playing recording.
      playbackTimer = setInterval(() => {
        sample.playback = {
          ...sample.playback,
          position_ms: sample.playback.position_ms + 500,
        };
        if (sample.playback.position_ms >= document.audio.duration_ms)
          stopPlayback();
        publish();
      }, 500);
    },
    audio_stop() {
      stopPlayback();
    },
    audio_delete(action) {
      const document = record(action.id);
      if (!document?.audio) throw Error("This recording has no kept audio.");
      if (
        action.confirm?.id !== action.id ||
        action.confirm?.bytes !== document.audio.bytes
      )
        throw Error("The kept audio changed. Confirm deleting it again.");
      if (sample.playback.session_id === action.id) stopPlayback();
      document.audio = null;
    },
    transcript_rebuild() {
      throw Error("Re-transcription runs in the desktop app.");
    },
    transcript_restore(action) {
      const document = record(action.id);
      if (!document || textRevision(document.text) !== action.expected)
        throw Error(
          "The transcript changed. Review the earlier transcripts again.",
        );
      const versions = document.transcript_versions ?? [];
      const index = versions.findIndex(
        (version) => versionId(version) === action.version,
      );
      if (index < 0) throw Error("That version is no longer available.");
      const [version] = versions.splice(index, 1);
      versions.push({
        replaced_ms: Date.now(),
        model: "",
        text: document.text,
        rows: document.rows,
        preview: document.text.slice(0, 160),
      });
      document.transcript_versions = versions;
      document.text = version.text;
      document.rows = version.rows;
    },
  };

  // Mirrors the desktop controller: watched apps, and a synthetic meeting a
  // few seconds after detection is switched on so the prompt can be checked.
  let detection = null;
  function syncMeetings() {
    const settings = sample.settings;
    const meetings = sample.meetings;
    meetings.settings = {
      teams: Boolean(settings.meetings_teams),
      zoom: Boolean(settings.meetings_zoom),
      meet: Boolean(settings.meetings_meet),
      start: settings.meetings_start || "ask",
    };
    const watched = Object.entries(meetings.settings)
      .filter(([key, on]) => key !== "start" && on)
      .map(([key]) => meetingApps[key]);
    meetings.enabled = watched.length > 0;
    meetings.active = meetings.active.filter((item) =>
      watched.some((app) => app.id === item.id),
    );
    for (const key of ["prompt", "owned"])
      if (meetings[key] && !watched.some((app) => app.id === meetings[key].id))
        meetings[key] = null;
    if (!meetings.enabled) {
      meetings.status = "";
      clearTimeout(detection);
      detection = null;
      return;
    }
    meetings.status = meetings.owned
      ? `Transcribing the ${meetings.owned.label} meeting.`
      : meetings.prompt
        ? `${meetings.prompt.label} meeting detected. Choose whether to start a transcript.`
        : "Watching for a meeting.";
    if (!meetings.active.length && !detection)
      detection = setTimeout(() => {
        detection = null;
        const app = watched[0];
        if (!app || !sample.meetings.enabled) return;
        sample.meetings.active = [app];
        sample.meetings.events = [
          ...sample.meetings.events.slice(-15),
          {
            app: app.id,
            label: app.label,
            kind: "started",
            at_unix_ms: Date.now(),
          },
        ];
        if (sample.meetings.settings.start === "automatic") {
          sample.meetings.owned = app;
          sample.meetings.status = `Transcribing the ${app.label} meeting.`;
        } else {
          sample.meetings.prompt = app;
          sample.meetings.status = `${app.label} meeting detected. Choose whether to start a transcript.`;
        }
        publish();
      }, 2500);
  }
  syncMeetings();

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

  // A drawn placeholder picture for the one sample participant with an avatar.
  let avatarBytes = null;
  async function avatar(uri) {
    if (uri !== sampleAvatarUri || !sample.settings.discord_avatars)
      return null;
    if (avatarBytes) return avatarBytes;
    const canvas = globalThis.document?.createElement("canvas");
    if (!canvas) return null;
    canvas.width = 64;
    canvas.height = 64;
    const context = canvas.getContext("2d");
    const gradient = context.createLinearGradient(0, 0, 64, 64);
    gradient.addColorStop(0, "#5b8def");
    gradient.addColorStop(1, "#93dfc2");
    context.fillStyle = gradient;
    context.fillRect(0, 0, 64, 64);
    context.fillStyle = "#10201b";
    context.font = "bold 30px sans-serif";
    context.textAlign = "center";
    context.textBaseline = "middle";
    context.fillText("C", 32, 34);
    const blob = await new Promise((resolve) =>
      canvas.toBlob(resolve, "image/png"),
    );
    avatarBytes = new Uint8Array(await blob.arrayBuffer());
    return avatarBytes;
  }

  return {
    snapshot,
    avatar,
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
