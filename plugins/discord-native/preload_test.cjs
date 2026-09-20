// SPDX-License-Identifier: AGPL-3.0-or-later
"use strict";
const fs = require("node:fs"), vm = require("node:vm"), assert = require("node:assert/strict");
const { join } = require("node:path"), { EventEmitter } = require("node:events");
let api, armed, stopped = 0, posts = 0, active = true;
const queue = [];
const addon = { start: () => 0, arm: (nonce, ids) => { armed = { nonce, ids }; return 0; },
    stop: () => { stopped++; }, poll: () => queue.shift() ?? Buffer.alloc(0) };
const source = fs.readFileSync(join(__dirname, "preload.cjs"), "utf8");
const context = { Buffer, BigInt, Set, Date, JSON, Error, setTimeout, clearTimeout,
    setInterval: () => ({ unref() {} }), process: { once() {} }, __dirname,
    require(name) {
        if (name === "electron/renderer") return { contextBridge: { exposeInMainWorld(name, value) {
            assert.equal(name, "ArticulateNativeAudio"); api = value;
        } } };
        if (name === "node:path") return require(name);
        if (name.endsWith("articulate_discord_audio.node")) return addon;
        assert.equal(name, "node:http");
        return { request(options, callback) {
            assert.equal(options.hostname, "127.0.0.1"); assert.equal(options.port, 9223);
            assert.equal(options.headers.Authorization, "Bearer " + "0".repeat(64));
            const req = new EventEmitter(); req.destroy = () => req.emit("close");
            req.end = body => {
                const res = new EventEmitter(); res.statusCode = options.path === "/capture" ? 200 : 204;
                callback(res);
                if (options.path === "/capture") {
                    assert.equal(options.method, "GET");
                    res.emit("data", Buffer.from(JSON.stringify({ active, capture_id: "7", channel_id: "123", participants: ["456"] })));
                } else {
                    assert.equal(options.path, "/pcm"); assert.equal(body.readBigUInt64LE(52), 7n); posts++;
                }
                res.emit("end"); req.emit("close");
            };
            return req;
        } };
    }
};
vm.runInNewContext(source + ";globalThis.harness = { control, drain };", context);
(async () => {
    assert(api.enable("0".repeat(64)));
    await context.harness.control();
    assert.equal(armed.nonce, 7n); assert.equal(armed.ids[0], 456n);
    assert.equal(api.status().state, "capturing");
    const packet = Buffer.alloc(66); packet.writeBigUInt64LE(7n, 52); queue.push(packet);
    await context.harness.drain(); assert.equal(posts, 1); assert(packet.every(byte => byte === 0));
    active = false; await context.harness.control(); assert.equal(api.status().state, "ready");
    api.disable(); assert.equal(api.status().state, "disabled"); assert(stopped >= 3);
    console.log("Renderer control, PCM, nonce and stopping pass with mocked localhost.");
})().catch(error => { console.error(error); process.exitCode = 1; });
