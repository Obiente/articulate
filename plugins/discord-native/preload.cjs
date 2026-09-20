// SPDX-License-Identifier: AGPL-3.0-or-later
// Loaded explicitly by the custom Vencord preload, never through native.ts IPC.
// Native voice and this addon must live in the same renderer process.
"use strict";
const { contextBridge } = require("electron/renderer");
const { request } = require("node:http");
const { join } = require("node:path");
let addon = null;
try {
    addon = require(join(__dirname, "articulate_discord_audio.node"));
    if (!["start", "arm", "poll", "stop"].every(name => typeof addon[name] === "function")) addon = null;
} catch { /* Unsupported addon loads must not prevent Discord from opening. */ }
let key = "", installed = false, unsupported = !addon, state = addon ? "disabled" : "addon-unavailable";
let controlBusy = false, drainBusy = false, generation = 0, lastAttempt = 0;
let capture = null;
const pending = new Set();

function exchange(method, path, body, token) {
    return new Promise(resolve => {
        let finished = false, chunks = [], size = 0;
        const finish = result => { if (!finished) { finished = true; resolve(result); } };
        const req = request({ hostname: "127.0.0.1", port: 9223, path, method, agent: false,
            headers: { Host: "127.0.0.1:9223", Authorization: `Bearer ${token}`,
                "Content-Type": method === "GET" ? "application/json" : "application/octet-stream",
                "Content-Length": body.length, Connection: "close" } }, res => {
            res.on("data", chunk => {
                size += chunk.length;
                if (size > 65536) { req.destroy(); finish(null); } else chunks.push(chunk);
            });
            res.on("end", () => finish({ status: res.statusCode, body: Buffer.concat(chunks) }));
            res.on("error", () => finish(null));
        });
        pending.add(req);
        const timer = setTimeout(() => { req.destroy(); finish(null); }, 500);
        req.once("close", () => { clearTimeout(timer); pending.delete(req); finish(null); });
        req.once("error", () => finish(null));
        req.end(body);
    });
}
function stopCapture() { capture = null; addon?.stop(); }
function disable() {
    ++generation; key = ""; state = "disabled"; stopCapture();
    for (const req of pending) req.destroy();
}
function enable(token) {
    if (!addon) return false;
    if (typeof token !== "string" || !/^[a-f0-9]{64}$/i.test(token)) { disable(); return false; }
    if (key !== token) { disable(); key = token; state = "waiting"; }
    return true;
}
async function control() {
    if (!key || unsupported || controlBusy) return;
    controlBusy = true;
    const operation = generation, token = key;
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
        if (operation !== generation) return;
        if (!response || response.status !== 200) { stopCapture(); state = "waiting-for-articulate"; return; }
        const value = JSON.parse(response.body.toString("utf8"));
        if (!value || typeof value.active !== "boolean") throw new Error("Invalid control state");
        if (!value.active) { stopCapture(); state = "ready"; return; }
        if (typeof value.capture_id !== "string" || !/^[0-9]{1,20}$/.test(value.capture_id)
            || typeof value.channel_id !== "string" || !/^[0-9]{1,20}$/.test(value.channel_id)
            || !Array.isArray(value.participants) || value.participants.length > 256
            || value.participants.some(id => typeof id !== "string" || !/^[0-9]{1,20}$/.test(id))
            || new Set(value.participants).size !== value.participants.length) throw new Error("Invalid capture controls");
        const nonce = BigInt(value.capture_id), ids = value.participants.map(id => BigInt(id));
        if (nonce <= 0n || nonce > 0xffffffffffffffffn || ids.some(id => id <= 0n || id > 0xffffffffffffffffn)) throw new Error("Invalid identifier");
        if (addon.arm(nonce, ids) !== 0) throw new Error("Capture could not start");
        capture = nonce; state = "capturing";
    } catch { if (operation === generation) { stopCapture(); state = "control-unavailable"; } }
    finally { controlBusy = false; }
}
async function drain() {
    if (!key || capture === null || drainBusy) return;
    drainBusy = true;
    const operation = generation, token = key, nonce = capture;
    try {
        for (let i = 0; i < 32 && operation === generation && capture === nonce; ++i) {
            const packet = addon.poll();
            if (!Buffer.isBuffer(packet) || !packet.length) break;
            if (packet.length < 64 || packet.length > 23104 || packet.readBigUInt64LE(52) !== nonce) continue;
            const response = await exchange("POST", "/pcm", packet, token);
            packet.fill(0);
            if (!response || response.status !== 204) {
                if (operation === generation) { stopCapture(); state = "audio-transport-unavailable"; }
                break;
            }
        }
    } catch { if (operation === generation) { stopCapture(); state = "audio-transport-unavailable"; } }
    finally { drainBusy = false; }
}
setInterval(() => void control(), 250).unref();
setInterval(() => void drain(), 20).unref();
process.once("exit", () => addon?.stop());
contextBridge.exposeInMainWorld("ArticulateNativeAudio", {
    enable, disable, status: () => ({ state, installed })
});
