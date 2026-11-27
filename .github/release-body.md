A native desktop client for **O2 Cloud / Movistar Cloud** (Telefónica España),
built with Tauri 2 and backed by
[`o2cloud-cli`](https://github.com/danielperez9430/o2cloud-cli).

## Changes

- **New CLI notice:** on startup the app checks GitHub for a newer o2cloud CLI
  and, if there is one, shows a notification with a button that opens
  Settings at «Actualizar CLI». It stays until it's closed, and says nothing
  when offline.
- **«Actualizar CLI» no longer stays enabled** after a successful update; the
  button goes away until a newer version turns up.
- **Pase:** «Ver como pase» is no longer offered in a moment or place with
  only videos (the slideshow shows photos), where it said there was nothing
  to show.
- **o2cloud CLI 1.4.1** bundled on macOS, Windows and Linux; the app requires
  1.2+ within 1.x, runs the newer of the bundled CLI and one updated from
  Settings, and only offers CLI updates within that major version.
- **Linux:** the bundled CLI is still built from source on Ubuntu 22.04, so the
  installers keep working there; the official Linux asset needs GLIBC 2.39.
- Includes everything from 0.2.3 (the bundled CLI is used again), 0.2.2
  (wrong-extension messages in Spanish, split files downloadable right after
  upload), 0.2.1 (overwriting a recent upload waits for O2, `MED-1017`) and
  0.2.0: Recuerdos (Momentos, Lugares, Personas), Pase, Biblioteca (Música and
  Documentos), upload conflicts, `--json-lines` progress and SHA-256, and the
  accessibility pass.

## Downloads

- **macOS** — universal `.dmg` for Apple Silicon and Intel (macOS 10.15 or later).
- **macOS (portable)** — `.app.tar.gz` archive with the same universal app.
- **Windows** — x64 `.msi` or NSIS `.exe` installer.
- **Linux** — x64 `.AppImage` or `.deb` package.

The [`o2cloud`](https://github.com/danielperez9430/o2cloud-cli) 1.4.1 CLI is
included with every installer, so the app works without a separate CLI install.
The app still accepts a newer CLI from Settings or `O2CLOUD_CLI`.

Verify the downloads against the attached `SHA256SUMS.txt`:

```sh
sha256sum -c SHA256SUMS.txt     # Linux
shasum -a 256 -c SHA256SUMS.txt # macOS
```

## These builds are not signed

macOS builds are **ad-hoc signed and not notarized**; Windows and Linux builds
are **not code-signed**. These are free, personal-project builds, so no paid
signing certificates are used. The operating system may warn the first time you
open them:

- **macOS:** right-click the app → **Open**, or allow it in **System Settings →
  Privacy & Security**.
- **Windows:** choose **More info → Run anyway** in the SmartScreen prompt.
- **Linux:** the packages are unsigned; check the SHA-256 of the downloaded file
  if you mirror them.

## Credits

The CLI, its O2 API implementation and the file-splitting logic come from
[`o2cloud-cli`](https://github.com/danielperez9430/o2cloud-cli), created by
[Daniel Pérez Fernández (@danielperez9430)](https://github.com/danielperez9430)
and released under the MIT license. This desktop app is an independent GUI
layer and is not affiliated with O2 or Telefónica.

See
[THIRD_PARTY_NOTICES.md](https://github.com/dcarnicer/o2cloud-app/blob/main/THIRD_PARTY_NOTICES.md)
for license details.
