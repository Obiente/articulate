import type { IpcMainInvokeEvent } from "electron";
import { lstat, open } from "node:fs/promises";
import { isAbsolute, join } from "node:path";

// Protocol version 2: a mutual HMAC-SHA256 handshake over the pairing key on a
// persistent loopback connection. The key itself never crosses the socket, and
// the session that results is bound to the socket object that completed the
// handshake: a connection the agent opens later has proven nothing, so it
// receives a fresh handshake before any voice metadata.
const PROTOCOL_VERSION = 2;
const ENDPOINT = { hostname: "127.0.0.1", port: 9223, host: "127.0.0.1:9223" };
const SERVER_LABEL = "articulate-companion-v2/server";
const CLIENT_LABEL = "articulate-companion-v2/client";
const audioStates = new Set(["disabled", "addon-unavailable", "waiting", "waiting-for-voice-engine",
    "unsupported-native-build", "native-hook-unavailable", "waiting-for-articulate", "ready",
    "capturing", "control-unavailable", "audio-transport-unavailable", "preload-unavailable", "update-required"]);

type TransportState = "ok" | "unavailable" | "outdated" | "newer" | "rejected";
interface Session { id: string; key: string; sequence: number; socket: object; }
let pending = false;
let lastRequest = -Infinity;
let session: Session | null = null;
let transportState: TransportState = "unavailable";
let agent: import("node:http").Agent | null = null;
const monotonic = () => typeof performance !== "undefined" ? performance.now() : Date.now();

const hex64 = (value: unknown): value is string => typeof value === "string" && /^[a-f0-9]{64}$/.test(value);
const hex32 = (value: unknown): value is string => typeof value === "string" && /^[a-f0-9]{32}$/.test(value);
const keyLike = (value: unknown): value is string => typeof value === "string" && /^[a-f0-9]{64}$/i.test(value);

// Reads one small JSON object out of the same user's fixed Articulate data
// folder. Links, oversized files and anything but an object are refused.
async function readJsonObject(directory: string, name: string): Promise<Record<string, unknown> | null> {
    const path = join(directory, name);
    try {
        const parent = await lstat(directory);
        const metadata = await lstat(path);
        if (!parent.isDirectory() || parent.isSymbolicLink()
            || !metadata.isFile() || metadata.isSymbolicLink() || metadata.size > 1024 * 1024) return null;
        const file = await open(path, "r");
        try {
            const current = await file.stat();
            if (!current.isFile() || current.size > 1024 * 1024) return null;
            const bytes = Buffer.alloc(1024 * 1024 + 1);
            let length = 0;
            while (length < bytes.length) {
                const read = await file.read(bytes, length, bytes.length - length, null);
                if (!read.bytesRead) break;
                length += read.bytesRead;
            }
            if (length > 1024 * 1024) return null;
            const value = JSON.parse(bytes.subarray(0, length).toString("utf8"));
            return value && typeof value === "object" && !Array.isArray(value) ? value : null;
        } finally { await file.close(); }
    } catch { return null; }
}

// Articulate publishes the pairing key in a small dedicated file, because its
// settings file may be encrypted at rest; older versions kept the key only in
// plaintext settings, which remain the fallback. Never accept a
// renderer-supplied path or expose any other preference through IPC.
export async function getPairingKey(_event: IpcMainInvokeEvent): Promise<string> {
    const localData = process.env.LOCALAPPDATA;
    if (process.platform !== "win32" || !localData || !isAbsolute(localData)) return "";
    // This directory name is retained by Articulate for existing user profiles.
    const directory = join(localData, "TranscribeLocal");
    const pairing = await readJsonObject(directory, "discord-pairing.json");
    if (pairing?.version === 1 && keyLike(pairing.key)) return pairing.key;
    const settings = await readJsonObject(directory, "settings.json");
    const key = settings?.discord_pairing_key;
    return keyLike(key) ? key : "";
}

/// Why the last publish failed, so the renderer can show an update notice.
export async function getTransportState(_event: IpcMainInvokeEvent): Promise<TransportState> {
    return transportState;
}

interface Reply { status: number; body: Buffer; socket: object; }
interface Replaced { replaced: true; }
// The connection a request must travel on, and the session it carries.
interface Bind { id?: string; socket?: object; }
const REPLACED: Replaced = Object.freeze({ replaced: true });
const isReply = (value: Reply | Replaced | null): value is Reply => !!value && !("replaced" in value);

// One keep-alive socket carries every request in order. The lazily created
// agent keeps module loading free of network objects. Nothing is written
// before the agent has assigned a connection; a connection other than the
// bound one belongs to an unproven listener, so the request is dropped unsent
// and the caller pairs again.
async function exchange(path: string, body: Buffer, bind: Bind | null): Promise<Reply | Replaced | null> {
    const http = await import("node:http");
    agent ??= new http.Agent({ keepAlive: true, maxSockets: 1, maxFreeSockets: 1, timeout: 10_000 });
    const headers: Record<string, string | number> = {
        Host: ENDPOINT.host, "Content-Type": "application/json", "Content-Length": body.length,
    };
    if (bind?.id) headers.Authorization = `Articulate-Session ${bind.id}`;
    return new Promise<Reply | Replaced | null>(resolve => {
        let finished = false;
        let socket: object | null = null;
        const finish = (value: Reply | Replaced | null) => { if (!finished) { finished = true; resolve(value); } };
        const req = http.request({ hostname: ENDPOINT.hostname, port: ENDPOINT.port, path, method: "POST", agent, headers }, res => {
            const chunks: Buffer[] = [];
            let size = 0;
            res.on("data", (chunk: Buffer) => {
                size += chunk.length;
                if (size > 65536) { req.destroy(); finish(null); } else chunks.push(chunk);
            });
            res.on("end", () => finish(socket ? { status: res.statusCode ?? 0, body: Buffer.concat(chunks), socket } : null));
            res.on("error", () => { finish(null); req.destroy(); });
        });
        const deadline = setTimeout(() => { req.destroy(); finish(null); }, 400);
        req.once("close", () => { clearTimeout(deadline); finish(null); });
        req.once("error", () => { finish(null); req.destroy(); });
        req.once("socket", (assigned: object) => {
            if (bind?.socket && assigned !== bind.socket) { finish(REPLACED); req.destroy(); return; }
            socket = assigned;
            req.end(body);
        });
    });
}

// Articulate must prove it holds the pairing key before this side proves
// anything. A listener without the key receives only a random nonce, and the
// proof answers the very connection that produced the server's proof.
async function handshake(key: string): Promise<Session | null> {
    const crypto = await import("node:crypto");
    const keyBytes = Buffer.from(key, "hex");
    const clientNonce = crypto.randomBytes(32);
    const opened = await exchange("/session", Buffer.from(JSON.stringify({ version: PROTOCOL_VERSION, client_nonce: clientNonce.toString("hex") })), null);
    if (!isReply(opened)) { transportState = "unavailable"; return null; }
    if (opened.status === 426) {
        let minimum = 0;
        try { minimum = Number(JSON.parse(opened.body.toString("utf8"))?.min_version); } catch { /* Treated as an unknown version below. */ }
        transportState = minimum > PROTOCOL_VERSION ? "outdated" : "newer";
        return null;
    }
    if (opened.status !== 200) { transportState = "rejected"; return null; }
    let reply: { version?: unknown; session?: unknown; server_nonce?: unknown; server_proof?: unknown; };
    try { reply = JSON.parse(opened.body.toString("utf8")); } catch { transportState = "rejected"; return null; }
    if (!reply || reply.version !== PROTOCOL_VERSION || !hex32(reply.session) || !hex64(reply.server_nonce) || !hex64(reply.server_proof)) {
        transportState = reply?.version !== undefined && reply.version !== PROTOCOL_VERSION ? "newer" : "rejected";
        return null;
    }
    const serverNonce = Buffer.from(reply.server_nonce, "hex");
    const expected = crypto.createHmac("sha256", keyBytes).update(SERVER_LABEL).update(clientNonce).update(serverNonce).digest();
    if (!crypto.timingSafeEqual(expected, Buffer.from(reply.server_proof, "hex"))) {
        // Whatever listens there does not hold our key. Send nothing further.
        transportState = "rejected";
        return null;
    }
    const clientProof = crypto.createHmac("sha256", keyBytes).update(CLIENT_LABEL).update(clientNonce).update(serverNonce).digest("hex");
    const confirmed = await exchange("/session/confirm", Buffer.from(JSON.stringify({ version: PROTOCOL_VERSION, session: reply.session, client_proof: clientProof })), { socket: opened.socket });
    if (!isReply(confirmed) || confirmed.status !== 204) { transportState = confirmed ? "rejected" : "unavailable"; return null; }
    transportState = "ok";
    return { id: reply.session, key, sequence: 0, socket: opened.socket };
}

// A deliberately narrow IPC operation. No caller-supplied URL, path or headers.
export async function publish(_event: IpcMainInvokeEvent, token: string, snapshot: string): Promise<boolean> {
    if (typeof token !== "string" || !/^[a-f0-9]{64}$/i.test(token)
        || typeof snapshot !== "string" || Buffer.byteLength(snapshot) > 128 * 1024
        || pending || monotonic() - lastRequest < 40) return false;
    let value: Record<string, unknown>;
    try {
        value = JSON.parse(snapshot);
        if (!value || typeof value !== "object" || value.version !== PROTOCOL_VERSION
            || !Number.isSafeInteger(value.observed_ms) || Math.abs(Date.now() - (value.observed_ms as number)) > 250
            || !Array.isArray(value.participants) || value.participants.length > 256
            || typeof value.valid !== "boolean"
            || (value.companion_revision !== undefined && !hex64(value.companion_revision))
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
    lastRequest = monotonic();
    try {
        const key = token.toLowerCase();
        if (session && session.key !== key) session = null;
        for (let attempt = 0; attempt < 2; attempt++) {
            session ??= await handshake(key);
            if (!session) return false;
            // The sequence orders snapshots within this session; wall time is
            // only checked for freshness, so a clock change cannot stall them.
            const sequence = session.sequence + 1;
            const body = Buffer.from(JSON.stringify({ ...value, sequence }));
            const reply = await exchange("/voice", body, session);
            if (!reply) { session = null; transportState = "unavailable"; return false; }
            // The agent opened another connection: whatever answers there has
            // not proven the key, so the snapshot waits for a fresh handshake.
            if (!isReply(reply)) { session = null; continue; }
            if (reply.status === 204) { session.sequence = sequence; transportState = "ok"; return true; }
            if (reply.status === 401) { session = null; continue; }
            if (reply.status === 426) { session = null; transportState = "outdated"; return false; }
            // Throttled or reordered snapshots are dropped by a healthy, paired receiver.
            if (reply.status === 429 || reply.status === 409) { session.sequence = sequence; transportState = "ok"; return reply.status === 429; }
            transportState = "rejected";
            return false;
        }
        return false;
    } finally { pending = false; }
}
