# Articulate companion for Vencord

The optional companion sends current voice channel membership, display names and
speaking activity directly to Articulate on the same computer. It does not require
Discord's debugging port. It does not read messages, account tokens or recorded
audio, and does not send anything to an internet service.

This is **source for a custom Vencord userplugin**, not a plugin available in
Vencord's official plugin list. Follow Vencord's
[custom plugin installation instructions](https://docs.vencord.dev/installing/custom-plugins/)
to prepare a source build. Copy `plugins/vencord/articulate` from this repository
into that checkout's `src/userplugins/articulate` directory, then build and inject
your custom Vencord build using its instructions. Restart Discord fully after
building native plugin code. Articulate does not install or modify Discord itself.

## Pairing

1. In Articulate's Discord speaker setup, select the Vencord companion and connect.
2. Copy the pairing key shown by Articulate.
3. Enable **Articulate** in Vencord's plugins and paste the key into its settings.
4. Join a voice channel. Articulate should report a connected speaker source.

Articulate opens its receiver only after you choose to connect. Disconnecting or
closing Articulate closes the listener. The companion can remain enabled and
reconnect when Articulate is listening again. Changing the pairing key requires
updating it on both sides. Treat the key like a local password; it authorizes
submitting speaker metadata. Both apps store it in their local settings.

## What this provides

Speaker activity improves name attribution for the existing microphone and call
audio transcript. The companion **does not separate individual participants'
audio streams**. Discord's mixed output remains the audio input, and overlapping
speech can remain uncertain. No verified remote PCM interface is implemented.

## Local protocol

The native Node bridge uses an authenticated HTTP POST to the fixed address
`127.0.0.1:9223/voice`. No remote host, proxy, redirect or user-supplied URL is used.
The 256-bit random key appears only in an authorization header, never in a URL or
log. The receiver rejects browser origins, duplicate headers, chunked framing,
oversized payloads, unknown payload fields, invalid or repeated participant IDs,
and delayed snapshots. It invalidates speaker state after 750 ms without updates.
Snapshots normally arrive every 150 ms and on voice store changes, capped at
20 per second by the companion. Only its own change listeners are removed on stop.

The native bridge follows Vencord's
[native plugin interface](https://docs.vencord.dev/plugins/natives/); it does not
change Discord's content security policy or replace Discord callbacks.

The companion sources use this repository's MIT license. Vencord itself retains
its own license and distribution requirements. No Vencord binary is bundled here.

## Validation

The companion typechecks in Vencord commit
`59a54286542651fff5ea53f0ce6cadf2a6aa7521` with its locked dependency graph.
Receiver tests cover pairing, request validation, actual TCP publication, stale
state invalidation and shutdown. The native IPC validation covers payload bounds,
duplicate participants, forbidden fields, stale samples and the fixed endpoint.
A custom Vencord installation and real voice call remain necessary to verify
runtime store compatibility on a particular Discord release.
