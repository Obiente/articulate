# Contributing

Use Windows x86_64, stable Rust, Node.js 22 LTS or newer with npm, GitHub CLI, and Visual Studio C++ Build Tools. The build script builds and embeds the React frontend before compiling the single Tauri application.

```powershell
./scripts/build-windows.ps1
./scripts/build-windows.ps1 -Test
cargo fmt --all -- --check
./scripts/build-windows.ps1 -Check
```

The native runtime is pinned and SHA-256 checked. No model is needed for normal unit tests. Real inference and interactive tests are explicitly ignored by default. See [the developer guide](docs/development.md).

Use conventional branches such as `feat/`, `fix/`, and `docs/`. Include the behavior changed and relevant validation in pull requests. Avoid unmeasured compatibility or accuracy claims.

Never commit recordings, personal transcripts, pairing keys, settings, diagnostic captures, private evaluation data, or absolute developer-machine paths. Keep these in ignored `.local/`. Use synthetic identities in fixtures. Review files before committing.

## Releases

`scripts/package-windows.ps1` creates a branded installer and portable ZIP, remaps compiler paths, includes runtime and license notices, and rejects model/data files. It requires Python 3.9+, NSIS, and the Visual Studio redistributable directory. Output belongs in ignored `dist/`.

Increment the Cargo package version for every published update. Do not replace published asset bytes. Updates accept stable semantic versions and require a GitHub-computed SHA-256 digest for the exact Windows installer. Models are always downloaded separately.

### Update signing

Signed updates are shipped disabled. The non-default Cargo feature `signed-updates` enables signature verification. Without it, builds and the updater use the existing size, SHA-256, URL and redirect checks and require no signing key or metadata. The release signing step runs only when repository variable `ENABLE_UPDATE_SIGNING` equals `true`.

When enabled, the release workflow signs the exact UTF-8 bytes of `update-manifest.json` after packaging. It contains `version`, `asset`, `size`, and `sha256`. `update-manifest.sig` contains the detached 64-byte Ed25519 signature as hexadecimal text. Both files must accompany the installer. The signing step checks that the private key matches the committed public key. Enabled clients reject missing signatures, invalid signatures, mismatched manifests and placeholder keys. Release builds with the feature enabled reject the placeholder public key; ordinary builds are unaffected.

### Enabling signed updates later

1. Generate a key with `scripts/new-update-key.ps1 -PrivateKeyPath <private-path-outside-repository>` and Node.js installed. Do not put the private key in this repository.
2. Store the complete private PEM as repository secret `UPDATE_SIGNING_KEY`.
3. Commit `assets/update-signing.pub`, the 32-byte Ed25519 public key encoded as 64 lowercase hexadecimal characters.
4. Set repository variable `ENABLE_UPDATE_SIGNING` to `true` and enable the `signed-updates` Cargo feature in release builds. The current packaging script uses default features, so add it to the Cargo default feature list when activating signed updates for distribution.
5. Once enabled, all future releases must be signed with that key. Keep the private key securely backed up. Do not replace published asset bytes or rotate keys without a client trust migration.
