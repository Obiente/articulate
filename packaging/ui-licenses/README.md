# UI notices missing from npm archives

The packaging script copies installed package notices. These exact-version
fallbacks cover npm packages whose archives omit their upstream license file.
Updates fail closed until the new package version's notice is reviewed.

- `@tauri-apps/api` 2.11.1: MIT license from Tauri's `tauri-v2.11.1` commit
  `e5ae5b93cdd310045191cc0526f253140ad64b87`, `LICENSE_MIT`.
  Source: https://github.com/tauri-apps/tauri/blob/e5ae5b93cdd310045191cc0526f253140ad64b87/LICENSE_MIT
- `react-remove-scroll-bar` 2.3.8: MIT license from upstream commit
  `8ca9ba5ea52de03308fe8ced94f7b159a44d28ff`, `LICENSE`. The published package
  declares MIT; its npm `gitHead` is unavailable in the public repository.
  Source: https://github.com/theKashey/react-remove-scroll-bar/blob/8ca9ba5ea52de03308fe8ced94f7b159a44d28ff/LICENSE
