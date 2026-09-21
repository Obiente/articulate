import type { IpcMainInvokeEvent } from "electron";
import { lstat, open } from "node:fs/promises";
import { request } from "node:http";
import { isAbsolute, join } from "node:path";

let pending = false;
let lastRequest = 0;
const audioStates = new Set(["disabled", "addon-unavailable", "waiting", "waiting-for-voice-engine",
    "unsupported-native-build", "native-hook-unavailable", "waiting-for-articulate", "ready",
    "capturing", "control-unavailable", "audio-transport-unavailable", "preload-unavailable"]);

// Read only the same user's fixed Articulate settings file. Never accept a
// renderer-supplied path or expose the remaining settings through IPC.
export async function getPairingKey(_event: IpcMainInvokeEvent): Promise<string> {
    const localData = process.env.LOCALAPPDATA;
    if (process.platform !== "win32" || !localData || !isAbsolute(localData)) return "";
    // This directory name is retained by Articulate for existing user profiles.
    const directory = join(localData, "TranscribeLocal");
    const path = join(directory, "settings.json");
    try {
        const parent = await lstat(directory);
        const metadata = await lstat(path);
        if (!parent.isDirectory() || parent.isSymbolicLink()
            || !metadata.isFile() || metadata.isSymbolicLink() || metadata.size > 1024 * 1024) return "";
        const file = await open(path, "r");
        try {
            const current = await file.stat();
            if (!current.isFile() || current.size > 1024 * 1024) return "";
            const bytes = Buffer.alloc(1024 * 1024 + 1);
            let length = 0;
            while (length < bytes.length) {
                const read = await file.read(bytes, length, bytes.length - length, null);
                if (!read.bytesRead) break;
                length += read.bytesRead;
            }
            if (length > 1024 * 1024) return "";
            const value = JSON.parse(bytes.subarray(0, length).toString("utf8"));
            const key = value?.discord_pairing_key;
            return typeof key === "string" && /^[a-f0-9]{64}$/i.test(key) ? key : "";
        } finally { await file.close(); }
    } catch { return ""; }
}

// A deliberately narrow IPC operation. No caller-supplied URL, path or headers.
export async function publish(_event: IpcMainInvokeEvent, token: string, snapshot: string): Promise<boolean> {
    if (typeof token !== "string" || !/^[a-f0-9]{64}$/i.test(token)
        || typeof snapshot !== "string" || Buffer.byteLength(snapshot) > 128 * 1024
        || pending || Date.now() - lastRequest < 40) return false;
    try {
        const value = JSON.parse(snapshot);
        if (!value || typeof value !== "object" || value.version !== 1
            || !Number.isSafeInteger(value.observed_ms) || Math.abs(Date.now() - value.observed_ms) > 250
            || !Array.isArray(value.participants) || value.participants.length > 256
            || typeof value.valid !== "boolean"
            || (value.companion_revision !== undefined && (typeof value.companion_revision !== "string" || !/^[a-f0-9]{64}$/.test(value.companion_revision)))
            || (value.audio_status !== undefined && (typeof value.audio_status !== "string" || !audioStates.has(value.audio_status)))
            || !(value.channel_id === null || (typeof value.channel_id === "string" && /^\d{1,24}$/.test(value.channel_id)))
            || Object.keys(value).some(key => !["version", "observed_ms", "channel_id", "participants", "valid", "companion_revision", "audio_status"].includes(key))) return false;
        const ids = new Set<string>();
        for (const participant of value.participants) {
            if (!participant || typeof participant !== "object"
                || Object.keys(participant).some(key => !["id", "name", "speaking", "is_self", "avatar"].includes(key))
                || typeof participant.id !== "string" || !/^\d{1,24}$/.test(participant.id) || ids.has(participant.id)
                || typeof participant.name !== "string" || !participant.name || [...participant.name].length > 128
                || /[\u0000-\u001f\u007f-\u009f]/.test(participant.name)
                || typeof participant.speaking !== "boolean" || typeof participant.is_self !== "boolean") return false;
            if (participant.avatar !== undefined) {
                const avatar = participant.avatar;
                if (!avatar || typeof avatar !== "object" || Array.isArray(avatar)
                    || Object.keys(avatar).length !== 2 || Object.keys(avatar).some(key => !["user_id", "hash"].includes(key))
                    || typeof avatar.user_id !== "string" || !/^[0-9]{1,20}$/.test(avatar.user_id) || avatar.user_id !== participant.id
                    || typeof avatar.hash !== "string" || !/^(?:a_)?[a-f0-9]{32}$/.test(avatar.hash)) return false;
            }
            ids.add(participant.id);
        }
        if (value.channel_id === null && value.participants.length) return false;
    } catch { return false; }
    pending = true;
    lastRequest = Date.now();
    try {
        return await new Promise<boolean>(resolve => {
            const req = request({ hostname: "127.0.0.1", port: 9223, path: "/voice", method: "POST", agent: false,
                headers: { "Host": "127.0.0.1:9223", "Authorization": `Bearer ${token}`,
                    "Content-Type": "application/json", "Content-Length": Buffer.byteLength(snapshot), "Connection": "close" } }, res => {
                res.resume();
                resolve(res.statusCode === 204);
            });
            const deadline = setTimeout(() => { req.destroy(); resolve(false); }, 400);
            req.once("close", () => clearTimeout(deadline));
            req.once("error", () => resolve(false));
            req.end(snapshot);
        });
    } finally { pending = false; }
}
