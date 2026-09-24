# Native Discord audio adapter

The custom Vencord build includes this adapter by default. It has passed synthetic
ABI and transport tests; it has not yet been validated against a real Discord call.
It loads with the custom companion, captures only during an active Articulate
transcript, and refuses unsupported Discord voice modules. Real identities,
channel layout and timestamps remain subject to controlled-call validation.

## Verified entry point

The inspected Windows x64 voice module exports `Discord::Connect`, which accepts
`Discord::AudioReceivedCallback` and `Discord::AudioCapturedCallback`. Its received
callback ABI is:

```cpp
void(const std::string& user, const int16_t* samples,
     uint64_t samplesPerChannel, int sampleRate, uint64_t channels,
     uint32_t opaque, bool& shouldMute, float gain);
```

The sample-count/rate/channel interpretation is supported by the matching public
[Discord Social SDK audio callback](https://github.com/discord/discord-api-docs/blob/main/developers/discord-social-sdk/development-guides/managing-voice-chat.mdx).
The desktop callback's extra integer and float are deliberately passed unchanged;
they are not treated as a timestamp or an identity. Actual desktop callback values
and behavior still require controlled-call validation.

The adapter accepts only these inspected module SHA-256 values:
`2bf2290dff75933c6728bbd8d80c4876d131a808f8cea9e1103e40a12e64b014`
and `69e9b852c253487c7eafa8f62f12895a00c3559c57037eb2fa103e43760fd16d`.
Both expose the same decorated `Discord::Connect` callback signature and checked
entry bytes. A different Discord build fails without modifying it. No account
tokens, messages or network voice packets are inspected.

## Build and isolated tests

From a Visual Studio x64 developer shell with CMake, Ninja and Git:

```powershell
cmake -S plugins/discord-native -B .local/discord-native-build -G Ninja -DCMAKE_BUILD_TYPE=Release
cmake --build .local/discord-native-build
ctest --test-dir .local/discord-native-build --output-on-failure
```

The C++ code is limited to the MSVC `std::function` ABI boundary. MinHook 1.3.4
and Node-API headers 1.9.0 are pinned by commit. CMake installs their license texts
alongside Articulate's AGPL-3.0-or-later license.

The harness hooks a synthetic C++ member function in its own process. It verifies
the hidden return-value ABI, original callback forwarding, unchanged samples and
mute flags, separate synthetic identities, bounded buffering, nonce invalidation
and stopping while old callbacks still exist. It never loads Discord's module.

## Renderer loading path for a controlled test

Vencord's `native.ts` runs in the main process, whereas this addon must load in the
renderer containing `discord_voice.node`. The custom installation puts
`articulate_discord_audio.node` and `articulate-audio-preload.cjs` beside the custom
Vencord `dist/preload.js`, then loads the latter from that preload after the normal
Discord preload. `preload.cjs` supplies the source for this additional preload.
Articulate's companion setup stages these files and a guarded loader
before swapping the complete build, with every artifact covered by its manifest.
Packaged app builds contain the precompiled addon, so end users do not need a
C++ compiler.

The metadata plugin enables this bridge when it exists. Loading the addon
alone installs nothing. The bridge waits for the voice module, checks compatibility,
and installs a hook for **future** connections. An already active connection needs
a deliberate leave/rejoin after activation; the adapter does not force that action.
There must be no unrelated/public call during the first controlled test.

The bridge polls authenticated `GET /capture` every 250 ms after successful
installation. Articulate supplies an active capture nonce, channel ID and allowed
remote participant IDs as decimal strings. Capture stops when inactive, unavailable,
disabled or its 1.5-second native lease expires. The hook stays dormant after stop
and its DLL is pinned until process exit, so surviving connection callbacks never
point into unloaded code. Installed Discord files are not rewritten by the addon.

A transient control-request failure has a bounded 1.2-second recovery window
from the last successful arm, without extending the native lease. An explicit
inactive response or disable stops immediately. Stale responses cannot re-arm
an intervening stop. When the same recording resumes, packet numbering continues;
only a new capture nonce starts at one. Any queued audio discarded on stop stays
visible as a forward sequence gap rather than being replayed.

Control, audio draining, diagnostics and HTTP deadlines use `node:timers`
explicitly. Chromium window timers can throttle a background Discord renderer,
allowing the bounded audio queue to fill between drain attempts. A synthetic
six-participant, 100-packets-per-second-per-person test exercises both a one-second
consumer pause and normal 20ms draining; this does not establish live timing.

The companion submits authenticated, numeric-only `POST /diagnostics` snapshots
every five seconds and after an audio transport failure. Articulate keeps one
bounded `logs/discord-audio.json` file in its data folder, containing queue/drop
counters, scheduling/request delays and receiver sequence-failure counters.
No audio, transcript text, participant IDs, capture nonces or pairing keys are
included. Periodic disk writes are rate-limited; terminal failures are saved
immediately. Diagnostic failures never change capture state.

## PCM transport

The control response advertises `pcm_batch: true`. The companion then sends
ordered `POST /pcm-batch` requests using the existing loopback pairing key in an
authorization header. Each batch contains up to 64 packets within 128 KiB, each
preceded by its `u32` little-endian byte length. Only one audio request is in
flight at a time. Older receivers retain the single-packet `POST /pcm` route.
Each packet is a 64-byte little-endian header followed by interleaved signed PCM16:

| Offset | Field |
| --- | --- |
| 0 | ASCII `APCM` |
| 4 | `u16` version, 1 |
| 6 | `u16` header length, 64 |
| 8 | `u64` native Connect callback serial, not a call-wide generation |
| 16 | `u64` sequence, global within a capture, starting at 1 |
| 24 | `u64` observation Unix microseconds |
| 32 | `u64` remote participant ID |
| 40 | `u32` sample rate |
| 44 | `u16` channel count |
| 46 | `u16` flags, zero |
| 48 | `u32` samples per channel |
| 52 | `u64` capture nonce supplied by Articulate |
| 60 | Four reserved zero bytes |

The maximum is 5,760 samples per channel, two channels and 96 kHz. The fixed queue
holds 128 packets in a bounded multi-producer ring. Audio callbacks never take the
consumer/control lock, wait for the consumer, or allocate memory. True overflow
and exhausted producer reservation retries remain visible as sequence gaps.
The receiver recovers gaps of up to four packets, with a limit of eight missing
packets in five seconds. It saves an interruption warning with that transcript
and retains timestamp gaps on the separate participant tracks. Larger or repeated
loss stops capture. Callback serials are tracked independently for each participant:
newer callbacks replace that person's old source, and late old-source packets are
acknowledged without recording duplicate audio. Different people may use different
serials at the same time. Same-person replacement trims already received samples
and preserves timestamp gaps. Replayed/reordered packets, stale nonces, unexpected
identities and voice-channel changes remain rejected. Observation time is the
native callback's wall-clock time, not a verified source-clock timestamp. Real
sample-clock alignment is a remaining validation task.
