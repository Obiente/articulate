// SPDX-License-Identifier: AGPL-3.0-or-later
"use strict";
const fs = require("node:fs"), vm = require("node:vm"), assert = require("node:assert/strict");
const { join } = require("node:path"), { EventEmitter } = require("node:events");
let api, armed, stopped = 0, posts = 0, active = false;
let captureId = "7", deferAudio = false, finishAudio;
let batchAdvertised = true, audioStatus = 204, diagnosticsStatus = 204;
let clockMs = 10000, controlStatus = 200, deferControl = false, finishControl;
let earlyAudioResponse = false, finishUpload, armCalls = 0;
const scheduled = new Map(); let unrefs = 0, nodeTimeouts = 0;
const diagnosticReports = [];
const nodeTimers = { setInterval(callback, ms) { scheduled.set(ms, callback); return { unref() { unrefs++; } }; },
    setTimeout(...args) { nodeTimeouts++; return setTimeout(...args); }, clearTimeout };
const forbiddenBrowserTimer = () => { throw new Error("Browser timers are throttled and must not own transport"); };
const receivedTags = [], sentBodies = [], batchCounts = [];
const queue = [];
const addon = { start: () => 0, arm: (nonce, ids) => { armCalls++; armed = { nonce, ids }; return 0; },
    stop: () => { stopped++; }, poll: () => queue.shift() ?? Buffer.alloc(0) };
const source = fs.readFileSync(join(__dirname, "preload.cjs"), "utf8");
const context = { Buffer, BigInt, Set, Date, JSON, Error, setTimeout: forbiddenBrowserTimer, clearTimeout: forbiddenBrowserTimer,
    performance: { now: () => clockMs },
    setInterval: forbiddenBrowserTimer, process: { once() {} }, __dirname,
    require(name) {
        if (name === "electron/renderer") return { contextBridge: { exposeInMainWorld(name, value) {
            assert.equal(name, "ArticulateNativeAudio"); api = value;
        } } };
        if (name === "node:path") return require(name);
        if (name === "node:timers") return nodeTimers;
        if (name.endsWith("articulate_discord_audio.node")) return addon;
        assert.equal(name, "node:http");
        return { request(options, callback) {
            assert.equal(options.hostname, "127.0.0.1"); assert.equal(options.port, 9223);
            assert.equal(options.headers.Authorization, "Bearer " + "0".repeat(64));
            const req = new EventEmitter(); req.destroy = () => req.emit("close");
            req.end = body => {
                const expectedNonce = BigInt(captureId);
                const controlValue = { active, pcm_batch: batchAdvertised, capture_id: captureId, channel_id: "123", participants: ["456"] };
                const respond = status => {
                const res = new EventEmitter(); res.statusCode = status;
                callback(res);
                if (options.path === "/capture") {
                    assert.equal(options.method, "GET");
                    res.emit("data", Buffer.from(JSON.stringify(controlValue)));
                } else if (options.path === "/diagnostics") {
                    assert.equal(options.method, "POST");
                    assert.equal(options.headers["Content-Type"], "application/json");
                    diagnosticReports.push(JSON.parse(body.toString("utf8")));
                } else {
                    const packets = [];
                    if (options.path === "/pcm-batch") {
                        assert(body.length <= 128 * 1024);
                        let offset = 0;
                        while (offset < body.length) {
                            const length = body.readUInt32LE(offset); offset += 4;
                            assert(length >= 64 && length <= 23104 && offset + length <= body.length);
                            packets.push(body.subarray(offset, offset + length)); offset += length;
                        }
                        assert(packets.length <= 64);
                    } else { assert.equal(options.path, "/pcm"); packets.push(body); }
                    for (const packet of packets) {
                        assert.equal(packet.readBigUInt64LE(52), expectedNonce);
                        receivedTags.push(packet[64]);
                    }
                    sentBodies.push(body); batchCounts.push(packets.length); posts++;
                }
                res.emit("end");
                if (earlyAudioResponse && options.path.startsWith("/pcm")) {
                    finishUpload = () => { req.emit("finish"); req.emit("close"); };
                } else req.emit("close");
                };
                if (options.path.startsWith("/pcm") && deferAudio) finishAudio = respond;
                else if (options.path === "/capture" && deferControl) finishControl = respond;
                else respond(options.path === "/capture" ? controlStatus : options.path === "/diagnostics" ? diagnosticsStatus : audioStatus);
            };
            return req;
        } };
    }
};
// Hostile browser timers simulate a hidden renderer. Scheduling and request
// deadlines must exclusively use the Node timer module.
vm.runInNewContext(source + ";globalThis.harness = { control, drain, publishDiagnostics };", context);
assert.equal(unrefs, 3);
assert.deepEqual([...scheduled.keys()], [250, 20, 5000]);
assert.equal(typeof api.enable, "function", "Hidden renderers still expose the audio bridge");
// Unresolved mock promises must fail instead of allowing a silent successful exit.
process.exitCode = 1;
(async () => {
    assert(api.enable("0".repeat(64)));
    // The adapter must advertise readiness before Articulate arms capture.
    // Waiting for a first audio packet here would deadlock automatic starts.
    await context.harness.control();
    assert.equal(api.status().state, "ready");
    assert.equal(armed, undefined); assert.equal(posts, 0);
    active = true;
    await context.harness.control();
    assert.equal(armed.nonce, 7n); assert.equal(armed.ids[0], 456n);
    assert.equal(api.status().state, "capturing");
    const packet = Buffer.alloc(66); packet.writeBigUInt64LE(7n, 52); queue.push(packet);
    await context.harness.drain(); assert.equal(posts, 1); assert(packet.every(byte => byte === 0));
    const frame = (size, tag) => { const packet = Buffer.alloc(size); packet.writeBigUInt64LE(7n, 52); packet[64] = tag; return packet; };
    // Six 10ms participant callbacks overflow a 128-frame native queue when
    // Chromium coalesces a background drain to one second. Node's 20ms drain
    // consumes all 600 frames with the exact same producer schedule.
    let backgroundDepth = 0, backgroundDrops = 0, nodeDrops = 0;
    const beforePaced = receivedTags.length;
    for (let tick = 1; tick <= 100; tick++) {
        clockMs += 10;
        for (let speaker = 0; speaker < 6; speaker++) {
            if (backgroundDepth === 128) backgroundDrops++; else backgroundDepth++;
            if (queue.length === 128) nodeDrops++; else queue.push(frame(66, speaker));
        }
        if (tick % 25 === 0) scheduled.get(250)();
        if (tick % 2 === 0) scheduled.get(20)();
        await new Promise(resolve => setImmediate(resolve));
    }
    assert.equal(backgroundDrops, 472, "A one-second background timer loses 472 of 600 callbacks");
    assert.equal(nodeDrops, 0);
    assert.equal(receivedTags.length - beforePaced, 600);
    assert(nodeTimeouts > 0, "HTTP deadlines use the Node timer module too");
    const beforeBurst = posts;
    const frames = Array.from({ length: 100 }, (_, index) => frame(66, index));
    queue.push(...frames);
    await context.harness.drain();
    assert.equal(posts - beforeBurst, 2, "100 frames use two ordered requests, not 100 TCP connections");
    assert.deepEqual(batchCounts.slice(-2), [64, 36]);
    assert.deepEqual(receivedTags.slice(-100), Array.from({ length: 100 }, (_, index) => index));
    assert(frames.every(packet => packet.every(byte => byte === 0)));
    const large = Array.from({ length: 8 }, (_, index) => frame(23104, index + 100));
    queue.push(...large);
    await context.harness.drain();
    assert.deepEqual(batchCounts.slice(-2), [5, 3], "Byte bound retains overflow frame for next batch");
    assert.deepEqual(receivedTags.slice(-8), Array.from({ length: 8 }, (_, index) => index + 100));
    assert(large.every(packet => packet.every(byte => byte === 0)));
    assert(sentBodies.every(body => body.every(byte => byte === 0)));
    // An older receiver without advertised batching keeps the original route.
    batchAdvertised = false;
    await context.harness.control();
    queue.push(frame(66, 120), frame(66, 121));
    const beforeLegacy = posts;
    await context.harness.drain();
    assert.equal(posts - beforeLegacy, 2);
    assert.deepEqual(receivedTags.slice(-2), [120, 121]);
    batchAdvertised = true;
    await context.harness.control();
    // Do not erase queued upload buffers when an HTTP response arrives before
    // Node has finished writing them to the socket.
    earlyAudioResponse = true;
    queue.push(frame(66, 122));
    const earlyDrain = context.harness.drain();
    await Promise.resolve();
    const earlyBody = sentBodies.at(-1);
    assert(earlyBody.some(byte => byte !== 0), "An in-flight upload must retain its PCM bytes");
    finishUpload(); await earlyDrain;
    assert(earlyBody.every(byte => byte === 0));
    earlyAudioResponse = false;

    // One failed control poll must not reset an otherwise live native stream.
    const stopsBeforeRetry = stopped, armsBeforeRetry = armCalls;
    controlStatus = 503; clockMs += 500;
    await context.harness.control();
    assert.equal(stopped, stopsBeforeRetry);
    assert.equal(armCalls, armsBeforeRetry, "Failure must not extend native lease");
    queue.push(frame(66, 123)); await context.harness.drain();
    assert.equal(receivedTags.at(-1), 123);
    controlStatus = 200; clockMs += 200;
    await context.harness.control();
    assert.equal(stopped, stopsBeforeRetry);
    assert.equal(armed.nonce, 7n);
    assert.equal(api.status().state, "capturing");

    // A real outage stops within the bounded grace. A paused old response must
    // not rearm capture after the drain watchdog has already stopped it.
    deferControl = true;
    const pausedControl = context.harness.control();
    clockMs += 1201;
    await context.harness.drain();
    assert.equal(stopped, stopsBeforeRetry + 1);
    const armsAfterExpiry = armCalls;
    finishControl(200); await pausedControl;
    assert.equal(armCalls, armsAfterExpiry);
    assert.equal(api.status().state, "waiting-for-articulate");
    deferControl = false;
    await context.harness.control();
    assert.equal(api.status().state, "capturing");
    // Stop/rearm can keep the same nonce. Both failed and successful responses
    // from the old grant must leave the new grant and its queued audio alone.
    for (const status of [400, 204]) {
        queue.push(frame(66, 124)); deferAudio = true;
        const oldDrain = context.harness.drain();
        clockMs += 1201;
        await context.harness.control(); // Expires old grant, then rearms nonce 7.
        const stopsAfterRearm = stopped;
        const freshFrame = frame(66, 125); queue.push(freshFrame);
        finishAudio(status); await oldDrain;
        assert.equal(stopped, stopsAfterRearm, "An old POST cannot revoke a resumed same-nonce grant");
        assert.equal(api.status().state, "capturing");
        assert.equal(queue.length, 1, "An old drain cannot poll the resumed stream");
        deferAudio = false; await context.harness.drain();
        assert.equal(receivedTags.at(-1), 125);
    }
    // A response from the prior capture must not stop a newly armed call.
    const stalePacket = Buffer.alloc(66); stalePacket.writeBigUInt64LE(7n, 52); queue.push(stalePacket);
    deferAudio = true;
    const draining = context.harness.drain();
    captureId = "8";
    await context.harness.control();
    assert.equal(armed.nonce, 8n);
    const stopsBeforeStaleReply = stopped;
    finishAudio(400);
    await draining;
    assert.equal(stopped, stopsBeforeStaleReply);
    assert.equal(api.status().state, "capturing");
    assert(stalePacket.every(byte => byte === 0));
    diagnosticsStatus = 503;
    await context.harness.publishDiagnostics();
    assert.equal(api.status().state, "capturing", "Diagnostics failures cannot affect audio capture");
    diagnosticsStatus = 204;
    // Real transport failure publishes a numeric report without waiting for the
    // periodic timer; it must never include packet bytes or capture identity.
    deferAudio = false; audioStatus = 400;
    const failedFrame = Buffer.alloc(66); failedFrame.writeBigUInt64LE(8n, 52); queue.push(failedFrame);
    await context.harness.drain();
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(api.status().state, "audio-transport-unavailable");
    assert.equal(diagnosticReports.at(-1).transport.audio_failures, 1);
    audioStatus = 204; await context.harness.control();
    // Diagnostics contain only allowlisted bounded aggregate numbers. A hostile
    // addon cannot smuggle its private fields into the authenticated report.
    addon.stats = () => ({ produced: 600, polled: 590, queue_full: 10, contention: NaN,
        configuration_discard: -5, stop_flush: Infinity, max_depth: 128, queue_depth: 2,
        queue_capacity: Number.MAX_SAFE_INTEGER + 100, user_id: "private", nonce: "private", audio: "private" });
    await context.harness.publishDiagnostics();
    const report = diagnosticReports.at(-1);
    assert.deepEqual(Object.keys(report).sort(), ["native", "transport", "version"]);
    assert.deepEqual(Object.keys(report.native).sort(), ["produced", "polled", "queue_full", "contention",
        "configuration_discard", "stop_flush", "max_depth", "queue_depth", "queue_capacity"].sort());
    assert.deepEqual(Object.keys(report.transport).sort(), ["control_failures", "audio_failures", "requests",
        "packets", "max_control_gap_ms", "max_drain_gap_ms", "max_request_ms"].sort());
    assert(Object.values(report.native).every(value => Number.isSafeInteger(value) && value >= 0));
    assert(Object.values(report.transport).every(value => Number.isSafeInteger(value) && value >= 0));
    assert.equal(report.native.queue_full, 10);
    assert.equal(report.transport.control_failures, 1);
    assert(report.transport.packets >= 600);
    delete addon.stats;
    assert.equal(api.diagnostics().native, undefined, "Older addons remain compatible");
    active = false;
    const beforeInactive = stopped;
    await context.harness.control(); assert.equal(api.status().state, "ready");
    assert.equal(stopped, beforeInactive + 1, "An explicit inactive control stops immediately");
    active = true; deferControl = true;
    const beforeDisableReply = context.harness.control();
    const beforeDisableArms = armCalls;
    api.disable();
    finishControl(200); await beforeDisableReply;
    assert.equal(armCalls, beforeDisableArms, "A paused GET cannot undo explicit disable");
    assert.equal(api.status().state, "disabled"); assert(stopped >= 3);
    console.log("Renderer control, PCM, nonce and stopping pass with mocked localhost.");
    process.exitCode = 0;
})().catch(error => { console.error(error); process.exitCode = 1; });
