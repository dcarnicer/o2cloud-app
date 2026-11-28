A native desktop client for **O2 Cloud / Movistar Cloud** (Telefónica España),
built with Tauri 2 and backed by
[`o2cloud-cli`](https://github.com/danielperez9430/o2cloud-cli).

## Changes

- **Fotos works, and keeps its permission:** the macOS app is now signed with
  the project's own (self-signed) certificate instead of an ad-hoc signature,
  which changed with every build and made macOS forget the Photos permission
  on each update; and it declares the Photos library access its hardened
  runtime needs. **Grant the Photos permission once more after installing
  this version**; later updates keep it.
- **Listen in any folder:** a song in a folder plays in the app (double click,
  Enter or ▶) instead of downloading, and the player goes through that
  folder's songs. Downloading is still in the menu and the row.
- **Música → Carpetas** shows each folder by its name, with where it is and
  how many songs beside it, sorted like the other columns, with a ▶ to play it.
- Includes everything from 0.2.6 (download whole folders, Música by folder,
  Lugares on the map, CLI 1.4.2), 0.2.5 (in-app updates) and earlier.
- **o2cloud CLI 1.4.2** bundled on macOS, Windows and Linux.

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

macOS builds are **signed with the project's own self-signed certificate and
not notarized**; Windows and Linux builds are **not code-signed**. These are free, personal-project builds, so no paid
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
