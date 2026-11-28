A native desktop client for **O2 Cloud / Movistar Cloud** (Telefónica España),
built with Tauri 2 and backed by
[`o2cloud-cli`](https://github.com/danielperez9430/o2cloud-cli).

## Changes

- **Download whole folders:** select a folder (or several, with files too) and
  «Descargar»: it comes down with everything in it, as a folder of its name.
  Files already there are skipped and a cut download carries on where it
  stopped, so downloading again finishes the job. Files the app once uploaded
  in `.o2part` parts are joined and checked against their SHA-256. Asked for
  in [o2cloud-cli#24](https://github.com/danielperez9430/o2cloud-cli/issues/24).
- **Música by folder:** «Canciones / Carpetas» shows one entry per folder
  holding songs; a folder plays through its own songs.
- **Lugares opens on the map**, and remembers if you prefer the list.
- **o2cloud CLI 1.4.2** bundled on macOS, Windows and Linux (`download-dir`
  finds files with emoji in their names); Linux still built from source on
  Ubuntu 22.04.
- From 0.2.5 on the app updates itself: if you have 0.2.5, it offers this
  version on startup. Older versions: install this one by hand once.
- Includes everything from 0.2.5 (in-app updates), 0.2.4 (new CLI notice),
  0.2.3, 0.2.2, 0.2.1 and 0.2.0: Recuerdos, Pase, Biblioteca, upload
  conflicts, `--json-lines` progress and SHA-256, and the accessibility pass.

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
