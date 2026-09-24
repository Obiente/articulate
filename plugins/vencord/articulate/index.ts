import definePlugin, { PluginNative } from "@utils/types";
import { findStoreLazy } from "@webpack";

// Replaced when Articulate prepares the source, then fixed in the running build.
const compiledRevision = "__ARTICULATE_COMPANION_REVISION__";
const revisionFields = /^[a-f0-9]{64}$/.test(compiledRevision) ? { companion_revision: compiledRevision } : {};

const Native = VencordNative.pluginHelpers.Articulate as PluginNative<typeof import("./native")>;
const nativeAudio = () => (globalThis as unknown as { ArticulateNativeAudio?: {
    enable(key: string): boolean; disable(): void; status(): { state: string; installed: boolean };
} }).ArticulateNativeAudio;
const audioStates = new Set(["disabled", "addon-unavailable", "waiting", "waiting-for-voice-engine",
    "unsupported-native-build", "native-hook-unavailable", "waiting-for-articulate", "ready",
    "capturing", "control-unavailable", "audio-transport-unavailable", "preload-unavailable"]);
function audioStatus(): string {
    try {
        const state = nativeAudio()?.status().state;
        return state && audioStates.has(state) ? state : "preload-unavailable";
    } catch { return "preload-unavailable"; }
}

interface VoiceState { userId: string; }
const selected = findStoreLazy("SelectedChannelStore") as { getVoiceChannelId(): string | null; };
const voices = findStoreLazy("VoiceStateStore") as { getVoiceStatesForChannel(id: string): Record<string, VoiceState>; };
const speaking = findStoreLazy("SpeakingStore") as { isSpeaking(id: string): boolean; };
const users = findStoreLazy("UserStore") as { getCurrentUser(): { id: string; }; getUser(id: string): { globalName?: string; username: string; avatar?: string | null; } | undefined; };
const members = findStoreLazy("GuildMemberStore") as { getMember(guild: string, id: string): { nick?: string; } | undefined; };
const channels = findStoreLazy("ChannelStore") as { getChannel(id: string): { guild_id?: string; } | undefined; };
let timer: ReturnType<typeof setInterval> | undefined;
let active = false;
let pending = false;
let last = 0;
let pairingKey = "";
let nextPairingCheck = 0;
let lastHealth = "";
function health(state: string) {
    if (state === lastHealth) return;
    lastHealth = state;
    if (state !== "connected") console.warn(`[Articulate companion] ${state}`);
}

async function sample() {
    if (!active) { nativeAudio()?.disable(); return; }
    if (pending || Date.now() - last < 50) return;
    last = Date.now();
    pending = true;
    try {
        if (Date.now() >= nextPairingCheck) {
            nextPairingCheck = Date.now() + 3000;
            pairingKey = await Native.getPairingKey();
        }
        if (!active || !/^[a-f0-9]{64}$/i.test(pairingKey)) {
            health("Pairing key unavailable");
            nativeAudio()?.disable();
            return;
        }
        nativeAudio()?.enable(pairingKey);
        let snapshot;
        try {
            const channelId = selected.getVoiceChannelId() || null;
            const channel = channelId ? channels.getChannel(channelId) : null;
            const localId = users.getCurrentUser()?.id;
            const states = channelId ? Object.values(voices.getVoiceStatesForChannel(channelId)) : [];
            if (states.length > 256 || (channelId && !localId)) throw new Error("Voice state unavailable");
            const participants = states.map(state => {
                const user = users.getUser(state.userId);
                const member = channel?.guild_id ? members.getMember(channel.guild_id, state.userId) : null;
                const name = user?.username || member?.nick || user?.globalName;
                if (!name) throw new Error("Display name unavailable");
                const avatar = user?.avatar && /^(?:a_)?[a-f0-9]{32}$/.test(user.avatar) && /^[0-9]{1,20}$/.test(state.userId)
                    ? { user_id: state.userId, hash: user.avatar } : undefined;
                return { id: state.userId, name: [...name].slice(0, 128).join(""), speaking: !!speaking.isSpeaking(state.userId), is_self: state.userId === localId, ...(avatar ? { avatar } : {}) };
            });
            snapshot = { version: 1, observed_ms: Date.now(), channel_id: channelId, participants, valid: true };
        } catch {
            health("Discord voice stores unavailable");
            snapshot = { version: 1, observed_ms: Date.now(), channel_id: null, participants: [], valid: false };
        }
        if (await Native.publish(pairingKey, JSON.stringify({ ...snapshot, ...revisionFields, audio_status: audioStatus() }))) {
            if (snapshot.valid) health("connected");
        } else {
            health("Local connection unavailable");
        }
    }
    catch {
        health("Companion bridge unavailable");
        pairingKey = "";
        nativeAudio()?.disable();
        // Articulate may be closed. Never log keys or voice metadata.
    }
    finally { pending = false; }
}

export default definePlugin({
    name: "Articulate",
    description: "Share current voice channel names and speaking activity with Articulate on this computer.",
    authors: [],
    start() {
        active = true;
        nextPairingCheck = 0;
        // Store listener APIs change between Discord builds. Sampling already
        // runs frequently, so a missing listener must never disable pairing.
        timer = setInterval(sample, 150);
        void sample();
    },
    stop() {
        active = false;
        pairingKey = "";
        nativeAudio()?.disable();
        clearInterval(timer);
        timer = undefined;
    }
});
