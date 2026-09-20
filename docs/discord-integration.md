# Discord participant attribution

Articulate offers an optional connection to the user-enabled local debugger in the official Discord desktop client. It reads current voice participants and speaking activity, then uses that metadata to support conservative transcript labels. Microphone and output audio capture and speech recognition remain local. This is an internal client interface, not a supported Discord API, and it can change between Discord releases.

## Connect the desktop client

1. In Articulate's **Calls > Setup**, choose **Relaunch Discord**, then **Relaunch and connect**. This disconnects any active Discord call. Articulate preserves its current transcript and capture session.
2. Articulate finds the latest stable client beneath `$env:LOCALAPPDATA`, closes only matching installed Discord processes, and opens it with `--remote-debugging-port=9222 --remote-debugging-address=127.0.0.1`. This runs in the background without modifying installed files. Articulate connects automatically afterward.
3. Rejoin your Discord voice channel. The card shows current participants and who is speaking. If Discord is already running with debugging enabled, use **Connect** without relaunching. **Manual connection** also retains the copyable PowerShell command; quit Discord completely before running it.
4. Select the output device Discord uses and start call capture. The microphone remains **You**. Names are used only where the activity evidence is sufficiently consistent; uncertain sections keep acoustic labels.
5. Disconnect in Articulate when finished. This stops its observer; it does not disable Discord's debugger. Quit Discord and reopen it normally to remove debugging access.

The debugger grants broad access to the local Discord renderer. Keep it on loopback, do not forward the port, and enable it only when wanted. Articulate does not read messages, account tokens, or account storage, and does not modify Discord's installed files or substitute its native audio callbacks. No Discord developer application or approved RPC scope is needed for this route. Discord itself still requires its normal network connection for calls.

Names and activity are held in bounded process memory. Copied or saved transcripts contain the display-name labels. A speaking indicator supplies metadata, not participant-separated audio: simultaneous speech remains overlap, and unrelated sounds from the same output device cannot reliably be assigned to a Discord participant. Timing alignment and real-world accuracy require controlled-call validation; local speech recognition remains fallible.

Developers can check connectivity without capturing audio:

```powershell
.\target\release\transcribe-local.exe --discord-check --seconds 10
```

The check prints aggregate counts, not names or account IDs.

## What StreamKit actually does

The official StreamKit voice widget does connect to the local Discord desktop client. Its current published source connects to `ws://127.0.0.1:PORT/?v=1&client_id=CLIENT_ID`, trying ports 6463 through 6472. It uses StreamKit's registered application identity, requests `rpc` and `messages.read`, and sends the resulting authorization code to the hosted `https://streamkit.discord.com/overlay/token` service. That service exchanges OAuth tokens; the widget stores its token in its own browser storage. The source identifies this service as a Cloudflare worker.

The voice widget obtains participants through `GET_CHANNEL`, then listens for voice-state and speaking events. It does not receive audio through those events. Loading the official widget also depends on hosted JavaScript and remote avatar images. Therefore, StreamKit is neither an unauthenticated localhost identity service nor an entirely offline integration.

Evidence: [official overlay](https://streamkit.discord.com/overlay), [Discord's installation and authorization guide](https://support.discord.com/hc/en-us/articles/223415707-Using-Discord-s-OBS-Streamkit-Overlay), and the deployed [JavaScript source map](https://streamkit.discord.com/static/js/main.daa4bcc9.js.map), specifically `Constants.tsx`, `lib/RPCClient.tsx`, and `components/Voice.tsx`. The hashed asset URL can change on deployment.

## Current supported transport and access constraints

Discord now marks WebSocket RPC as deprecated and available only to earlier private-beta participants. Its preferred native transport is IPC. On Windows, probe `\\?\pipe\discord-ipc-{n}`. Frames contain little-endian 32-bit opcode and payload length followed by JSON. Start with opcode 0 and `{"v":1,"client_id":"YOUR_APPLICATION_ID"}`; subsequent commands use opcode 1. Handle close, ping, and pong opcodes 2, 3, and 4.

The protocol supports these relevant operations:

| Operation | Information |
| --- | --- |
| `GET_SELECTED_VOICE_CHANNEL` | Current voice channel, or null |
| `GET_CHANNEL` | Participant `voice_states`, including `user.id`, `user.username`, and `nick` |
| `VOICE_CHANNEL_SELECT` | Current channel changes |
| `VOICE_STATE_CREATE`, `VOICE_STATE_UPDATE`, `VOICE_STATE_DELETE` | Participants joining, changing, or leaving |
| `SPEAKING_START`, `SPEAKING_STOP` | `data.user_id`; subscribe with `args.channel_id` |

Speaking events provide no audio timestamp. RPC commands require authentication. Unapproved applications are restricted to an approved tester list, with up to 50 testers. See [Discord RPC documentation](https://docs.discord.com/developers/topics/rpc).

Both `rpc` and `rpc.voice.read` are currently limited to approved partners. An Articulate integration needs its own application registration, eligible access, and user authorization. The documented authorization-code exchange requires application credentials, so distributing a secret in a desktop executable is not an acceptable production design. A developer-owned test application or a separately reviewed authentication arrangement would be needed. Do not assume an unverified public-client flow works for these restricted RPC scopes. See [Discord OAuth2 scopes and authorization flow](https://docs.discord.com/developers/topics/oauth2).

Do not reuse StreamKit's client identity, impersonate its origin, extract Discord user tokens, or read StreamKit's browser token storage. An embedded official widget could retain its own authorized session, but that would still add a hosted dependency and would not constitute a documented participant-event API for Articulate.

## Supported RPC alternative

The following describes a future supported RPC transport. The current implementation uses the optional local debugger described above. Neither transport supplies participant-separated audio.

1. Add an optional Rust IPC worker with bounded event delivery, cancellation, reconnect backoff, payload-size limits, and nonce-correlated command replies. Enable it only after an explicit connection action and valid authorization. Keep inference independent of this worker.
2. Track participants by Discord user ID, with a session-scoped display-name snapshot. Refresh the roster after authentication and channel changes. Subscribe to the new channel and unsubscribe from the old one. Never infer account identity from a voice embedding alone.
3. Timestamp incoming events with the same monotonic clock used by `call_capture::Control`. Expose a small clock method rather than starting a second independent timer. Buffer activity spans alongside the bounded revisable processing windows in `calls::run`.
4. Treat event arrival time as an estimate. Discord activity indicators, IPC delivery, and WASAPI playback have different latency. Measure their offset with controlled calls before choosing a tolerance. Do not invent a universal fixed alignment value.
5. Combine activity intervals with acoustic turns in `calls::process_window`. A single consistent remote participant can support a name assignment when timing and acoustic evidence agree. Retain uncertainty near boundaries or when coverage is weak. The authenticated local user remains `You` on the microphone track.
6. Preserve multiple active speakers as overlap. A speaking indicator does not separate mixed voices or recover missing words. Do not assign all overlapping text to whichever event arrived last. A muted local playback participant or unrelated desktop audio can also make activity diverge from captured sound.
7. On disconnect, channel switch, or missing state, close active intervals and mark the gap unknown. After reconnect, clear stale activity, rebuild the roster, and resubscribe. Preserve completed transcript attribution and fall back to anonymous acoustic labels for uncertain spans.

Prefer process-specific Discord loopback as a later audio-capture enhancement. The current output-device loopback includes other applications; participant metadata cannot reliably identify those sounds. Process capture requires its own Windows compatibility and device-routing validation.

## Attribution requirements

Call rows retain acoustic labels and can carry a Discord identity with a display-name snapshot. Identity must distinguish session generations and channels, and survive grouping, exports, and notes. Merge consecutive rows only when their identity and source match, retaining the existing single-label behavior. A name must never be learned permanently from a single acoustic cluster match.

Keep OAuth state out of ordinary settings and exports. Store any persistent authorization material using platform-protected storage, avoid token logging, and offer disconnect/revoke behavior. Only participant metadata needs this optional connector; speech recognition and notes remain local.

Required validation includes synthetic event sequences, rapid speaker changes, overlaps, local mute/deafen, duplicate or missing stop events, reconnects, channel changes, Unicode names, worker cancellation, and timestamp alignment against real captured playback. Controlled audio tests should use consenting participants. Any future supported RPC transport must use Articulate's own eligible application; the debugger connection does not reuse StreamKit credentials.
