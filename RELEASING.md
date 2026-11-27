# Releasing O2 Cloud

This is the canonical release checklist. Follow it in order so every release is
built, checked and published the same way.

The pipeline is tag-driven:

```
version bump -> commit to main -> gitleaks green -> annotated tag -> CI
  -> draft release -> checksums -> review -> publish
```

## Signing policy

Unsigned builds are the accepted default. This is a free personal project and
does not buy code-signing certificates or notarization. Do not block a release
on signing.

macOS is ad-hoc signed; Windows and Linux are unsigned. Keep the warning and
the bypass instructions in the release body. Signing is optional future work,
not a release requirement.

The in-app updater does not wait for code signing. Its updates are signed with
the app's own updater key (minisign, free), and the app refuses anything not
signed with it; see [The updater signing key](#the-updater-signing-key).

## Rules

- The app version lives in `package.json`, `package-lock.json`,
  `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock` and
  `src-tauri/tauri.conf.json`. Use the bump script; never edit only one file.
- The tag is `v` + the app version (`0.2.0` -> `v0.2.0`). CI rejects a tag that
  does not match.
- Releases are **drafts created by CI**. Do not upload installers by hand.
- Never move or delete a tag that already has a **published** release. Bump the
  version and publish a new one instead.
- The unsigned warning is expected. Never remove it from the release body
  unless the build is actually signed.
- Keep the bundled CLI pinned until changing it is a deliberate, reviewed
  change (see [Changing the bundled CLI](#changing-the-bundled-cli)).

## 1. Start from a clean main

```bash
git switch main
git pull --ff-only
git status --short          # must be empty
gh run list --workflow gitleaks --limit 1
```

The latest `gitleaks` run on `main` must be green.

## 2. Bump the version

Pick the next [SemVer](https://semver.org/) version and run:

```bash
npm run release:version -- 0.2.0
```

The script updates the five version files together:

```
package.json
package-lock.json
src-tauri/Cargo.toml
src-tauri/Cargo.lock
src-tauri/tauri.conf.json
```

Check the diff:

```bash
git diff -- package.json package-lock.json \
  src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/tauri.conf.json
```

All five must show the same version.

## 3. Validate locally

```bash
npm ci
npm run build
(cd src-tauri && cargo test --locked)
gitleaks git --redact --no-banner --verbose .
```

`cargo test --locked` is important: it is what CI runs, and it fails if
`Cargo.lock` was not bumped.

Optional macOS universal smoke test:

```bash
rustup target add aarch64-apple-darwin x86_64-apple-darwin
# The repo tracks the arm64 CLI; CI downloads both. To test locally, place the
# Intel CLI at src-tauri/bin/macos-x86_64/o2cloud too.
APPLE_SIGNING_IDENTITY=- \
  npm run tauri build -- --target universal-apple-darwin --bundles app,dmg
```

The CI workflow is the source of truth for Windows and Linux; they cannot be
built on macOS.

## 4. Commit and push

```bash
git add package.json package-lock.json \
  src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/tauri.conf.json
git commit -m "Release 0.2.0"
git push origin main
```

Wait for the `gitleaks` run on that commit to pass before tagging:

```bash
gh run list --workflow gitleaks --limit 1
```

## 5. Create and push the tag

```bash
git tag -a v0.2.0 -m "O2 Cloud 0.2.0"
git push origin v0.2.0
```

Pushing the tag triggers `.github/workflows/release.yml`. It:

1. verifies the tag against all version files;
2. creates the draft release from `.github/release-body.md`;
3. builds macOS universal, Windows x64 and Linux x64;
4. uploads the installers and their updater signatures (`.sig`) to the draft;
5. writes `latest.json` for the in-app updater from those signatures,
   refusing any not made with the app's key;
6. writes and uploads `SHA256SUMS.txt`.

## 6. Watch the workflow

```bash
gh run list --workflow release --limit 1
gh run watch <run-id> --exit-status
gh run view <run-id>
```

Expected jobs:

```
create-release
build (macos-latest, ...)
build (windows-latest, ...)
build (ubuntu-22.04, ...)
updater-json
checksums
```

Expected draft assets (names include the version):

- macOS: universal `.dmg` (arm64 + x86_64).
- Windows: `.msi` and NSIS `-setup.exe`.
- Linux: `.AppImage` and `.deb`.
- A `.sig` next to the macOS `.app.tar.gz`, both Windows installers and both
  Linux packages.
- `latest.json`.
- `SHA256SUMS.txt`.

All build, `updater-json` and checksum jobs must be green. If one fails, see
[Recovering from a failed run](#recovering-from-a-failed-run).

## 7. Review the draft

Inspect the draft and its assets:

```bash
gh release view v0.2.0 --json isDraft,assets,url
```

Download and verify the checksums:

```bash
mkdir -p "/tmp/o2cloud-v0.2.0"
gh release download v0.2.0 --dir "/tmp/o2cloud-v0.2.0"
cd "/tmp/o2cloud-v0.2.0"

# Linux:
sha256sum -c SHA256SUMS.txt
# macOS:
shasum -a 256 -c SHA256SUMS.txt
```

Every asset must report `OK`.

Add a `## Changes` section at the top of the release notes and keep the
standard sections (downloads, unsigned warning, credits) below it:

```bash
git log --oneline v0.1.0..v0.2.0
```

Smoke-test the installers when possible:

- macOS: mount the `.dmg`, open the app, accept the Gatekeeper prompt, check
  that Settings shows the bundled CLI 1.4.1.
- Windows: install the `.msi` or `-setup.exe`, accept the SmartScreen prompt,
  check the same.
- Linux: run the `.AppImage` or install the `.deb`, check the same.
- Re-run the opt-in `live_cli` with the bundled CLI: overwrite a file uploaded
  seconds earlier and confirm it waits for O2 instead of leaving two copies.
- Check `latest.json`: its version is the tag's, and every platform points to
  an asset of this release.

```bash
gh release download v0.2.0 --pattern latest.json --output - | jq '.version, (.platforms | map_values(.url))'
```

## 8. Publish

For a stable release:

```bash
gh release edit v0.2.0 --draft=false --latest
```

For a prerelease, mark it and do not make it latest:

```bash
gh release edit v0.2.0 --draft=false --prerelease --latest=false
```

Publishing makes the release GitHub's latest, and with it its `latest.json`:
from then on every installed app from 0.2.5 on offers it on startup. A draft
or prerelease is not offered.

## 9. After publishing

- Open the public release URL in a private window and download one asset.
- Confirm the downloaded file matches `SHA256SUMS.txt`.
- Install on each platform and confirm the app lists files and Settings shows
  the CLI version/origin.
- On a machine with the previous release installed, open it: it must offer the
  new version, and «Actualizar y reiniciar» must leave the new version running
  with no second copy of the app.
- Update the release checklist in `PENDING_TESTS.md` if a platform was tested
  for the first time.
- Announce the release with the public URL.

## Recovering from a failed run

### The workflow failed before the release was published

The release is still a draft. If the fix changes code, delete the draft and
the tag, fix on `main`, and recreate the tag:

```bash
gh release delete v0.2.0 --yes
git push --delete origin v0.2.0
git tag -d v0.2.0

# fix, commit and push to main; wait for gitleaks
git tag -a v0.2.0 -m "O2 Cloud 0.2.0"
git push origin v0.2.0
```

If the code is correct and only a runner failed, re-run the failed jobs from
the Actions UI or with `gh run rerun <run-id> --failed`. The action replaces
assets with the same names.

Whenever a build asset is replaced, re-run the `checksums` job too; otherwise
`SHA256SUMS.txt` still describes the old file. You can re-run just that job
from the Actions UI, or with
`gh run rerun <run-id> --job <checksums-job-id>`.

### The release was already published

Do not move the tag or replace its assets. Bump the patch version and publish
a new release.

### The version check rejects the tag

The tag does not match the app version, or the five version files disagree.
Delete the tag, fix the version with `npm run release:version -- X.Y.Z`, commit,
push, and recreate the tag.

### A checksum does not match

Do not publish. Delete the draft and re-run the release. If it happens again,
check whether a proxy or download mirror altered the asset.

### CI refuses because a published release already exists

That is intentional: the workflow will not replace assets on a published
release. Bump the version.

## Changing the bundled CLI

The workflow pins `o2cloud` 1.4.1 in three places:

- macOS download URLs and SHA-256 hashes;
- Windows download URL and SHA-256 hash;
- Linux source tag and commit (`e10aa6a`), built on Ubuntu 22.04.

The CI-only configs `src-tauri/tauri.macos.release.conf.json`,
`tauri.windows.release.conf.json` and `tauri.linux.release.conf.json` select the
CLI that goes into each installer. Keep them in sync when the layout changes.

To change the CLI version:

1. Update the URLs, hashes and commit in `.github/workflows/release.yml`.
2. Update `MIN_VERSION` in `src-tauri/src/cli.rs` if the minimum changes,
   and `SUPPORTED_MAJOR` when moving to a new major version (Settings only
   offers updates within it).
3. Update the CLI section in `README.md` and `THIRD_PARTY_NOTICES.md` if
   licenses or notices changed.
4. Run the smoke test and a full release build on all three platforms.
5. Document the new Linux GLIBC requirement, if any.

Do this as a separate pull request, not as part of an app version bump.

## If code signing is ever added

This is optional and is not required for a release. When an Apple Developer ID
is available:

1. Add the signing secrets to the repository.
2. Replace the ad-hoc step (`APPLE_SIGNING_IDENTITY=-`) with the real identity
   and notarization variables.
3. Remove the macOS unsigned warning from `.github/release-body.md`.
4. Test the notarized `.dmg` on a clean Mac.

For Windows, configure Authenticode signing in the Tauri bundle settings and
remove the Windows unsigned warning. Linux packages can be signed separately.

The in-app updater keeps working with code signing; it only needs its own
key (below).

## The updater signing key

The app (0.2.5 and later) installs an update only if it is signed with the
private key whose public half is `plugins.updater.pubkey` in
`src-tauri/tauri.conf.json`.

- Private key: `~/.tauri/o2cloud-app-updater.key` on the release Mac. Its
  password is in the macOS Keychain as «O2 Cloud updater signing key
  password» (service `o2cloud-app updater key password`).
- CI reads both from the repository secrets `TAURI_SIGNING_PRIVATE_KEY` (the
  key file's contents) and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. The
  `create-release` job fails at once if either is missing.
- **Keep a backup of the key and its password** (a password manager). If the
  key is lost, installed apps can never be updated again from the app: a new
  key means a new `pubkey`, which only a manual install brings.
- Never commit the private key. The public key is meant to be public.

To set or replace the secrets:

```bash
gh secret set TAURI_SIGNING_PRIVATE_KEY < ~/.tauri/o2cloud-app-updater.key
security find-generic-password -s "o2cloud-app updater key password" -w \
  | tr -d '\n' | gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD
```

Local release builds with the release configs need the same two variables:

```bash
export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/o2cloud-app-updater.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(security find-generic-password -s "o2cloud-app updater key password" -w)"
```

## Checklist

Copy this into the release pull request or issue:

```
- [ ] main is clean and the latest gitleaks run is green
- [ ] npm run release:version -- X.Y.Z (five files match)
- [ ] npm run build
- [ ] cargo test --locked
- [ ] commit "Release X.Y.Z" pushed to main
- [ ] gitleaks green on that commit
- [ ] annotated tag vX.Y.Z pushed
- [ ] create-release, three builds, updater-json and checksums are green
- [ ] SHA256SUMS.txt downloaded and verified
- [ ] latest.json has this version and points to this release's assets
- [ ] draft notes have a Changes section and the standard warnings
- [ ] installers smoke-tested on macOS, Windows and Linux when available
- [ ] release published (not draft), prerelease flag if needed
- [ ] public download and checksum verified
- [ ] PENDING_TESTS.md updated
```
