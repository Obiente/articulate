import test from "node:test";
import assert from "node:assert/strict";
import { prepareAction } from "../src/actions.js";
import { createPreviewWorkspace } from "../src/preview/workspace.js";

function workspace(initial) {
  const values = new Map();
  if (initial)
    values.set("articulate-ui-sample-workspace-v1", JSON.stringify(initial));
  const storage = {
    fail: false,
    getItem: (key) => values.get(key) ?? null,
    setItem(key, value) {
      if (this.fail) throw Error("Synthetic storage failure");
      values.set(key, value);
    },
  };
  return { storage, current: createPreviewWorkspace(storage) };
}

test("sound controls validate booleans and leave old save actions valid", () => {
  for (const value of ["true", 1, null]) {
    assert.throws(() =>
      prepareAction({ type: "dictionary_sound_alike", enabled: value }),
    );
    assert.throws(() =>
      prepareAction({
        type: "dictionary_save",
        heard: "obi ante",
        wanted: "Obiente",
        sound_alike: value,
      }),
    );
  }
  assert.doesNotThrow(() =>
    prepareAction({
      type: "dictionary_save",
      heard: "obi ante",
      wanted: "Obiente",
    }),
  );
  assert.doesNotThrow(() =>
    prepareAction({ type: "dictionary_sound_alike", enabled: false }),
  );
});

test("global and entry sound choices survive reload independently", async () => {
  const { current, storage } = workspace({ dictionary: [] });
  assert.equal(current.snapshot().vocabulary_sound_alike, true);
  await current.action(
    prepareAction({
      type: "dictionary_save",
      heard: "obi ante",
      wanted: "Obiente",
      sound_alike: false,
    }),
  );
  await current.action(
    prepareAction({ type: "dictionary_sound_alike", enabled: false }),
  );
  let restored = createPreviewWorkspace(storage).snapshot();
  assert.equal(restored.vocabulary_sound_alike, false);
  assert.equal(restored.dictionary[0].sound_alike, false);
  await current.action(
    prepareAction({ type: "dictionary_sound_alike", enabled: true }),
  );
  restored = createPreviewWorkspace(storage).snapshot();
  assert.equal(restored.vocabulary_sound_alike, true);
  assert.equal(restored.dictionary[0].sound_alike, false);
});

test("entry edits preserve sound choice and reject a stale toggle", async () => {
  const { current } = workspace({
    dictionary: [
      {
        heard: "obi ante",
        wanted: "Obiente",
        sound_alike: false,
        app: null,
        cues: [],
        enabled: true,
      },
    ],
  });
  const original = current.snapshot().dictionary[0];
  const edit = {
    type: "dictionary_save",
    heard: original.heard,
    wanted: original.wanted,
    index: 0,
    expected_heard: original.heard,
    expected_entry: original,
  };
  await current.action(prepareAction({ ...edit, cues: "website" }));
  assert.equal(current.snapshot().dictionary[0].sound_alike, false);
  await assert.rejects(
    current.action(prepareAction({ ...edit, sound_alike: true })),
    /Vocabulary changed/,
  );
});

test("failed sound preference save restores visible state", async () => {
  const { current, storage } = workspace();
  storage.fail = true;
  await assert.rejects(
    current.action(
      prepareAction({ type: "dictionary_sound_alike", enabled: false }),
    ),
    /Synthetic storage failure/,
  );
  assert.equal(current.snapshot().vocabulary_sound_alike, true);
});
