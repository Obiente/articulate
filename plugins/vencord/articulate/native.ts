import type { IpcMainInvokeEvent } from "electron";
import { request } from "node:http";

let pending = false;
let lastRequest = 0;

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
            || !(value.channel_id === null || (typeof value.channel_id === "string" && /^\d{1,24}$/.test(value.channel_id)))
            || Object.keys(value).some(key => !["version", "observed_ms", "channel_id", "participants", "valid"].includes(key))) return false;
        const ids = new Set<string>();
        for (const participant of value.participants) {
            if (!participant || typeof participant !== "object"
                || Object.keys(participant).some(key => !["id", "name", "speaking", "is_self"].includes(key))
                || typeof participant.id !== "string" || !/^\d{1,24}$/.test(participant.id) || ids.has(participant.id)
                || typeof participant.name !== "string" || !participant.name || [...participant.name].length > 128
                || /[\u0000-\u001f\u007f-\u009f]/.test(participant.name)
                || typeof participant.speaking !== "boolean" || typeof participant.is_self !== "boolean") return false;
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
