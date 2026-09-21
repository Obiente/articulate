// SPDX-License-Identifier: AGPL-3.0-or-later
// Loaded explicitly by the custom Vencord preload, never through native.ts IPC.
// Native voice and this addon must live in the same renderer process.
"use strict";
const { contextBridge } = require("electron/renderer");
const { request } = require("node:http");
const { join } = require("node:path");
// Chromium throttles window timers in background renderers. Audio transport
// must use Node timers even when Discord is minimized or its window is hidden.
const { setInterval, setTimeout, clearTimeout } = require("node:timers");
let addon = null;
try {
    addon = require(join(__dirname, "articulate_discord_audio.node"));
    if (!["start", "arm", "poll", "stop"].every(name => typeof addon[name] === "function")) addon = null;
} catch { /* Unsupported addon loads must not prevent Discord from opening. */ }
let key = "", installed = false, unsupported = !addon, state = addon ? "disabled" : "addon-unavailable";
let controlBusy = false, drainBusy = false, generation = 0, lastAttempt = 0;
let capture = null;
let captureEpoch = 0, lastControl = 0;
const CONTROL_GRACE_MS = 1200; // Native callback lease expires after 1500 ms.
const clock = () => typeof performance !== "undefined" ? performance.now() : Date.now();
let batchSupported = false, pendingPacket = null;
const pending = new Set();
const transport = { control_failures: 0, audio_failures: 0, requests: 0, packets: 0,
    max_control_gap_ms: 0, max_drain_gap_ms: 0, max_request_ms: 0 };
const nativeCounters = ["produced", "polled", "queue_full", "contention", "configuration_discard",
    "stop_flush", "max_depth", "queue_depth", "queue_capacity"];
let lastControlTick = null, lastDrainTick = null, diagnosticsBusy = false;
const bounded = value => typeof value === "number" && Number.isFinite(value)
    ? Math.min(Number.MAX_SAFE_INTEGER, Math.max(0, Math.floor(value))) : 0;
function count(name, amount = 1) { transport[name] = bounded(transport[name] + amount); }
function maximum(name, value) { transport[name] = Math.max(transport[name], bounded(value)); }
function diagnostics() {
    let native;
    try {
        if (typeof addon?.stats === "function") {
            const raw = addon.stats();
            native = Object.fromEntries(nativeCounters.map(name => [name, bounded(raw?.[name])]));
        }
    } catch { /* Older or unavailable addons must not interrupt audio transport. */ }
    return { version: 1, ...(native ? { native } : {}), transport: { ...transport } };
}
async function publishDiagnostics() {
    if (!key || diagnosticsBusy) return;
    diagnosticsBusy = true;
    const token = key;
    const body = Buffer.from(JSON.stringify(diagnostics()));
    try { await exchange("POST", "/diagnostics", body, token); }
    catch { /* Best effort and independent of capture state. */ }
    finally { body.fill(0); diagnosticsBusy = false; }
}

function exchange(method, path, body, token) {
    const started = clock(); count("requests");
    return new Promise(resolve => {
        let finished = false, chunks = [], size = 0, written = false, closed = false, responseReady = false, result = null;
        const settle = () => {
            if (!finished && responseReady && (written || closed)) { finished = true; maximum("max_request_ms", clock() - started); resolve(result); }
        };
        const finish = value => { if (!responseReady) { result = value; responseReady = true; } settle(); };
        const req = request({ hostname: "127.0.0.1", port: 9223, path, method, agent: false,
            headers: { Host: "127.0.0.1:9223", Authorization: `Bearer ${token}`,
                "Content-Type": (method === "GET" || path === "/diagnostics") ? "application/json" : "application/octet-stream",
                "Content-Length": body.length, Connection: "close" } }, res => {
            res.on("data", chunk => {
                size += chunk.length;
                if (size > 65536) { req.destroy(); finish(null); } else chunks.push(chunk);
            });
            res.on("end", () => finish({ status: res.statusCode, body: Buffer.concat(chunks) }));
            res.on("error", () => { finish(null); req.destroy(); });
        });
        pending.add(req);
        const timer = setTimeout(() => { req.destroy(); finish(null); }, 500);
        // An early HTTP response can precede upload completion. The caller may
        // erase its PCM buffer only once Node flushed it or closed the request.
        req.once("finish", () => { written = true; settle(); });
        req.once("close", () => { closed = true; clearTimeout(timer); pending.delete(req); finish(null); });
        req.once("error", () => { finish(null); req.destroy(); });
        req.end(body);
    });
}
function clearPendingPacket() { pendingPacket?.fill(0); pendingPacket = null; }
function stopCapture() { ++captureEpoch; capture = null; lastControl = 0; lastDrainTick = null; clearPendingPacket(); addon?.stop(); }
function controlFailure() {
    if (capture !== null && clock() - lastControl >= CONTROL_GRACE_MS) stopCapture();
    state = "waiting-for-articulate";
}
function expireControlLease() {
    if (capture !== null && clock() - lastControl >= CONTROL_GRACE_MS) controlFailure();
}
function disable() {
    ++generation; key = ""; lastControlTick = null; state = "disabled"; stopCapture();
    for (const req of pending) req.destroy();
}
function enable(token) {
    if (!addon) return false;
    if (typeof token !== "string" || !/^[a-f0-9]{64}$/i.test(token)) { disable(); return false; }
    if (key !== token) { disable(); key = token; state = "waiting"; }
    return true;
}
async function control() {
    if (key) {
        if (lastControlTick !== null) maximum("max_control_gap_ms", clock() - lastControlTick);
        lastControlTick = clock();
    }
    expireControlLease();
    if (!key || unsupported || controlBusy) return;
    controlBusy = true;
    const operation = generation, epoch = captureEpoch, token = key;
    try {
        if (!installed) {
            if (Date.now() - lastAttempt < 1000) return;
            lastAttempt = Date.now();
            const result = addon.start();
            if (result === 1) { state = "waiting-for-voice-engine"; return; }
            if (result !== 0) { unsupported = true; state = result === 2 || result === 3 ? "unsupported-native-build" : "native-hook-unavailable"; return; }
            installed = true;
        }
        const response = await exchange("GET", "/capture", Buffer.alloc(0), token);
        if (operation !== generation || epoch !== captureEpoch) return;
        if (!response || response.status !== 200) { count("control_failures"); controlFailure(); return; }
        const value = JSON.parse(response.body.toString("utf8"));
        if (!value || typeof value.active !== "boolean") throw new Error("Invalid control state");
        batchSupported = value.pcm_batch === true;
        if (!value.active) { stopCapture(); state = "ready"; return; }
        if (typeof value.capture_id !== "string" || !/^[0-9]{1,20}$/.test(value.capture_id)
            || typeof value.channel_id !== "string" || !/^[0-9]{1,20}$/.test(value.channel_id)
            || !Array.isArray(value.participants) || value.participants.length > 256
            || value.participants.some(id => typeof id !== "string" || !/^[0-9]{1,20}$/.test(id))
            || new Set(value.participants).size !== value.participants.length) throw new Error("Invalid capture controls");
        const nonce = BigInt(value.capture_id), ids = value.participants.map(id => BigInt(id));
        if (nonce <= 0n || nonce > 0xffffffffffffffffn || ids.some(id => id <= 0n || id > 0xffffffffffffffffn)) throw new Error("Invalid identifier");
        if (addon.arm(nonce, ids) !== 0) throw new Error("Capture could not start");
        lastControl = clock();
        if (capture !== nonce) clearPendingPacket();
        capture = nonce; state = "capturing";
    } catch { if (operation === generation && epoch === captureEpoch) { count("control_failures"); stopCapture(); state = "control-unavailable"; } }
    finally { controlBusy = false; }
}
async function drain() {
    if (capture !== null) {
        if (lastDrainTick !== null) maximum("max_drain_gap_ms", clock() - lastDrainTick);
        lastDrainTick = clock();
    }
    expireControlLease();
    if (!key || capture === null || drainBusy) return;
    drainBusy = true;
    const operation = generation, epoch = captureEpoch, token = key, nonce = capture;
    try {
        // One request carries ordered frames from all participants. Keep network
        // requests sequential so a later batch cannot overtake an earlier one.
        for (let batch = 0; batch < (batchSupported ? 8 : 32) && operation === generation && epoch === captureEpoch && capture === nonce; ++batch) {
            const packets = [];
            let bytes = 0;
            for (let i = 0; i < (batchSupported ? 64 : 1); ++i) {
                const packet = pendingPacket ?? addon.poll();
                pendingPacket = null;
                if (!Buffer.isBuffer(packet) || !packet.length) break;
                if (packet.length < 64 || packet.length > 23104 || packet.readBigUInt64LE(52) !== nonce) { packet.fill(0); continue; }
                if (batchSupported && bytes + 4 + packet.length > 128 * 1024) { pendingPacket = packet; break; }
                packets.push(packet);
                bytes += packet.length + (batchSupported ? 4 : 0);
            }
            if (!packets.length) break;
            const body = batchSupported ? Buffer.alloc(bytes) : packets[0];
            if (batchSupported) {
                let offset = 0;
                for (const packet of packets) {
                    body.writeUInt32LE(packet.length, offset); offset += 4;
                    packet.copy(body, offset); offset += packet.length;
                    packet.fill(0);
                }
            }
            count("packets", packets.length);
            let response;
            try { response = await exchange("POST", batchSupported ? "/pcm-batch" : "/pcm", body, token); }
            finally { body.fill(0); }
            if (!response || response.status !== 204) {
                if (operation === generation && epoch === captureEpoch && capture === nonce) { count("audio_failures"); stopCapture(); state = "audio-transport-unavailable"; void publishDiagnostics(); }
                break;
            }
        }
    } catch { if (operation === generation && epoch === captureEpoch && capture === nonce) { count("audio_failures"); stopCapture(); state = "audio-transport-unavailable"; void publishDiagnostics(); } }
    finally { drainBusy = false; }
}
// Use Node scheduling for control, PCM draining, diagnostics and HTTP deadlines.
const controlTimer = setInterval(() => void control(), 250);
const drainTimer = setInterval(() => void drain(), 20);
const diagnosticsTimer = setInterval(() => void publishDiagnostics(), 5000);
controlTimer?.unref?.();
drainTimer?.unref?.();
diagnosticsTimer?.unref?.();
process.once("exit", () => addon?.stop());
contextBridge.exposeInMainWorld("ArticulateNativeAudio", {
    enable, disable, diagnostics, status: () => ({ state, installed })
});
