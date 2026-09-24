import test from "node:test";
import assert from "node:assert/strict";
import {
  createUpdateNoticeTracker,
  updateNotices,
} from "../src/update-notices.js";

test("app and companion updates appear together and advance through their stages", () => {
  const state = {
    updater: { state: "available", version: "0.3.0" },
    discord: {
      companion: {
        runtime_status: "update_required",
        plugin_status: "update_available",
      },
    },
  };
  assert.deepEqual(
    updateNotices(state).map((notice) => notice.id),
    ["app-available-0.3.0", "companion-update-available"],
  );
  state.updater.state = "ready";
  state.discord.companion.runtime_status = "restart_required";
  assert.deepEqual(
    updateNotices(state).map((notice) => notice.id),
    ["app-ready-0.3.0", "companion-restart-required"],
  );
});

test("polling does not repeat toasts and a later retry can notify again", () => {
  const track = createUpdateNoticeTracker();
  const notice = updateNotices({
    updater: { state: "error", error: "Offline" },
  });
  assert.equal(track(notice).length, 1);
  assert.equal(track(notice).length, 0);
  assert.equal(track([]).length, 0);
  assert.equal(track(notice).length, 1);
});

test("quiet and preview states do not claim an update", () => {
  assert.deepEqual(updateNotices(null), []);
  assert.deepEqual(updateNotices({ updater: { state: "latest" } }), []);
  assert.deepEqual(
    updateNotices({
      preview: true,
      updater: { state: "available", version: "0.3.0" },
    }),
    [],
  );
  assert.deepEqual(
    updateNotices({
      discord: { companion: { busy: true, plugin_status: "update_available" } },
    }),
    [],
  );
});
