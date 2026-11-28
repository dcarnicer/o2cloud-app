A native desktop client for **O2 Cloud / Movistar Cloud** (Telefónica España),
built with Tauri 2 and backed by
[`o2cloud-cli`](https://github.com/danielperez9430/o2cloud-cli).

## Changes

- **Previews:** photos, videos and PDFs show a thumbnail in the file list, and
  Space, a double click or Enter opens a large preview (← → for the next and
  previous one, «Descargar» to keep it). A video shows a still and a PDF its
  first page. Asked for in
  [o2cloud-cli#24](https://github.com/danielperez9430/o2cloud-cli/issues/24).
- **Música by album:** «Canciones / Álbumes / Carpetas». Albums come from the
  songs' tags, play in track order, and compilations show as «Varios
  artistas». The player shows each song's title and artist, also on the
  system's media controls.
- **o2cloud CLI 1.5.0** bundled on macOS, Windows and Linux, and required: it
  brings the thumbnails and the song tags
  ([o2cloud-cli#64](https://github.com/danielperez9430/o2cloud-cli/issues/64)).
- Includes everything from 0.2.x: in-app updates, Fotos keeping its
  permission, download whole folders, songs playing in any folder, big videos
  in parts recognised, Recuerdos, Pase and Biblioteca.

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
