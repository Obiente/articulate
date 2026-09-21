import test from "node:test";
import assert from "node:assert/strict";

let moduleId = 0;
const storageKey = "articulate-ui-sample-workspace-v1";
async function browser(initial) {
  const data = new Map(initial ? [[storageKey, JSON.stringify(initial)]] : []);
  const storage = {
    fail: false,
    getItem: (key) => data.get(key) ?? null,
    setItem(key, value) {
      if (this.fail) throw new Error("Synthetic storage failure");
      data.set(key, value);
    },
  };
  globalThis.localStorage = storage;
  globalThis.isTauri = false;
  const bridge = await import(`../src/bridge.js?test=${++moduleId}`);
  return { ...bridge, storage, data };
}

test("dictation corrections persist their original and reject stale saves", async () => {
  const before = "Start with a kit. Considering a kit is a sample project.";
  const after = before.replaceAll("a kit", "AcmeKit");
  const bridge = await browser({
    records: [
      {
        id: "edit-dictation",
        kind: "dictation",
        text: before,
        original: before,
        title: "Project",
        created_ms: 1,
        rows: [],
        personal_notes: "Keep my note.",
      },
    ],
    dictionary: [],
  });
  await bridge.action({ type: "history_open", id: "edit-dictation" });
  await bridge.action({
    type: "dictation_edit",
    id: "edit-dictation",
    expected_text: before,
    text: after,
    correction: { heard: "a kit", wanted: "AcmeKit", cues: "sample" },
  });
  assert.equal((await bridge.snapshot()).selected.text, after);
  assert.equal((await bridge.snapshot()).selected.original, before);
  await assert.rejects(
    bridge.action({
      type: "dictation_edit",
      id: "edit-dictation",
      expected_text: before,
      text: "Stale",
      correction: null,
    }),
  );
  const reloaded = await browser(JSON.parse(bridge.data.get(storageKey)));
  await reloaded.action({ type: "history_open", id: "edit-dictation" });
  assert.equal((await reloaded.snapshot()).selected.text, after);
  assert.equal(
    (await reloaded.snapshot()).selected.personal_notes,
    "Keep my note.",
  );
  assert.equal((await reloaded.snapshot()).dictionary[0].wanted, "AcmeKit");
});

test("a document edit stays with its owner and survives reopening", async () => {
  const bridge = await browser();
  const before = await bridge.snapshot();
  const [first, second] = before.history;
  await bridge.action({ type: "history_open", id: first.id });
  await bridge.action({
    type: "session_patch",
    id: first.id,
    personal_notes: "Synthetic private reminder.",
    title: "  Reviewed note  ",
  });
  await bridge.action({ type: "history_open", id: second.id });
  assert.notEqual(
    (await bridge.snapshot()).selected.personal_notes,
    "Synthetic private reminder.",
  );
  await bridge.action({ type: "history_open", id: first.id });
  const saved = (await bridge.snapshot()).selected;
  assert.equal(saved.personal_notes, "Synthetic private reminder.");
  assert.equal(saved.title, "Reviewed note");
  assert.equal(saved.text, first.text);
  const reloaded = await browser(JSON.parse(bridge.data.get(storageKey)));
  await reloaded.action({ type: "history_open", id: first.id });
  assert.equal(
    (await reloaded.snapshot()).selected.personal_notes,
    saved.personal_notes,
  );
});

test("older preview settings keep user choices and gain new shortcut defaults", async () => {
  const customHotkey = {
    ctrl: true,
    alt: false,
    shift: true,
    win: false,
    key: "D",
  };
  const bridge = await browser({
    settings: {
      clean_speech: false,
      hotkey: customHotkey,
      hotkey_mode: "Toggle",
      microphone: "Synthetic microphone",
    },
  });
  const settings = (await bridge.snapshot()).settings;
  assert.equal(settings.clean_speech, false);
  assert.equal(settings.hotkey_mode, "Toggle");
  assert.equal(settings.microphone, "Synthetic microphone");
  assert.deepEqual(settings.hotkey, customHotkey);
  assert.deepEqual(settings.quick_note_hotkey, {
    ctrl: true,
    alt: true,
    shift: false,
    win: false,
    key: "N",
  });
  assert.equal(settings.vencord_auto_update, false);
  await bridge.action({ type: "settings_patch", audio_feedback: false });
  const reloaded = await browser(JSON.parse(bridge.data.get(storageKey)));
  assert.deepEqual(
    (await reloaded.snapshot()).settings.quick_note_hotkey,
    settings.quick_note_hotkey,
  );
  assert.deepEqual((await reloaded.snapshot()).settings.hotkey, customHotkey);
  assert.equal((await reloaded.snapshot()).settings.audio_feedback, false);
});

test("failed storage writes do not become successful in a later snapshot", async () => {
  const bridge = await browser();
  const first = (await bridge.snapshot()).history[0];
  await bridge.action({ type: "history_open", id: first.id });
  bridge.storage.fail = true;
  await assert.rejects(
    bridge.action({
      type: "session_patch",
      id: first.id,
      personal_notes: "Must not appear saved",
    }),
    /storage failure/,
  );
  assert.equal(
    (await bridge.snapshot()).selected.personal_notes,
    first.personal_notes,
  );
  bridge.storage.fail = false;
  await bridge.action({
    type: "session_patch",
    id: first.id,
    personal_notes: "Retry succeeds",
  });
  assert.equal(
    (await bridge.snapshot()).selected.personal_notes,
    "Retry succeeds",
  );
});

test("render snapshots cannot modify persistent records by alias", async () => {
  const bridge = await browser();
  const value = await bridge.snapshot();
  value.history[0].personal_notes = "Unsubmitted draft";
  value.dictionary[0].wanted = "Unsubmitted spelling";
  const after = await bridge.snapshot();
  assert.notEqual(after.history[0].personal_notes, "Unsubmitted draft");
  assert.notEqual(after.dictionary[0].wanted, "Unsubmitted spelling");
});

test("vocabulary edits preserve rules not exposed by the editor", async () => {
  const original = {
    heard: "synthetic term",
    wanted: "Synthetic Term",
    app: "code.exe",
    cues: ["build"],
    enabled: false,
    ignore_case: true,
    contexts: [{ cues: ["compile"], ignore_case: false }],
  };
  const bridge = await browser({ dictionary: [original] });
  await bridge.action({
    type: "dictionary_save",
    index: 0,
    expected_heard: original.heard,
    expected_entry: original,
    heard: original.heard,
    wanted: "Synthetic term",
  });
  const edited = (await bridge.snapshot()).dictionary[0];
  assert.equal(edited.enabled, false);
  assert.equal(edited.ignore_case, true);
  assert.deepEqual(edited.contexts, original.contexts);
  assert.deepEqual(edited.cues, original.cues);
  assert.equal(edited.app, original.app);
  await assert.rejects(
    bridge.action({
      type: "dictionary_delete",
      index: 0,
      expected_heard: "stale term",
    }),
    /Vocabulary changed/,
  );
  assert.equal((await bridge.snapshot()).dictionary.length, 1);
});

test("stale snippet edits and deletes cannot target a different item", async () => {
  const bridge = await browser({
    macros: [
      { trigger: "one", expansion: "First", app: null, enabled: false },
      { trigger: "two", expansion: "Second", app: null, enabled: true },
    ],
  });
  await bridge.action({
    type: "macro_delete",
    index: 0,
    expected_trigger: "one",
    expected_macro: (await bridge.snapshot()).macros[0],
  });
  await assert.rejects(
    bridge.action({
      type: "macro_save",
      index: 0,
      expected_trigger: "one",
      trigger: "updated",
      expansion: "Wrong item",
    }),
    /Snippets changed/,
  );
  await assert.rejects(
    bridge.action({ type: "macro_delete", index: 0, expected_trigger: "one" }),
    /Snippets changed/,
  );
  assert.equal((await bridge.snapshot()).macros[0].trigger, "two");
});

test("same-named app-scoped rules require the complete original identity", async () => {
  const first = {
    heard: "at casey",
    wanted: "@Casey",
    app: "discord.exe",
    cues: [],
    enabled: true,
  };
  const second = { ...first, app: "teams.exe" };
  const firstMacro = {
    trigger: "signature",
    expansion: "Personal signature",
    app: "discord.exe",
    enabled: true,
  };
  const secondMacro = {
    ...firstMacro,
    expansion: "Work signature",
    app: "teams.exe",
  };
  const bridge = await browser({
    dictionary: [first, second],
    macros: [firstMacro, secondMacro],
  });
  await bridge.action({
    type: "dictionary_delete",
    index: 0,
    expected_heard: first.heard,
    expected_entry: first,
  });
  await assert.rejects(
    bridge.action({
      type: "dictionary_delete",
      index: 0,
      expected_heard: first.heard,
      expected_entry: first,
    }),
    /Vocabulary changed/,
  );
  await assert.rejects(
    bridge.action({
      type: "dictionary_save",
      index: 0,
      expected_heard: first.heard,
      expected_entry: first,
      heard: first.heard,
      wanted: "Incorrect scope",
    }),
    /Vocabulary changed/,
  );
  await bridge.action({
    type: "macro_delete",
    index: 0,
    expected_trigger: firstMacro.trigger,
    expected_macro: firstMacro,
  });
  await assert.rejects(
    bridge.action({
      type: "macro_delete",
      index: 0,
      expected_trigger: firstMacro.trigger,
      expected_macro: firstMacro,
    }),
    /Snippets changed/,
  );
  await assert.rejects(
    bridge.action({
      type: "macro_save",
      index: 0,
      expected_trigger: firstMacro.trigger,
      expected_macro: firstMacro,
      trigger: firstMacro.trigger,
      expansion: "Incorrect scope",
    }),
    /Snippets changed/,
  );
  const saved = await bridge.snapshot();
  assert.deepEqual(saved.dictionary, [second]);
  assert.deepEqual(saved.macros, [secondMacro]);
});

test("a rejected asynchronous clipboard request cannot roll back newer edits", async () => {
  const bridge = await browser();
  let rejectCopy;
  Object.defineProperty(globalThis, "navigator", {
    configurable: true,
    value: {
      clipboard: {
        writeText: () =>
          new Promise((_, reject) => {
            rejectCopy = reject;
          }),
      },
    },
  });
  const copying = bridge.action({ type: "copy", text: "Synthetic text" });
  const first = (await bridge.snapshot()).history[0];
  await bridge.action({
    type: "session_patch",
    id: first.id,
    personal_notes: "A newer change",
  });
  rejectCopy(new Error("Synthetic clipboard failure"));
  await assert.rejects(copying, /clipboard failure/);
  assert.equal(
    (await bridge.snapshot()).history[0].personal_notes,
    "A newer change",
  );
  delete globalThis.navigator;
});

test("invalid document title leaves both title and notes intact", async () => {
  const bridge = await browser();
  const first = (await bridge.snapshot()).history[0];
  await assert.rejects(
    bridge.action({
      type: "session_patch",
      id: first.id,
      title: "  ",
      personal_notes: "Changed draft",
    }),
    /title from 1 to 160/,
  );
  const saved = (await bridge.snapshot()).history[0];
  assert.equal(saved.title, first.title);
  assert.equal(saved.personal_notes, first.personal_notes);
});

test("browser recording commands never claim to capture audio", async () => {
  const bridge = await browser();
  await assert.rejects(
    bridge.action({ type: "call_start" }),
    /Open the desktop app/,
  );
  await assert.rejects(
    bridge.action({ type: "dictation_start" }),
    /Open the desktop app/,
  );
  await assert.rejects(
    bridge.action({ type: "note_capture_start" }),
    /Open the desktop app/,
  );
  const state = await bridge.snapshot();
  assert.equal(state.recording, false);
  assert.equal(state.call_recording, false);
  assert.equal(state.note_recording, false);
});

test("source moves require an explicit original topic and preserve null when detaching", async () => {
  const bridge = await browser();
  assert.deepEqual(
    JSON.parse(
      JSON.stringify(bridge.sourceMoveAction("source-a", null, "topic-a")),
    ),
    {
      type: "source_move",
      id: "source-a",
      topic_id: null,
      expected_topic_id: "topic-a",
    },
  );
  assert.deepEqual(bridge.sourceMoveAction("source-a", "topic-b", null), {
    type: "source_move",
    id: "source-a",
    topic_id: "topic-b",
    expected_topic_id: null,
  });
  for (const args of [
    ["", "topic-b", null],
    ["source-a", "topic-b"],
    ["source-a", "", null],
    ["source-a", "source-a", null],
  ])
    assert.throws(() => bridge.sourceMoveAction(...args));
});

test("native source moves send the expected topic instead of resolving a newer snapshot", async () => {
  const received = [];
  globalThis.isTauri = true;
  globalThis.window = {
    __TAURI_INTERNALS__: {
      invoke: async (command, payload) => {
        received.push({
          command,
          payload: JSON.parse(JSON.stringify(payload)),
        });
      },
    },
  };
  try {
    const bridge = await import(`../src/bridge.js?test=${++moduleId}`);
    await bridge.action(
      bridge.sourceMoveAction("source-a", "topic-b", "topic-a"),
    );
    await bridge.action(bridge.sourceMoveAction("source-a", null, "topic-b"));
    assert.deepEqual(received, [
      {
        command: "desktop_action",
        payload: {
          action: {
            type: "source_move",
            id: "source-a",
            topic_id: "topic-b",
            expected_topic_id: "topic-a",
          },
        },
      },
      {
        command: "desktop_action",
        payload: {
          action: {
            type: "source_move",
            id: "source-a",
            topic_id: null,
            expected_topic_id: "topic-b",
          },
        },
      },
    ]);
  } finally {
    delete globalThis.window;
    globalThis.isTauri = false;
  }
});

test("native transport failures never fall back to synthetic documents", async () => {
  globalThis.isTauri = true;
  globalThis.window = {
    __TAURI_INTERNALS__: {
      invoke: async () => {
        throw new Error("Synthetic native disconnect");
      },
    },
  };
  const bridge = await import(`../src/bridge.js?test=${++moduleId}`);
  assert.equal(bridge.native, true);
  await assert.rejects(bridge.snapshot(), /native disconnect/);
  await assert.rejects(
    bridge.action({ type: "note_new" }),
    /native disconnect/,
  );
  delete globalThis.window;
  globalThis.isTauri = false;
});

test("moving preview sources preserves originals and manual notes and rejects stale membership", async () => {
  const bridge = await browser();
  await bridge.action({ type: "history_open", id: "sample-topic-release" });
  const topic = (await bridge.snapshot()).selected;
  const source = topic.sources[0];
  await bridge.action(
    bridge.sourceMoveAction(source.id, "sample-note-1", topic.id),
  );
  const remaining = (await bridge.snapshot()).selected;
  assert.equal(remaining.sources.length, 1);
  assert.equal(remaining.personal_notes, topic.personal_notes);
  await bridge.action({ type: "history_open", id: source.id });
  const moved = (await bridge.snapshot()).selected;
  assert.equal(moved.text, source.text);
  assert.equal(moved.topic_id, "sample-note-1");
  await assert.rejects(
    bridge.action(bridge.sourceMoveAction(source.id, null, topic.id)),
    /already moved/,
  );
  bridge.storage.fail = true;
  await assert.rejects(
    bridge.action(bridge.sourceMoveAction(source.id, null, "sample-note-1")),
    /storage failure/,
  );
  assert.equal((await bridge.snapshot()).selected.topic_id, "sample-note-1");
});
