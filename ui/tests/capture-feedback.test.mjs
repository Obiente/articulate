import test from "node:test";
import assert from "node:assert/strict";
import { createCaptureFeedbackTracker } from "../src/capture-feedback.js";

const event = (sequence, type = "started", kind = "call") => ({
  sequence,
  event: type,
  kind,
  session_id: "synthetic-call",
  status: "Native capture status",
});

test("capture feedback waits for a lifecycle acknowledgement and deduplicates polling", () => {
  const observe = createCaptureFeedbackTracker();
  assert.equal(observe(undefined), null);
  assert.equal(observe({ sequence: 0, event: null, kind: null }), null);
  const started = observe(event(1));
  assert.equal(started.title, "Call recording started");
  assert.equal(started.sessionId, "synthetic-call");
  assert.equal(observe(event(1)), null);
  assert.equal(observe(event(0)), null);
  const stopped = observe(event(2, "stopped"));
  assert.equal(stopped.title, "Call recording stopped");
  assert.match(stopped.message, /Finishing the transcript/);
  assert.doesNotMatch(stopped.message, /saved/i);
});

test("opening a renderer during an existing recording never replays its start cue", () => {
  const observe = createCaptureFeedbackTracker();
  assert.equal(observe(event(10)), null);
  assert.equal(observe(event(10)), null);
  assert.equal(observe(event(11, "stopped")).event, "stopped");
});

test("ordinary dictation events do not duplicate their existing feedback", () => {
  const observe = createCaptureFeedbackTracker();
  observe({ sequence: 0 });
  assert.equal(observe(event(1, "started", "dictation")), null);
  assert.equal(observe(event(2, "stopped", "dictation")), null);
  assert.equal(
    observe(event(3, "started", "note")).title,
    "Spoken note started",
  );
});

test("failed existing capture reports attention without claiming that recording started", () => {
  const observe = createCaptureFeedbackTracker();
  observe({ sequence: 0 });
  const failed = observe({
    ...event(1, "failed"),
    status: "The microphone is unavailable.",
  });
  assert.equal(failed.title, "Call recording needs attention");
  assert.equal(failed.message, "The microphone is unavailable.");
  assert.equal(failed.sessionId, "synthetic-call");
  assert.equal(observe(event(1, "failed")), null);
  assert.equal(observe({ ...event(2), sequence: Number.NaN }), null);
});

test("pre-capture failure consumes its sequence without duplicating the command error", () => {
  const observe = createCaptureFeedbackTracker();
  observe({ sequence: 0 });
  assert.equal(observe({ ...event(1, "failed"), session_id: null }), null);
  assert.equal(observe(event(1, "failed")), null);
  assert.equal(observe(event(2)).title, "Call recording started");
  assert.equal(observe({ ...event(3, "failed"), session_id: undefined }), null);
  assert.equal(observe(event(3, "failed")), null);
  assert.equal(observe(event(4, "failed")).sessionId, "synthetic-call");
});
