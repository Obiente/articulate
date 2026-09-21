import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import vm from "node:vm";
import { transform } from "esbuild";

const source = await readFile(new URL("../../plugins/vencord/articulate/native.ts", import.meta.url), "utf8");
const { code } = await transform(source, { loader: "ts", format: "cjs" });

function nativeFixture(settings, options = {}) {
    const bytes = Buffer.from(settings);
    const directory = "C:\\SyntheticLocalData\\TranscribeLocal";
    let closes = 0;
    const metadata = (isDirectory) => ({
        isDirectory: () => isDirectory,
        isFile: () => !isDirectory,
        isSymbolicLink: () => !!options.link,
        size: options.size ?? bytes.length,
    });
    const filesystem = {
        lstat: async (location) => {
            assert.ok([directory, `${directory}\\settings.json`].includes(location));
            return metadata(location === directory);
        },
        open: async (location, mode) => {
            assert.equal(location, `${directory}\\settings.json`);
            assert.equal(mode, "r");
            let offset = 0;
            return {
                stat: async () => metadata(false),
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
