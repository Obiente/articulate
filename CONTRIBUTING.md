# Contributing

Use Windows x86_64, stable Rust, GitHub CLI, and Visual Studio C++ Build Tools.

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
