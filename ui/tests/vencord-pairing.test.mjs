import assert from "node:assert/strict";
import crypto, { createHmac, randomBytes, timingSafeEqual } from "node:crypto";
import { EventEmitter } from "node:events";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import vm from "node:vm";
import { transform } from "esbuild";

const source = await readFile(new URL("../../plugins/vencord/articulate/native.ts", import.meta.url), "utf8");
// The bridge loads its network and crypto modules lazily. The sandbox has no
// module loader for dynamic imports, so they go through the mocked `require`.
const { code } = await transform(source.replace(/await import\(("node:(?:http|crypto)")\)/g, "require($1)"), { loader: "ts", format: "cjs" });

// `files` is the settings.json text, or a map of file name to text for the
// pairing file and the settings file; a file missing from the map does not exist.
function nativeFixture(files, options = {}) {
    const contents = typeof files === "string" ? { "settings.json": files } : files;
    const directory = "C:\\SyntheticLocalData\\TranscribeLocal";
    let closes = 0;
    const named = (location) => {
        const name = location.slice(directory.length + 1);
        assert.ok(["settings.json", "discord-pairing.json"].includes(name), location);
        if (!(name in contents)) throw Object.assign(new Error("ENOENT"), { code: "ENOENT" });
        return Buffer.from(contents[name]);
    };
    const metadata = (isDirectory, bytes) => ({
        isDirectory: () => isDirectory,
        isFile: () => !isDirectory,
        isSymbolicLink: () => !!options.link,
        size: options.size ?? bytes?.length ?? 0,
    });
    const filesystem = {
        lstat: async (location) => {
            if (location === directory) return metadata(true);
            return metadata(false, named(location));
        },
        open: async (location, mode) => {
            const bytes = named(location);
            assert.equal(mode, "r");
            let offset = 0;
            return {
                stat: async () => metadata(false, bytes),
                read: async (target, targetOffset, count) => {
                    const length = Math.min(count, bytes.length - offset, 23);
                    bytes.copy(target, targetOffset, offset, offset + length);
                    offset += length;
                    return { bytesRead: length };
                },
                close: async () => { closes++; },
            };
        },
    };
    const module = { exports: {} };
    vm.runInNewContext(code, {
        module, exports: module.exports, Buffer,
        process: { platform: "win32", env: { LOCALAPPDATA: "C:\\SyntheticLocalData" } },
        require: (name) => {
            if (name === "node:fs/promises") return filesystem;
            if (name === "node:path") return path.win32;
            if (name === "node:http") return { request: () => { throw new Error("Pairing cannot use HTTP"); } };
            throw new Error(`Unexpected import: ${name}`);
        },
    });
    return { read: () => module.exports.getPairingKey({}), closes: () => closes,
        publish: (snapshot) => module.exports.publish({}, "a".repeat(64), JSON.stringify(snapshot)) };
}

test("companion discovers only the local pairing key without HTTP or a renderer path", async () => {
    const key = "a".repeat(64);
    const fixture = nativeFixture(JSON.stringify({ discord_pairing_key: key, private_other_settings: "not returned" }));
    assert.equal(await fixture.read(), key);
    assert.equal(fixture.closes(), 1);
});

test("companion rejects malformed, missing, oversized and linked pairing settings", async () => {
    for (const settings of ["invalid json", "null", "{}", JSON.stringify({ discord_pairing_key: "invalid" })]) {
        const fixture = nativeFixture(settings);
        assert.equal(await fixture.read(), "");
        assert.equal(fixture.closes(), 1);
    }
    for (const options of [{ link: true }, { size: 1024 * 1024 + 1 }]) {
        const fixture = nativeFixture(JSON.stringify({ discord_pairing_key: "b".repeat(64) }), options);
        assert.equal(await fixture.read(), "");
        assert.equal(fixture.closes(), 0);
    }
});

test("companion prefers the pairing file and falls back to plaintext settings", async () => {
    const key = "c".repeat(64), legacy = "d".repeat(64);
    let fixture = nativeFixture({ "discord-pairing.json": JSON.stringify({ version: 1, key }), "settings.json": JSON.stringify({ discord_pairing_key: legacy }) });
    assert.equal(await fixture.read(), key);
    assert.equal(fixture.closes(), 1, "settings are not opened when the pairing file is usable");
    // Settings encrypted at rest are not JSON at all; the pairing file still pairs.
    fixture = nativeFixture({ "discord-pairing.json": JSON.stringify({ version: 1, key }), "settings.json": "\0ARTICULATE-DPAPI-1\nsealed" });
    assert.equal(await fixture.read(), key);
    fixture = nativeFixture({ "settings.json": "\0ARTICULATE-DPAPI-1\nsealed" });
    assert.equal(await fixture.read(), "");
    // Older Articulate versions publish no pairing file.
    fixture = nativeFixture({ "settings.json": JSON.stringify({ discord_pairing_key: legacy }) });
    assert.equal(await fixture.read(), legacy);
    // A damaged or foreign pairing file is ignored, never trusted.
    for (const pairing of ["garbage", "[]", JSON.stringify({ version: 2, key }), JSON.stringify({ version: 1, key: "short" }), JSON.stringify({ key })]) {
        fixture = nativeFixture({ "discord-pairing.json": pairing, "settings.json": JSON.stringify({ discord_pairing_key: legacy }) });
        assert.equal(await fixture.read(), legacy, pairing);
    }
});

// A scripted receiver whose sessions belong to the connection that completed
// the handshake, like the real one. Replacing the listener yields a new socket
// object whose server-side state starts empty.
function transportFixture() {
    const pairingKey = Buffer.from("a".repeat(64), "hex"), wrongKey = Buffer.from("b".repeat(64), "hex");
    let serverKey = pairingKey, serverSession = null, pendingProof = null, handshakeSocket = null;
    let socketSerial = 0, connection = { id: ++socketSerial }, clock = 10_000;
    const counts = { session: 0, confirm: 0, voice: 0 };
    const voiceBodies = [];
    class Agent { constructor(options) { assert.equal(options.keepAlive, true); assert.equal(options.maxSockets, 1); } }
    const http = { Agent, request(options, callback) {
        assert.equal(options.hostname, "127.0.0.1"); assert.equal(options.port, 9223);
        assert(options.agent instanceof Agent);
        const req = new EventEmitter();
        req.destroy = () => { req.destroyed = true; req.emit("close"); };
        req.end = body => {
            assert(req.socket, "Nothing may be written before the agent assigned a connection");
            const respond = (status, replyBody) => {
                const res = new EventEmitter(); res.statusCode = status;
                callback(res);
                if (replyBody) res.emit("data", Buffer.from(replyBody));
                res.emit("end"); req.emit("close");
            };
            if (options.path === "/session") {
                assert.equal(options.headers.Authorization, undefined);
                counts.session++;
                const { client_nonce } = JSON.parse(body.toString("utf8"));
                const clientNonce = Buffer.from(client_nonce, "hex"), serverNonce = randomBytes(32), id = randomBytes(16).toString("hex");
                pendingProof = { id, socket: req.socket, proof: createHmac("sha256", pairingKey).update("articulate-companion-v2/client").update(clientNonce).update(serverNonce).digest() };
                const serverProof = createHmac("sha256", serverKey).update("articulate-companion-v2/server").update(clientNonce).update(serverNonce).digest("hex");
                return respond(200, JSON.stringify({ version: 2, session: id, server_nonce: serverNonce.toString("hex"), server_proof: serverProof }));
            }
            if (options.path === "/session/confirm") {
                counts.confirm++;
                assert.equal(req.socket, pendingProof?.socket, "A proof answers the connection that received the server's proof");
                const { session, client_proof } = JSON.parse(body.toString("utf8"));
                if (session !== pendingProof.id || !timingSafeEqual(Buffer.from(client_proof, "hex"), pendingProof.proof)) { pendingProof = null; return respond(403); }
                serverSession = pendingProof.id; handshakeSocket = req.socket; pendingProof = null;
                return respond(204);
            }
            assert.equal(req.socket, handshakeSocket, `${options.path} written to an unproven connection`);
            if (!serverSession || options.headers.Authorization !== `Articulate-Session ${serverSession}`) return respond(401);
            assert.equal(options.path, "/voice");
            counts.voice++;
            voiceBodies.push(JSON.parse(body.toString("utf8")));
            return respond(204);
        };
        setImmediate(() => { if (!req.destroyed) { req.socket = connection; req.emit("socket", connection); } });
        return req;
    } };
    const module = { exports: {} };
    vm.runInNewContext(code, {
        module, exports: module.exports, Buffer, setTimeout, clearTimeout,
        performance: { now: () => clock },
        process: { platform: "win32", env: {} },
        require: (name) => {
            if (name === "node:http") return http;
            if (name === "node:crypto") return crypto;
            if (name === "node:fs/promises") return {};
            if (name === "node:path") return path.win32;
            throw new Error(`Unexpected import: ${name}`);
        },
    });
    return {
        counts, voiceBodies,
        publish: async () => {
            clock += 50;
            return module.exports.publish({}, "a".repeat(64), JSON.stringify({ version: 2, observed_ms: Date.now(), channel_id: null, participants: [], valid: true }));
        },
        state: () => module.exports.getTransportState({}),
        replaceListener: (key = pairingKey) => { serverKey = key; connection = { id: ++socketSerial }; pendingProof = null; serverSession = null; },
        wrongKey,
    };
}

test("native bridge binds its session to the connection that proved the key", async () => {
    const fixture = transportFixture();
    assert.equal(await fixture.publish(), true);
    assert.deepEqual(fixture.counts, { session: 1, confirm: 1, voice: 1 });
    assert.equal(await fixture.state(), "ok");
    assert.equal(await fixture.publish(), true);
    assert.deepEqual(fixture.counts, { session: 1, confirm: 1, voice: 2 }, "an established session is reused");
    assert.equal(fixture.voiceBodies.at(-1).sequence, 2);
    // Articulate restarted, so the agent opens a new connection. The cached
    // session is not reused there: a fresh handshake precedes the snapshot.
    fixture.replaceListener();
    assert.equal(await fixture.publish(), true);
    assert.deepEqual(fixture.counts, { session: 2, confirm: 2, voice: 3 });
    assert.equal(fixture.voiceBodies.at(-1).sequence, 1, "a new session starts its own sequence");
    // Another process bound the port: it receives the nonce and nothing else.
    fixture.replaceListener(fixture.wrongKey);
    assert.equal(await fixture.publish(), false);
    assert.equal(await fixture.state(), "rejected");
    assert.equal(await fixture.publish(), false);
    assert.deepEqual(fixture.counts, { session: 4, confirm: 2, voice: 3 });
    // Articulate is back on yet another connection: pairing resumes there.
    fixture.replaceListener();
    assert.equal(await fixture.publish(), true);
    assert.deepEqual(fixture.counts, { session: 5, confirm: 3, voice: 4 });
    assert.equal(await fixture.state(), "ok");
});

test("native bridge rejects malformed compiled revisions", async () => {
    for (const companion_revision of ["", "A".repeat(64), "g".repeat(64), "a".repeat(63), null]) {
        const fixture = nativeFixture("{}");
        assert.equal(await fixture.publish({ version: 1, observed_ms: Date.now(), channel_id: null,
            participants: [], valid: true, companion_revision }), false);
    }
});

test("native bridge rejects unknown or malformed audio states", async () => {
    for (const audio_status of ["", "unknown-state", 7, null, { state: "ready" }]) {
        const fixture = nativeFixture("{}");
        assert.equal(await fixture.publish({ version: 1, observed_ms: Date.now(), channel_id: null,
            participants: [], valid: true, audio_status }), false);
    }
});

for (const revision of [undefined, "c".repeat(64)]) {
test(`companion automatically pairs and reports ${revision ? "compiled" : "unknown manual"} revision`, async () => {
    let renderer = await readFile(new URL("../../plugins/vencord/articulate/index.ts", import.meta.url), "utf8");
    if (revision) renderer = renderer.replace("__ARTICULATE_COMPANION_REVISION__", revision);
    const built = await transform(renderer, { loader: "ts", format: "cjs" });
    const module = { exports: {} };
    let clock = 10_000, timer, currentKey = "a".repeat(64), disabled = 0, audioState = "ready";
    const enabled = [], published = [], snapshots = [];
    const store = {
        addChangeListener() {}, removeChangeListener() {},
        getVoiceChannelId: () => null, getCurrentUser: () => ({ id: "123" }),
    };
    vm.runInNewContext(built.code, {
        module, exports: module.exports,
        Date: { now: () => clock },
        setInterval: (callback) => { timer = callback; return 1; },
        clearInterval: () => { timer = undefined; },
        ArticulateNativeAudio: {
            enable: (key) => { enabled.push(key); return true; },
            disable: () => { disabled++; },
            status: () => ({ state: audioState, installed: true }),
        },
        VencordNative: { pluginHelpers: { Articulate: {
            getPairingKey: async () => currentKey,
            publish: async (key, snapshot) => { published.push(key); snapshots.push(JSON.parse(snapshot)); return true; },
        } } },
        require: (name) => {
            if (name === "@utils/types") return { default: (plugin) => plugin, __esModule: true };
            if (name === "@webpack") return { findStoreLazy: () => store };
            throw new Error(`Unexpected import: ${name}`);
        },
    });
    const plugin = module.exports.default;
    assert.equal(plugin.settings, undefined);
    plugin.start();
    await new Promise(setImmediate);
    assert.equal(published.at(-1), currentKey);
    assert.equal(enabled.at(-1), currentKey);
    assert.equal(snapshots.at(-1).companion_revision, revision);
    assert.equal(snapshots.at(-1).audio_status, "ready");
    currentKey = "b".repeat(64);
    audioState = "capturing";
    clock += 3001;
    await timer();
    assert.equal(published.at(-1), currentKey);
    assert.equal(enabled.at(-1), currentKey);
    assert.equal(snapshots.at(-1).companion_revision, revision);
    assert.equal(snapshots.at(-1).audio_status, "capturing");
    currentKey = "";
    clock += 3001;
    await timer();
    assert.equal(published.length, 2);
    assert.equal(disabled, 1);
    plugin.stop();
    assert.equal(disabled, 2);
    assert.equal(timer, undefined);
});
}
