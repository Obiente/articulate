# Articulate companion for Vencord

The optional companion sends current voice channel membership, usernames and
speaking activity and separate participant audio directly to Articulate on the
same computer. It does not require Discord's debugging port. Audio capture runs
only while Articulate is recording. It does not read messages or account tokens,
and does not send anything to an internet service.

Speaker labels use the server nickname when one is set, then the display name,
and fall back to the account username only when neither is available.
Participant identity continues to use the stable Discord user ID.

This is a **custom Vencord userplugin**, not a plugin available in Vencord's
official plugin list. Vencord requires a source build to use custom plugins;
putting loose plugin files beside a normal installation does not load them.

## Install from Articulate

Open the Vencord companion setup in Articulate. Detection distinguishes an
installed Vencord build from a usable source checkout. If you already use a
custom checkout, select its `package.json`; Articulate keeps its other plugins.
Otherwise choose automatic setup to prepare a managed source folder.

Articulate uses Node.js 22 or newer if it is already available. Otherwise, on
Windows it downloads a pinned official Node.js runtime into its own data folder,
verifies the SHA-256 from the official release checksums and uses it only for
these child build processes. No administrator access or system PATH change is
needed. Articulate also downloads a pinned official Vencord source archive,
verifies its SHA-256, installs the pinned pnpm build dependencies, and builds the
companion. Downloads use HTTPS only and are streamed to a temporary file while
their digest is computed. Dependencies are installed with `--ignore-scripts`, so
no package in the pinned lockfile runs code on this machine during installation;
the build needs only esbuild, whose Windows binary arrives as an optional
dependency rather than through its install script. These preparation steps do not
close or change Discord. The managed build disables Vencord's own source updater
so it cannot replace or race Articulate's prepared build. Existing user-selected
checkouts keep their own updater configuration.

When the build is ready, choose **Install custom build**. This opens the official
Vencord installer in custom-build mode. Select the Discord installation you want
to patch there. **The installer closes the selected Discord client, disconnecting
any active call.** Restart Discord after installation and enable the Articulate
plugin. Articulate checks the injector's actual source target; a source build on
disk alone does not prove that the running Discord client has loaded it.

The managed source stays in Articulate's local data directory. Keep it there while
using this custom build because the Discord injector references its `dist` folder.
No machine-wide environment variables are changed and existing Vencord settings
are not copied, rewritten or moved.

## Updating the companion

Articulate ships the matching companion files with every app version. If you opt
in to keeping it updated, it compares local file hashes and rebuilds the selected
checkout when its managed companion revision changes. It does not restart or
reinject Discord automatically. Restart Discord when the update is ready.
Articulate also checks a configured companion for updates when it opens, even
when automatic installation is off. An update or restart notice appears in the
global notification menu and opens the relevant Settings section. Checking does
not change the running Discord client.

Only Articulate's three files and ownership manifest inside
`src/userplugins/articulate` are managed. Unknown files, edited managed files,
links and redirected directories are rejected instead of overwritten. Previous
managed source and compiled output are retained as backups. A complete build is
prepared in a staging folder before replacing `dist`; a failed build restores
the previous companion source and keeps the old compiled output.
On Windows, build tools and their child processes are terminated together if a
build times out or Articulate closes.

For manual installation, follow Vencord's
[custom plugin instructions](https://docs.vencord.dev/installing/custom-plugins/)
and [source installation instructions](https://docs.vencord.dev/installing/).
Copy `plugins/vencord/articulate` into `src/userplugins/articulate`, rebuild and
install the custom build. Manual copies with different or modified source are
not automatically adopted as app-owned files.

## Pairing

1. In Articulate's Discord speaker setup, select the Vencord companion and connect.
2. Enable **Articulate** in Vencord's plugins. The current companion pairs
   automatically with Articulate on the same Windows account.
3. Join a voice channel. Articulate should report a connected speaker source.

No key needs to be copied or pasted. The companion's native helper reads only
the pairing key from Articulate's local preferences and refreshes it every three
seconds. It does not expose other preferences or offer a network endpoint for
retrieving the key. Settings offers **Rotate pairing key**, which generates a new
key that the current companion picks up within a few seconds while every copy
of the old key stops working, and **Forget pairing**, which removes the key and
stops listening until you connect again.

If you installed an older custom build that still asks for a pairing key, update
the companion from Articulate's setup, wait for the build to finish, and restart
Discord. If that checkout is not the one Discord currently loads, choose
**Open Discord installer** and select your Discord installation first. Reconnect
from Articulate after the update; rebuilding source files alone does not replace
the plugin already running in Discord.

The local protocol is versioned. A companion speaking an older version is
refused with an **Update the companion** state in Settings instead of being
trusted, and a companion newer than Articulate reports **Update Articulate**.
The companion shows the same guidance in Discord's console.

After pairing, Articulate automatically opens its receiver on startup and the
companion reconnects. Explicitly disconnecting disables automatic connection;
closing Articulate closes the listener. Reconnecting retries the local pairing;
the current companion picks up key changes automatically. The random key stays
in Articulate's local settings and is held in memory by the companion. It
authorizes submitting speaker metadata and participant audio during an active
capture. Authentication remains required for every local protocol request.

The optional **Transcribe calls automatically** setting starts a transcript on a
confirmed voice-channel join when the speech model and separate-audio adapter
are ready. A confirmed leave finishes only a transcript that this setting
started. Brief metadata gaps are tolerated; five seconds without confirmed
presence stops an automatic capture. Manually finishing an automatic transcript
prevents another start until you leave and join again or choose **Retry automatic
capture**. Settings shows pairing, participant audio readiness, and any reason
automatic capture is waiting.

If participant audio is interrupted and then resumes in the same authorized
capture within 15 seconds, recording continues. The native hook stops copying
audio during a control outage and rearms only after fresh membership is verified.
Missing local audio packets and transport interruptions are counted and the
transcript shows a missing-audio warning; the original timing gap is preserved.
Packet loss alone does not end the call. A changed voice channel, expired local
capture permission, invalid audio or exhausted capture buffer still stops it.
Audio delivered after a section was finalized is trimmed at that boundary;
new audio continues and the missing-audio warning is retained.

Articulate uses a bundled local voice detector before speech recognition to
reject silence and background noise. The detector works on each audio clip
independently and does not filter text by language.

The companion badge compares the version reported by the running Discord
renderer with the companion bundled in this Articulate build. A successful
build on disk does not confirm that Discord loaded it. **Restart Discord**
means the prepared files and the running plugin differ; **Update needed** means
the prepared files need updating. A disconnected companion cannot be verified.
Updating this repository alone does not update an already running Articulate
executable: rebuild and reopen the app before updating the companion.

If speaker names connect but participant audio does not, Settings reports the
adapter state separately. A missing adapter, unsupported voice version, or
failed attachment prevents separate-track capture. It does not silently record
mixed system audio instead.

Actual recording starts and stops produce distinct sounds and a brief overlay
without taking focus from Discord. **Sound feedback** in Settings controls the
sounds. Failed starts show an error instead of playing the recording-start cue.

## What this provides

The custom build includes the [native audio adapter](../plugins/discord-native/README.md)
by default. On its supported Windows Discord build it hooks the native receive
callback for individual participant PCM, preserving each participant's identity
through overlapping speech. Synthetic tests pass; real Discord call behavior
still needs controlled validation. While the companion is selected, capture
requires its audio connection and never silently switches to mixed output.
To record other apps, explicitly switch to mixed output capture in Setup.
Mixed output uses activity-based attribution or acoustic speaker recognition
and cannot reliably separate overlap.
It accepts two inspected native-module builds listed in the adapter README.
Compatibility with other Discord voice-module versions is not yet verified.

## Local protocol

Protocol version 2. The native Node bridge and the audio adapter each hold one
persistent HTTP/1.1 keep-alive connection to the fixed address `127.0.0.1:9223`.
No remote host, proxy, redirect or user-supplied URL is used. Articulate reserves
that port with `SO_EXCLUSIVEADDRUSE`, so no other local socket can share it.

Every connection starts with a mutual challenge-response on the 256-bit pairing
key. The companion sends `POST /session` with a random nonce and nothing else.
Articulate answers with its own nonce and
`HMAC-SHA256(key, "articulate-companion-v2/server" || client nonce || server nonce)`.
The companion verifies that proof in constant time before sending anything that
needs the key; a listener that cannot prove the key receives only the nonce.
It then sends `POST /session/confirm` with
`HMAC-SHA256(key, "articulate-companion-v2/client" || client nonce || server nonce)`.
Articulate compares it in constant time and marks that connection authenticated.
Later requests on the connection carry `Authorization: Articulate-Session <id>`,
which must match the session that authenticated it; the key itself never crosses
the socket in either direction, and a new connection must pair again. A request
on an unauthenticated connection is answered `401`, a wrong proof `403`, and a
version mismatch `426` together with the supported version, which the user sees
as an **update** state. Articulate serves up to eight connections concurrently,
closes a connection that is idle for fifteen seconds or whose request stalls for
half a second, and the companion retries a failed audio batch a bounded number of
times before reporting the transport as unavailable.

The receiver rejects browser origins, duplicate headers, chunked framing,
oversized payloads, unknown payload fields, invalid or repeated participant IDs,
and delayed snapshots. Snapshots carry a connection-scoped sequence number that
must increase, so a wall-clock change can neither reorder nor stall them; the
timestamp is checked only against a bounded freshness window. It invalidates
speaker state after 750 ms without updates. Snapshots normally arrive every
150 ms and on voice store changes, capped at 20 per second by the companion,
which throttles with a monotonic clock. Only its own change listeners are removed
on stop.

The `/voice` metadata bridge follows Vencord's
[native plugin interface](https://docs.vencord.dev/plugins/natives/); it does not
change Discord's content security policy or replace Discord callbacks. The
separate participant-audio adapter wraps the verified native receive callback,
preserving its original behavior before copying allowed audio to Articulate.

The companion sources use this repository's AGPL-3.0-or-later license. Vencord itself retains
its own license and distribution requirements. No Vencord binary is bundled here.

## Validation

The companion targets Vencord commit
`59a54286542651fff5ea53f0ce6cadf2a6aa7521` with its locked dependency graph.
Receiver tests cover the mutual handshake, session binding, version mismatches,
request validation, actual TCP publication over concurrent keep-alive
connections, sequence ordering, connection limits, stale state invalidation and
shutdown. The native IPC validation covers payload bounds, duplicate
participants, forbidden fields, stale samples and the fixed endpoint. A
protocol-version-2 typecheck against the pinned checkout and a controlled call
with the rebuilt companion remain to be run.
Installer tests cover file ownership, preserving other plugins, rollback,
compiled artifact validation and reading the configured injector target. A
separate opt-in smoke test downloads and builds the pinned source with an isolated
data folder and no Node.js, pnpm or Git on PATH; it does not run the installer.
A custom Vencord installation and real voice call remain necessary to verify
runtime store compatibility on a particular Discord release.
