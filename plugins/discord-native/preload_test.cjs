// SPDX-License-Identifier: AGPL-3.0-or-later
"use strict";
const fs = require("node:fs"), vm = require("node:vm"), assert = require("node:assert/strict");
const { join } = require("node:path"), { EventEmitter } = require("node:events");
const { createHmac, randomBytes, timingSafeEqual } = require("node:crypto");
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
// Mock receiver speaking protocol version 2 with the pairing key below.
const pairingKey = Buffer.from("0".repeat(64), "hex"), wrongKey = Buffer.from("1".repeat(64), "hex");
let serverKey = pairingKey, handshakes = 0, confirmations = 0, serverSession = null, pendingProof = null;
let forgetSession = false, transientFailures = 0, upgradeRequired = false, unauthenticatedBodies = 0;
let agentInstances = 0, agentUsed = 0;
// The connection the keep-alive agent hands out, and the one that completed
// the receiver's handshake. Replacing the listener (Articulate restarted, or
// another process bound the port) yields a new socket object whose server-side
// session state starts empty, exactly like a real new TCP connection.
let socketSerial = 0, connection = { id: ++socketSerial }, handshakeSocket = null;
const replaceListener = () => { connection = { id: ++socketSerial }; pendingProof = null; serverSession = null; };
class Agent { constructor(options) { agentInstances++; assert.equal(options.keepAlive, true); assert.equal(options.maxSockets, 1); } }
const source = fs.readFileSync(join(__dirname, "preload.cjs"), "utf8");
const context = { Buffer, BigInt, Set, Date, JSON, Error, Promise, setTimeout: forbiddenBrowserTimer, clearTimeout: forbiddenBrowserTimer,
    performance: { now: () => clockMs },
    setInterval: forbiddenBrowserTimer, process: { once() {} }, __dirname,
    require(name) {
        if (name === "electron/renderer") return { contextBridge: { exposeInMainWorld(name, value) {
            assert.equal(name, "ArticulateNativeAudio"); api = value;
        } } };
        if (name === "node:path" || name === "node:crypto") return require(name);
        if (name === "node:timers") return nodeTimers;
        if (name.endsWith("articulate_discord_audio.node")) return addon;
        assert.equal(name, "node:http");
        return { Agent, request(options, callback) {
            assert.equal(options.hostname, "127.0.0.1"); assert.equal(options.port, 9223);
            assert.equal(options.headers.Host, "127.0.0.1:9223");
            assert(options.agent instanceof Agent, "Every request reuses the persistent connection");
            agentUsed++;
            const req = new EventEmitter(); req.destroy = () => { req.destroyed = true; req.emit("close"); };
            let ended = false;
            req.end = body => {
                assert(req.socket, "Nothing may be written before the agent assigned a connection");
                ended = true;
                const respond = (status, replyBody) => {
                    const res = new EventEmitter(); res.statusCode = status;
                    callback(res);
                    if (replyBody) res.emit("data", Buffer.from(replyBody));
                    res.emit("end");
                    if (earlyAudioResponse && options.path.startsWith("/pcm")) {
                        finishUpload = () => { req.emit("finish"); req.emit("close"); };
                    } else req.emit("close");
                };
                if (options.path === "/session") {
                    assert.equal(options.headers.Authorization, undefined, "The handshake request carries no secret");
                    handshakes++;
                    if (upgradeRequired) return respond(426, JSON.stringify({ version: 3, min_version: 3 }));
                    const { version, client_nonce } = JSON.parse(body.toString("utf8"));
                    assert.equal(version, 2);
                    assert.match(client_nonce, /^[a-f0-9]{64}$/);
                    const clientNonce = Buffer.from(client_nonce, "hex"), serverNonce = randomBytes(32);
                    const id = randomBytes(16).toString("hex");
                    pendingProof = { id, socket: req.socket, proof: createHmac("sha256", pairingKey).update("articulate-companion-v2/client").update(clientNonce).update(serverNonce).digest() };
                    const serverProof = createHmac("sha256", serverKey).update("articulate-companion-v2/server").update(clientNonce).update(serverNonce).digest("hex");
                    return respond(200, JSON.stringify({ version: 2, session: id, server_nonce: serverNonce.toString("hex"), server_proof: serverProof }));
                }
                if (options.path === "/session/confirm") {
                    confirmations++;
                    assert.equal(req.socket, pendingProof?.socket, "A proof answers the connection that received the server's proof");
                    const { version, session, client_proof } = JSON.parse(body.toString("utf8"));
                    const valid = version === 2 && pendingProof && session === pendingProof.id
                        && timingSafeEqual(Buffer.from(client_proof, "hex"), pendingProof.proof);
                    if (!valid) { pendingProof = null; return respond(403); }
                    serverSession = pendingProof.id; handshakeSocket = req.socket; pendingProof = null;
                    return respond(204);
                }
                // Application data on a connection other than the one that completed
                // the handshake has been handed to a listener that never proved the key.
                assert.equal(req.socket, handshakeSocket, `${options.path} written to an unproven connection`);
                if (forgetSession) { forgetSession = false; serverSession = null; }
                if (options.headers.Authorization !== `Articulate-Session ${serverSession}` || !serverSession) {
                    unauthenticatedBodies += body.length; return respond(401);
                }
                const expectedNonce = BigInt(captureId);
                const controlValue = { active, pcm_batch: batchAdvertised, capture_id: captureId, channel_id: "123", participants: ["456"] };
                const deliver = status => {
                    if (options.path === "/capture") {
                        assert.equal(options.method, "GET");
                        return respond(status, JSON.stringify(controlValue));
                    }
                    if (options.path === "/diagnostics") {
                        assert.equal(options.method, "POST");
                        assert.equal(options.headers["Content-Type"], "application/json");
                        diagnosticReports.push(JSON.parse(body.toString("utf8")));
                        return respond(status);
                    }
                    if (transientFailures > 0 && status === 204) { transientFailures--; return respond(503); }
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
                    respond(status);
                };
                if (options.path.startsWith("/pcm") && deferAudio) finishAudio = deliver;
                else if (options.path === "/capture" && deferControl) finishControl = deliver;
                else deliver(options.path === "/capture" ? controlStatus : options.path === "/diagnostics" ? diagnosticsStatus : audioStatus);
            };
            // Like Node, the agent assigns a connection asynchronously; the
            // request body may only follow that assignment.
            setImmediate(() => { if (!req.destroyed) { req.socket = connection; req.emit("socket", connection); } });
            return req;
        } };
    }
};
// Requests are issued after the session lookup, so the harness waits for a
// deferred request to exist before changing receiver state around it.
const settled = async check => { while (!check()) await new Promise(resolve => setImmediate(resolve)); };
// Hostile browser timers simulate a hidden renderer. Scheduling and request
// deadlines must exclusively use the Node timer module.
vm.runInNewContext(source + ";globalThis.harness = { control, drain, publishDiagnostics };", context);
assert.equal(unrefs, 3);
assert.deepEqual([...scheduled.keys()], [250, 20, 5000]);
assert.equal(typeof api.enable, "function", "Hidden renderers still expose the audio bridge");
assert.equal(agentInstances, 1, "One keep-alive agent carries every request");
// Unresolved mock promises must fail instead of allowing a silent successful exit.
process.exitCode = 1;
(async () => {
    // A listener that cannot prove the pairing key receives nothing but a nonce.
    serverKey = wrongKey;
    assert(api.enable("0".repeat(64)));
    await context.harness.control();
    assert.equal(handshakes, 1); assert.equal(confirmations, 0, "No client proof for an unproven listener");
    assert.equal(api.status().state, "waiting-for-articulate");
    serverKey = pairingKey;
    // The adapter must advertise readiness before Articulate arms capture.
    // Waiting for a first audio packet here would deadlock automatic starts.
    await context.harness.control();
    assert.equal(handshakes, 2); assert.equal(confirmations, 1);
    assert.equal(api.status().state, "ready");
    assert.equal(armed, undefined); assert.equal(posts, 0);
    active = true;
    await context.harness.control();
    assert.equal(handshakes, 2, "An established session is reused");
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
        // Each batch waits one turn for the agent to assign its connection.
        for (let hop = 0; hop < 3; hop++) await new Promise(resolve => setImmediate(resolve));
    }
    assert.equal(backgroundDrops, 472, "A one-second background timer loses 472 of 600 callbacks");
    assert.equal(nodeDrops, 0);
    assert.equal(receivedTags.length - beforePaced, 600);
    assert(nodeTimeouts > 0, "HTTP deadlines use the Node timer module too");
    const beforeBurst = posts;
    const frames = Array.from({ length: 100 }, (_, index) => frame(66, index));
    queue.push(...frames);
    await context.harness.drain();
    assert.equal(posts - beforeBurst, 2, "100 frames use two ordered requests on one connection");
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
    finishUpload = undefined;
    const earlyDrain = context.harness.drain();
    await settled(() => finishUpload);
    const earlyBody = sentBodies.at(-1);
    assert(earlyBody.some(byte => byte !== 0), "An in-flight upload must retain its PCM bytes");
    finishUpload(); await earlyDrain;
    assert(earlyBody.every(byte => byte === 0));
    earlyAudioResponse = false;

    // Transient receiver errors are retried within the same drain; the capture
    // continues and nothing is replayed out of order.
    const stopsBeforeTransient = stopped, postsBeforeTransient = posts;
    transientFailures = 2;
    queue.push(frame(66, 126));
    await context.harness.drain();
    assert.equal(stopped, stopsBeforeTransient, "Two transient failures must not stop capture");
    assert.equal(posts - postsBeforeTransient, 1); assert.equal(receivedTags.at(-1), 126);
    assert.equal(api.status().state, "capturing");
    // A receiver that forgot the session gets exactly one fresh handshake.
    const handshakesBeforeLoss = handshakes;
    forgetSession = true;
    queue.push(frame(66, 127));
    await context.harness.drain();
    assert.equal(handshakes, handshakesBeforeLoss + 1);
    assert.equal(receivedTags.at(-1), 127);
    assert.equal(unauthenticatedBodies, 70, "The refused batch is resent once on the new session, not duplicated");
    assert.equal(api.status().state, "capturing");

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
    finishControl = undefined;
    const pausedControl = context.harness.control();
    await settled(() => finishControl);
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
        queue.push(frame(66, 124)); deferAudio = true; finishAudio = undefined;
        const oldDrain = context.harness.drain();
        await settled(() => finishAudio);
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
    deferAudio = true; finishAudio = undefined;
    const draining = context.harness.drain();
    await settled(() => finishAudio);
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
    // A rejected packet is final: the receiver has already stopped the capture.
    deferAudio = false; audioStatus = 400;
    const failedFrame = Buffer.alloc(66); failedFrame.writeBigUInt64LE(8n, 52); queue.push(failedFrame);
    await context.harness.drain();
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(api.status().state, "audio-transport-unavailable");
    assert.equal(diagnosticReports.at(-1).transport.audio_failures, 1);
    audioStatus = 204; await context.harness.control();
    // Exhausted transport retries stop capture and publish a report.
    transientFailures = 3;
    const exhaustedFrame = Buffer.alloc(66); exhaustedFrame.writeBigUInt64LE(8n, 52); queue.push(exhaustedFrame);
    await context.harness.drain();
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(api.status().state, "audio-transport-unavailable");
    assert.equal(diagnosticReports.at(-1).transport.audio_failures, 2);
    transientFailures = 0; await context.harness.control();
    assert.equal(api.status().state, "capturing");
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
    assert.equal(report.transport.control_failures, 2, "The unproven listener and the 503 poll");
    assert(report.transport.packets >= 600);
    delete addon.stats;
    assert.equal(api.diagnostics().native, undefined, "Older addons remain compatible");

    // Articulate restarted, so the agent opens a new connection. The cached
    // session must not be reused there: whatever answers has proven nothing,
    // so only a fresh handshake is written before the queued audio.
    const current = tag => { const packet = Buffer.alloc(66); packet.writeBigUInt64LE(8n, 52); packet[64] = tag; return packet; };
    const handshakesBeforeReplace = handshakes, confirmationsBeforeReplace = confirmations;
    replaceListener();
    queue.push(current(128));
    await context.harness.drain();
    assert.equal(handshakes, handshakesBeforeReplace + 1);
    assert.equal(confirmations, confirmationsBeforeReplace + 1);
    assert.equal(receivedTags.at(-1), 128, "the batch follows the proven handshake on the new connection");
    assert.equal(unauthenticatedBodies, 70, "no body reached the new listener before it proved the key");
    assert.equal(api.status().state, "capturing");
    // Another process bound the port while Articulate was away. It gets a
    // nonce and nothing else, however eagerly it answers, and capture stops.
    serverKey = wrongKey; replaceListener();
    const confirmationsBeforeRogue = confirmations, postsBeforeRogue = posts, stopsBeforeRogue = stopped;
    queue.push(current(129));
    await context.harness.drain();
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(api.status().state, "audio-transport-unavailable");
    assert.equal(stopped, stopsBeforeRogue + 1);
    assert.equal(confirmations, confirmationsBeforeRogue, "an unproven listener never receives a client proof");
    assert.equal(posts, postsBeforeRogue, "an unproven listener never receives audio");
    clockMs += 1201;
    await context.harness.control();
    assert.equal(api.status().state, "waiting-for-articulate");
    assert.equal(confirmations, confirmationsBeforeRogue);
    // Articulate is back on a fresh connection: pairing resumes there.
    serverKey = pairingKey; replaceListener();
    await context.harness.control();
    assert.equal(confirmations, confirmationsBeforeRogue + 1);
    assert.equal(api.status().state, "capturing");
    queue.push(current(130));
    await context.harness.drain();
    assert.equal(receivedTags.at(-1), 130);
    assert.equal(unauthenticatedBodies, 70);
    active = false;
    const beforeInactive = stopped;
    await context.harness.control(); assert.equal(api.status().state, "ready");
    assert.equal(stopped, beforeInactive + 1, "An explicit inactive control stops immediately");
    active = true; deferControl = true; finishControl = undefined;
    const beforeDisableReply = context.harness.control();
    await settled(() => finishControl);
    const beforeDisableArms = armCalls;
    api.disable();
    finishControl(200); await beforeDisableReply;
    assert.equal(armCalls, beforeDisableArms, "A paused GET cannot undo explicit disable");
    assert.equal(api.status().state, "disabled"); assert(stopped >= 3);
    // A newer receiver tells the companion to update instead of being trusted.
    const handshakesBeforeUpgrade = handshakes, confirmationsBeforeUpgrade = confirmations;
    upgradeRequired = true;
    assert(api.enable("0".repeat(64)));
    await context.harness.control();
    assert.equal(api.status().state, "update-required");
    assert.equal(handshakes, handshakesBeforeUpgrade + 1, "The version check is one handshake");
    assert.equal(confirmations, confirmationsBeforeUpgrade, "Only proven listeners ever receive a client proof");
    assert.equal(confirmations, 4, "Pairing, session loss, the replaced listener and Articulate's return");
    assert(agentUsed > 0);
    console.log("Renderer handshake, control, PCM, retry, nonce and stopping pass with mocked localhost.");
    process.exitCode = 0;
})().catch(error => { console.error(error); process.exitCode = 1; });
