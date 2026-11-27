A native desktop client for **O2 Cloud / Movistar Cloud** (Telefónica España),
built with Tauri 2 and backed by
[`o2cloud-cli`](https://github.com/danielperez9430/o2cloud-cli).

## Changes

- **o2cloud CLI 1.3.1** bundled; the app requires 1.2+ within 1.x and only
  offers CLI updates within that major version.
- **Overwrite fix:** CLI 1.2.2 makes replacing a file uploaded seconds earlier
  wait for O2 to validate it (`MED-1017`) instead of leaving both copies.
- **Slow overwrites:** the transfer row says when O2 is checking a recent copy,
  and the upload watchdog no longer mistakes that wait for a stalled transfer.
- **Recuerdos:** Momentos, Lugares (list or OpenStreetMap map) and Personas.
- **Pase:** a slideshow of a moment or place, with optional CC0 piano music.
- **Biblioteca:** Música with a streaming player, and Documentos.
- **Upload conflicts:** when O2 has the same name with another size, choose
  overwrite (the old copy goes to O2's trash) or skip instead of silently
  creating `name (1)`.
- **Upload progress** and SHA-256 come from the CLI's `--json-lines` events;
  the old per-platform process/file polling is gone.
- **Accessibility:** dialogs, focus, live regions, contrast and keyboard
  behavior checked with axe-core (0 violations).
- Security: includes the CLI 0.15.4 fix that kept session keys/signed URLs out
  of failed-request errors.
- Faster first full listing: 12.1 s vs 24.6 s on a 106k-file account.

## Downloads

- **macOS** — universal `.dmg` for Apple Silicon and Intel (macOS 10.15 or later).
- **macOS (portable)** — `.app.tar.gz` archive with the same universal app.
- **Windows** — x64 `.msi` or NSIS `.exe` installer.
- **Linux** — x64 `.AppImage` or `.deb` package.

The [`o2cloud`](https://github.com/danielperez9430/o2cloud-cli) 1.3.1 CLI is
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
