import test from "node:test";
import assert from "node:assert/strict";
import {
  nameSuggestions,
  playingRowIndex,
  rosterOf,
  speakerKind,
  transcriptText,
  withInterruptions,
} from "../src/features/calls/speakers.js";

const row = (start_ms, speaker, key, text = "Words.") => ({
  start_ms,
  end_ms: start_ms + 1000,
  speaker,
  speaker_labels: [speaker],
  speaker_keys: [key],
  text,
});

test("the roster lists each speaker once in order with turn counts", () => {
  const overlap = {
    ...row(4000, "Overlap: Speaker 1 + Zoë", "s1"),
    speaker_labels: ["Speaker 1", "Zoë"],
    speaker_keys: ["s1", "n:abc123"],
  };
  const roster = rosterOf([
    row(0, "Speaker 1", "s1"),
    row(2000, "You", "you"),
    overlap,
    row(6000, "Other audio", "other"),
  ]);
  assert.deepEqual(
    roster.map((speaker) => [speaker.key, speaker.turns, speaker.renamable]),
    [
      ["s1", 2, true],
      ["you", 1, false],
      ["n:abc123", 1, true],
      ["other", 1, false],
    ],
  );
  assert.equal(roster[2].label, "Zoë");
  assert.equal(speakerKind("uncertain"), "other");
  assert.equal(speakerKind("s9"), "person");
});

test("rows from older snapshots without keys still form a roster", () => {
  const legacy = { ...row(0, "Casey", "s1") };
  delete legacy.speaker_keys;
  const [speaker] = rosterOf([legacy]);
  assert.equal(speaker.key, "label:Casey");
  assert.equal(speaker.renamable, false);
});

test("interruption markers sit between turns by time", () => {
  const rows = [row(0, "A", "s1"), row(10000, "B", "s2")];
  const gaps = [
    { start_ms: 20000, end_ms: 21000, message: "Late" },
    { start_ms: 5000, end_ms: 8000, message: "Audio interrupted" },
  ];
  assert.deepEqual(
    withInterruptions(rows, gaps).map((item) =>
      item.type === "gap" ? item.gap.message : item.row.speaker,
    ),
    ["A", "Audio interrupted", "B", "Late"],
  );
  assert.equal(withInterruptions(rows)[1].index, 1);
  assert.equal(
    transcriptText(rows, "", gaps),
    "[00:00] A: Words.\n\n[Audio interrupted]\n\n[00:10] B: Words.\n\n[Late]",
  );
  assert.equal(transcriptText([], "Fallback", gaps), "Fallback");
});

test("playback highlights the last turn that has started", () => {
  const rows = [row(0, "A", "s1"), row(5000, "B", "s2"), row(9000, "A", "s1")];
  assert.equal(playingRowIndex(rows, 6000), 1);
  assert.equal(playingRowIndex(rows, 9000), 2);
  assert.equal(playingRowIndex([row(1000, "A", "s1")], 500), -1);
  assert.equal(playingRowIndex(rows, undefined), -1);
});

test("rename suggestions skip names already used by other speakers", () => {
  const people = [{ name: "Morgan" }, { name: "Casey" }, { name: "casey" }];
  const roster = [{ label: "Casey" }, { label: "Speaker 2" }];
  assert.deepEqual(nameSuggestions(people, roster, "Speaker 2"), ["Morgan"]);
  assert.deepEqual(nameSuggestions(people, roster, "Casey"), [
    "Morgan",
    "Casey",
  ]);
});

test("speaker names are validated like the desktop controller", async () => {
  const { validSpeakerName, prepareAction } = await import("../src/actions.js");
  assert.equal(validSpeakerName("  Zoë   Li "), "Zoë Li");
  for (const bad of ["", "   ", "You", "other audio", "x".repeat(81)])
    assert.throws(() => validSpeakerName(bad));
  assert.throws(() =>
    prepareAction({ type: "speaker_reassign", id: "a", rows: [] }),
  );
  assert.throws(() =>
    prepareAction({
      type: "speaker_reassign",
      id: "a",
      rows: [{ index: 0, start_ms: 0 }],
      speaker: "s1",
      name: "Casey",
    }),
  );
});

test("the preview workspace corrects speakers and undoes changes", async () => {
  const { createPreviewWorkspace } = await import(
    "../src/preview/workspace.js"
  );
  const storage = new Map();
  const workspace = createPreviewWorkspace({
    getItem: (key) => storage.get(key) ?? null,
    setItem: (key, value) => storage.set(key, value),
  });
  const id = "sample-call-1";
  await workspace.action({ type: "history_open", id });
  const rows = () => workspace.snapshot().selected.rows;
  assert.equal(rows()[4].speaker, "Speaker 3");
  await workspace.action({ type: "speaker_merge", id, from: "s3", into: "s1" });
  assert.equal(rows()[4].speaker, "Casey");
  assert.equal(
    workspace.snapshot().selected.speaker_undo,
    "Merged Speaker 3 into Casey",
  );
  await workspace.action({
    type: "speaker_reassign",
    id,
    rows: [{ index: 1, start_ms: 33000 }],
    name: "Morgan",
  });
  assert.equal(rows()[1].speaker, "Morgan");
  await workspace.action({ type: "speaker_undo", id });
  await workspace.action({ type: "speaker_undo", id });
  assert.deepEqual(
    rows().map((row) => row.speaker),
    ["Casey", "Jordan", "You", "Casey", "Speaker 3"],
  );
  await assert.rejects(workspace.action({ type: "speaker_undo", id }));
});
