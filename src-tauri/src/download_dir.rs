//! Downloading a whole folder with the CLI's `download-dir`: its progress in
//! the transfer row, and afterwards the files this app once uploaded in its
//! own parts (`name.o2part.NNN` + `name.o2manifest.json`), which the CLI
//! brings down as they are, joined and checked against their SHA-256. The
//! CLI joins the files it split itself.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tauri::AppHandle;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::cli;
use crate::transfer::{self, Watchdog};
use crate::types::{ChunkManifest, TransferProgress};

/// What a `download-dir --json-lines` run has done so far.
#[derive(Debug, Default)]
pub struct Progress {
    /// The CLI has listed the folder and said how much there is.
    pub started: bool,
    pub files_total: u64,
    pub bytes_total: u64,
    /// Files downloaded, already there or failed.
    pub files_done: u64,
    pub downloaded: u64,
    pub skipped: u64,
    /// Bytes of the files done (downloaded or already there).
    bytes_finished: u64,
    /// Bytes so far of each file being downloaded, by its local path.
    in_flight: HashMap<String, u64>,
    /// «cloud path: error» of each file that failed.
    pub failed: Vec<String>,
    /// The app's own part manifests now on disk, to join their parts.
    pub manifests: BTreeSet<PathBuf>,
    /// The app's manifests of files the CLI split (and joined).
    pub split_manifests: BTreeSet<PathBuf>,
    /// The `error` event the run ended with: its code and message.
    pub error: Option<(String, String)>,
}

impl Progress {
    /// Take one line of the CLI's stdout. Whether it was an event.
    pub fn take(&mut self, line: &str) -> bool {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return false;
        };
        let local = event["local"].as_str().unwrap_or("").to_string();
        match event["event"].as_str() {
            Some("start") => {
                self.started = true;
                self.files_total = event["files"].as_u64().unwrap_or(0);
                self.bytes_total = event["bytes"].as_u64().unwrap_or(0);
            }
            Some("progress") => {
                self.in_flight
                    .insert(local, event["bytes"].as_u64().unwrap_or(0));
            }
            Some("file") => {
                self.in_flight.remove(&local);
                self.files_done += 1;
                match event["action"].as_str() {
                    Some(action @ ("downloaded" | "skipped")) => {
                        if action == "downloaded" {
                            self.downloaded += 1;
                        } else {
                            self.skipped += 1;
                        }
                        self.bytes_finished += event["size"].as_u64().unwrap_or(0);
                        if let Some(path) = event["path"].as_str() {
                            if path.ends_with(".o2manifest.json") {
                                self.manifests.insert(PathBuf::from(path));
                            } else if path.ends_with(".o2split.json") {
                                self.split_manifests.insert(PathBuf::from(path));
                            }
                        }
                    }
                    _ => {
                        let cloud = event["cloud"]
                            .as_str()
                            .or_else(|| event["name"].as_str())
                            .unwrap_or("?");
                        let error = event["error"].as_str().unwrap_or("error");
                        self.failed.push(format!("{cloud}: {error}"));
                    }
                }
            }
            Some("error") => {
                self.error = Some((
                    event["code"].as_str().unwrap_or("").to_string(),
                    event["message"].as_str().unwrap_or("error").to_string(),
                ));
            }
            _ => return false,
        }
        true
    }

    /// Bytes downloaded or found already there, never past the total.
    pub fn bytes_done(&self) -> u64 {
        let done = self.bytes_finished + self.in_flight.values().sum::<u64>();
        done.min(self.bytes_total.max(self.bytes_finished))
    }

    /// What the transfer row says while it runs.
    fn message(&self) -> String {
        if self.started {
            format!("{} de {} archivos", self.files_done, self.files_total)
        } else {
            "Preparando la lista de archivos…".into()
        }
    }
}

/// How a folder download ended, for the frontend's notice.
#[derive(Debug, Serialize)]
pub struct FolderDownload {
    pub files: u64,
    pub downloaded: u64,
    pub skipped: u64,
    /// Files the app had uploaded in its own parts, joined again.
    pub joined: u64,
    /// One line per file that failed (or couldn't be joined).
    pub failed: Vec<String>,
}

/// Download the cloud folder `target` (its ID) into the local folder `dest`,
/// where the CLI creates a folder of its name. Shown as the transfer
/// `transfer_id`, named `name`. Files already there with the same size are
/// skipped and cut downloads resumed; nothing local is overwritten.
pub async fn download_folder(
    app: &AppHandle,
    cli_path: &Path,
    target: &str,
    name: &str,
    dest: &Path,
    transfer_id: &str,
) -> Result<FolderDownload, String> {
    let row = |progress: &Progress, phase: &str, message: String| TransferProgress {
        id: transfer_id.into(),
        kind: "download".into(),
        name: name.into(),
        phase: phase.into(),
        current: progress.files_done,
        total: progress.files_total,
        bytes_done: progress.bytes_done(),
        bytes_total: progress.bytes_total,
        message,
    };
    let mut progress = Progress::default();
    transfer::emit(app, row(&progress, "download", progress.message()));

    let args = vec![
        "download-dir".to_string(),
        target.to_string(),
        "-o".to_string(),
        dest.to_string_lossy().into_owned(),
        "--json-lines".to_string(),
    ];
    let (events, mut lines) = tokio::sync::mpsc::unbounded_channel();
    let mut watchdog = Watchdog::new();
    let exit = cli::run_lines(app, cli_path, &args, events, |_| {
        let mut changed = false;
        while let Ok(line) = lines.try_recv() {
            changed |= progress.take(&line);
        }
        if changed {
            watchdog.moved();
            transfer::emit(app, row(&progress, "download", progress.message()));
        }
        // Listing a large folder the first time takes a while before the
        // first event; after that, files keep moving.
        watchdog.check(if progress.started {
            transfer::STALLED_AFTER
        } else {
            transfer::WAITING_LIMIT
        })
    })
    .await?;
    // What came after the last look.
    while let Ok(line) = lines.try_recv() {
        progress.take(&line);
    }

    if exit.code != 0 {
        match &progress.error {
            Some((code, _)) if code == "session_expired" || code == "not_logged_in" => {
                return Err(cli::SESSION_EXPIRED.into());
            }
            // Some files failed: the rest are there, and joined below.
            Some((code, _)) if code == "partial_failure" => {}
            Some((_, message)) => return Err(message.clone()),
            None => return Err(cli::error_detail(&exit.output.stderr).to_string()),
        }
    }

    let mut failed = std::mem::take(&mut progress.failed);
    let mut joined = 0;
    for manifest in &progress.manifests {
        match join_app_parts(manifest).await {
            Ok(true) => joined += 1,
            Ok(false) => {}
            Err(err) => failed.push(format!("{}: {err}", manifest.display())),
        }
    }
    // The app's manifest of a file the CLI split and has joined: only for
    // the app, and the file it describes is there.
    for manifest in &progress.split_manifests {
        if let Some(file) = manifest
            .to_str()
            .and_then(|p| p.strip_suffix(".o2split.json"))
            .map(Path::new)
            && file.is_file()
        {
            let _ = tokio::fs::remove_file(manifest).await;
        }
    }

    let outcome = FolderDownload {
        files: progress.files_total,
        downloaded: progress.downloaded,
        skipped: progress.skipped,
        joined,
        failed,
    };
    let (phase, message) = if outcome.failed.is_empty() {
        ("done", done_message(&outcome))
    } else {
        ("error", failed_message(&outcome))
    };
    transfer::emit(app, row(&progress, phase, message));
    Ok(outcome)
}

fn done_message(outcome: &FolderDownload) -> String {
    let mut message = format!("{} descargados", outcome.downloaded);
    if outcome.skipped > 0 {
        message.push_str(&format!(", {} ya estaban", outcome.skipped));
    }
    if outcome.joined > 0 {
        message.push_str(&format!(", {} unidos de sus partes", outcome.joined));
    }
    message
}

/// The first line for the row, the files that failed below (on hover).
fn failed_message(outcome: &FolderDownload) -> String {
    const SHOWN: usize = 20;
    let mut message = format!(
        "No se pudieron descargar {} de {} archivos",
        outcome.failed.len(),
        outcome.files
    );
    for line in outcome.failed.iter().take(SHOWN) {
        message.push('\n');
        message.push_str(line);
    }
    if outcome.failed.len() > SHOWN {
        message.push_str(&format!("\n…y {} más", outcome.failed.len() - SHOWN));
    }
    message
}

/// Join the parts a manifest (`name.o2manifest.json`) lists, next to it,
/// into `name`, checking each part's size and SHA-256 and the whole file's.
/// Then the parts and the manifest go. `Ok(false)` if `name` was already
/// there whole (its leftover parts go too). On any mismatch nothing is
/// saved and the parts stay.
pub async fn join_app_parts(manifest_path: &Path) -> Result<bool, String> {
    let dir = manifest_path
        .parent()
        .ok_or("el manifiesto no está en una carpeta")?;
    let base = manifest_path
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_suffix(".o2manifest.json"))
        .ok_or("no es un manifiesto de partes")?;
    let raw = tokio::fs::read(manifest_path)
        .await
        .map_err(|e| format!("no se puede leer el manifiesto: {e}"))?;
    let manifest: ChunkManifest =
        serde_json::from_slice(&raw).map_err(|e| format!("manifiesto no válido: {e}"))?;
    // Only plain names next to the manifest: a manifest from the cloud
    // mustn't make the app read or write anywhere else.
    let plain =
        |name: &str| !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\']);
    if manifest.name != base
        || manifest.parts.is_empty()
        || !manifest.parts.iter().all(|p| plain(&p.name))
    {
        return Err("manifiesto no válido".into());
    }
    let dest = dir.join(base);
    let parts: Vec<PathBuf> = manifest.parts.iter().map(|p| dir.join(&p.name)).collect();
    let remove_parts = || async {
        for part in &parts {
            let _ = tokio::fs::remove_file(part).await;
        }
        let _ = tokio::fs::remove_file(manifest_path).await;
    };

    if let Ok(meta) = tokio::fs::metadata(&dest).await {
        if meta.len() == manifest.size {
            remove_parts().await;
            return Ok(false);
        }
        return Err(format!("ya hay un {base} de otro tamaño"));
    }

    let partial = transfer::partial_path(&dest);
    let result: Result<(), String> = async {
        let mut out = tokio::fs::File::create(&partial)
            .await
            .map_err(|e| format!("no se puede crear {}: {e}", partial.display()))?;
        let mut whole = Sha256::new();
        let mut buffer = vec![0u8; transfer::READ_BUFFER];
        for (part, path) in manifest.parts.iter().zip(&parts) {
            let mut input = tokio::fs::File::open(path)
                .await
                .map_err(|_| format!("falta la parte {}", part.name))?;
            let mut hasher = Sha256::new();
            let mut len = 0u64;
            loop {
                let read = input.read(&mut buffer).await.map_err(|e| e.to_string())?;
                if read == 0 {
                    break;
                }
                out.write_all(&buffer[..read])
                    .await
                    .map_err(|e| e.to_string())?;
                hasher.update(&buffer[..read]);
                whole.update(&buffer[..read]);
                len += read as u64;
            }
            if len != part.size {
                return Err(format!(
                    "la parte {} mide {len} bytes y debería medir {}",
                    part.name, part.size
                ));
            }
            if let Some(expected) = &part.sha256
                && !hex::encode(hasher.finalize()).eq_ignore_ascii_case(expected)
            {
                return Err(format!("el SHA-256 de la parte {} no coincide", part.name));
            }
        }
        out.flush().await.map_err(|e| e.to_string())?;
        drop(out);
        if !hex::encode(whole.finalize()).eq_ignore_ascii_case(&manifest.sha256) {
            return Err("el SHA-256 del archivo unido no coincide; no se ha guardado".into());
        }
        transfer::move_into_place(&partial, &dest).await
    }
    .await;
    if let Err(err) = result {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(err);
    }
    remove_parts().await;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ManifestPart;

    #[test]
    fn follows_download_dir_events() {
        let mut p = Progress::default();
        assert!(!p.take("→ Downloading /Fotos"));
        assert!(p.take(
            r#"{"event":"start","command":"download-dir","files":3,"bytes":300,"schema":1}"#
        ));
        assert_eq!((p.started, p.files_total, p.bytes_total), (true, 3, 300));
        p.take(r#"{"event":"progress","local":"Fotos/a.jpg","bytes":40,"size":100}"#);
        p.take(r#"{"event":"progress","local":"Fotos/b.jpg","bytes":10,"size":100}"#);
        assert_eq!(p.bytes_done(), 50);
        p.take(r#"{"event":"file","local":"Fotos/a.jpg","path":"/d/Fotos/a.jpg","cloud":"/Fotos/a.jpg","size":100,"action":"downloaded"}"#);
        assert_eq!((p.files_done, p.downloaded, p.bytes_done()), (1, 1, 110));
        p.take(r#"{"event":"file","local":"Fotos/b.jpg","cloud":"/Fotos/b.jpg","size":100,"action":"failed","error":"HTTP request failed"}"#);
        p.take(r#"{"event":"file","local":"Fotos/big.mkv.o2manifest.json","path":"/d/Fotos/big.mkv.o2manifest.json","size":100,"action":"skipped"}"#);
        assert_eq!((p.files_done, p.skipped), (3, 1));
        assert_eq!(p.failed, ["/Fotos/b.jpg: HTTP request failed"]);
        assert!(
            p.manifests
                .contains(Path::new("/d/Fotos/big.mkv.o2manifest.json"))
        );
        // Never past the total, even if sizes disagree.
        assert!(p.bytes_done() <= 300);
        p.take(r#"{"event":"error","code":"partial_failure","message":"1 of 3 files failed to download","exit_code":6}"#);
        assert_eq!(
            p.error.as_ref().map(|e| e.0.as_str()),
            Some("partial_failure")
        );
    }

    fn manifest_for(dir: &Path, name: &str, parts: &[&[u8]]) -> PathBuf {
        let mut whole = Sha256::new();
        let mut entries = Vec::new();
        for (i, data) in parts.iter().enumerate() {
            let part_name = format!("{name}.o2part.{:03}", i + 1);
            std::fs::write(dir.join(&part_name), data).unwrap();
            whole.update(data);
            entries.push(ManifestPart {
                name: part_name,
                size: data.len() as u64,
                sha256: Some(hex::encode(Sha256::digest(data))),
            });
        }
        let manifest = ChunkManifest {
            format: "o2cloud-chunks".into(),
            version: 1,
            name: name.into(),
            size: parts.iter().map(|p| p.len() as u64).sum(),
            sha256: hex::encode(whole.finalize()),
            chunk_size: 4,
            parts: entries,
            created_at: None,
        };
        let path = dir.join(format!("{name}.o2manifest.json"));
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        path
    }

    #[tokio::test]
    async fn joins_the_apps_parts_and_checks_them() {
        let dir = std::env::temp_dir().join(format!("o2-join-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        // Joined, then parts and manifest gone.
        let manifest = manifest_for(&dir, "movie.mkv", &[b"abcd", b"efgh", b"ij"]);
        assert_eq!(join_app_parts(&manifest).await, Ok(true));
        assert_eq!(std::fs::read(dir.join("movie.mkv")).unwrap(), b"abcdefghij");
        assert!(!manifest.exists() && !dir.join("movie.mkv.o2part.001").exists());

        // A damaged part: nothing saved, the parts stay.
        let manifest = manifest_for(&dir, "bad.bin", &[b"1234", b"56"]);
        std::fs::write(dir.join("bad.bin.o2part.002"), b"XX").unwrap();
        assert!(
            join_app_parts(&manifest)
                .await
                .unwrap_err()
                .contains("SHA-256")
        );
        assert!(!dir.join("bad.bin").exists());
        assert!(dir.join("bad.bin.o2part.001").exists() && manifest.exists());

        // Already there whole (an earlier run): only the leftovers go.
        let manifest = manifest_for(&dir, "done.bin", &[b"1234", b"56"]);
        std::fs::write(dir.join("done.bin"), b"123456").unwrap();
        assert_eq!(join_app_parts(&manifest).await, Ok(false));
        assert!(!manifest.exists() && !dir.join("done.bin.o2part.001").exists());

        // A part name that leaves the folder is refused.
        let manifest = manifest_for(&dir, "evil.bin", &[b"1"]);
        let mut m: ChunkManifest =
            serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
        m.parts[0].name = "../outside".into();
        std::fs::write(&manifest, serde_json::to_vec(&m).unwrap()).unwrap();
        assert_eq!(
            join_app_parts(&manifest).await,
            Err("manifiesto no válido".into())
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
