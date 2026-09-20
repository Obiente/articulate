# Security

Please use [private vulnerability reporting](https://github.com/Obiente/articulate/security/advisories/new). Do not include tokens, private transcripts, or recordings in public issues.

The latest published version is supported for security fixes. Local text and settings are plaintext under the user's application-data directory. Anyone with access to that Windows account may access those files.

Discord integrations are optional. Vencord uses a local pairing key and loopback endpoint for voice metadata. The debugger route exposes Discord's local developer interface. Neither route requests account tokens or message history.

Updates trust this repository's HTTPS GitHub release metadata and asset digest. Current Windows binaries are not Authenticode-signed. A SHA-256 digest verifies bytes; it is not a Windows publisher certificate.
