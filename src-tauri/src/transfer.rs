use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::cli;
use crate::types::{
    ChunkManifest, CliSplitManifest, LogicalFile, ManifestPart, PartRef, RemoteFile,
    TransferProgress, UploadOutput,
};

const READ_BUFFER: usize = 1024 * 1024;

/// A CLI split without a manifest is only grouped when its first part is
/// large enough to be a real split (the CLI uses 4000 MiB). This keeps
/// ordinary `photo.001` files from being treated as one file.
const CLI_SPLIT_MIN_FIRST_PART: u64 = 3_900 * 1024 * 1024;

/// The CLI's split part size in 0.9/0.10: 4000 MiB. The app uses the same
/// layout to write the manifest that turns a CLI split into a verified
/// logical file. Keep this in sync with `o2cloud-cli`'s `DEFAULT_PART_SIZE`.
const CLI_PART_SIZE: u64 = 4000 * 1024 * 1024;
const CLI_SPLIT_THRESHOLD: u64 = CLI_PART_SIZE;
const CLI_MAX_PARTS: u64 = 999;

/// Fallback part size for the app's own `.o2part` chunking if
/// `PREFER_CLI_SPLIT` is turned off. It is a constant now: the user-facing
/// chunk-size setting was removed because the CLI owns that decision.
pub const APP_CHUNK_SIZE: u64 = 3500 * 1024 * 1024;

/// Prototype switch: files above 4 GiB are split by the CLI and get an
/// `o2split.json` manifest from the app; smaller files go up as a single
/// file. Set this to false to return to the app's own `.o2part` chunking.
const PREFER_CLI_SPLIT: bool = true;

/// Hashing a file before uploading costs one full read. On a slow external
/// disk that takes minutes per file (a 20 GB video at 40 MB/s is over eight
/// minutes), and a batch would not start uploading until every file had been
/// read. Above this size the file is only hashed when something in O2 or the
/// library already has its size: a content duplicate must have the same size,
/// so a unique size cannot be one. Name+size matching still runs for every
/// file, and the SHA-256 is recorded after the upload.
pub const PREFLIGHT_HASH_MAX_BYTES: u64 = 512 * 1024 * 1024;

pub fn should_hash_before_upload(size: u64, same_size_exists: bool) -> bool {
    size <= PREFLIGHT_HASH_MAX_BYTES || same_size_exists
}

fn emit(app: &AppHandle, progress: TransferProgress) {
    let _ = app.emit("transfer-progress", progress);
}

/// Mark a transfer as failed, so its row doesn't look stuck.
pub fn emit_failed(app: &AppHandle, transfer_id: &str, kind: &str, name: &str, err: &str) {
    emit(
        app,
        TransferProgress {
            id: transfer_id.into(),
            kind: kind.into(),
            name: name.into(),
            phase: "error".into(),
            current: 0,
            total: 0,
            bytes_done: 0,
            bytes_total: 0,
            message: err.into(),
        },
    );
}

fn append_folder_arg(args: &mut Vec<String>, folder_id: Option<u64>) {
    if let Some(id) = folder_id {
        args.push("--folder".into());
        args.push(id.to_string());
    }
}

/// A transfer whose data stops moving for this long is stopped and tried
/// again. The CLI sets no time limit on sending or receiving file data, so
/// a dead connection would otherwise hold up the queue forever.
const STALLED_AFTER: Duration = Duration::from_secs(3 * 60);

/// How long O2 may take while no data can move: before a download's first
/// byte (the CLI lists the whole account first) and once an upload is sent
/// (O2 stores it before answering).
const WAITING_LIMIT: Duration = Duration::from_secs(15 * 60);

/// The waits before each new attempt at a transfer that failed for a reason
/// that may pass. At least a minute, so that an upload O2 stored despite the
/// error is in its listings by then (that takes up to 60 s).
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(60),
    Duration::from_secs(2 * 60),
    Duration::from_secs(4 * 60),
];

/// How the error of a transfer stopped by its `Watchdog` starts.
const STALLED: &str = "La transferencia se ha atascado";

/// Notices a transfer that has gone too long without moving.
struct Watchdog {
    last_move: Instant,
}

impl Watchdog {
    fn new() -> Self {
        Self {
            last_move: Instant::now(),
        }
    }

    fn moved(&mut self) {
        self.last_move = Instant::now();
    }

    /// Break once nothing has moved for `limit`.
    fn check(&self, limit: Duration) -> ControlFlow<String> {
        if self.last_move.elapsed() < limit {
            ControlFlow::Continue(())
        } else {
            ControlFlow::Break(format!("{STALLED}: {} sin avanzar", minutes(limit)))
        }
    }
}

fn minutes(duration: Duration) -> String {
    format!("{} min", duration.as_secs().div_ceil(60))
}

/// Whether a failed transfer may work if tried again: the connection
/// failed, O2 was busy or the transfer stalled. Not when O2 refused it
/// (e.g. MED-1020, the 4 GB limit), the session expired or a local file
/// couldn't be read or written.
fn worth_retrying(err: &str) -> bool {
    err.starts_with(STALLED)
        || err.starts_with("HTTP request failed")
        // 0.9.0 checks that every byte arrived and removes the partial file;
        // a short read is worth another attempt. A file uploaded a moment
        // ago may also have no signed URL until O2 finishes validating it.
        || err.contains("Download incomplete")
        || err.contains("has no download URL yet")
        || err.match_indices("(status ").any(|(at, marker)| {
            let code = err[at + marker.len()..].get(..3);
            matches!(
                code.and_then(|c| c.parse::<u16>().ok()),
                Some(429 | 500..=599)
            )
        })
}

/// How long to wait before trying a failed transfer again, if at all.
fn retry_delay(err: &str, retries: usize) -> Option<Duration> {
    RETRY_DELAYS
        .get(retries)
        .copied()
        .filter(|_| worth_retrying(err))
}

/// What a transfer's row says while it waits to try again. The row shows
/// the first line; the error is there on hover.
fn retry_notice(err: &str, delay: Duration, retry: usize) -> String {
    format!(
        "Ha fallado; reintento {retry} de {} en {}…\n{err}",
        RETRY_DELAYS.len(),
        minutes(delay)
    )
}

fn gave_up(err: String, retries: usize) -> String {
    if retries == 0 {
        err
    } else {
        format!("Ha fallado {} veces: {err}", retries + 1)
    }
}

/// Upload `path` with the CLI and return its new ID, trying again when it
/// fails for a reason that may pass. `sent` is called with how much of the
/// file the CLI has sent so far, each time that grows, and with 0 and a
/// notice while waiting to try again.
async fn cli_upload(
    app: &AppHandle,
    cli_path: &Path,
    path: &Path,
    folder_id: Option<u64>,
    sent: impl FnMut(u64, Option<&str>),
) -> Result<u64, String> {
    cli_upload_full(app, cli_path, path, folder_id, OnConflict::Ask, sent)
        .await?
        .single_id()
}

/// What to do when the folder already has a file of that name with another
/// size (the CLI's `--on-conflict`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnConflict {
    /// Don't upload it: the upload fails with a conflict (see
    /// `conflict_of`) for the user to decide.
    Ask,
    /// Replace the cloud's copy (it goes to O2's trash once the new one is
    /// up).
    Overwrite,
    /// Leave the cloud's copy: used to resume a split upload, whose parts
    /// already in the folder are skipped.
    Skip,
}

/// `cli_upload` with the conflict policy, returning what the CLI said:
/// the ID(s), and the SHA-256 it computed while sending the file.
async fn cli_upload_full(
    app: &AppHandle,
    cli_path: &Path,
    path: &Path,
    folder_id: Option<u64>,
    on_conflict: OnConflict,
    mut sent: impl FnMut(u64, Option<&str>),
) -> Result<UploadOutput, String> {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let size = std::fs::metadata(path)
        .map_err(|e| format!("No se puede leer {}: {e}", path.display()))?
        .len();
    let mut retries = 0;
    loop {
        let result: Result<UploadOutput, String> = async {
            // An upload can fail after O2 got all of it; sending it again
            // would make a duplicate.
            if retries > 0
                && let Some(id) = find_uploaded(app, cli_path, folder_id, &name, size).await?
            {
                return Ok(UploadOutput {
                    id: Some(id),
                    ..Default::default()
                });
            }
            upload_attempt(app, cli_path, path, folder_id, size, on_conflict, &mut sent).await
        }
        .await;
        let err = match result {
            Ok(id) => return Ok(id),
            Err(err) => err,
        };
        let Some(delay) = retry_delay(&err, retries) else {
            return Err(gave_up(err, retries));
        };
        retries += 1;
        sent(0, Some(&retry_notice(&err, delay, retries)));
        tokio::time::sleep(delay).await;
    }
}

/// Upload a file with the CLI's native split and `--skip-existing`. The CLI
/// skips the parts already in the folder, so retrying is safe. Returns the
/// full `parts` list, including skipped parts.
async fn cli_upload_split(
    app: &AppHandle,
    cli_path: &Path,
    path: &Path,
    folder_id: Option<u64>,
    size: u64,
    on_conflict: OnConflict,
    mut sent: impl FnMut(u64, Option<&str>),
) -> Result<UploadOutput, String> {
    // Resuming keeps the parts already there; the user's choice to
    // overwrite replaces another version's.
    let policy = match on_conflict {
        OnConflict::Overwrite => OnConflict::Overwrite,
        _ => OnConflict::Skip,
    };
    let mut retries = 0;
    loop {
        let result = upload_attempt(app, cli_path, path, folder_id, size, policy, &mut sent).await;
        let err = match result {
            Ok(output) => return Ok(output),
            Err(err) => err,
        };
        let Some(delay) = retry_delay(&err, retries) else {
            return Err(gave_up(err, retries));
        };
        retries += 1;
        sent(0, Some(&retry_notice(&err, delay, retries)));
        tokio::time::sleep(delay).await;
    }
}

/// The ID of a file named `name` with `size` bytes in `folder_id`, if O2
/// lists one.
async fn find_uploaded(
    app: &AppHandle,
    cli_path: &Path,
    folder_id: Option<u64>,
    name: &str,
    size: u64,
) -> Result<Option<u64>, String> {
    let files = current_folder_files(app, cli_path, folder_id).await?;
    Ok(files
        .into_iter()
        .find(|file| file.name == name && file.size == Some(size))
        .map(|file| file.id))
}

/// One run of `o2cloud upload --json-lines`, stopped if it stalls. The
/// CLI reports how much it has sent (`progress` events) and what became of
/// the file (a `file` event); a file of another size already in the folder
/// is a conflict (see `conflict_of`) unless `on_conflict` says otherwise.
async fn upload_attempt(
    app: &AppHandle,
    cli_path: &Path,
    path: &Path,
    folder_id: Option<u64>,
    size: u64,
    on_conflict: OnConflict,
    sent: &mut impl FnMut(u64, Option<&str>),
) -> Result<UploadOutput, String> {
    let args = upload_args(path, folder_id, on_conflict);
    let progress = Arc::new(AtomicU64::new(0));
    let waiting = Arc::new(AtomicBool::new(false));
    let mut last = 0;
    let mut watchdog = Watchdog::new();
    let exit = cli::run_events(
        app,
        cli_path,
        &args,
        progress.clone(),
        waiting.clone(),
        |_pid| {
            // The CLI is waiting for O2 to validate a recent copy
            // (`MED-1017`): a replacement may take up to 2 min per step,
            // so this is not a stalled transfer.
            if waiting.swap(false, Ordering::Relaxed) {
                // The replacement starts after the file is sent: treat it
                // as a completed upload, with O2's own wait as the limit.
                last = size;
                watchdog.moved();
                sent(
                    size,
                    Some("O2 está comprobando una copia reciente; esperando…"),
                );
            }
            let now = progress.load(Ordering::Relaxed);
            if now > last {
                last = now;
                watchdog.moved();
                sent(now, None);
            }
            // Data moves while the CLI sends the file. Before, it is getting
            // ready (listing, waiting for recent uploads); after, O2 is storing
            // what it got.
            watchdog.check(if last > 0 && last < size {
                STALLED_AFTER
            } else {
                WAITING_LIMIT
            })
        },
    )
    .await?;
    upload_result(exit.code, &exit.output)
}

/// The CLI arguments of one upload.
fn upload_args(path: &Path, folder_id: Option<u64>, on_conflict: OnConflict) -> Vec<String> {
    let mut args = vec![
        "upload".to_string(),
        path.to_string_lossy().to_string(),
        "--json-lines".to_string(),
    ];
    match on_conflict {
        OnConflict::Ask => {}
        OnConflict::Overwrite => args.extend(["--on-conflict".into(), "overwrite".into()]),
        OnConflict::Skip => args.extend(["--on-conflict".into(), "skip".into()]),
    }
    append_folder_arg(&mut args, folder_id);
    args
}

/// What an `upload --json-lines` run ended with, from its exit code and
/// events.
fn upload_result(code: i32, output: &cli::CliOutput) -> Result<UploadOutput, String> {
    let events: Vec<serde_json::Value> = output
        .stdout
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let last = |kind: &str| events.iter().rev().find(|e| e["event"] == kind);
    if let Some(error) = last("error") {
        let code = error["code"].as_str().unwrap_or_default();
        if code == "session_expired" || code == "not_logged_in" {
            return Err(cli::SESSION_EXPIRED.into());
        }
        if code != "conflict" {
            return Err(explain_rejection(
                error["message"].as_str().unwrap_or("error"),
            ));
        }
    }
    match (code, last("file")) {
        (0, Some(file)) => {
            let output: UploadOutput =
                serde_json::from_value(file.clone()).map_err(|e| e.to_string())?;
            match output.action.as_deref() {
                // Resuming a split upload found parts of another version.
                Some("differs") => Err(format!(
                    "O2 ya tiene otra versión de {} (de otro tamaño) en esta carpeta; \
                     bórrala o cambia el nombre antes de subir esta.",
                    file["name"].as_str().unwrap_or("este archivo")
                )),
                _ => Ok(output),
            }
        }
        (cli::EXIT_CONFLICT, Some(file)) if file["action"] == "conflict" => {
            Err(format!("{CONFLICT_PREFIX}{file}"))
        }
        (0, None) => Err("El CLI no informó del archivo subido.".into()),
        _ => Err(explain_rejection(cli::error_detail(&output.stderr))),
    }
}

/// An upload error as the row shows it. When O2 rejects a file whose
/// extension isn't what it is (`MED-1050`, `MED-1000`), CLI 1.4+ adds
/// "… — it's a HEIC image named .jpg: `o2cloud fix <file> --apply` renames
/// it to .heic". The app's user has no CLI to run, so the first line (what
/// the row shows) says it in Spanish with how to fix it by hand; the CLI's
/// text stays below it, shown on hover.
fn explain_rejection(message: &str) -> String {
    match wrong_extension(message) {
        Some(notice) => format!("{notice}\n{message}"),
        None => message.to_string(),
    }
}

fn wrong_extension(message: &str) -> Option<String> {
    let (_, reason) = message.split_once(" — it's ")?;
    let (described, rest) = reason.split_once(" named .")?;
    let (named, rest) = rest.split_once(": ")?;
    let (_, right) = rest.rsplit_once(" renames it to .")?;
    let described = described
        .strip_prefix("an ")
        .or_else(|| described.strip_prefix("a "))?;
    let kind = [
        ("image", "una imagen"),
        ("video", "un vídeo"),
        ("audio file", "un audio"),
        ("document", "un documento"),
    ]
    .iter()
    .find_map(|(what, es)| {
        let name = described.strip_suffix(what)?.trim_end();
        (!name.is_empty()).then(|| format!("{es} {name}"))
    })?;
    Some(format!(
        "O2 no lo acepta: es {kind}, aunque su nombre acabe en .{named}. \
         Cambia la extensión del archivo a .{right} y vuelve a subirlo."
    ))
}

/// How an upload error that is a conflict starts: the CLI's `file` event
/// follows, for the frontend to ask what to do.
pub const CONFLICT_PREFIX: &str = "\u{1}conflict:";

/// A file of another size already in the folder: what the CLI said about
/// it (`local`, `name`, `size`, `modified`, and `cloud` with the cloud
/// copy's `size`, `modified` and `id` or `parts`).
pub fn conflict_of(err: &str) -> Option<serde_json::Value> {
    serde_json::from_str(err.strip_prefix(CONFLICT_PREFIX)?).ok()
}

/// A conflict as a person reads it.
pub fn conflict_message(conflict: &serde_json::Value) -> String {
    let size = |v: &serde_json::Value| v.as_u64().map(human_size).unwrap_or_else(|| "?".into());
    format!(
        "Ya hay un {} de otro tamaño en O2 ({} allí, {} aquí)",
        conflict["name"].as_str().unwrap_or("archivo"),
        size(&conflict["cloud"]["size"]),
        size(&conflict["size"]),
    )
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} B", bytes)
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

/// Download media `id` to `path` with the CLI, trying again when it fails
/// for a reason that may pass. `received` is called with the size of the
/// file so far, each time that grows, and with 0 and a notice while waiting
/// to try again.
async fn cli_download(
    app: &AppHandle,
    cli_path: &Path,
    target: &str,
    path: &Path,
    mut received: impl FnMut(u64, Option<&str>),
) -> Result<(), String> {
    let mut retries = 0;
    loop {
        let err = match download_attempt(app, cli_path, target, path, &mut received).await {
            Ok(()) => return Ok(()),
            Err(err) => err,
        };
        let Some(delay) = retry_delay(&err, retries) else {
            // The CLI marks a download in progress with a hidden file next
            // to it; the app removes its own partial file, so that too.
            let _ = tokio::fs::remove_file(download_marker(path)).await;
            return Err(gave_up(err, retries));
        };
        retries += 1;
        received(0, Some(&retry_notice(&err, delay, retries)));
        tokio::time::sleep(delay).await;
    }
}

/// The hidden file the CLI keeps next to a download while it's incomplete
/// (`.<name>.o2dl`), so running it again resumes it.
fn download_marker(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.o2dl"))
}

/// One run of `o2cloud download`, stopped if it stalls. `target` is a media
/// ID, a path, or a base path the CLI can join from `.001` parts.
async fn download_attempt(
    app: &AppHandle,
    cli_path: &Path,
    target: &str,
    path: &Path,
    received: &mut impl FnMut(u64, Option<&str>),
) -> Result<(), String> {
    // `path` is always the app's own file. One left by a stopped attempt
    // would make the CLI refuse to write.
    let _ = tokio::fs::remove_file(path).await;
    let args = vec![
        "download".to_string(),
        target.to_string(),
        "-o".to_string(),
        path.to_string_lossy().to_string(),
    ];
    let mut last = None;
    let mut watchdog = Watchdog::new();
    cli::run_watched(app, cli_path, &args, |_| {
        // The CLI creates the file once O2 starts sending it.
        if let Ok(meta) = std::fs::metadata(path)
            && Some(meta.len()) > last
        {
            last = Some(meta.len());
            watchdog.moved();
            received(meta.len(), None);
        }
        watchdog.check(if last.is_some() {
            STALLED_AFTER
        } else {
            WAITING_LIMIT
        })
    })
    .await
    .map(|_| ())
}

pub async fn current_folder_files(
    app: &AppHandle,
    cli_path: &Path,
    folder_id: Option<u64>,
) -> Result<Vec<RemoteFile>, String> {
    let mut args = vec!["ls".to_string(), "--json".to_string()];
    if let Some(id) = folder_id {
        args.push(id.to_string());
    }
    let listing: crate::types::LsResponse = cli::run_json(app, cli_path, &args).await?;
    Ok(listing.files)
}

/// Files already in a folder: the sizes seen for each name.
pub type ExistingFiles = HashMap<String, Vec<u64>>;

pub fn existing_files(files: &[RemoteFile]) -> ExistingFiles {
    let mut existing = ExistingFiles::new();
    for file in files {
        if let Some(size) = file.size {
            existing.entry(file.name.clone()).or_default().push(size);
        }
    }
    existing
}

pub(crate) fn already_uploaded(existing: &ExistingFiles, name: &str, size: u64) -> bool {
    existing
        .get(name)
        .is_some_and(|sizes| sizes.contains(&size))
}

/// Logical files already in a listing, with their full size. Unlike
/// `existing_files`, a completed chunked file appears under its logical name
/// (not its parts), which lets the Photos view skip it before exporting it.
pub fn logical_files_existing(files: &[RemoteFile]) -> ExistingFiles {
    let (logical, _) = group_logical_files(files.to_vec());
    let mut existing = ExistingFiles::new();
    for file in logical.into_iter().filter(|file| !file.incomplete) {
        existing.entry(file.name).or_default().push(file.size);
    }
    existing
}

pub enum UploadOutcome {
    Uploaded,
    /// A file with the same name and size was already in the folder, from
    /// an earlier upload. Uploading it again would only make a duplicate.
    Skipped,
}

/// What an upload produced, so the shared upload library can record it.
pub struct UploadReceipt {
    pub outcome: UploadOutcome,
    pub sha256: Option<String>,
    /// The new file ID, or a chunked file's manifest ID.
    pub remote_id: Option<u64>,
    /// The name O2 now uses for the file/manifest.
    pub remote_name: String,
    pub size: u64,
}

/// SHA-256 of a local file, read in chunks.
pub async fn sha256_file(path: &Path) -> Result<String, String> {
    sha256_file_with_progress(path, |_| {}).await
}

/// SHA-256 of a local file, read in chunks. `progress` is called with the
/// bytes hashed so far after each read, so a long hash can show it is alive.
pub async fn sha256_file_with_progress(
    path: &Path,
    mut progress: impl FnMut(u64),
) -> Result<String, String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| format!("No se puede abrir {}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; READ_BUFFER];
    let mut hashed = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        hashed += read as u64;
        progress(hashed);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// SHA-256 of the whole file and of each part, in one sequential pass.
/// `parts` are the CLI layout ranges; the returned hashes are in the same
/// order.
async fn sha256_split_file(
    path: &Path,
    parts: &[(String, u64, u64)],
    mut progress: impl FnMut(u64),
) -> Result<(String, Vec<String>), String> {
    if parts.is_empty() {
        return Err("No hay partes que verificar".into());
    }
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|error| format!("No se puede abrir {}: {error}", path.display()))?;
    let mut whole = Sha256::new();
    let mut part_hashes = Vec::with_capacity(parts.len());
    let mut part_index = 0usize;
    let mut part_left = parts[0].2;
    let mut part_hasher = Sha256::new();
    let mut hashed = 0u64;
    let mut buffer = vec![0u8; READ_BUFFER];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        let mut at = 0usize;
        while at < read {
            if part_index >= parts.len() {
                return Err("El archivo tiene más datos que partes".into());
            }
            let take = ((read - at) as u64).min(part_left) as usize;
            whole.update(&buffer[at..at + take]);
            part_hasher.update(&buffer[at..at + take]);
            hashed += take as u64;
            part_left -= take as u64;
            at += take;
            if part_left == 0 {
                part_hashes.push(hex::encode(
                    std::mem::replace(&mut part_hasher, Sha256::new()).finalize(),
                ));
                part_index += 1;
                if part_index < parts.len() {
                    part_left = parts[part_index].2;
                }
            }
        }
        progress(hashed);
    }
    if part_index != parts.len() || part_left != 0 {
        return Err(format!(
            "El archivo no coincide con las {} partes esperadas",
            parts.len()
        ));
    }
    Ok((hex::encode(whole.finalize()), part_hashes))
}

/// A logical file is a normal object, the app's chunk group or a CLI split.
pub fn group_logical_files(files: Vec<RemoteFile>) -> (Vec<LogicalFile>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut old_manifests: HashMap<String, RemoteFile> = HashMap::new();
    let mut old_parts: HashMap<String, Vec<RemoteFile>> = HashMap::new();
    let mut cli_manifests: HashMap<(String, String), RemoteFile> = HashMap::new();
    let mut cli_candidates: HashMap<(String, String), Vec<(u64, RemoteFile)>> = HashMap::new();
    let mut normals: Vec<RemoteFile> = Vec::new();
    let mut normal_keys: HashSet<(String, String)> = HashSet::new();

    for file in files {
        let folder = remote_parent(file.path.as_deref());
        if let Some(base) = file.name.strip_suffix(".o2manifest.json") {
            old_manifests.insert(base.to_string(), file);
        } else if let Some(base) = file.name.strip_suffix(".o2split.json") {
            cli_manifests.insert((folder, base.to_string()), file);
        } else if let Some((base, _)) = split_part_name(&file.name) {
            old_parts.entry(base).or_default().push(file);
        } else if let Some((base, number)) = cli_part_name(&file.name) {
            cli_candidates
                .entry((folder, base))
                .or_default()
                .push((number, file));
        } else {
            normal_keys.insert((folder, file.name.clone()));
            normals.push(file);
        }
    }

    let mut logical = Vec::new();

    // CLI-native split: `name.001`, `name.002`, … plus an optional
    // `name.o2split.json` written by the app. Without a manifest, only group
    // parts large enough to be a real CLI split, so ordinary `photo.001`
    // files are not mistaken for one.
    for ((folder, base), mut entries) in cli_candidates {
        entries.sort_by_key(|(number, _)| *number);
        let manifest = cli_manifests.remove(&(folder.clone(), base.clone()));
        let has_manifest = manifest.is_some();
        let numbers_ok = entries
            .first()
            .map(|(number, _)| *number == 1)
            .unwrap_or(false)
            && entries.windows(2).all(|pair| pair[1].0 == pair[0].0 + 1);
        let sizes: Vec<Option<u64>> = entries.iter().map(|(_, file)| file.size).collect();
        let sizes_known = sizes.iter().all(Option::is_some);
        let first_size = sizes.first().copied().flatten().unwrap_or(0);
        let non_last_equal = sizes[..sizes.len().saturating_sub(1)]
            .iter()
            .all(|size| *size == sizes.first().copied().flatten());
        // The CLI always makes the last part strictly smaller, including when
        // the size is an exact multiple. This also catches a split whose last
        // part is still missing: the highest part found is then full-size.
        let last_ok = sizes
            .last()
            .copied()
            .flatten()
            .is_some_and(|last| last < first_size);
        let base_exists = normal_keys.contains(&(folder, base.clone()));
        let large_enough = first_size >= CLI_SPLIT_MIN_FIRST_PART;
        let valid = entries.len() >= 2
            && numbers_ok
            && sizes_known
            && non_last_equal
            && last_ok
            && !base_exists
            && (has_manifest || large_enough);

        if !valid {
            if has_manifest || (large_enough && entries.len() >= 2) {
                warnings.push(format!(
                    "'{base}' tiene partes .001 que no forman un conjunto completo; se muestran por separado."
                ));
            }
            if let Some(manifest) = manifest {
                normals.push(manifest);
            }
            normals.extend(entries.into_iter().map(|(_, file)| file));
            continue;
        }

        let total: u64 = sizes.iter().flatten().sum();
        let refs: Vec<PartRef> = entries
            .iter()
            .map(|(_, file)| PartRef {
                id: file.id,
                name: file.name.clone(),
                size: file.size.unwrap_or(0),
            })
            .collect();

        if !has_manifest {
            warnings.push(format!(
                "'{base}' tiene partes .001 del CLI; se puede descargar unido, pero sin verificación."
            ));
        }

        logical.push(LogicalFile {
            name: base,
            size: total,
            date: entries.first().and_then(|(_, file)| file.date.clone()),
            chunked: true,
            cli_split: true,
            manifest_id: manifest.as_ref().map(|m| m.id),
            normal_id: None,
            parts: refs,
            incomplete: false,
        });
    }

    for file in normals {
        logical.push(LogicalFile {
            name: file.name,
            size: file.size.unwrap_or(0),
            date: file.date,
            chunked: false,
            cli_split: false,
            manifest_id: None,
            normal_id: Some(file.id),
            parts: Vec::new(),
            incomplete: false,
        });
    }

    // The app's own chunks: `name.o2part.001`, … + `name.o2manifest.json`.
    for (base, mut chunk_files) in old_parts {
        chunk_files.sort_by(|a, b| a.name.cmp(&b.name));
        let manifest = old_manifests.remove(&base);
        let total: u64 = chunk_files.iter().map(|f| f.size.unwrap_or(0)).sum();
        let refs: Vec<PartRef> = chunk_files
            .iter()
            .map(|f| PartRef {
                id: f.id,
                name: f.name.clone(),
                size: f.size.unwrap_or(0),
            })
            .collect();

        if manifest.is_none() {
            warnings.push(format!(
                "'{base}' tiene fragmentos pero no un manifiesto; se puede descargar, pero sin verificación."
            ));
        }

        logical.push(LogicalFile {
            name: base,
            size: total,
            date: chunk_files.first().and_then(|f| f.date.clone()),
            chunked: true,
            cli_split: false,
            manifest_id: manifest.as_ref().map(|m| m.id),
            normal_id: None,
            parts: refs,
            incomplete: manifest.is_none(),
        });
    }

    for (base, manifest) in old_manifests {
        warnings.push(format!(
            "'{base}' tiene manifiesto pero no se han encontrado sus fragmentos."
        ));
        logical.push(LogicalFile {
            name: base,
            size: 0,
            date: manifest.date,
            chunked: true,
            cli_split: false,
            manifest_id: Some(manifest.id),
            normal_id: None,
            parts: Vec::new(),
            incomplete: true,
        });
    }

    for ((_folder, base), manifest) in cli_manifests {
        warnings.push(format!(
            "'{base}' tiene un manifiesto de split del CLI pero no se han encontrado sus partes."
        ));
        logical.push(LogicalFile {
            name: base,
            size: 0,
            date: manifest.date,
            chunked: true,
            cli_split: true,
            manifest_id: Some(manifest.id),
            normal_id: None,
            parts: Vec::new(),
            incomplete: true,
        });
    }

    logical.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    (logical, warnings)
}

/// The folder part of a full `ls -a` path, or "" when the listing came from a
/// single folder and has no paths.
fn remote_parent(path: Option<&str>) -> String {
    match path.and_then(|path| path.rsplit_once('/')) {
        Some(("", _)) => "/".into(),
        Some((folder, _)) => folder.to_string(),
        None => String::new(),
    }
}

fn split_part_name(name: &str) -> Option<(String, u64)> {
    let marker = ".o2part.";
    let idx = name.rfind(marker)?;
    let base = &name[..idx];
    let number = &name[idx + marker.len()..];
    if number.is_empty() || !number.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((base.to_string(), number.parse().ok()?))
}

/// `name.001` → (`name`, 1). Excludes the app's own `.o2part.NNN` format and
/// only accepts the three-digit suffix the CLI produces.
fn cli_part_name(name: &str) -> Option<(String, u64)> {
    if name.contains(".o2part.") {
        return None;
    }
    let (base, number) = name.rsplit_once('.')?;
    if base.is_empty() || number.len() != 3 || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let number = number.parse().ok()?;
    (number > 0).then(|| (base.to_string(), number))
}

/// The CLI's layout for a file (`name`, offset, length per part). It mirrors
/// `upload_pieces` in `o2cloud-cli` 0.9/0.10, including the one-byte shrink
/// when the size is an exact multiple of the part size. The last part is
/// always smaller, which is what `download` uses to detect a truncated set.
fn cli_split_layout(name: &str, size: u64) -> Result<(u64, Vec<(String, u64, u64)>), String> {
    cli_split_layout_with(name, size, CLI_PART_SIZE)
}

fn cli_split_layout_with(
    name: &str,
    size: u64,
    part_size: u64,
) -> Result<(u64, Vec<(String, u64, u64)>), String> {
    if part_size < 2 {
        return Err("El tamaño de parte debe ser al menos 2 bytes".into());
    }
    if size <= part_size {
        return Ok((part_size, vec![(name.to_string(), 0, size)]));
    }
    let mut part = part_size;
    while size % part == 0 && part > 2 {
        part -= 1;
    }
    let count = size.div_ceil(part);
    if count > CLI_MAX_PARTS {
        return Err(format!(
            "{name} necesitaría {count} partes; el máximo de la CLI es {CLI_MAX_PARTS}"
        ));
    }
    let pieces = (0..count)
        .map(|index| {
            let offset = index * part;
            let length = part.min(size - offset);
            (format!("{name}.{:03}", index + 1), offset, length)
        })
        .collect();
    Ok((part, pieces))
}

/// Upload one local file into `folder_id`, chunked if it is larger than
/// `chunk_size`. `existing` is what the folder already holds.
#[allow(clippy::too_many_arguments)]
pub async fn upload_one(
    app: &AppHandle,
    cli_path: &Path,
    path: &Path,
    folder_id: Option<u64>,
    chunk_size: u64,
    transfer_id: &str,
    existing: &ExistingFiles,
    known_sha256: Option<&str>,
    on_conflict: OnConflict,
) -> Result<UploadReceipt, String> {
    let result = upload_file(
        app,
        cli_path,
        path,
        folder_id,
        chunk_size,
        transfer_id,
        existing,
        known_sha256,
        on_conflict,
    )
    .await;
    if let Err(err) = &result {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        // A conflict waits for the user's answer; its row says why.
        let shown = conflict_of(err).map(|c| conflict_message(&c));
        emit_failed(app, transfer_id, "upload", &name, shown.as_deref().unwrap_or(err));
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn upload_file(
    app: &AppHandle,
    cli_path: &Path,
    path: &Path,
    folder_id: Option<u64>,
    chunk_size: u64,
    transfer_id: &str,
    existing: &ExistingFiles,
    known_sha256: Option<&str>,
    on_conflict: OnConflict,
) -> Result<UploadReceipt, String> {
    let meta = tokio::fs::metadata(path)
        .await
        .map_err(|e| format!("No se puede leer {}: {e}", path.display()))?;
    if !meta.is_file() {
        return Err(format!("{} no es un archivo", path.display()));
    }
    let size = meta.len();
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("Nombre de archivo inválido")?
        .to_string();

    if PREFER_CLI_SPLIT {
        if size > CLI_SPLIT_THRESHOLD {
            return upload_cli_split(
                app,
                cli_path,
                path,
                &name,
                size,
                folder_id,
                transfer_id,
                existing,
                known_sha256,
                on_conflict,
            )
            .await;
        }
    } else if size > chunk_size {
        return upload_chunked(
            app,
            cli_path,
            path,
            &name,
            size,
            folder_id,
            chunk_size,
            transfer_id,
            existing,
            known_sha256,
        )
        .await;
    }

    if already_uploaded(existing, &name, size) {
        emit(
            app,
            TransferProgress {
                id: transfer_id.into(),
                kind: "upload".into(),
                name: name.clone(),
                phase: "done".into(),
                current: 1,
                total: 1,
                bytes_done: size,
                bytes_total: size,
                message: "Ya estaba en O2; no se vuelve a subir".into(),
            },
        );
        return Ok(UploadReceipt {
            outcome: UploadOutcome::Skipped,
            sha256: None,
            remote_id: None,
            remote_name: name,
            size,
        });
    }

    emit(
        app,
        TransferProgress {
            id: transfer_id.into(),
            kind: "upload".into(),
            name: name.clone(),
            phase: "upload".into(),
            current: 0,
            total: 1,
            bytes_done: 0,
            bytes_total: size,
            message: format!("Subiendo {}", name),
        },
    );
    let uploaded = cli_upload_full(app, cli_path, path, folder_id, on_conflict, |sent, notice| {
        emit(
            app,
            TransferProgress {
                id: transfer_id.into(),
                kind: "upload".into(),
                name: name.clone(),
                phase: "upload".into(),
                current: 0,
                total: 1,
                bytes_done: sent.min(size),
                bytes_total: size,
                message: match notice {
                    Some(notice) => notice.to_string(),
                    // Once everything is sent, O2 still has to store it and answer.
                    None if sent >= size => "Enviado; O2 lo está procesando…".into(),
                    None => format!("Subiendo {name}"),
                },
            },
        );
    })
    .await?;
    let id = uploaded.single_id()?;
    let sha256 = match (known_sha256, uploaded.sha256) {
        (Some(hash), _) => Some(hash.to_string()),
        // Computed by the CLI as it sent the file: no second read.
        (None, Some(hash)) => Some(hash),
        (None, None) => {
            emit(
                app,
                TransferProgress {
                    id: transfer_id.into(),
                    kind: "upload".into(),
                    name: name.clone(),
                    phase: "hash".into(),
                    current: 0,
                    total: 1,
                    bytes_done: 0,
                    bytes_total: size,
                    message: "Calculando SHA-256…".into(),
                },
            );
            let mut last = Instant::now();
            sha256_file_with_progress(path, |done| {
                if done < size && last.elapsed() < Duration::from_millis(500) {
                    return;
                }
                last = Instant::now();
                emit(
                    app,
                    TransferProgress {
                        id: transfer_id.into(),
                        kind: "upload".into(),
                        name: name.clone(),
                        phase: "hash".into(),
                        current: 0,
                        total: 1,
                        bytes_done: done.min(size),
                        bytes_total: size,
                        message: "Calculando SHA-256…".into(),
                    },
                );
            })
            .await
            .ok()
        }
    };
    emit(
        app,
        TransferProgress {
            id: transfer_id.into(),
            kind: "upload".into(),
            name: name.clone(),
            phase: "done".into(),
            current: 1,
            total: 1,
            bytes_done: size,
            bytes_total: size,
            message: format!("Subido (id {id})"),
        },
    );
    Ok(UploadReceipt {
        outcome: UploadOutcome::Uploaded,
        sha256,
        remote_id: Some(id),
        remote_name: name,
        size,
    })
}

/// Upload a file above the CLI's split threshold with the CLI's own split,
/// then add `name.o2split.json` with the whole-file and per-part SHA-256.
/// This keeps the CLI as the only split/join implementation while preserving
/// the app's verifiable, content-addressed manifest.
#[allow(clippy::too_many_arguments)]
async fn upload_cli_split(
    app: &AppHandle,
    cli_path: &Path,
    path: &Path,
    name: &str,
    size: u64,
    folder_id: Option<u64>,
    transfer_id: &str,
    existing: &ExistingFiles,
    known_sha256: Option<&str>,
    on_conflict: OnConflict,
) -> Result<UploadReceipt, String> {
    let (part_size, pieces) = cli_split_layout(name, size)?;
    let total_parts = pieces.len() as u64;
    let manifest_name = format!("{name}.o2split.json");

    if existing.contains_key(&manifest_name) {
        let same = pieces
            .iter()
            .all(|(part_name, _, part_size)| already_uploaded(existing, part_name, *part_size));
        if !same {
            return Err(format!(
                "Ya hay un «{name}» troceado distinto en esta carpeta. Bórralo antes de volver a subirlo."
            ));
        }
        emit(
            app,
            TransferProgress {
                id: transfer_id.into(),
                kind: "upload".into(),
                name: name.to_string(),
                phase: "done".into(),
                current: total_parts,
                total: total_parts,
                bytes_done: size,
                bytes_total: size,
                message: "Ya estaba en O2; no se vuelve a subir".into(),
            },
        );
        return Ok(UploadReceipt {
            outcome: UploadOutcome::Skipped,
            sha256: None,
            remote_id: None,
            remote_name: manifest_name,
            size,
        });
    }

    emit(
        app,
        TransferProgress {
            id: transfer_id.into(),
            kind: "upload".into(),
            name: name.to_string(),
            phase: "upload".into(),
            current: 0,
            total: total_parts,
            bytes_done: 0,
            bytes_total: size,
            message: format!("Subiendo {name} (el CLI lo trocea)…"),
        },
    );
    let output = cli_upload_split(app, cli_path, path, folder_id, size, on_conflict, |sent, notice| {
        emit(
            app,
            TransferProgress {
                id: transfer_id.into(),
                kind: "upload".into(),
                name: name.to_string(),
                phase: "upload".into(),
                current: 0,
                total: total_parts,
                bytes_done: sent.min(size),
                bytes_total: size,
                message: match notice {
                    Some(notice) => notice.to_string(),
                    None if sent >= size => "Enviado; O2 lo está procesando…".into(),
                    None => format!("Subiendo {name} (el CLI lo trocea)…"),
                },
            },
        );
    })
    .await?;
    if output.parts.len() != pieces.len() {
        return Err(format!(
            "El CLI devolvió {} partes y la app esperaba {total_parts}; actualiza la app.",
            output.parts.len()
        ));
    }

    emit(
        app,
        TransferProgress {
            id: transfer_id.into(),
            kind: "upload".into(),
            name: name.to_string(),
            phase: "hash".into(),
            current: 0,
            total: total_parts,
            bytes_done: 0,
            bytes_total: size,
            message: "Calculando SHA-256…".into(),
        },
    );
    let mut last = Instant::now();
    let (sha256, part_hashes) = sha256_split_file(path, &pieces, |done| {
        if done < size && last.elapsed() < Duration::from_millis(500) {
            return;
        }
        last = Instant::now();
        emit(
            app,
            TransferProgress {
                id: transfer_id.into(),
                kind: "upload".into(),
                name: name.to_string(),
                phase: "hash".into(),
                current: 0,
                total: total_parts,
                bytes_done: done.min(size),
                bytes_total: size,
                message: "Calculando SHA-256…".into(),
            },
        );
    })
    .await?;
    if let Some(expected) = known_sha256
        && expected != sha256
    {
        return Err("El archivo ha cambiado desde que se comprobó si ya estaba en O2.".into());
    }
    if part_hashes.len() != pieces.len() {
        return Err("No se han podido calcular los hashes de todas las partes".into());
    }

    let manifest = CliSplitManifest {
        format: "o2cloud-cli-split".into(),
        version: 1,
        name: name.to_string(),
        size,
        sha256: sha256.clone(),
        part_size: Some(part_size),
        parts: pieces
            .iter()
            .zip(part_hashes)
            .map(|((part_name, _, part_size), hash)| ManifestPart {
                name: part_name.clone(),
                size: *part_size,
                sha256: Some(hash),
            })
            .collect(),
        created_at: None,
    };
    let temp_root = app_cache_dir(app)?.join("transfers").join(transfer_id);
    tokio::fs::create_dir_all(&temp_root)
        .await
        .map_err(|error| error.to_string())?;
    let manifest_path = temp_root.join(&manifest_name);
    let manifest_json = serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?;
    tokio::fs::write(&manifest_path, manifest_json)
        .await
        .map_err(|error| error.to_string())?;
    let modified = tokio::fs::metadata(path)
        .await
        .ok()
        .and_then(|meta| meta.modified().ok());
    set_modified(&manifest_path, modified);

    emit(
        app,
        TransferProgress {
            id: transfer_id.into(),
            kind: "upload".into(),
            name: name.to_string(),
            phase: "manifest".into(),
            current: total_parts,
            total: total_parts,
            bytes_done: size,
            bytes_total: size,
            message: "Subiendo manifiesto…".into(),
        },
    );
    let manifest_result = cli_upload(app, cli_path, &manifest_path, folder_id, |_, notice| {
        if let Some(notice) = notice {
            emit(
                app,
                TransferProgress {
                    id: transfer_id.into(),
                    kind: "upload".into(),
                    name: name.to_string(),
                    phase: "manifest".into(),
                    current: total_parts,
                    total: total_parts,
                    bytes_done: size,
                    bytes_total: size,
                    message: format!("Manifiesto: {notice}"),
                },
            );
        }
    })
    .await;
    let _ = tokio::fs::remove_dir_all(&temp_root).await;
    let manifest_id = manifest_result?;

    emit(
        app,
        TransferProgress {
            id: transfer_id.into(),
            kind: "upload".into(),
            name: name.to_string(),
            phase: "done".into(),
            current: total_parts,
            total: total_parts,
            bytes_done: size,
            bytes_total: size,
            message: format!("Subido en {total_parts} fragmentos (manifiesto incluido)"),
        },
    );
    Ok(UploadReceipt {
        outcome: UploadOutcome::Uploaded,
        sha256: Some(sha256),
        remote_id: Some(manifest_id),
        remote_name: manifest_name,
        size,
    })
}

/// Split, hash and upload a large file one part at a time, so it only needs
/// one part's worth of free disk, then upload the manifest. Parts already in
/// the folder with the right name and size (from an interrupted upload) are
/// hashed but not uploaded again.
#[allow(clippy::too_many_arguments)]
async fn upload_chunked(
    app: &AppHandle,
    cli_path: &Path,
    path: &Path,
    name: &str,
    total_size: u64,
    folder_id: Option<u64>,
    chunk_size: u64,
    transfer_id: &str,
    existing: &ExistingFiles,
    known_sha256: Option<&str>,
) -> Result<UploadReceipt, String> {
    let total_parts = total_size.div_ceil(chunk_size);
    let part_name_of = |index: u64| format!("{name}.o2part.{index:03}");
    let part_size_of = |index: u64| chunk_size.min(total_size - (index - 1) * chunk_size);
    let manifest_name = format!("{name}.o2manifest.json");

    if existing.contains_key(&manifest_name) {
        let same = (1..=total_parts)
            .all(|index| already_uploaded(existing, &part_name_of(index), part_size_of(index)));
        if !same {
            return Err(format!(
                "Ya hay un «{name}» troceado distinto en esta carpeta. Bórralo antes de volver a subirlo."
            ));
        }
        emit(
            app,
            TransferProgress {
                id: transfer_id.into(),
                kind: "upload".into(),
                name: name.to_string(),
                phase: "done".into(),
                current: total_parts,
                total: total_parts,
                bytes_done: total_size,
                bytes_total: total_size,
                message: "Ya estaba en O2; no se vuelve a subir".into(),
            },
        );
        return Ok(UploadReceipt {
            outcome: UploadOutcome::Skipped,
            sha256: None,
            remote_id: None,
            remote_name: manifest_name,
            size: total_size,
        });
    }

    let temp_root = app_cache_dir(app)?.join("transfers").join(transfer_id);
    tokio::fs::create_dir_all(&temp_root)
        .await
        .map_err(|e| e.to_string())?;

    let result: Result<UploadReceipt, String> = async {
        let mut src = tokio::fs::File::open(path)
            .await
            .map_err(|e| format!("No se puede abrir {}: {e}", path.display()))?;
        let modified = src.metadata().await.ok().and_then(|m| m.modified().ok());
        let mut whole = Sha256::new();
        let mut manifest_parts = Vec::new();
        let mut buffer = vec![0u8; READ_BUFFER];
        let mut bytes_done = 0u64;
        let mut resumed = 0u64;

        for index in 1..=total_parts {
            let part_name = part_name_of(index);
            let size = part_size_of(index);
            let already_there = already_uploaded(existing, &part_name, size);
            let part_path = temp_root.join(&part_name);

            emit(
                app,
                TransferProgress {
                    id: transfer_id.into(),
                    kind: "upload".into(),
                    name: name.to_string(),
                    phase: "split".into(),
                    current: index - 1,
                    total: total_parts,
                    bytes_done,
                    bytes_total: total_size,
                    message: format!("Preparando fragmento {index}/{total_parts}…"),
                },
            );

            let mut dst = if already_there {
                None
            } else {
                Some(
                    tokio::fs::File::create(&part_path)
                        .await
                        .map_err(|e| e.to_string())?,
                )
            };
            let mut part_hasher = Sha256::new();
            let mut remaining = size;
            let mut split_emit = Instant::now();
            while remaining > 0 {
                let want = remaining.min(READ_BUFFER as u64) as usize;
                let read = src
                    .read(&mut buffer[..want])
                    .await
                    .map_err(|e| e.to_string())?;
                if read == 0 {
                    return Err(format!(
                        "{} ha cambiado de tamaño durante la subida",
                        path.display()
                    ));
                }
                if let Some(dst) = dst.as_mut() {
                    dst.write_all(&buffer[..read])
                        .await
                        .map_err(|e| e.to_string())?;
                }
                whole.update(&buffer[..read]);
                part_hasher.update(&buffer[..read]);
                remaining -= read as u64;
                // Splitting a part reads gigabytes; say how far it is instead
                // of leaving the row unchanged until the part is written.
                if split_emit.elapsed() >= Duration::from_millis(500) {
                    split_emit = Instant::now();
                    let done = size - remaining;
                    let percent = done * 100 / size.max(1);
                    emit(
                        app,
                        TransferProgress {
                            id: transfer_id.into(),
                            kind: "upload".into(),
                            name: name.to_string(),
                            phase: "split".into(),
                            current: index - 1,
                            total: total_parts,
                            bytes_done,
                            bytes_total: total_size,
                            message: format!(
                                "Preparando fragmento {index}/{total_parts}… {percent}%"
                            ),
                        },
                    );
                }
            }
            manifest_parts.push(ManifestPart {
                name: part_name.clone(),
                size,
                sha256: Some(hex::encode(part_hasher.finalize())),
            });

            if let Some(mut dst) = dst {
                dst.flush().await.map_err(|e| e.to_string())?;
                drop(dst);
                set_modified(&part_path, modified);
                emit(
                    app,
                    TransferProgress {
                        id: transfer_id.into(),
                        kind: "upload".into(),
                        name: name.to_string(),
                        phase: "upload".into(),
                        current: index - 1,
                        total: total_parts,
                        bytes_done,
                        bytes_total: total_size,
                        message: format!("Subiendo fragmento {index}/{total_parts}"),
                    },
                );
                cli_upload(app, cli_path, &part_path, folder_id, |sent, notice| {
                    emit(
                        app,
                        TransferProgress {
                            id: transfer_id.into(),
                            kind: "upload".into(),
                            name: name.to_string(),
                            phase: "upload".into(),
                            current: index - 1,
                            total: total_parts,
                            bytes_done: bytes_done + sent.min(size),
                            bytes_total: total_size,
                            message: match notice {
                                Some(notice) => format!("Fragmento {index}/{total_parts}: {notice}"),
                                None if sent >= size => format!(
                                    "Fragmento {index}/{total_parts} enviado; O2 lo está procesando…"
                                ),
                                None => format!("Subiendo fragmento {index}/{total_parts}"),
                            },
                        },
                    );
                })
                .await?;
                let _ = tokio::fs::remove_file(&part_path).await;
            } else {
                resumed += 1;
            }

            bytes_done += size;
            emit(
                app,
                TransferProgress {
                    id: transfer_id.into(),
                    kind: "upload".into(),
                    name: name.to_string(),
                    phase: "upload".into(),
                    current: index,
                    total: total_parts,
                    bytes_done,
                    bytes_total: total_size,
                    message: if already_there {
                        format!("Fragmento {index}/{total_parts} ya estaba subido")
                    } else {
                        format!("Fragmento {index}/{total_parts} subido")
                    },
                },
            );
        }

        let computed = hex::encode(whole.finalize());
        let sha256 = match known_sha256 {
            Some(hash) if hash == computed => computed,
            Some(_) => {
                return Err(
                    "El archivo ha cambiado desde que se comprobó si ya estaba en O2.".into(),
                );
            }
            None => computed,
        };
        let manifest = ChunkManifest {
            format: "o2cloud-chunks".into(),
            version: 1,
            name: name.to_string(),
            size: total_size,
            sha256: sha256.clone(),
            chunk_size,
            parts: manifest_parts,
            created_at: None,
        };
        let manifest_path = temp_root.join(&manifest_name);
        let manifest_json = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
        tokio::fs::write(&manifest_path, manifest_json)
            .await
            .map_err(|e| e.to_string())?;
        set_modified(&manifest_path, modified);

        emit(
            app,
            TransferProgress {
                id: transfer_id.into(),
                kind: "upload".into(),
                name: name.to_string(),
                phase: "manifest".into(),
                current: total_parts,
                total: total_parts,
                bytes_done: total_size,
                bytes_total: total_size,
                message: "Subiendo manifiesto…".into(),
            },
        );

        let manifest_id = cli_upload(app, cli_path, &manifest_path, folder_id, |_, notice| {
            if let Some(notice) = notice {
                emit(
                    app,
                    TransferProgress {
                        id: transfer_id.into(),
                        kind: "upload".into(),
                        name: name.to_string(),
                        phase: "manifest".into(),
                        current: total_parts,
                        total: total_parts,
                        bytes_done: total_size,
                        bytes_total: total_size,
                        message: format!("Manifiesto: {notice}"),
                    },
                );
            }
        })
        .await?;

        emit(
            app,
            TransferProgress {
                id: transfer_id.into(),
                kind: "upload".into(),
                name: name.to_string(),
                phase: "done".into(),
                current: total_parts,
                total: total_parts,
                bytes_done: total_size,
                bytes_total: total_size,
                message: if resumed > 0 {
                    format!("Subido en {total_parts} fragmentos ({resumed} ya estaban)")
                } else {
                    format!("Subido en {total_parts} fragmentos")
                },
            },
        );
        Ok(UploadReceipt {
            outcome: UploadOutcome::Uploaded,
            sha256: Some(sha256),
            remote_id: Some(manifest_id),
            remote_name: manifest_name,
            size: total_size,
        })
    }
    .await;

    // Parts are deleted as they go up; this also removes what an error left.
    let _ = tokio::fs::remove_dir_all(&temp_root).await;
    result
}

/// Give a part or manifest the original's modification time. The CLI sends
/// a file's modification time to O2 as its date, so without this a chunked
/// file is dated when it was uploaded rather than like the original.
fn set_modified(path: &Path, time: Option<std::time::SystemTime>) {
    let Some(time) = time else { return };
    if let Ok(file) = std::fs::File::options().write(true).open(path) {
        let _ = file.set_modified(time);
    }
}

/// The name to save a download under. O2 allows characters and device names
/// that Windows rejects, so they are replaced there. On every platform a
/// path separator in the remote name is replaced too, so a bad name cannot
/// escape the chosen folder.
pub fn download_file_name(name: &str) -> String {
    let name = name.replace('\0', "_");
    if name.is_empty() || name == "." || name == ".." {
        return "_".into();
    }
    #[cfg(windows)]
    {
        windows_safe_file_name(&name)
    }
    #[cfg(not(windows))]
    {
        name.replace('/', "_")
    }
}

#[cfg(windows)]
fn windows_safe_file_name(name: &str) -> String {
    /// Leave room for `.o2download` and for the folder part of the path.
    const MAX_UTF16: usize = 180;

    let mut safe: String = name
        .chars()
        .map(|character| match character {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            character if (character as u32) < 0x20 => '_',
            character => character,
        })
        .collect();

    // Windows silently strips these, which can make the name collide with
    // another file or become a reserved device name.
    safe = safe
        .trim_end_matches(|character| character == ' ' || character == '.')
        .to_string();
    if safe.is_empty() || safe == "." || safe == ".." {
        safe = "_".into();
    }

    // CON, PRN, AUX, NUL, COM1-9 and LPT1-9 are reserved even with an
    // extension (`CON.txt`).
    let base = safe
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&base.as_str()) {
        safe.insert(0, '_');
    }

    // Windows counts a file name in UTF-16 units, with a 255 limit per
    // component. Truncate long names but keep the extension.
    if safe.encode_utf16().count() > MAX_UTF16 {
        let extension_at = safe
            .rfind('.')
            .filter(|at| *at > 0 && safe[*at..].encode_utf16().count() <= 32);
        let (stem, extension) = match extension_at {
            Some(at) => (&safe[..at], &safe[at..]),
            None => (safe.as_str(), ""),
        };
        let extension_units = extension.encode_utf16().count();
        let mut truncated = truncate_utf16(stem, MAX_UTF16 - extension_units);
        truncated.push_str(extension);
        truncated = truncated
            .trim_end_matches(|character| character == ' ' || character == '.')
            .to_string();
        safe = if truncated.is_empty() {
            "_".into()
        } else {
            truncated
        };
    }

    safe
}

#[cfg(windows)]
fn truncate_utf16(value: &str, max_units: usize) -> String {
    let mut out = String::new();
    let mut units = 0;
    for character in value.chars() {
        let width = character.len_utf16();
        if units + width > max_units {
            break;
        }
        out.push(character);
        units += width;
    }
    out
}

/// Where a download is written until it is complete and verified: next to
/// the destination, so putting it in place is a rename.
fn partial_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".o2download");
    dest.with_file_name(name)
}

/// Put a finished download in place. The save dialog has already asked
/// before replacing an existing file.
async fn move_into_place(partial: &Path, dest: &Path) -> Result<(), String> {
    // On Windows a rename does not replace an existing file.
    #[cfg(windows)]
    {
        if dest.exists() {
            tokio::fs::remove_file(dest)
                .await
                .map_err(|e| format!("No se puede reemplazar {}: {e}", dest.display()))?;
        }
    }
    tokio::fs::rename(partial, dest)
        .await
        .map_err(|e| format!("No se puede guardar {}: {e}", dest.display()))
}

fn part_id(candidates: &[PartRef], part: &ManifestPart) -> Option<u64> {
    candidates
        .iter()
        .find(|c| c.name == part.name && c.size == part.size)
        .map(|c| c.id)
}

/// Download a chunked file: fetch the manifest, then each part, appending
/// it to the output and checking its SHA-256 on the way, then the SHA-256
/// of the whole file. `known_parts` are the parts in the listing on screen.
#[allow(clippy::too_many_arguments)]
pub async fn download_chunked(
    app: &AppHandle,
    cli_path: &Path,
    folder_id: Option<u64>,
    manifest_id: u64,
    known_parts: &[PartRef],
    name: &str,
    dest: &Path,
    transfer_id: &str,
) -> Result<(), String> {
    emit(
        app,
        TransferProgress {
            id: transfer_id.into(),
            kind: "download".into(),
            name: name.to_string(),
            phase: "download".into(),
            current: 0,
            total: 0,
            bytes_done: 0,
            bytes_total: known_parts.iter().map(|p| p.size).sum(),
            message: "Preparando descarga…".into(),
        },
    );
    let temp_root = app_cache_dir(app)?.join("transfers").join(transfer_id);
    tokio::fs::create_dir_all(&temp_root)
        .await
        .map_err(|e| e.to_string())?;
    let partial = partial_path(dest);
    // Left over by an earlier attempt that was killed.
    let _ = tokio::fs::remove_file(&partial).await;

    let result: Result<ChunkManifest, String> = async {
        let manifest_path = temp_root.join("manifest.json");
        let target = manifest_id.to_string();
        cli_download(app, cli_path, &target, &manifest_path, |_, notice| {
            if let Some(notice) = notice {
                emit(
                    app,
                    TransferProgress {
                        id: transfer_id.into(),
                        kind: "download".into(),
                        name: name.to_string(),
                        phase: "download".into(),
                        current: 0,
                        total: 0,
                        bytes_done: 0,
                        bytes_total: known_parts.iter().map(|p| p.size).sum(),
                        message: format!("Manifiesto: {notice}"),
                    },
                );
            }
        })
        .await?;
        let raw = tokio::fs::read(&manifest_path)
            .await
            .map_err(|e| e.to_string())?;
        let manifest: ChunkManifest = serde_json::from_slice(&raw)
            .map_err(|e| format!("Manifiesto inválido: {e}"))?;

        // Only list the folder again if the listing on screen lacks a part.
        let mut candidates = known_parts.to_vec();
        if manifest
            .parts
            .iter()
            .any(|part| part_id(&candidates, part).is_none())
        {
            candidates = current_folder_files(app, cli_path, folder_id)
                .await?
                .into_iter()
                .map(|f| PartRef {
                    id: f.id,
                    name: f.name,
                    size: f.size.unwrap_or(0),
                })
                .collect();
        }

        let mut out = tokio::fs::File::create(&partial)
            .await
            .map_err(|e| format!("No se puede crear {}: {e}", partial.display()))?;
        let total_parts = manifest.parts.len() as u64;
        let mut whole = Sha256::new();
        let mut buffer = vec![0u8; READ_BUFFER];
        let mut bytes_done = 0u64;

        for (idx, part) in manifest.parts.iter().enumerate() {
            let id = part_id(&candidates, part)
                .ok_or_else(|| format!("Falta el fragmento {}", part.name))?;
            // Use an app-controlled name for the temporary part: the remote
            // name may contain characters Windows rejects, or `..`.
            let part_path = temp_root.join(format!("part-{:03}", idx + 1));

            // The CLI looks the part up in the account before data flows.
            emit(
                app,
                TransferProgress {
                    id: transfer_id.into(),
                    kind: "download".into(),
                    name: name.to_string(),
                    phase: "download".into(),
                    current: idx as u64,
                    total: total_parts,
                    bytes_done,
                    bytes_total: manifest.size,
                    message: format!("Preparando fragmento {}/{}…", idx + 1, total_parts),
                },
            );
            let target = id.to_string();
            cli_download(app, cli_path, &target, &part_path, |received, notice| {
                emit(
                    app,
                    TransferProgress {
                        id: transfer_id.into(),
                        kind: "download".into(),
                        name: name.to_string(),
                        phase: "download".into(),
                        current: idx as u64,
                        total: total_parts,
                        bytes_done: bytes_done + received.min(part.size),
                        bytes_total: manifest.size,
                        message: match notice {
                            Some(notice) => format!("Fragmento {}/{}: {notice}", idx + 1, total_parts),
                            None => format!("Descargando fragmento {}/{}", idx + 1, total_parts),
                        },
                    },
                );
            })
            .await?;

            let mut part_file = tokio::fs::File::open(&part_path)
                .await
                .map_err(|e| e.to_string())?;
            let mut part_hasher = Sha256::new();
            let mut part_len = 0u64;
            loop {
                let read = part_file
                    .read(&mut buffer)
                    .await
                    .map_err(|e| e.to_string())?;
                if read == 0 {
                    break;
                }
                out.write_all(&buffer[..read])
                    .await
                    .map_err(|e| e.to_string())?;
                part_hasher.update(&buffer[..read]);
                whole.update(&buffer[..read]);
                part_len += read as u64;
            }
            drop(part_file);
            let _ = tokio::fs::remove_file(&part_path).await;

            if part_len != part.size {
                return Err(format!(
                    "Tamaño inesperado en {}: {} (esperado {})",
                    part.name, part_len, part.size
                ));
            }
            if let Some(expected) = &part.sha256
                && &hex::encode(part_hasher.finalize()) != expected
            {
                return Err(format!("SHA-256 incorrecto en {}", part.name));
            }
            bytes_done += part.size;
        }
        out.flush().await.map_err(|e| e.to_string())?;
        drop(out);

        let actual = hex::encode(whole.finalize());
        if actual != manifest.sha256 {
            return Err(format!(
                "El SHA-256 del archivo reconstruido no coincide (esperado {}, obtenido {}); no se ha guardado.",
                manifest.sha256, actual
            ));
        }
        move_into_place(&partial, dest).await?;
        Ok(manifest)
    }
    .await;

    let _ = tokio::fs::remove_dir_all(&temp_root).await;
    let manifest = match result {
        Ok(manifest) => manifest,
        Err(err) => {
            let _ = tokio::fs::remove_file(&partial).await;
            return Err(err);
        }
    };

    let total_parts = manifest.parts.len() as u64;
    emit(
        app,
        TransferProgress {
            id: transfer_id.into(),
            kind: "download".into(),
            name: name.to_string(),
            phase: "done".into(),
            current: total_parts,
            total: total_parts,
            bytes_done: manifest.size,
            bytes_total: manifest.size,
            message: "Descarga y verificación completadas".into(),
        },
    );
    Ok(())
}

/// Download a normal file. `size` is its size in the listing, if known.
pub async fn download_normal(
    app: &AppHandle,
    cli_path: &Path,
    id: u64,
    size: Option<u64>,
    dest: &Path,
    transfer_id: &str,
    name: &str,
) -> Result<(), String> {
    let size = size.unwrap_or(0);
    // The CLI looks the file up in the account before data flows.
    emit(
        app,
        TransferProgress {
            id: transfer_id.into(),
            kind: "download".into(),
            name: name.to_string(),
            phase: "download".into(),
            current: 0,
            total: 1,
            bytes_done: 0,
            bytes_total: size,
            message: "Preparando descarga…".into(),
        },
    );
    let partial = partial_path(dest);
    // Left over by an earlier attempt that was killed.
    let _ = tokio::fs::remove_file(&partial).await;
    let target = id.to_string();
    let downloaded = cli_download(app, cli_path, &target, &partial, |received, notice| {
        emit(
            app,
            TransferProgress {
                id: transfer_id.into(),
                kind: "download".into(),
                name: name.to_string(),
                phase: "download".into(),
                current: 0,
                total: 1,
                bytes_done: if size > 0 {
                    received.min(size)
                } else {
                    received
                },
                bytes_total: size,
                message: match notice {
                    Some(notice) => notice.to_string(),
                    None => format!("Descargando {name}"),
                },
            },
        );
    })
    .await;
    let result = match downloaded {
        Ok(()) => move_into_place(&partial, dest).await,
        Err(err) => Err(err),
    };
    if result.is_err() {
        let _ = tokio::fs::remove_file(&partial).await;
    }
    result?;
    emit(
        app,
        TransferProgress {
            id: transfer_id.into(),
            kind: "download".into(),
            name: name.to_string(),
            phase: "done".into(),
            current: 1,
            total: 1,
            bytes_done: size,
            bytes_total: size,
            message: "Descarga completada".into(),
        },
    );
    Ok(())
}

/// The remote path of a file in `folder`, as `download` expects it.
pub fn join_remote_path(folder: &str, name: &str) -> String {
    let folder = folder.trim();
    if folder.is_empty() || folder == "/" {
        format!("/{name}")
    } else {
        format!("{}/{}", folder.trim_end_matches('/'), name)
    }
}

/// Download a file the CLI split (`name.001`, …) by asking the CLI to join it.
/// `target` is the base path (`/folder/name`). With a manifest, verify the
/// whole-file SHA-256 before saving.
#[allow(clippy::too_many_arguments)]
pub async fn download_cli_split(
    app: &AppHandle,
    cli_path: &Path,
    target: &str,
    manifest_id: Option<u64>,
    known_parts: &[PartRef],
    name: &str,
    size: Option<u64>,
    dest: &Path,
    transfer_id: &str,
) -> Result<(), String> {
    let mut total = size.unwrap_or_else(|| known_parts.iter().map(|part| part.size).sum());
    let total_parts = known_parts.len().max(1) as u64;
    emit(
        app,
        TransferProgress {
            id: transfer_id.into(),
            kind: "download".into(),
            name: name.to_string(),
            phase: "download".into(),
            current: 0,
            total: total_parts,
            bytes_done: 0,
            bytes_total: total,
            message: "Preparando descarga…".into(),
        },
    );

    let temp_root = app_cache_dir(app)?.join("transfers").join(transfer_id);
    let partial = partial_path(dest);
    let _ = tokio::fs::remove_file(&partial).await;
    let mut verified = false;

    let result: Result<(), String> = async {
        let mut expected_sha = None;
        if let Some(manifest_id) = manifest_id {
            tokio::fs::create_dir_all(&temp_root)
                .await
                .map_err(|error| error.to_string())?;
            let manifest_path = temp_root.join("manifest.json");
            let manifest_target = manifest_id.to_string();
            cli_download(app, cli_path, &manifest_target, &manifest_path, |_, notice| {
                if let Some(notice) = notice {
                    emit(
                        app,
                        TransferProgress {
                            id: transfer_id.into(),
                            kind: "download".into(),
                            name: name.to_string(),
                            phase: "download".into(),
                            current: 0,
                            total: total_parts,
                            bytes_done: 0,
                            bytes_total: total,
                            message: format!("Manifiesto: {notice}"),
                        },
                    );
                }
            })
            .await?;
            let raw = tokio::fs::read(&manifest_path)
                .await
                .map_err(|error| error.to_string())?;
            let manifest: CliSplitManifest = serde_json::from_slice(&raw)
                .map_err(|error| format!("Manifiesto de split inválido: {error}"))?;
            if manifest.format != "o2cloud-cli-split" {
                return Err(format!(
                    "Manifiesto de split con formato desconocido: {}",
                    manifest.format
                ));
            }
            if manifest.name != name {
                return Err(format!(
                    "El manifiesto es de '{}', no de '{name}'",
                    manifest.name
                ));
            }
            if total > 0 && manifest.size != total {
                return Err(format!(
                    "El manifiesto dice {} bytes y la lista {total}",
                    manifest.size
                ));
            }
            if manifest.sha256.trim().is_empty() {
                return Err("El manifiesto de split no tiene SHA-256".into());
            }
            total = manifest.size;
            expected_sha = Some(manifest.sha256.clone());
        }

        cli_download(app, cli_path, target, &partial, |received, notice| {
            emit(
                app,
                TransferProgress {
                    id: transfer_id.into(),
                    kind: "download".into(),
                    name: name.to_string(),
                    phase: "download".into(),
                    current: 0,
                    total: total_parts,
                    bytes_done: if total > 0 {
                        received.min(total)
                    } else {
                        received
                    },
                    bytes_total: total,
                    message: match notice {
                        Some(notice) => notice.to_string(),
                        None => format!("Descargando {name} (el CLI une las partes)…"),
                    },
                },
            );
        })
        .await?;

        let downloaded = tokio::fs::metadata(&partial)
            .await
            .map_err(|error| error.to_string())?
            .len();
        if total > 0 && downloaded != total {
            return Err(format!(
                "Tamaño inesperado: {downloaded} bytes (esperado {total})"
            ));
        }

        if let Some(expected) = expected_sha {
            let mut last = Instant::now();
            emit(
                app,
                TransferProgress {
                    id: transfer_id.into(),
                    kind: "download".into(),
                    name: name.to_string(),
                    phase: "hash".into(),
                    current: total_parts,
                    total: total_parts,
                    bytes_done: 0,
                    bytes_total: total,
                    message: "Verificando SHA-256…".into(),
                },
            );
            let actual = sha256_file_with_progress(&partial, |done| {
                if done < total && last.elapsed() < Duration::from_millis(500) {
                    return;
                }
                last = Instant::now();
                emit(
                    app,
                    TransferProgress {
                        id: transfer_id.into(),
                        kind: "download".into(),
                        name: name.to_string(),
                        phase: "hash".into(),
                        current: total_parts,
                        total: total_parts,
                        bytes_done: done.min(total),
                        bytes_total: total,
                        message: "Verificando SHA-256…".into(),
                    },
                );
            })
            .await?;
            if actual != expected {
                return Err(format!(
                    "El SHA-256 del archivo unido no coincide (esperado {expected}, obtenido {actual}); no se ha guardado."
                ));
            }
            verified = true;
        }

        Ok(())
    }
    .await;

    let _ = tokio::fs::remove_dir_all(&temp_root).await;
    let result = match result {
        Ok(()) => move_into_place(&partial, dest).await,
        Err(error) => {
            let _ = tokio::fs::remove_file(&partial).await;
            Err(error)
        }
    };
    result?;

    emit(
        app,
        TransferProgress {
            id: transfer_id.into(),
            kind: "download".into(),
            name: name.to_string(),
            phase: "done".into(),
            current: total_parts,
            total: total_parts,
            bytes_done: total,
            bytes_total: total,
            message: if verified {
                "Descarga y verificación completadas".into()
            } else {
                "Descarga completada".into()
            },
        },
    );
    Ok(())
}

pub fn app_cache_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_cache_dir()
        .map_err(|e| e.to_string())?
        .join("o2cloud");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(stdout: &str, stderr: &str) -> cli::CliOutput {
        cli::CliOutput {
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    #[test]
    fn reads_upload_events() {
        let start = r#"{"event":"start","command":"upload","files":1,"bytes":4,"schema":1}"#;
        let summary = r#"{"event":"summary","files":1}"#;
        // Uploaded, with the CLI's SHA-256.
        let done = format!(
            "{start}\n{{\"event\":\"progress\",\"name\":\"a.txt\",\"bytes\":4,\"size\":4}}\n\
             {{\"event\":\"file\",\"name\":\"a.txt\",\"action\":\"uploaded\",\"id\":7,\"ids\":[7],\"sha256\":\"abc\"}}\n{summary}\n"
        );
        let out = upload_result(0, &output(&done, "")).unwrap();
        assert_eq!((out.id, out.sha256.as_deref()), (Some(7), Some("abc")));
        // Already there: the cloud's ID.
        let skipped = format!(
            "{start}\n{{\"event\":\"file\",\"name\":\"a.txt\",\"action\":\"skipped\",\"id\":8}}\n{summary}\n"
        );
        assert_eq!(upload_result(0, &output(&skipped, "")).unwrap().id, Some(8));
        // Another version's parts when resuming: never taken for ours.
        let differs = format!(
            "{start}\n{{\"event\":\"file\",\"name\":\"big.iso\",\"action\":\"differs\",\"parts\":[1,2]}}\n{summary}\n"
        );
        let err = upload_result(0, &output(&differs, "")).unwrap_err();
        assert!(err.contains("otra versión de big.iso"), "{err}");
        // A conflict: exit 5, then asked about.
        let conflict = format!(
            "{start}\n{{\"event\":\"file\",\"name\":\"a.txt\",\"action\":\"conflict\",\"size\":8,\"cloud\":{{\"size\":4,\"id\":7}}}}\n{summary}\n\
             {{\"event\":\"error\",\"code\":\"conflict\",\"message\":\"1 file(s)…\",\"exit_code\":5}}\n"
        );
        let err = upload_result(cli::EXIT_CONFLICT, &output(&conflict, "")).unwrap_err();
        let c = conflict_of(&err).expect("a conflict");
        assert_eq!((c["size"].as_u64(), c["cloud"]["size"].as_u64()), (Some(8), Some(4)));
        assert!(conflict_message(&c).contains("8 B aquí"), "{}", conflict_message(&c));
        // The session: the app's own message, which opens its login.
        let expired = r#"{"event":"error","code":"session_expired","message":"Session expired: …","exit_code":4}"#;
        assert_eq!(
            upload_result(4, &output(expired, "")).unwrap_err(),
            cli::SESSION_EXPIRED
        );
        // A network failure keeps the CLI's words, which retries look for.
        let network = r#"{"event":"error","code":"network","message":"HTTP request failed: timed out [run 1]","exit_code":7}"#;
        let err = upload_result(7, &output(network, "")).unwrap_err();
        assert!(worth_retrying(&err), "{err}");
        // No events at all (a crash): stderr's error.
        let err = upload_result(1, &output("", "→ Uploading a\nError: boom")).unwrap_err();
        assert_eq!(err, "boom");
    }

    #[test]
    fn explains_a_wrong_extension_in_spanish() {
        // CLI 1.4+: a HEIC photo named .jpg, rejected by O2.
        let cli = "API error: Upload rejected: MED-1050: Invalid media — it's a HEIC image \
                   named .jpg: `o2cloud fix <file> --apply` renames it to .heic";
        let event = serde_json::json!({
            "event": "error", "code": "api", "message": cli, "exit_code": 1
        });
        let err = upload_result(1, &output(&event.to_string(), "")).unwrap_err();
        let (first, rest) = err.split_once('\n').expect("two lines");
        assert_eq!(
            first,
            "O2 no lo acepta: es una imagen HEIC, aunque su nombre acabe en .jpg. \
             Cambia la extensión del archivo a .heic y vuelve a subirlo."
        );
        // The CLI's words stay, on hover.
        assert_eq!(rest, cli);
        // Also from stderr, when there are no events.
        let err = upload_result(1, &output("", &format!("Error: {cli}"))).unwrap_err();
        assert!(
            err.starts_with("O2 no lo acepta: es una imagen HEIC"),
            "{err}"
        );

        // Videos, audio and an "an".
        let first_line = |m: &str| explain_rejection(m).lines().next().unwrap().to_string();
        assert!(
            first_line(
                "API error: Upload rejected: MED-1000: x — it's a QuickTime (MOV) video named .mp4: \
                 `o2cloud fix <file> --apply` renames it to .mov"
            )
            .starts_with("O2 no lo acepta: es un vídeo QuickTime (MOV), aunque su nombre acabe en .mp4.")
        );
        assert!(
            first_line(
                "API error: Upload rejected: MED-1050: x — it's an M4A audio file named .mp3: \
                 `o2cloud fix <file> --apply` renames it to .m4a"
            )
            .starts_with("O2 no lo acepta: es un audio M4A, aunque su nombre acabe en .mp3.")
        );

        // Anything else is left as it is.
        for other in [
            "API error: Upload rejected: MED-1020: File size exceeds configured max file size limit",
            "API error: Upload rejected: MED-1000: x — its colour profile is damaged, which O2 Cloud can't read",
            "HTTP request failed: timed out [run 1]",
        ] {
            assert_eq!(explain_rejection(other), other);
        }
    }

    #[test]
    fn builds_upload_arguments() {
        let p = Path::new("/tmp/a.txt");
        assert_eq!(
            upload_args(p, Some(5), OnConflict::Ask),
            ["upload", "/tmp/a.txt", "--json-lines", "--folder", "5"]
        );
        assert_eq!(
            upload_args(p, None, OnConflict::Overwrite),
            ["upload", "/tmp/a.txt", "--json-lines", "--on-conflict", "overwrite"]
        );
        assert!(upload_args(p, None, OnConflict::Skip).contains(&"skip".to_string()));
    }

    /// The real CLI against a real account, with the app's own arguments
    /// and parsing. Run with `O2CLOUD_LIVE_CLI=<path to o2cloud 1.x>
    /// cargo test live_cli -- --ignored --nocapture`. It works in a folder
    /// `/o2app-live-test` it creates and deletes.
    #[test]
    #[ignore]
    fn live_cli() {
        let Ok(cli) = std::env::var("O2CLOUD_LIVE_CLI") else {
            panic!("set O2CLOUD_LIVE_CLI to the o2cloud binary");
        };
        let run = |args: &[String], env: &[(&str, &str)]| {
            let out = std::process::Command::new(&cli)
                .args(args)
                .envs(env.iter().copied())
                .output()
                .unwrap();
            (
                out.status.code().unwrap_or(-1),
                output(
                    &String::from_utf8_lossy(&out.stdout),
                    &String::from_utf8_lossy(&out.stderr),
                ),
            )
        };
        let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        let (code, out) = run(&s(&["mkdir", "-p", "/o2app-live-test", "--json"]), &[]);
        assert_eq!(code, 0, "{}", out.stderr);
        let folder: serde_json::Value = cli::parse_json(&out).unwrap();
        let folder = folder["id"].as_u64();
        let dir = std::env::temp_dir().join(format!("o2app-live-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        let wait = || std::thread::sleep(Duration::from_secs(15));

        // A new file: uploaded, with the CLI's SHA-256 of what it sent.
        std::fs::write(&file, b"hola").unwrap();
        let (code, out) = run(&upload_args(&file, folder, OnConflict::Ask), &[]);
        let up = upload_result(code, &out).unwrap();
        println!("uploaded: {:?}", up);
        assert_eq!(up.action.as_deref(), Some("uploaded"));
        assert_eq!(up.sha256.as_deref(), Some(hex::encode(Sha256::digest(b"hola")).as_str()));
        let first_id = up.id.unwrap();
        wait();

        // The same again: skipped, with the cloud's ID.
        let (code, out) = run(&upload_args(&file, folder, OnConflict::Ask), &[]);
        let up = upload_result(code, &out).unwrap();
        println!("again: {:?}", up);
        assert_eq!((up.action.as_deref(), up.id), (Some("skipped"), Some(first_id)));

        // Another size: a conflict for the user, then overwritten.
        std::fs::write(&file, b"hola, otra version").unwrap();
        let (code, out) = run(&upload_args(&file, folder, OnConflict::Ask), &[]);
        assert_eq!(code, cli::EXIT_CONFLICT);
        let conflict = conflict_of(&upload_result(code, &out).unwrap_err()).unwrap();
        println!("conflict: {}", conflict_message(&conflict));
        assert_eq!(conflict["cloud"]["size"].as_u64(), Some(4));
        let (code, out) = run(&upload_args(&file, folder, OnConflict::Overwrite), &[]);
        let up = upload_result(code, &out).unwrap();
        println!("overwritten: {:?}", up);
        assert_eq!(up.action.as_deref(), Some("updated"));

        // A file in parts (100 B parts for the test): resuming against
        // another version's parts is an error, not their IDs.
        let big = dir.join("big.bin");
        std::fs::write(&big, vec![1u8; 250]).unwrap();
        let parts = [("O2CLOUD_PART_SIZE", "100")];
        let (code, out) = run(&upload_args(&big, folder, OnConflict::Skip), &parts);
        let up = upload_result(code, &out).unwrap();
        println!("in parts: {:?}", up);
        assert_eq!(up.parts.len(), 3);
        wait();
        std::fs::write(&big, vec![2u8; 260]).unwrap();
        let (code, out) = run(&upload_args(&big, folder, OnConflict::Skip), &parts);
        let err = upload_result(code, &out).unwrap_err();
        println!("other version's parts: {err}");
        assert!(err.contains("otra versión"), "{err}");

        // Progress events, from a bigger file.
        let large = dir.join("large.bin");
        std::fs::write(&large, vec![3u8; 40 * 1024 * 1024]).unwrap();
        let (code, out) = run(&upload_args(&large, folder, OnConflict::Ask), &[]);
        assert_eq!(code, 0, "{}", out.stderr);
        let progress: Vec<u64> = out.stdout.lines().filter_map(cli::progress_bytes_for_tests).collect();
        println!("progress events: {}", progress.len());
        assert!(!progress.is_empty(), "no progress events");
        assert!(progress.windows(2).all(|w| w[0] <= w[1]));
        assert!(*progress.last().unwrap() <= 40 * 1024 * 1024);

        let _ = run(&s(&["rm", "-r", "-y", "/o2app-live-test"]), &[]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn file(id: u64, name: &str, size: u64) -> RemoteFile {
        RemoteFile {
            id,
            name: name.to_string(),
            size: Some(size),
            date: None,
            path: None,
        }
    }

    fn file_at(id: u64, name: &str, size: u64, path: &str) -> RemoteFile {
        RemoteFile {
            path: Some(path.to_string()),
            ..file(id, name, size)
        }
    }

    #[test]
    fn large_files_skip_the_preflight_hash_unless_a_same_size_file_exists() {
        assert!(should_hash_before_upload(0, false));
        assert!(should_hash_before_upload(PREFLIGHT_HASH_MAX_BYTES, false));
        assert!(!should_hash_before_upload(
            PREFLIGHT_HASH_MAX_BYTES + 1,
            false
        ));
        assert!(should_hash_before_upload(
            PREFLIGHT_HASH_MAX_BYTES + 1,
            true
        ));
    }

    #[tokio::test]
    async fn hashing_reports_the_bytes_read() {
        let path = std::env::temp_dir().join(format!("o2-hash-progress-{}", std::process::id()));
        let data = vec![7u8; 3 * 1024 * 1024 + 123];
        std::fs::write(&path, &data).unwrap();

        let mut seen = Vec::new();
        let hash = sha256_file_with_progress(&path, |done| seen.push(done))
            .await
            .unwrap();

        assert_eq!(seen.last().copied(), Some(data.len() as u64));
        assert!(seen.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(hash, sha256_file(&path).await.unwrap());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cli_split_layout_matches_the_cli_parts() {
        let (part, pieces) = cli_split_layout_with("movie.mkv", 25_000, 10_000).unwrap();
        assert_eq!(part, 10_000);
        assert_eq!(
            pieces,
            vec![
                ("movie.mkv.001".into(), 0, 10_000),
                ("movie.mkv.002".into(), 10_000, 10_000),
                ("movie.mkv.003".into(), 20_000, 5_000),
            ]
        );

        // An exact multiple shrinks the part by one, so the last part is
        // always smaller (the CLI uses this to detect a missing tail).
        let (part, pieces) = cli_split_layout_with("movie.mkv", 30_000, 10_000).unwrap();
        assert_eq!(part, 9_999);
        assert_eq!(pieces.len(), 4);
        assert_eq!(pieces.last().unwrap().2, 3);

        let (part, pieces) = cli_split_layout("movie.mkv", CLI_PART_SIZE + 1).unwrap();
        assert_eq!(part, CLI_PART_SIZE);
        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces[1].2, 1);

        assert!(cli_split_layout_with("x", 2_001, 2).is_err());
    }

    #[tokio::test]
    async fn hashes_the_whole_file_and_each_cli_part_in_one_pass() {
        let path = std::env::temp_dir().join(format!("o2-split-hash-{}", std::process::id()));
        let data = vec![9u8; 25_000];
        std::fs::write(&path, &data).unwrap();
        let (_, pieces) = cli_split_layout_with("movie.mkv", data.len() as u64, 10_000).unwrap();

        let mut seen = Vec::new();
        let (whole, parts) = sha256_split_file(&path, &pieces, |done| seen.push(done))
            .await
            .unwrap();

        assert_eq!(parts.len(), 3);
        assert_eq!(whole, hex::encode(Sha256::digest(&data)));
        assert_eq!(parts[0], hex::encode(Sha256::digest(&data[..10_000])));
        assert_eq!(parts[1], hex::encode(Sha256::digest(&data[10_000..20_000])));
        assert_eq!(parts[2], hex::encode(Sha256::digest(&data[20_000..])));
        assert_eq!(seen.last().copied(), Some(data.len() as u64));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn logical_file_keys_cover_completed_chunks_but_not_incomplete_ones() {
        let existing = logical_files_existing(&[
            file(1, "movie.mkv.o2part.001", 100),
            file(2, "movie.mkv.o2part.002", 50),
            file(3, "movie.mkv.o2manifest.json", 200),
            file(4, "broken.mkv.o2part.001", 10),
            file(5, "notes.txt", 10),
        ]);
        assert_eq!(existing.get("movie.mkv"), Some(&vec![150]));
        assert_eq!(existing.get("notes.txt"), Some(&vec![10]));
        assert!(!existing.contains_key("broken.mkv"));
    }

    #[test]
    fn groups_chunks_and_manifest_into_one_logical_file() {
        let files = vec![
            file(1, "movie.mkv.o2part.001", 100),
            file(2, "movie.mkv.o2part.002", 50),
            file(3, "movie.mkv.o2manifest.json", 200),
            file(4, "notes.txt", 10),
        ];
        let (logical, warnings) = group_logical_files(files);
        assert!(warnings.is_empty());
        assert_eq!(logical.len(), 2);

        let movie = logical.iter().find(|f| f.name == "movie.mkv").unwrap();
        assert!(movie.chunked);
        assert_eq!(movie.size, 150);
        assert_eq!(movie.manifest_id, Some(3));
        assert_eq!(movie.parts.len(), 2);
        assert!(!movie.incomplete);

        let notes = logical.iter().find(|f| f.name == "notes.txt").unwrap();
        assert!(!notes.chunked);
        assert_eq!(notes.normal_id, Some(4));
    }

    #[test]
    fn marks_chunks_without_manifest_as_incomplete() {
        let files = vec![file(1, "movie.mkv.o2part.001", 100)];
        let (logical, warnings) = group_logical_files(files);
        assert_eq!(warnings.len(), 1);
        assert_eq!(logical.len(), 1);
        assert!(logical[0].chunked);
        assert!(!logical[0].cli_split);
        assert!(logical[0].incomplete);
        assert_eq!(logical[0].manifest_id, None);
    }

    #[test]
    fn groups_cli_split_parts_with_their_manifest() {
        let files = vec![
            file(1, "movie.mkv.001", 100),
            file(2, "movie.mkv.002", 50),
            file(3, "movie.mkv.o2split.json", 200),
            file(4, "notes.txt", 10),
        ];
        let (logical, warnings) = group_logical_files(files);
        assert!(warnings.is_empty());
        assert_eq!(logical.len(), 2);

        let movie = logical.iter().find(|f| f.name == "movie.mkv").unwrap();
        assert!(movie.chunked);
        assert!(movie.cli_split);
        assert_eq!(movie.size, 150);
        assert_eq!(movie.manifest_id, Some(3));
        assert_eq!(movie.parts.len(), 2);
        assert!(!movie.incomplete);

        let notes = logical.iter().find(|f| f.name == "notes.txt").unwrap();
        assert!(!notes.chunked);
        assert!(!notes.cli_split);
    }

    #[test]
    fn groups_large_cli_parts_without_a_manifest() {
        let files = vec![
            file(1, "movie.mkv.001", CLI_SPLIT_MIN_FIRST_PART),
            file(2, "movie.mkv.002", 100),
        ];
        let (logical, warnings) = group_logical_files(files);
        assert_eq!(warnings.len(), 1);
        assert_eq!(logical.len(), 1);
        assert!(logical[0].cli_split);
        assert_eq!(logical[0].manifest_id, None);
        assert!(!logical[0].incomplete);
    }

    #[test]
    fn groups_old_and_cli_splits_side_by_side() {
        let files = vec![
            file(1, "old.mkv.o2part.001", 100),
            file(2, "old.mkv.o2part.002", 50),
            file(3, "old.mkv.o2manifest.json", 200),
            file(4, "new.mkv.001", 100),
            file(5, "new.mkv.002", 50),
            file(6, "new.mkv.o2split.json", 200),
            file(7, "notes.txt", 10),
        ];
        let (logical, warnings) = group_logical_files(files);
        assert!(warnings.is_empty());
        assert_eq!(logical.len(), 3);

        let old = logical.iter().find(|file| file.name == "old.mkv").unwrap();
        assert!(old.chunked);
        assert!(!old.cli_split);
        let new = logical.iter().find(|file| file.name == "new.mkv").unwrap();
        assert!(new.chunked);
        assert!(new.cli_split);
    }

    #[test]
    fn does_not_group_small_numbered_files_without_a_manifest() {
        let files = vec![file(1, "photo.001", 100), file(2, "photo.002", 50)];
        let (logical, warnings) = group_logical_files(files);
        assert!(warnings.is_empty());
        assert_eq!(logical.len(), 2);
        assert!(logical.iter().all(|file| !file.cli_split));
    }

    #[test]
    fn does_not_group_cli_parts_when_the_base_file_exists() {
        let files = vec![
            file(1, "movie.mkv", 10),
            file(2, "movie.mkv.001", CLI_SPLIT_MIN_FIRST_PART),
            file(3, "movie.mkv.002", 100),
        ];
        let (logical, warnings) = group_logical_files(files);
        assert_eq!(warnings.len(), 1);
        assert_eq!(logical.len(), 3);
        assert!(logical.iter().all(|file| !file.cli_split));
    }

    #[test]
    fn does_not_group_cli_parts_across_folders() {
        let files = vec![
            file_at(1, "movie.001", CLI_SPLIT_MIN_FIRST_PART, "/A/movie.001"),
            file_at(2, "movie.002", 100, "/B/movie.002"),
        ];
        let (logical, warnings) = group_logical_files(files);
        assert!(warnings.is_empty());
        assert_eq!(logical.len(), 2);
        assert!(logical.iter().all(|file| !file.cli_split));
    }

    #[test]
    fn recognises_a_cli_split_as_existing_for_photos_and_library() {
        let existing = logical_files_existing(&[
            file(1, "movie.mkv.001", 100),
            file(2, "movie.mkv.002", 50),
            file(3, "movie.mkv.o2split.json", 200),
        ]);
        assert_eq!(existing.get("movie.mkv"), Some(&vec![150]));
    }

    #[test]
    fn reads_cli_part_names_without_matching_the_app_format() {
        assert_eq!(
            cli_part_name("movie.mkv.001"),
            Some(("movie.mkv".into(), 1))
        );
        assert_eq!(
            cli_part_name("movie.mkv.999"),
            Some(("movie.mkv".into(), 999))
        );
        assert_eq!(cli_part_name("movie.mkv.o2part.001"), None);
        assert_eq!(cli_part_name("movie.mkv.0001"), None);
        assert_eq!(cli_part_name("movie.mkv.001.txt"), None);
        assert_eq!(cli_part_name("movie.mkv.000"), None);
    }

    #[test]
    fn joins_remote_folder_and_file_names() {
        assert_eq!(join_remote_path("/", "movie.mkv"), "/movie.mkv");
        assert_eq!(
            join_remote_path("/Vídeos", "movie.mkv"),
            "/Vídeos/movie.mkv"
        );
        assert_eq!(
            join_remote_path("/Vídeos/", "movie.mkv"),
            "/Vídeos/movie.mkv"
        );
        assert_eq!(join_remote_path("", "movie.mkv"), "/movie.mkv");
    }

    #[test]
    fn reads_the_upload_receipt() {
        let output = cli::CliOutput {
            stdout: "{\"id\": 1544124548, \"name\": \"movie.mkv\", \"folderid\": 12, \"size\": 1048576, \"skipped\": false}\n".into(),
            stderr: "→ Uploading movie.mkv (1.0 MB)...\n✓ Uploaded successfully (id: 1544124548)\n".into(),
        };
        let uploaded: UploadOutput = cli::parse_json(&output).unwrap();
        assert_eq!(uploaded.single_id().unwrap(), 1544124548);
    }

    #[test]
    fn reports_a_cli_split_upload_instead_of_a_missing_id() {
        // 0.9.0 splits files over its 4 GB limit and returns `parts` instead
        // of `id`. The app chunks below that limit, so this should not
        // happen; if it does, say why instead of failing on `id`.
        let output = cli::CliOutput {
            stdout: "{\"name\": \"movie.mkv\", \"folderid\": 12, \"size\": 5000000000, \"skipped\": false, \"parts\": [1544124548, 1544124549]}\n".into(),
            stderr: String::new(),
        };
        let uploaded: UploadOutput = cli::parse_json(&output).unwrap();
        let err = uploaded.single_id().unwrap_err();
        assert!(err.contains("2 partes"), "{err}");
    }

    #[test]
    fn recognises_files_already_uploaded_by_name_and_size() {
        let existing = existing_files(&[
            file(1, "a.jpg", 10),
            file(2, "a.jpg", 20),
            RemoteFile {
                id: 3,
                name: "unknown-size.jpg".into(),
                size: None,
                date: None,
                path: None,
            },
        ]);
        assert!(already_uploaded(&existing, "a.jpg", 10));
        assert!(already_uploaded(&existing, "a.jpg", 20));
        assert!(!already_uploaded(&existing, "a.jpg", 30));
        assert!(!already_uploaded(&existing, "b.jpg", 10));
        assert!(!already_uploaded(&existing, "unknown-size.jpg", 0));
    }

    #[test]
    fn finds_parts_by_name_and_size() {
        let candidates = vec![
            PartRef {
                id: 1,
                name: "m.o2part.001".into(),
                size: 99,
            },
            PartRef {
                id: 2,
                name: "m.o2part.001".into(),
                size: 100,
            },
        ];
        let part = ManifestPart {
            name: "m.o2part.001".into(),
            size: 100,
            sha256: None,
        };
        assert_eq!(part_id(&candidates, &part), Some(2));
    }

    #[test]
    fn parts_take_the_original_modification_time() {
        let path = std::env::temp_dir().join(format!("o2-mtime-{}", std::process::id()));
        std::fs::write(&path, b"part").unwrap();
        let original = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_662_150_659);
        set_modified(&path, Some(original));
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            original
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn downloads_go_to_a_partial_file_next_to_the_destination() {
        assert_eq!(
            partial_path(Path::new("/tmp/x/movie.mkv")),
            PathBuf::from("/tmp/x/movie.mkv.o2download")
        );
    }

    #[test]
    fn download_names_never_escape_the_destination_folder() {
        assert_eq!(download_file_name("../escape.txt"), ".._escape.txt");
        assert_eq!(download_file_name("a/b.txt"), "a_b.txt");
        assert_eq!(download_file_name(".."), "_");
        assert_eq!(download_file_name(""), "_");
    }

    #[cfg(windows)]
    #[test]
    fn windows_download_names_replace_reserved_characters_and_names() {
        assert_eq!(download_file_name("video: final?.mp4"), "video_ final_.mp4");
        assert_eq!(download_file_name("CON.txt"), "_CON.txt");
        assert_eq!(download_file_name("lpt9.tar.gz"), "_lpt9.tar.gz");
        assert_eq!(download_file_name("name. "), "name");

        let long = format!("{}.mp4", "a".repeat(300));
        let safe = download_file_name(&long);
        assert!(safe.encode_utf16().count() <= 180);
        assert!(safe.ends_with(".mp4"));
    }

    #[tokio::test]
    async fn move_into_place_replaces_an_existing_file() {
        let dir = std::env::temp_dir().join(format!("o2-move-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("file.bin");
        let partial = partial_path(&dest);
        std::fs::write(&dest, b"old").unwrap();
        std::fs::write(&partial, b"new").unwrap();

        move_into_place(&partial, &dest).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"new");
        assert!(!partial.exists());

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn retries_network_failures_busy_servers_and_stalls_only() {
        assert!(worth_retrying(
            "HTTP request failed: error sending request for url (https://upload.cloud.o2online.es/sapi/upload)"
        ));
        assert!(worth_retrying(
            "API error: Upload failed (status 502 Bad Gateway): <html>"
        ));
        assert!(worth_retrying(
            "API error: Download failed (status 429 Too Many Requests): "
        ));
        assert!(worth_retrying(&format!("{STALLED}: 3 min sin avanzar")));
        assert!(worth_retrying(
            "API error: Download incomplete: got 1048576 of 2097152 bytes"
        ));
        assert!(worth_retrying(
            "Not found: Media 1544124548 has no download URL yet (the server may still be processing it; try again in a moment)"
        ));

        assert!(!worth_retrying(
            "API error: Upload rejected: MED-1020: File size exceeds configured max file size limit"
        ));
        assert!(!worth_retrying(
            "API error: Download failed (status 404 Not Found): "
        ));
        assert!(!worth_retrying("Authentication failed: session expired"));
        assert!(!worth_retrying(
            "I/O error: Cannot write to /a: No space left on device"
        ));
        assert!(!worth_retrying("API error: Upload failed (status 5"));
    }

    #[test]
    fn gives_up_after_the_last_retry() {
        let err = "HTTP request failed: connection reset";
        let delays: Vec<_> = (0..4).map(|retries| retry_delay(err, retries)).collect();
        assert_eq!(
            delays,
            [
                Some(Duration::from_secs(60)),
                Some(Duration::from_secs(120)),
                Some(Duration::from_secs(240)),
                None
            ]
        );
        assert_eq!(retry_delay("Not found: media 7", 0), None);
        assert_eq!(gave_up(err.into(), 0), err);
        assert_eq!(gave_up(err.into(), 3), format!("Ha fallado 4 veces: {err}"));
    }

    #[test]
    fn retry_notice_shows_the_wait_first_and_the_error_below() {
        let notice = retry_notice(
            "HTTP request failed: timed out",
            Duration::from_secs(120),
            2,
        );
        assert_eq!(
            notice,
            "Ha fallado; reintento 2 de 3 en 2 min…\nHTTP request failed: timed out"
        );
    }

    #[test]
    fn watchdog_breaks_only_after_nothing_moved_for_the_limit() {
        let mut watchdog = Watchdog::new();
        assert_eq!(
            watchdog.check(Duration::from_secs(60)),
            ControlFlow::Continue(())
        );
        watchdog.last_move -= Duration::from_secs(61);
        let ControlFlow::Break(reason) = watchdog.check(Duration::from_secs(60)) else {
            panic!("a minute without moving should stop it");
        };
        assert!(worth_retrying(&reason));
        watchdog.moved();
        assert_eq!(
            watchdog.check(Duration::from_secs(60)),
            ControlFlow::Continue(())
        );
    }
}
