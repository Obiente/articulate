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

interface Store { addChangeListener(callback: () => void): void; removeChangeListener(callback: () => void): void; }
interface VoiceState { userId: string; }
const selected = findStoreLazy("SelectedChannelStore") as Store & { getVoiceChannelId(): string | null; };
const voices = findStoreLazy("VoiceStateStore") as Store & { getVoiceStatesForChannel(id: string): Record<string, VoiceState>; };
const speaking = findStoreLazy("SpeakingStore") as Store & { isSpeaking(id: string): boolean; };
const users = findStoreLazy("UserStore") as Store & { getCurrentUser(): { id: string; }; getUser(id: string): { globalName?: string; username: string; avatar?: string | null; } | undefined; };
const members = findStoreLazy("GuildMemberStore") as Store & { getMember(guild: string, id: string): { nick?: string; } | undefined; };
const channels = findStoreLazy("ChannelStore") as Store & { getChannel(id: string): { guild_id?: string; } | undefined; };
let timer: ReturnType<typeof setInterval> | undefined;
let active = false;
let pending = false;
let last = 0;
let pairingKey = "";
let nextPairingCheck = 0;
const subscribed: Store[] = [];

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
        if (!active || !/^[a-f0-9]{64}$/i.test(pairingKey)) { nativeAudio()?.disable(); return; }
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
            snapshot = { version: 1, observed_ms: Date.now(), channel_id: null, participants: [], valid: false };
        }
        await Native.publish(pairingKey, JSON.stringify({ ...snapshot, ...revisionFields, audio_status: audioStatus() }));
    }
    catch {
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
        try {
            for (const store of [selected, voices, speaking]) {
                store.addChangeListener(sample);
                subscribed.push(store);
            }
            timer = setInterval(sample, 150);
            void sample();
        } catch {
            active = false;
            for (const store of subscribed.splice(0)) store.removeChangeListener(sample);
        }
    },
    stop() {
        active = false;
        pairingKey = "";
        nativeAudio()?.disable();
        clearInterval(timer);
        timer = undefined;
        for (const store of subscribed.splice(0)) store.removeChangeListener(sample);
    }
});
