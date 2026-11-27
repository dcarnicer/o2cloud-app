# O2 Cloud Desktop

A native, cross-platform desktop client for **O2 Cloud / Movistar Cloud** (Telefónica España), built with **Tauri 2 + Rust** and backed by the [`o2cloud-cli`](https://github.com/danielperez9430/o2cloud-cli) binary.

The app is a graphical layer on top of the CLI: it does not reimplement O2's private API. That keeps authentication, uploads and downloads in one audited place, and lets the app update the CLI independently.

> **Unofficial project.** Not affiliated with, endorsed by or supported by Telefónica, O2, Movistar or Synchronoss. Use it only with your own account and in accordance with the service terms.

## Why this exists

O2 Cloud gives 10 TB but enforces a **4 GB limit per file** on the server. The official apps and web UI stop there. The gateway/WebDAV approach does not bypass it either: the rejection comes from O2 (`MED-1020: File size exceeds configured max file size limit`).

This app keeps the convenience of a desktop client and adds a **verifiable chunking layer**:

```
movie.mkv                         -> logical file shown in the app
movie.mkv.001 / .002              -> CLI-split parts for new files over 4 GB
movie.mkv.o2split.json            -> sizes + SHA-256 hashes (written by the app)
```

New files over 4 GB are split by `o2cloud-cli`. The app adds the manifest, verifies the whole-file SHA-256 after the CLI joins the parts on download, and keeps the same hash in its shared upload library. Each part is a normal object below O2's limit; `cat` or 7-Zip can also reconstruct the file without the app.

Files uploaded with older versions keep their `.o2part.NNN` + `.o2manifest.json` format and remain readable, downloadable and deletable. The O2 web UI shows the parts and the manifest; the app hides them behind a single logical file.

## Features

- Native Tauri UI, no Docker.
- Login/logout through an O2 login window (phone + SMS); the CLI validates and stores the session.
- Finder-style window: a sidebar with the root, «Cámara» and the root folders; a path bar; search within the folder; columns sortable by name, size or date; multiple selection (click, ⌘/Ctrl, ⇧, ⌘A/Ctrl+A) to download or move to the O2 trash at once; a context menu; and keyboard shortcuts (listed in Settings, with the right symbols on each platform).
- Download, delete (to the O2 trash) and create folders. Several files download into a chosen folder without replacing anything there.
- Transfers live in a popover from the toolbar, with bytes, speed and time left; the toolbar button shows how many are running. Upload progress and the file's SHA-256 come from the CLI's `--json-lines` events, so a completed upload isn't read a second time.
- Drag and drop files/folders. Uploads run one at a time, in order, with progress, and skip files already in the folder with the same name and size (so retrying a folder upload doesn't duplicate it).
- Automatic chunking. New files over 4 GB use the CLI's native split (`name.001`, …) and get an app-written `name.o2split.json` with whole-file and per-part SHA-256; files up to 4 GB go up as a single file. Existing `.o2part` files remain readable. An interrupted upload resumes from the parts already in O2.
- Downloads go to `<name>.o2download` and are renamed into place only when complete; `.o2part` files are checked part by part and as a whole, and CLI splits are joined by the CLI and checked against the manifest's whole-file SHA-256.
- Listings are cached for 5 minutes while browsing: with CLI 1.2.1 the first full `ls` on 106,000 files took 12.1 s (0.12 took 24.6 s on the same account) and later ones about 0.5 s / 2 requests. Startup asks the CLI for a fresh listing, which its own cache makes fast, so the window never waits on a stale saved copy. The last few listings are still saved on disk for navigation and to keep the root visible after a transfer; uploads mark that cache stale instead of deleting it.
- **Recuerdos:** O2's own **Momentos**, **Lugares** (list or OpenStreetMap map) and **Personas**, each opening as a virtual folder of its photos. A moment or place can play as a slideshow (5 s per photo, keyboard controls, optional CC0 music) and places show their photo count on the map.
- **Biblioteca:** O2's **Música** and **Documentos** lists, wherever the files live. Songs stream from O2's signed links with a player (previous, next, seek, media keys); documents open/download like any file. Uploads from these views go to the root, where O2 files them by kind.
- **Upload conflicts:** when O2 already has the same name with another size, the app shows both sizes and dates and asks, per file, whether to overwrite (the old copy goes to O2's trash) or skip, instead of silently creating `name (1)`.
- **Accessibility:** dialogs trap focus and are announced, notifications are live regions, primary buttons keep 5.9:1 contrast, and the key views were checked with axe-core with no violations left.
- **Virtual "Cámara" folder**: groups photos/videos by year at the UI level without moving anything in O2. Obvious non-camera media (screenshots, WhatsApp, Telegram, screen recordings, wallpapers) stays in the root.
- CLI manager: shows the current CLI version/origin, checks GitHub releases, downloads a new binary, tries it against the account and keeps the current CLI if it cannot list files. It only offers updates within the supported CLI major version (1.x).
- **Subir desde Fotos (macOS)**: a Photos view lists the library's videos, photos or both, filters by name, type, album, favorites, date range and size (including «más de 4 GB»), loads thumbnails and pages of 300, marks what is already in O2, can select only the pending ones, and exports the selected originals — iCloud-only ones included — straight into the chunked upload. The date range is prefilled with the last month and enabled with the «Por fecha» checkbox; «Limpiar» removes it. Files already in the target folder with the same name and size are skipped before downloading them from iCloud, and a download can be cancelled while it runs.
- **Shared upload library**: every upload records its SHA-256 in a hidden `/.o2cloud-gui/library/` folder in O2. Each device writes an immutable snapshot and merges the snapshots from the others, so a photo or video uploaded from another Mac is recognised before exporting it again. An optional Settings toggle also hashes non-media files before upload, so renamed documents are matched by content too. If a match is only in another folder, the app lists the duplicates and asks, file by file, whether to skip each one or upload another copy. Settings shows the library, syncs it, reconciles it against the account, forgets this device's copy and can index `.o2manifest.json` or `.o2split.json` files to recover their hashes.
- Settings for camera grouping, the optional preflight hashing of non-media files, and the slideshow's music (two CC0 piano pieces or none).

## Tested status

Tested on macOS 27 (Apple Silicon) against a real O2 Spain account:

- Login via SMS: works through the CLI's login window; the app's own login window (same OAuth flow) still needs an end-to-end run.
- 20 MB upload: works.
- 2 MB upload through the app: works; queue reaches `Subido (id …)`.
- A raw 4.2 GB single-file upload is rejected by O2 after the full transfer with `MED-1020`; the limit is server-side.
- **4.2 GB upload through the app: works.** Split into two parts (3.5 GB + 0.7 GB), both accepted; the manifest SHA-256 matched the local file byte for byte.
- Three real videos already stored this way in `/Vídeos grandes`: 4.4 GB, 6.6 GB and 7.2 GB, each split into two parts.
- `.o2part.001` and `.o2manifest.json` filenames are accepted by O2 and preserved in listings.
- O2 is eventually consistent on writes: new files can take ~10–60 s to appear in `ls`.
- Virtual camera folder: 46,433 items / ~525 GB classified. The 161 non-camera root files were moved to `/Otros`, leaving the root clean.
- PhotoKit (standalone harness, macOS 27): listed the library's videos with their original filenames and sizes, and exported one original byte for byte. The permission prompt and the full Photos view still need a run in the bundled app.
- Upload library: a snapshot was uploaded to, listed in, downloaded from and deleted from the real `/.o2cloud-gui/library/` folder; merge, fingerprint and prune unit tests pass. Syncing between two physical devices still needs an end-to-end run.
- macOS bundle: the debug `.app` carries `NSPhotoLibraryUsageDescription` and `LSMinimumSystemVersion` 10.15, so the Photos permission is declared where TCC looks for it.

The bundled CLI is now **1.3.1**, and the app requires 1.2 or later within 1.x.
The last full smoke test (26 Nov 2026) used 1.2.1; the 1.3.1 checks are tracked
in [`PENDING_TESTS.md`](PENDING_TESTS.md).
A smoke on 26 Nov 2026 against the real account passed: `status`/`quota`,
root and whole-account `ls`, `mkdir`, upload/download/rm, a 25 KB split with
join + SHA-256, resume after deleting one part, `--on-conflict
duplicate/fail/skip`, empty-JPEG rejection, JSON errors, the app's
create/upload/download/delete flows, and the in-app CLI update from managed
0.12.0 to 1.2.1. On 106,606 files the first full `ls` took **12.1 s** with
1.2.1 vs **24.6 s** with 0.12 (later ones about 0.5 s). The opt-in
`live_labels` test passed with 119 moments, 268 places, 0 people (face
recognition is off), 898 songs and 28,880 documents.

The opt-in `live_cli` test used to fail in one edge case: overwriting a file
uploaded seconds earlier. O2 rejected the old copy's delete with `MED-1017`
(not validated yet) and the CLI left both copies. CLI 1.2.2 fixed it by
waiting for O2 to finish checking the recent copy (up to 2 minutes per step),
and the app now treats its `waiting` events as activity so the transfer
watchdog does not stop it. The test should be re-run with 1.3.1 before the
next release.
The macOS flows above were last exercised with 0.8.0; Windows/Linux and the
3.5–4 GB `.o2part` comparison are still tracked in
[`PENDING_TESTS.md`](PENDING_TESTS.md).

Tested on Windows 11 (x86_64) with the `o2cloud-cli` 0.8.0 Windows build:

- `cargo test` (39 tests), `npm run build`, the debug app and the release bundles build and run.
- The app finds `o2cloud.exe` through `O2CLOUD_CLI`, a managed download or `PATH`, shows its version and reports `Not logged in` as expected.
- The Settings update flow downloaded and installed the `o2cloud-cli` 0.8.0 Windows build (without a session, so it skipped the account test), wrote `current.json` and switched to it. The CLI's own login window («O2 Cloud - Iniciar sesión») opens and responds.
- The NSIS installer installs, the installed app finds the managed CLI, and the uninstaller removes it cleanly.
- Real O2 account on Windows: SMS login, 46,463-item virtual camera view, a 5,321-file folder, a 32.3 MB download, a 79 B upload with spaces/`ñ` and a byte-identical download, a 300 MB chunked upload (256 MB + 44 MB) with a matching SHA-256 on download, and a folder upload that skipped a hidden file. The test files were deleted afterwards.
- UI checks on Windows: folder navigation, search, `Ctrl+F`/`Ctrl+A`/`Esc`, context menu, Settings/update check, library sync, the transfers panel and the native «Guardar archivo» dialog all work.
- Windows-specific paths were fixed and exercised: invalid download names, atomic cache saves, hidden/system files in folder uploads, temporary chunk names and the upload progress watchdog.
- Still pending on Windows: cancelling/resuming a >3.5 GB chunked upload, session expiry, native drag and drop, and the installer bootstrap on a machine without WebView2.

Tested on Linux x86_64 under WSL2 (Ubuntu 24.04, WSLg):

- `cargo check --all-targets`, `cargo test` (38 tests), `npm run build` and the debug app build and run.
- The app opened under WSLg with a real account in read-only mode: root listing, folder rows, search and keyboard navigation.
- The published `o2cloud-cli` 0.8.0 Linux binary requires GLIBC 2.43 and fails on Ubuntu 24.04 (2.39); the CLI was built from source instead.
- The CLI's own login window does not work under WSLg; an existing `~/.config/o2cli/auth.json` was used for the read-only test. See [`LINUX.md`](LINUX.md) for the setup and details.

Pending validation tests are tracked in [`PENDING_TESTS.md`](PENDING_TESTS.md). See [`WINDOWS.md`](WINDOWS.md) for the Windows audit and [`LINUX.md`](LINUX.md) for the WSL2/Linux notes.

## Requirements

- macOS 10.15 or later, Windows 10/11 or Linux.
- Rust 1.85+ and Node 22+ to build.
- An O2 Cloud / Movistar Cloud account.
- The `o2cloud` CLI from [`o2cloud-cli`](https://github.com/danielperez9430/o2cloud-cli). Release installers bundle 1.3.1 on macOS, Windows and Linux, and the app needs 1.2 or later within 1.x (Settings only offers updates within 1.x); local builds use `src-tauri/bin/<platform>/` when present, and the app can download the CLI from its GitHub release on first run.
- On Windows: Microsoft Edge WebView2 Runtime (included in Windows 10/11; the installer can bootstrap it) and the Visual Studio 2022 Build Tools with the «Desktop development with C++» workload for the MSVC Rust target.
- On Linux: WebKitGTK 4.1 and GTK 3 development packages; the full `apt` command is in [`LINUX.md`](LINUX.md).
- For «Subir desde Fotos»: permission to read the Photos library (macOS only).

## Build

On macOS, `app.sh` installs the dependencies when needed and runs the right command:

```bash
./app.sh dev      # development window, reloads on changes
./app.sh build    # .app
./app.sh open     # open the built app, building it first if missing
./app.sh dmg      # .app + .dmg installer
```

On Windows, `app.ps1` does the same:

```powershell
.\app.ps1 dev        # development window, reloads on changes
.\app.ps1 build      # release .exe
.\app.ps1 open       # open the built app, building it first if missing
.\app.ps1 installer  # .exe + .msi installers
```

If PowerShell blocks the script, run
`powershell -ExecutionPolicy Bypass -File .\app.ps1 dev` (or `pwsh` if you use
PowerShell 7).

### Git hooks

Commits are checked with [gitleaks](https://github.com/gitleaks/gitleaks) before
they are created. Install it and enable the versioned hook:

```bash
brew install gitleaks        # macOS; see the link above for other systems
./scripts/setup-git-hooks.sh
```

The same check runs in CI on every push and pull request. If a finding is a
confirmed false positive, skip the local check once with
`SKIP_GITLEAKS=1 git commit ...`; the CI scan still runs.

Or by hand:

```bash
npm install
npm run tauri dev      # development
npm run tauri build    # .app + .dmg on macOS, .exe + .msi on Windows
```

The macOS bundle is produced at:

```
src-tauri/target/release/bundle/macos/O2 Cloud.app
src-tauri/target/release/bundle/dmg/O2 Cloud_<version>_aarch64.dmg
```

The Windows build is produced at:

```
src-tauri\target\release\o2cloud-app.exe
src-tauri\target\release\bundle\nsis\O2 Cloud_<version>_x64-setup.exe
src-tauri\target\release\bundle\msi\O2 Cloud_<version>_x64_en-US.msi
```

The Windows installers are not code-signed, so SmartScreen may warn the first
time they are run. Building from source avoids the installers entirely:
`npm run tauri build -- --no-bundle` only produces the `.exe`.

## Releases

Releases are built by GitHub Actions from an annotated `v*` tag and left as
drafts. The canonical, step-by-step process is
[`RELEASING.md`](RELEASING.md); follow it for every release instead of tagging
by hand from memory.

In short:

```bash
npm run release:version -- 0.2.0   # syncs all five version files
npm run build
(cd src-tauri && cargo test --locked)
git commit -am "Release 0.2.0" && git push origin main
git tag -a v0.2.0 -m "O2 Cloud 0.2.0"
git push origin v0.2.0
```

CI verifies that the tag matches every version file, creates a draft release,
builds macOS universal, Windows x64 and Linux x64, bundles the pinned `o2cloud`
1.3.1 CLI, and uploads `SHA256SUMS.txt`. Review the draft, verify the
checksums and smoke-test the installers before publishing it.

These artifacts are **not code-signed**: macOS is ad-hoc signed but not
notarized, and Windows/Linux are unsigned. Gatekeeper and SmartScreen will warn
on other machines. This is intentional for a free personal project; manual
downloads work after confirming the prompt. Code signing and notarization
would only be required to enable the OTA updater.

## CLI management

The app requires `o2cloud` **1.2.0 or newer within 1.x**; release builds bundle
**1.3.1**. 1.0 made the app rely on exit codes, JSON errors and
`--on-conflict`; 1.1 added labels; 1.2 added `media`, labels' covers,
locations and thumbnails, and lists a file as soon as it is uploaded. 1.2.1
stops re-uploading names with emoji (O2 drops characters outside Unicode's
BMP) and lists the name O2 keeps. 1.2.2 makes overwriting a file uploaded
moments earlier wait for O2 to validate it (`MED-1017`), with `waiting`
events that the app counts as activity. 1.3 adds
`upload-dir --update --delete` to mirror a folder; the app does not use it.
Settings only offers updates within the supported major version (1.x), and
older CLIs are detected but not used.

The CLI also brought the earlier performance work: 0.10 made `download`/`rm`
by ID or path take 3 requests instead of listing
the whole account, 0.11 made `ls` linear, and 0.12 added the file-list cache.
On 106,606 files the first full `ls` took 12.1 s with 1.2.1 (24.6 s with 0.12)
and later ones about 0.5 s / 2 requests.

Uploads run with `--json-lines`: progress and the SHA-256 come from the CLI's
events, and a same-name file of another size is a conflict the app asks about
(overwrite or skip) instead of a silent `name (1)`. Resuming a split upload
rejects another version's parts instead of taking their IDs.

0.9.0 also added native split uploads (`name.001`, `name.002`, joined again by
`o2cloud download name`), `--skip-existing` and parallel `upload-dir`. The app
now uses that mode for **new files over 4 GB**: the CLI splits and joins them,
and the app adds `name.o2split.json` with the whole-file and per-part SHA-256,
so the upload library and Photos deduplication keep working. Files up to 4 GB
are uploaded as a single file; the app's `.o2part.NNN` + `.o2manifest.json`
format is kept for files uploaded before the hybrid (and as a fallback if
`PREFER_CLI_SPLIT` is disabled). A file uploaded from the CLI in parts is
grouped by the app when it has a `name.o2split.json` manifest or its first part
is large enough to be a real split; otherwise it appears as its individual
`.001` files.

### File format compatibility

Existing `.o2part.NNN` + `.o2manifest.json` files stay readable, downloadable
and deletable; no migration is required. New files over 4 GB use
`name.001` + `name.o2split.json`. The app decides per file from the part names,
so both formats can coexist in the same folder, and the shared library indexes
both manifest types.

The app resolves the CLI in this order:

1. `O2CLOUD_CLI` environment variable.
2. Managed update: `~/Library/Application Support/es.dabi.o2cloud/cli/current.json` on macOS, `%APPDATA%\es.dabi.o2cloud\cli\current.json` on Windows.
3. Bundled binary: `Contents/Resources/bin/<platform>/o2cloud` on macOS, `bin\windows-x86_64\o2cloud.exe` on Windows.
4. `o2cloud` in `PATH`.
5. `~/.local/bin` and `~/.cargo/bin`.

This Windows build does not bundle the CLI. On first run Settings opens and
**Buscar actualización** downloads the
`o2cloud-x86_64-pc-windows-msvc.zip` release published by `o2cloud-cli`, tests
it against the account when there is an active session and makes it current.

On Linux, the published 0.9.0–0.12.0 binaries required GLIBC 2.43; since
0.13 they need GLIBC 2.39 (Ubuntu 24.04, Debian 13, Fedora 40 or newer). On
older systems build `o2cloud-cli` 1.3.1 from source and place the binary at
`src-tauri/bin/linux-x86_64/o2cloud`; [`LINUX.md`](LINUX.md) has the
commands.

An older CLI is kept only as a last resort, so the app can show which version
needs updating.

### Bundled CLI

Release builds bundle `o2cloud` **1.3.1** on every platform:

- **macOS:** the official Apple Silicon and Intel binaries, so the universal
  app has the right one in each slice.
- **Windows:** the official `o2cloud.exe`. Local Windows builds do not bundle
  it and download it on first run.
- **Linux:** compiled from source at tag `v1.3.1`, pinned to commit
  `49c81bd`, because the official 0.13.3+ binary needs GLIBC 2.39 and the app
  still supports Ubuntu 22.04 (GLIBC 2.35). The official binary works on
  Ubuntu 24.04 and later.

Each release build passes a platform-specific config
(`src-tauri/tauri.{macos,windows,linux}.release.conf.json`) so an installer only
contains its own CLI binary.

The 1.3.1 CLI includes the fix for accounts with negative `creationdate`
values ([o2cloud-cli#8](https://github.com/danielperez9430/o2cloud-cli/pull/8)),
the download/ID/SAPI fixes from 0.9.0, the by-ID/path fast path from 0.10.0,
the linear `ls` from 0.11.0, the file-list cache from 0.12.0, the pre-1970
names/depth fixes and `fix-jpeg` from 0.13.x, `download-dir` from 0.14, the
0.15.4 security fix, the 1.x exit codes, JSON errors, `--on-conflict`,
`--json-lines`, labels and media, the `MED-1017` overwrite wait from 1.2.2,
and the directory mirror from 1.3, so no local patch is applied.

With `--json`, the payload (including new IDs) is a single JSON value on
stdout; progress stays on stderr.

## Known limitations

- O2's 4 GB per-file limit is enforced server-side. Chunking is a workaround, not a bypass.
- A web session ends after a while without use. CLI 1.x reports it with exit code 4 (no login window); the app maps it to its own sign-in prompt.
- Changes made outside the app (phone, web) show up after the refresh button, or after 5 minutes.
- A stale saved listing is shown with a «Pulsa Recargar» notice instead of refreshing automatically. Two large listing responses in quick succession can lose the second one in WKWebView, so the refresh is left to an explicit user action.
- Overwriting a file uploaded moments earlier can take up to 2 minutes per replacement step while O2 validates the recent copy (`MED-1017`); CLI 1.2.2+ waits for it instead of failing. If O2 takes longer, the CLI reports the error.
- The map loads OpenStreetMap tiles only when `Mapa` is chosen; OpenStreetMap sees which areas are viewed, never the photos.
- O2's internal API is unofficial and can change without notice.
- The virtual camera folder is a heuristic. It may include downloaded photos and excludes some unusual camera filenames. It is a UI view; the O2 web UI still shows everything at the root.
- Chunked files are visible as parts in the O2 web/app. Do not rename or delete individual parts.
- The Photos view loads pages of 300 and requests thumbnails for the visible rows; counting the library for the summary walks every asset, so the first «Fotos y vídeos» page takes about 1–2 minutes with 52,000 items. An iCloud download can be cancelled, and the app checks for the original's size plus 128 MB of free space before downloading it.
- Photos permission is tied to the app's code signature. An ad-hoc development build may ask for it again after a rebuild.
- Hashing non-media files before upload is opt-in. It reads each selected file once, applies to individual files (folders keep the name+size check) and, when the selection has no media, uses the shared library instead of a whole-account scan.
- The shared library lives in `/.o2cloud-gui/library/` inside the O2 account. The app hides that folder, but the O2 web UI shows it and its snapshot files. Do not delete it there unless you are willing to lose the shared hashes; the app recreates the folder but not the old snapshots. The `-gui` suffix keeps the plain `.o2cloud` name free for the official app.
- Matching against files that are not in the library yet uses a whole-account listing (`ls -a`): about 5.6 s the first time and 0.3 s after that with CLI 0.12, and the app caches it for 5 minutes.
- Files uploaded before the library existed or outside this app are matched by original name + size (Photos also use the capture date), not by hash. Local photo/video files are hashed during preflight when that is cheap (up to 512 MB, or larger if a same-size file already exists); other local files are not hashed unless the opt-in is on.
- The preflight conflict check applies to individual files. Folder uploads still use the target-folder name + size check.
- Preflight hashing is skipped for individual files over 512 MB unless O2 or the shared library already has a file of the same size: reading a 20 GB video from a slow external disk before starting the upload took minutes per file and made the app look stuck. A unique size cannot be a content duplicate, so those files are matched by name + size, and their SHA-256 is recorded after the upload.
- The conflict dialog lists every duplicate and lets each file be skipped or uploaded separately.
- Each upload batch writes a small snapshot with just its own entries. After 8 own snapshots the app publishes a full one and removes the old deltas, which are moved to the O2 trash.
- Syncing the library between two physical Macs has not been tested end to end yet.
- The macOS build is ad-hoc signed, not notarized. For personal use it runs locally; distributing it requires an Apple Developer ID. Release builds are ad-hoc signed too, so Gatekeeper warns on other Macs.
- The Windows installers are not code-signed. SmartScreen may warn; for personal use, build from source or use the unsigned installer. Release builds are unsigned as well.
- Linux release packages are unsigned. The Linux CLI is built from source in CI (1.3.1) so it runs on Ubuntu 22.04 and later; the official 0.13.3+ Linux binary needs GLIBC 2.39 and runs on Ubuntu 24.04 and later.
- macOS ARM64 is the main tested target. Windows x86_64 builds, its unit tests pass and the main flows were tested against a real O2 account; very large chunked transfers, cancellation/resume and session expiry still need a Windows run. Linux x86_64 builds and runs under WSL2, but uploads/downloads, the CLI login window and native desktop packaging still need a Linux run. The CLI publishes macOS, Linux x86_64 and Windows x86_64 binaries (no Linux ARM64).
- The published `o2cloud-cli` Linux 0.8.0–0.12.0 binaries need GLIBC 2.43; 0.13.3+ need GLIBC 2.39, so the official 1.3.1 binary runs on Ubuntu 24.04 and later. The app's release workflow builds from source on Ubuntu 22.04 to keep the 22.04+ installer working. Under WSLg, the CLI's login window fails; use a native Linux desktop or an existing `~/.config/o2cli/auth.json`.
- On Windows, upload progress comes from the CLI's `--json-lines` events, the same as on macOS; the old per-process I/O counters are gone.
- Windows rejects some names that O2 accepts (`<>:"/\|?*`, `CON`, `PRN`, trailing dots/spaces, very long names). Downloads replace those characters and the saved file may have a slightly different name than the one shown in O2. Chunk temporary files use app-controlled names, so a remote part name cannot escape the download folder.

## Roadmap

### OTA updates: GUI and CLI

- Add Tauri's updater plugin for the GUI: sign the update manifest and each
  artifact (macOS `.app`/`.dmg`, Windows `.exe`/`.msi`, Linux AppImage/deb),
  check for updates on launch and periodically, download in the background and
  apply on restart.
- Keep the managed CLI updater and show both versions in Settings. A GUI update
  must not switch the CLI version on its own, and neither update should run
  while a transfer is active.
- Publish a per-platform `latest.json` (stable/beta channels), keep the
  previous version for rollback, and refuse downgrades.
- Prerequisites: Apple Developer ID and notarization for macOS (today the app
  is ad-hoc signed), code signing for Windows, and signed Linux packages.
- Preserve settings, the upload library, cached listings and the O2 session
  across updates; verify that the new GUI can still list files before applying.
- Add a manual «Buscar actualizaciones» path for both GUI and CLI, with clear
  states (checking, downloading, ready to restart, failed).
- Test: update from the previous release on macOS/Windows/Linux, offline and
  interrupted downloads, invalid signatures, downgrade attempts, update while
  an upload/download is running, and GUI/CLI version-mismatch handling.

### CLI split support: next steps

- Stage 1 (done): read-only support for `name.001`, `name.002`, … groups plus
  an optional `name.o2split.json` manifest: group them as one logical file,
  download them with a single `o2cloud download <base>` (the CLI joins them),
  verify the whole-file SHA-256 when the manifest exists, and delete all parts
  together.
- Stage 2 (prototype in place): new files over 4 GB use the CLI's native split
  (`upload --skip-existing`) and get `name.o2split.json` with the whole-file
  and per-part SHA-256. Smaller files still use the app's chunking while this
  is evaluated; the app hashes the file once after the CLI upload and uses
  the CLI's part layout for the manifest.
- Stage 3 (after evaluation): remove the fallback `.o2part` upload path and
  keep read/download support for old `.o2part` files.
- Validate with a real file over 4 GB on macOS, Windows and Linux, and compare
  disk use, upload time and restart behaviour against the old chunking.
  Deletion still costs one `rm` per part until the upstream CLI supports
  deleting a split group by base name.

### Upload library: next steps

- Test a real two-Mac sync (one snapshot each, then an upload from the other).
- Empty the old snapshots left in O2's trash after compaction.

### Photos: remaining validation

- Test cancelling a real iCloud download when nothing is cached.

### Photos: performance

- Count the library with `PHFetchResult.count` and `PHFetchOptions` predicates
  instead of walking all 52,000 assets; the first «Fotos y vídeos» page takes
  about 1–2 minutes.

### PhotoKit notes (25 Oct 2026)

What a manual run showed with three videos of 4.4–7.2 GB on an "Optimize Mac Storage" library:

- AppleScript `export … with using originals` returns without error but writes nothing when the original is only in iCloud.
- With a VPN on, every iCloud download failed, in Photos itself too, while playback still streamed. PhotoKit reports this as `PHPhotosErrorDomain` 3169 (network error).
- `cloudphotod` downloads the whole original to `~/Library/Caches/CloudKit/com.apple.cloudphotod/…/MMCS/` before handing over any data, so each video needs about its size in free disk on top of the upload's part file. Large libraries need one asset at a time and a free-space check.
- The asset's original size is in the Photos database (`ZADDITIONALASSETATTRIBUTES.ZORIGINALFILESIZE`); PhotoKit only exposes it through `PHAssetResource`'s undocumented `fileSize` key.

## Building the bundled CLI

Use the official release asset:

```bash
gh release download v1.3.1 --repo danielperez9430/o2cloud-cli \
  --pattern o2cloud-aarch64-apple-darwin.tar.gz
tar -xzf o2cloud-aarch64-apple-darwin.tar.gz
cp o2cloud /path/to/o2cloud-app/src-tauri/bin/macos-aarch64/o2cloud
```

Or build it from source:

```bash
git clone https://github.com/danielperez9430/o2cloud-cli.git
cd o2cloud-cli
git checkout v1.3.1
cargo build --release
cp target/release/o2cloud /path/to/o2cloud-app/src-tauri/bin/macos-aarch64/o2cloud
```

## Project layout

```
src/                    Tauri frontend (vanilla TS)
src-tauri/src/          Rust backend
  cache.rs              listing cache
  cli.rs                CLI discovery, execution, updates
  transfer.rs           chunking, SHA-256, upload/download
  camera.rs             virtual camera classification
  photos.rs             Photos library commands and export (PhotoKit)
  photos.m              Objective-C PhotoKit bridge
  library.rs            shared SHA-256 upload library and O2 snapshots
  commands.rs           Tauri commands
src-tauri/Info.plist    NSPhotoLibraryUsageDescription (Photos permission)
src-tauri/bin/          bundled CLI binaries
.githooks/              versioned Git hooks (gitleaks pre-commit)
scripts/                hook setup and version bump
.github/workflows/      CI (gitleaks, release)
app.sh                  macOS build/run helper
app.ps1                 Windows build/run helper
RELEASING.md            release checklist and recovery steps
WINDOWS.md              Windows audit, fixes and manual test checklist
LINUX.md                Linux/WSL2 setup and test notes
```

## Acknowledgements

This app would not exist without [`o2cloud-cli`](https://github.com/danielperez9430/o2cloud-cli), by **[Daniel Pérez Fernández (@danielperez9430)](https://github.com/danielperez9430)**. The CLI implements the O2 Cloud/SAPI access, login, native split uploads, the fast by-ID operations, the linear `ls` and the file-list cache. This app is the GUI, verification, Photos and shared-library layer on top of it.

Thanks also for the quick fixes and the detailed work on 0.10–1.2, for the labels/media commands that power the new views, and for answering the performance, split-upload and conflict issues that shaped this client.

## Disclaimer

O2 Cloud and Movistar Cloud are trademarks of Telefónica. This is an unofficial client, not affiliated with, endorsed by or supported by Telefónica, O2, Movistar or Synchronoss. It uses the same unofficial SAPI as the CLI; the service can change at any time. Use it only with your own account and in accordance with the service terms.

## Contributing

Issues and pull requests are welcome. Changes to login, the O2 API, splitting or the CLI cache belong in [`o2cloud-cli`](https://github.com/danielperez9430/o2cloud-cli); this repository focuses on the desktop UI, verification, Photos and the shared library. Please read [`SECURITY.md`](SECURITY.md) before sharing logs or reproducing a problem. Release steps are in [`RELEASING.md`](RELEASING.md).

## License

The app code is MIT; see [`LICENSE`](LICENSE). The bundled `o2cloud-cli` is also MIT; see [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).
