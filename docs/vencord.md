# Articulate companion for Vencord

The optional companion sends current voice channel membership, display names and
speaking activity and separate participant audio directly to Articulate on the
same computer. It does not require Discord's debugging port. Audio capture runs
only while Articulate is recording. It does not read messages or account tokens,
and does not send anything to an internet service.

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
companion. These preparation steps do not
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
2. Copy the pairing key shown by Articulate.
3. Enable **Articulate** in Vencord's plugins and paste the key into its settings.
4. Join a voice channel. Articulate should report a connected speaker source.

After pairing, Articulate automatically opens its receiver on startup and the
companion reconnects. Explicitly disconnecting disables automatic connection;
closing Articulate closes the listener. Changing the pairing key requires
updating it on both sides. Treat the key like a local password; it authorizes
submitting speaker metadata and participant audio during an active capture.
Both apps store it in their local settings.

The optional **Transcribe calls automatically** setting starts a transcript on a
confirmed voice-channel join when the speech model and separate-audio adapter
are ready. A confirmed leave finishes only a transcript that this setting
started. Temporary missing metadata does not finish a call. Manually finishing
an automatic transcript prevents another start until you leave and join again.

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
It accepts one verified native-module build. Compatibility with other Discord
voice-module versions is not yet verified.

## Local protocol

The native Node bridge uses an authenticated HTTP POST to the fixed address
`127.0.0.1:9223/voice`. No remote host, proxy, redirect or user-supplied URL is used.
The 256-bit random key appears only in an authorization header, never in a URL or
log. The receiver rejects browser origins, duplicate headers, chunked framing,
oversized payloads, unknown payload fields, invalid or repeated participant IDs,
and delayed snapshots. It invalidates speaker state after 750 ms without updates.
Snapshots normally arrive every 150 ms and on voice store changes, capped at
20 per second by the companion. Only its own change listeners are removed on stop.

The `/voice` metadata bridge follows Vencord's
[native plugin interface](https://docs.vencord.dev/plugins/natives/); it does not
change Discord's content security policy or replace Discord callbacks. The
separate participant-audio adapter wraps the verified native receive callback,
preserving its original behavior before copying allowed audio to Articulate.

The companion sources use this repository's AGPL-3.0-or-later license. Vencord itself retains
its own license and distribution requirements. No Vencord binary is bundled here.

## Validation

The companion typechecks in Vencord commit
`59a54286542651fff5ea53f0ce6cadf2a6aa7521` with its locked dependency graph.
Receiver tests cover pairing, request validation, actual TCP publication, stale
state invalidation and shutdown. The native IPC validation covers payload bounds,
duplicate participants, forbidden fields, stale samples and the fixed endpoint.
Installer tests cover file ownership, preserving other plugins, rollback,
compiled artifact validation and reading the configured injector target. A
separate opt-in smoke test downloads and builds the pinned source with an isolated
data folder and no Node.js, pnpm or Git on PATH; it does not run the installer.
A custom Vencord installation and real voice call remain necessary to verify
runtime store compatibility on a particular Discord release.
