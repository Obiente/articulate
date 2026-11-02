import assert from "node:assert/strict";
import test from "node:test";
import {
  createMeetingPromptTracker,
  meetingIndicator,
  meetingPrompt,
} from "../src/features/meetings/meeting-notices.js";
import { prepareAction } from "../src/actions.js";

const teams = { id: "teams", label: "Microsoft Teams", best_effort: false };
const meet = { id: "meet", label: "Google Meet", best_effort: true };

test("a prompt carries the app, the best-effort flag and the consent reminder", () => {
  assert.equal(meetingPrompt(null), null);
  assert.equal(meetingPrompt({ meetings: { prompt: null } }), null);
  const prompt = meetingPrompt({
    meetings: { prompt: meet, consent_reminder: "Tell everyone." },
  });
  assert.deepEqual(prompt, {
    id: "meeting-prompt-meet",
    app: "meet",
    label: "Google Meet",
    bestEffort: true,
    consent: "Tell everyone.",
  });
});

test("the tracker shows each detected meeting once and hides it when cleared", () => {
  const track = createMeetingPromptTracker();
  const first = meetingPrompt({ meetings: { prompt: teams } });
  assert.deepEqual(track(first), { show: first, hide: null });
  assert.deepEqual(track(first), { show: null, hide: null });
  const second = meetingPrompt({ meetings: { prompt: meet } });
  assert.deepEqual(track(second), {
    show: second,
    hide: "meeting-prompt-teams",
  });
  assert.deepEqual(track(null), { show: null, hide: "meeting-prompt-meet" });
  assert.deepEqual(track(null), { show: null, hide: null });
});

test("the indicator follows the meeting detection owns", () => {
  assert.equal(meetingIndicator({ meetings: { owned: null } }), null);
  assert.deepEqual(
    meetingIndicator({
      meetings: { owned: teams, status: "Transcribing the meeting." },
    }),
    {
      app: "teams",
      label: "Microsoft Teams",
      title: "Transcribing the Microsoft Teams meeting",
      status: "Transcribing the meeting.",
    },
  );
});

test("meeting and Discord settings are validated before either transport", () => {
  assert.ok(prepareAction({ type: "meeting_respond", accept: true }));
  assert.throws(() =>
    prepareAction({ type: "meeting_respond", accept: "yes" }),
  );
  assert.ok(prepareAction({ type: "settings_patch", discord_flavor: "ptb" }));
  assert.ok(prepareAction({ type: "settings_patch", discord_flavor: "" }));
  assert.throws(() =>
    prepareAction({ type: "settings_patch", discord_flavor: "beta" }),
  );
  assert.ok(prepareAction({ type: "settings_patch", meetings_start: "ask" }));
  assert.throws(() =>
    prepareAction({ type: "settings_patch", meetings_start: "always" }),
  );
  for (const type of [
    "discord_relaunch_normally",
    "discord_rotate_key",
    "discord_forget_pairing",
  ]) {
    assert.ok(prepareAction({ type }));
  }
});
