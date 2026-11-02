# Security

Please use [private vulnerability reporting](https://github.com/Obiente/articulate/security/advisories/new). Do not include tokens, private transcripts, or recordings in public issues.

The latest published version is supported for security fixes. Local text and settings are plaintext under the user's application-data directory. Anyone with access to that Windows account may access those files.

Discord integrations are optional. The Vencord companion and Articulate authenticate each other over loopback with an HMAC challenge-response on a shared pairing key; the key itself never crosses the socket, the companion sends nothing until Articulate has proven possession, and the port is reserved exclusively so no other local socket can share it. The key can be rotated or forgotten from Settings, which invalidates every companion holding the old key.

The debugger route exposes Discord's local developer interface on a loopback TCP port chosen at random for each relaunch. While it is open, any program running under the same Windows account can read or control that Discord window, including messages and account storage. Articulate shows a notice while it knows the debugger is open and offers to restart Discord without it; restart Discord normally when you are done. Neither route requests account tokens or message history.

Meeting detection for Teams, Zoom and Google Meet is off by default. It reads the local process list and Windows audio session state only; it never joins, reads or records a meeting by itself, and transcripts start through the same path as Discord's automatic capture.

Companion build dependencies are installed with lifecycle scripts disabled, and all verified downloads use HTTPS only and are streamed to a temporary file while their SHA-256 digest is checked.

Updates trust this repository's HTTPS GitHub release metadata and asset digest. Current Windows binaries are not Authenticode-signed. A SHA-256 digest verifies bytes; it is not a Windows publisher certificate.
