// SPDX-License-Identifier: AGPL-3.0-or-later
"use strict";
const assert = require("node:assert/strict");
const addon = require(process.argv[2]);
assert.deepEqual(Object.keys(addon).sort(), ["arm", "poll", "start", "stats", "stop"]);
assert.equal(addon.start(), 1); // This isolated Node process has no Discord module.
assert.equal(addon.arm(1n, [123n]), 1);
assert.equal(addon.arm("1", [123n]), 1);
assert.equal(addon.poll().length, 0);
assert.equal(addon.stop(), 0);
assert.deepEqual(addon.stats(), { produced: 0, polled: 0, queue_full: 0, contention: 0,
    configuration_discard: 0, stop_flush: 0, max_depth: 0, queue_depth: 0, queue_capacity: 128 });
console.log("Node-API addon loads and refuses to hook when Discord is absent.");
