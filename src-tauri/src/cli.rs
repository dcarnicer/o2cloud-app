use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

use crate::types::{CliInfo, CliRelease};

const RELEASES_API: &str =
    "https://api.github.com/repos/danielperez9430/o2cloud-cli/releases/latest";
const USER_AGENT: &str = "o2cloud-desktop/0.1";

/// The oldest CLI the app supports. 1.2.0 is required for Momentos,
/// Lugares, Música, Documentos and the slideshow (`labels` with covers,
/// locations and thumbnails, `media`, signed URLs), and lists a file as
/// soon as it's uploaded. 1.0.0 brought `--json-lines`
/// (upload progress and SHA-256 from the CLI itself), `--on-conflict`
/// (a file of another size is asked about, not uploaded as `name (1)`), and
/// its exit codes and JSON errors. It also has everything earlier versions
/// brought: linear `ls` and the file-list cache (0.11, 0.12), the by-ID/path
/// fast path (0.10), and the download-integrity, SAPI, fresh-ID and native
/// split fixes (0.9). Older versions are detected but not used; Settings
/// offers the update.
pub const MIN_VERSION: &str = "1.2.0";

/// The CLI's major version the app is written for. A new major version can
/// change behaviour the app relies on, so Settings doesn't offer it until the
/// app is updated for it.
pub const SUPPORTED_MAJOR: u64 = 1;

/// Returned when O2 no longer accepts the stored session. The frontend opens
/// the app's login window when it sees this.
pub const SESSION_EXPIRED: &str = "La sesión de O2 ha caducado. Vuelve a iniciar sesión.";

/// Any authenticated command opens the CLI's own login window when the session
/// has expired. That window crashes on macOS 27 when it loses key, and the app
/// has its own login flow, so the CLI is stopped as soon as it announces one.
const LOGIN_WINDOW_MARKER: &str = "Opening login window";

pub fn version_supported(version: &str) -> bool {
    let (Ok(version), Ok(minimum)) = (
        semver::Version::parse(version),
        semver::Version::parse(MIN_VERSION),
    ) else {
        return false;
    };
    version >= minimum && version.major == SUPPORTED_MAJOR
}

pub struct CliManager {
    path: RwLock<PathBuf>,
    version: RwLock<String>,
    source: RwLock<String>,
}

impl CliManager {
    pub fn discover(app: &AppHandle) -> Self {
        match discover_cli(app) {
            Ok((path, version, source)) => Self {
                path: RwLock::new(path),
                version: RwLock::new(version),
                source: RwLock::new(source),
            },
            Err(_) => Self {
                path: RwLock::new(PathBuf::new()),
                version: RwLock::new("0.0.0".into()),
                source: RwLock::new("none".into()),
            },
        }
    }

    pub fn is_available(&self) -> bool {
        !self.path.read().unwrap().as_os_str().is_empty()
    }

    pub fn path_or_err(&self) -> Result<PathBuf, String> {
        if !self.is_available() {
            return Err("El CLI de O2 Cloud no está instalado. Instálalo desde Ajustes.".into());
        }
        let path = self.path();
        let version = self.version();
        if !version_supported(&version) {
            return Err(format!(
                "El CLI de O2 Cloud instalado ({version}) es anterior a la versión mínima {MIN_VERSION}. Actualízalo desde Ajustes."
            ));
        }
        Ok(path)
    }

    pub fn info(&self) -> CliInfo {
        let path = self.path();
        let version = self.version();
        let available = !path.as_os_str().is_empty();
        CliInfo {
            path: path.to_string_lossy().to_string(),
            version: version.clone(),
            source: self.source.read().unwrap().clone(),
            available,
            supported: available && version_supported(&version),
            minimum_version: MIN_VERSION.into(),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.path.read().unwrap().clone()
    }

    pub fn version(&self) -> String {
        self.version.read().unwrap().clone()
    }

    pub fn set(&self, path: PathBuf, version: String, source: String) {
        *self.path.write().unwrap() = path;
        *self.version.write().unwrap() = version;
        *self.source.write().unwrap() = source;
    }
}

pub fn exe_name() -> &'static str {
    if cfg!(windows) {
        "o2cloud.exe"
    } else {
        "o2cloud"
    }
}

fn bundled_dir() -> String {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "macos-aarch64".into()
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "macos-x86_64".into()
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "windows-x86_64".into()
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "linux-x86_64".into()
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "linux-aarch64".into()
    } else {
        "unknown".into()
    }
}

fn managed_current_path(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_data_dir().ok()?.join("cli");
    let file = dir.join("current.json");
    let raw = std::fs::read_to_string(file).ok()?;
    #[derive(Deserialize)]
    struct Current {
        path: String,
    }
    let current: Current = serde_json::from_str(&raw).ok()?;
    let path = PathBuf::from(current.path);
    if path.is_file() { Some(path) } else { None }
}

fn version_of(path: &Path) -> Option<String> {
    let mut command = std::process::Command::new(path);
    command.arg("--version");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // This runs while the app starts; don't flash a console window.
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_version(&String::from_utf8_lossy(&output.stdout))
}

fn parse_version(raw: &str) -> Option<String> {
    raw.lines()
        .find_map(|line| line.trim().strip_prefix("o2cloud "))
        .map(|v| v.trim().to_string())
}

fn discover_cli(app: &AppHandle) -> Result<(PathBuf, String, String), String> {
    let mut candidates: Vec<(PathBuf, String)> = Vec::new();

    if let Ok(path) = std::env::var("O2CLOUD_CLI") {
        let path = PathBuf::from(path);
        candidates.push((path, "env".into()));
    }

    if let Some(path) = managed_current_path(app) {
        candidates.push((path, "managed".into()));
    }

    if let Ok(resource_dir) = app.path().resource_dir() {
        let bundled = resource_dir
            .join("bin")
            .join(bundled_dir())
            .join(exe_name());
        candidates.push((bundled, "bundled".into()));
    }

    if let Ok(path) = which::which("o2cloud") {
        candidates.push((path, "system".into()));
    }
    if let Some(home) = dirs::home_dir() {
        candidates.push((home.join(".local/bin").join(exe_name()), "system".into()));
        candidates.push((home.join(".cargo/bin").join(exe_name()), "system".into()));
    }

    let mut unsupported: Option<(PathBuf, String, String)> = None;
    for (path, source) in candidates {
        if !path.is_file() {
            continue;
        }
        if let Some(version) = version_of(&path) {
            // Use an older CLI only if no candidate meets the minimum, so
            // the app can still show which version needs updating.
            if version_supported(&version) {
                return Ok((path, version, source));
            }
            if unsupported.is_none() {
                unsupported = Some((path, version, source));
            }
        }
    }

    unsupported.ok_or_else(|| "No se ha encontrado el CLI de O2 Cloud. Instálalo con `cargo install o2cloud-cli` o descarga un binario desde GitHub.".into())
}

#[derive(Serialize, Clone)]
struct LogLine {
    stream: String,
    line: String,
}

pub struct CliOutput {
    pub stdout: String,
    pub stderr: String,
}

impl CliOutput {
    pub fn text(&self) -> &str {
        if self.stdout.trim().is_empty() {
            &self.stderr
        } else {
            &self.stdout
        }
    }
}

async fn collect_stream<R: AsyncRead + Unpin>(
    reader: R,
    app: AppHandle,
    stream: &'static str,
    mut login_window: Option<tokio::sync::oneshot::Sender<()>>,
    progress: Option<Arc<AtomicU64>>,
    waiting: Option<Arc<AtomicBool>>,
) -> String {
    let mut lines = BufReader::new(reader).lines();
    let mut collected = String::new();
    while let Ok(Some(line)) = lines.next_line().await {
        // `--json-lines`: the bytes the CLI has sent of the file so far.
        if let Some(bytes) = progress.as_ref().and_then(|_| progress_bytes(&line)) {
            if let Some(sink) = &progress {
                sink.fetch_max(bytes, Ordering::Relaxed);
            }
            // Several a second: not worth a log line each.
            continue;
        }
        // `--json-lines`: a replacement is waiting for O2 to validate a
        // recent copy (`MED-1017`). It is not a stalled transfer.
        if let Some(waiting) = waiting.as_ref()
            && is_waiting(&line)
        {
            waiting.store(true, Ordering::Relaxed);
        }
        if stream == "stderr"
            && opens_login_window(&line)
            && let Some(tx) = login_window.take()
        {
            let _ = tx.send(());
        }
        collected.push_str(&line);
        collected.push('\n');
        // Only the CLI's messages reach the page's log: its JSON output
        // holds signed links (and a line per field).
        if stream == "stderr" {
            let _ = app.emit(
                "cli-log",
                LogLine {
                    stream: stream.to_string(),
                    line,
                },
            );
        }
    }
    collected
}

/// Whether `line` is a `--json-lines` `waiting` event: the CLI is waiting
/// for O2 to finish checking a copy uploaded moments ago (`MED-1017`).
fn is_waiting(line: &str) -> bool {
    line.starts_with('{')
        && serde_json::from_str::<serde_json::Value>(line)
            .map(|event| event["event"] == "waiting")
            .unwrap_or(false)
}

/// The `bytes` of a `--json-lines` `progress` event, if `line` is one.
fn progress_bytes(line: &str) -> Option<u64> {
    // The CLI writes the keys in alphabetical order: `event` isn't first.
    if !line.starts_with('{') || !line.contains("\"event\":\"progress\"") {
        return None;
    }
    let event: serde_json::Value = serde_json::from_str(line).ok()?;
    if event["event"] != "progress" {
        return None;
    }
    event["bytes"].as_u64()
}

/// `progress_bytes`, for the transfer module's live test.
#[cfg(test)]
pub fn progress_bytes_for_tests(line: &str) -> Option<u64> {
    progress_bytes(line)
}

/// What a CLI run ended with, when the caller looks at the exit code itself
/// (see `run_events`).
pub struct CliExit {
    pub code: i32,
    pub output: CliOutput,
}

/// The CLI's exit code for "something with that name is already there".
pub const EXIT_CONFLICT: i32 = 5;

fn opens_login_window(line: &str) -> bool {
    line.trim_start_matches(['→', ' '])
        .starts_with(LOGIN_WINDOW_MARKER)
}

/// Run the CLI and capture its output. Stdout/stderr lines are forwarded to
/// the frontend as `cli-log` events.
pub async fn run(app: &AppHandle, path: &Path, args: &[String]) -> Result<CliOutput, String> {
    run_watched_with_env(app, path, args, &[], |_| ControlFlow::Continue(())).await
}

/// Like `run`, with extra environment variables. Used for secrets such as the
/// login cookies, which would otherwise show up in the process list.
pub async fn run_with_env(
    app: &AppHandle,
    path: &Path,
    args: &[String],
    env: &[(&str, &str)],
) -> Result<CliOutput, String> {
    run_watched_with_env(app, path, args, env, |_| ControlFlow::Continue(())).await
}

/// How often `run_watched` samples a running CLI.
const WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// After the CLI exits, its stdout/stderr should close at once. A descendant
/// holding the pipes must not leave the app waiting forever.
const STREAM_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

async fn drain_stream(mut task: tokio::task::JoinHandle<String>) -> Result<String, String> {
    match tokio::time::timeout(STREAM_DRAIN_TIMEOUT, &mut task).await {
        Ok(result) => result.map_err(|error| error.to_string()),
        Err(_) => {
            task.abort();
            Err("El CLI dejó una salida abierta; se ha cancelado la espera.".into())
        }
    }
}

/// Like `run`, calling `watch` with the CLI's process ID every
/// `WATCH_INTERVAL` while it runs, to sample how far a transfer has got.
/// If `watch` breaks, the CLI is stopped and the reason is the error.
pub async fn run_watched(
    app: &AppHandle,
    path: &Path,
    args: &[String],
    watch: impl FnMut(u32) -> ControlFlow<String>,
) -> Result<CliOutput, String> {
    run_watched_with_env(app, path, args, &[], watch).await
}

async fn run_watched_with_env(
    app: &AppHandle,
    path: &Path,
    args: &[String],
    env: &[(&str, &str)],
    watch: impl FnMut(u32) -> ControlFlow<String>,
) -> Result<CliOutput, String> {
    let exit = run_inner(app, path, args, env, None, None, watch).await?;
    if exit.code != 0 {
        let detail = if exit.output.stderr.trim().is_empty() {
            exit.output.stdout
        } else {
            exit.output.stderr
        };
        return Err(error_detail(&detail).to_string());
    }
    Ok(exit.output)
}

/// Run a CLI command with `--json-lines`: `progress` events go to
/// `progress` (bytes sent so far), `waiting` events set `waiting`, `watch`
/// is called as in `run_watched`, and the exit code is returned with the
/// output for the caller to read (the events are in `stdout`).
pub async fn run_events(
    app: &AppHandle,
    path: &Path,
    args: &[String],
    progress: Arc<AtomicU64>,
    waiting: Arc<AtomicBool>,
    watch: impl FnMut(u32) -> ControlFlow<String>,
) -> Result<CliExit, String> {
    run_inner(app, path, args, &[], Some(progress), Some(waiting), watch).await
}

async fn run_inner(
    app: &AppHandle,
    path: &Path,
    args: &[String],
    env: &[(&str, &str)],
    progress: Option<Arc<AtomicU64>>,
    waiting: Option<Arc<AtomicBool>>,
    mut watch: impl FnMut(u32) -> ControlFlow<String>,
) -> Result<CliExit, String> {
    let mut cmd = tokio::process::Command::new(path);
    cmd.args(args)
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        // tokio's Command has this method on Windows without the std trait.
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("No se pudo ejecutar {}: {e}", path.display()))?;
    let stdout = child.stdout.take().ok_or("sin stdout")?;
    let stderr = child.stderr.take().ok_or("sin stderr")?;

    let (login_tx, mut login_rx) = tokio::sync::oneshot::channel();
    let out_task = tokio::spawn(collect_stream(
        stdout,
        app.clone(),
        "stdout",
        None,
        progress,
        waiting,
    ));
    let err_task = tokio::spawn(collect_stream(
        stderr,
        app.clone(),
        "stderr",
        Some(login_tx),
        None,
        None,
    ));

    let pid = child.id();
    let mut ticks = tokio::time::interval(WATCH_INTERVAL);
    ticks.tick().await; // the first tick is immediate
    let mut watch_login = true;
    let status = loop {
        tokio::select! {
            status = child.wait() => break status.map_err(|e| e.to_string())?,
            result = &mut login_rx, if watch_login => {
                if result.is_ok() {
                    // The CLI was about to open its login window, which crashes
                    // on macOS and would wait for input behind the app's back.
                    let _ = child.kill().await;
                    let _ = drain_stream(out_task).await;
                    let _ = drain_stream(err_task).await;
                    return Err(SESSION_EXPIRED.into());
                }
                // The stderr stream ended normally: stop watching it.
                watch_login = false;
            }
            _ = ticks.tick() => {
                if let Some(pid) = pid
                    && let ControlFlow::Break(reason) = watch(pid)
                {
                    let _ = child.kill().await;
                    return Err(reason);
                }
            }
        }
    };
    // If the CLI announced a login window and exited before the select could
    // stop it, treat the command as an expired session anyway.
    if matches!(login_rx.try_recv(), Ok(())) {
        return Err(SESSION_EXPIRED.into());
    }
    let stdout = drain_stream(out_task).await?;
    let stderr = drain_stream(err_task).await?;
    Ok(CliExit {
        // Killed by a signal: no code, so an error.
        code: status.code().unwrap_or(-1),
        output: CliOutput { stdout, stderr },
    })
}

/// The CLI ends a failure with `Error: …`, after progress lines such as
/// `→ Uploading …`. Keep the error, so it is what the app shows first.
pub fn error_detail(output: &str) -> &str {
    let output = output.trim();
    match output.rfind("\nError: ") {
        Some(start) => &output[start + "\nError: ".len()..],
        None => output.strip_prefix("Error: ").unwrap_or(output),
    }
}

/// Parse the JSON value a `--json` command printed. Progress and human
/// messages stay on stderr, but skip any leading text just in case.
pub fn parse_json<T: for<'de> Deserialize<'de>>(output: &CliOutput) -> Result<T, String> {
    let raw = output.text();
    // What the CLI printed isn't repeated in the error: it can hold
    // signed links to files.
    let json_start = raw
        .find(['{', '['])
        .ok_or_else(|| "El CLI no devolvió JSON.".to_string())?;
    serde_json::from_str(&raw[json_start..]).map_err(|e| format!("JSON inválido del CLI: {e}"))
}

pub async fn run_json<T: for<'de> Deserialize<'de>>(
    app: &AppHandle,
    path: &Path,
    args: &[String],
) -> Result<T, String> {
    parse_json(&run(app, path, args).await?)
}

fn asset_name_for_platform() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("o2cloud-aarch64-apple-darwin.tar.gz")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("o2cloud-x86_64-apple-darwin.tar.gz")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("o2cloud-x86_64-pc-windows-msvc.zip")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("o2cloud-x86_64-unknown-linux-gnu.tar.gz")
    } else {
        None
    }
}

fn clean_version(tag: &str) -> String {
    tag.trim_start_matches('v').to_string()
}

/// Whether Settings offers `latest` over `current`: newer, and of the major
/// version the app supports (or newer at all if `current` isn't parseable).
fn update_offered(current: &str, latest: &str) -> bool {
    match (
        semver::Version::parse(current),
        semver::Version::parse(latest),
    ) {
        (Ok(a), Ok(b)) => b > a && b.major == SUPPORTED_MAJOR,
        (Err(_), Ok(b)) => b.major == SUPPORTED_MAJOR,
        _ => false,
    }
}

pub async fn check_update(current: &str) -> Result<CliRelease, String> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(RELEASES_API)
        .send()
        .await
        .map_err(|e| format!("No se pudo consultar GitHub: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("GitHub respondió HTTP {}", resp.status()));
    }
    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let tag = json["tag_name"].as_str().unwrap_or_default();
    let latest = clean_version(tag);
    let asset_name = asset_name_for_platform();
    let asset = asset_name.and_then(|name| {
        json["assets"]
            .as_array()?
            .iter()
            .find(|a| a["name"].as_str() == Some(name))
            .cloned()
    });
    let asset_url = asset
        .as_ref()
        .and_then(|a| a["browser_download_url"].as_str())
        .map(|s| s.to_string());
    let notes = json["body"].as_str().map(|s| s.to_string());

    let update_available = update_offered(current, &latest);

    Ok(CliRelease {
        current: current.to_string(),
        latest: if latest.is_empty() {
            None
        } else {
            Some(latest)
        },
        update_available,
        asset_name: asset_name.map(|s| s.to_string()),
        asset_url,
        notes,
    })
}

pub async fn download_and_install(
    app: &AppHandle,
    version: &str,
    url: &str,
) -> Result<CliInfo, String> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Descarga fallida: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Descarga fallida: HTTP {}", resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| e.to_string())?;

    let cli_root = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("cli");
    let target_dir = cli_root.join("versions").join(version);
    std::fs::create_dir_all(&target_dir).map_err(|e| e.to_string())?;

    let temp_dir = cli_root.join("tmp");
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;

    let archive_name = url.rsplit('/').next().unwrap_or("download");
    let archive_path = temp_dir.join(archive_name);
    std::fs::write(&archive_path, &bytes).map_err(|e| e.to_string())?;

    let extracted = if archive_name.ends_with(".zip") {
        extract_zip(&archive_path, &temp_dir)?
    } else {
        extract_tar_gz(&archive_path, &temp_dir)?
    };

    let target = target_dir.join(exe_name());
    std::fs::copy(&extracted, &target).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
    }
    #[cfg(target_os = "macos")]
    {
        // A binary downloaded by the app normally has no quarantine attribute,
        // but clear it defensively if a previous copy did.
        let _ = std::process::Command::new("xattr")
            .args(["-d", "com.apple.quarantine"])
            .arg(&target)
            .output();
    }

    let installed_version = version_of(&target)
        .ok_or_else(|| "El binario descargado no responde a --version".to_string())?;
    if !version_supported(&installed_version) {
        return Err(format!(
            "La versión descargada ({installed_version}) es anterior a la mínima ({MIN_VERSION})"
        ));
    }
    if installed_version != version {
        return Err(format!(
            "La versión instalada ({installed_version}) no coincide con la esperada ({version})"
        ));
    }

    // The caller tests it and only then makes it current (`write_current`).
    let _ = std::fs::remove_dir_all(&temp_dir);
    Ok(CliInfo {
        path: target.to_string_lossy().to_string(),
        version: installed_version,
        source: "managed".into(),
        available: true,
        supported: true,
        minimum_version: MIN_VERSION.into(),
    })
}

#[derive(Serialize, Deserialize)]
struct CliCurrent {
    version: String,
    path: String,
}

pub fn write_current(app: &AppHandle, path: &Path, version: &str) -> Result<(), String> {
    let cli_root = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("cli");
    std::fs::create_dir_all(&cli_root).map_err(|e| e.to_string())?;
    let current = CliCurrent {
        version: version.to_string(),
        path: path.to_string_lossy().to_string(),
    };
    std::fs::write(
        cli_root.join("current.json"),
        serde_json::to_vec_pretty(&current).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<PathBuf, String> {
    let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    tar.unpack(dest)
        .map_err(|e| format!("Error al extraer tar.gz: {e}"))?;
    find_binary(dest).ok_or_else(|| "El archivo no contiene el binario o2cloud".into())
}

fn extract_zip(archive: &Path, dest: &Path) -> Result<PathBuf, String> {
    let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("ZIP inválido: {e}"))?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(name) = entry.enclosed_name().map(|p| p.to_path_buf()) else {
            continue;
        };
        let out = dest.join(name);
        if entry.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let mut f = std::fs::File::create(&out).map_err(|e| e.to_string())?;
            std::io::copy(&mut entry, &mut f).map_err(|e| e.to_string())?;
        }
    }
    find_binary(dest).ok_or_else(|| "El archivo no contiene el binario o2cloud".into())
}

fn find_binary(dir: &Path) -> Option<PathBuf> {
    let direct = dir.join(exe_name());
    if direct.is_file() {
        return Some(direct);
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_binary(&path) {
                return Some(found);
            }
        } else if path.file_name().and_then(|n| n.to_str()) == Some(exe_name()) {
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_cli_error_without_its_progress_lines() {
        let output =
            "→ Uploading /a/b.jpg (2.1 MB)...\nError: HTTP request failed: error sending request\n";
        assert_eq!(
            error_detail(output),
            "HTTP request failed: error sending request"
        );
        assert_eq!(
            error_detail("Error: Not found: media 7"),
            "Not found: media 7"
        );
        assert_eq!(error_detail("✗ Not logged in.\n"), "✗ Not logged in.");
    }

    #[test]
    fn spots_the_cli_login_window_announcement() {
        assert!(opens_login_window(
            "→ Opening login window... (without one, run `o2cloud login --jsessionid ...` instead)"
        ));
        assert!(opens_login_window(
            "→ Opening login window (enter your phone number there)..."
        ));
        assert!(!opens_login_window("→ Uploading /a/b.jpg (2.1 MB)..."));
        assert!(!opens_login_window(
            "→ Uploading /tmp/Opening login window.txt (2.1 MB)..."
        ));
    }

    #[test]
    fn requires_cli_1_x() {
        assert!(!version_supported("0.12.0"));
        assert!(!version_supported("0.15.4"));
        assert!(!version_supported("not-a-version"));
        // Labels, media and their URLs came in 1.2.
        assert!(!version_supported("1.1.0"));
        assert!(version_supported("1.2.0"));
        assert!(version_supported("1.4.2"));
        // A new major version may change what the app relies on.
        assert!(!version_supported("2.0.0"));
    }

    #[test]
    fn offers_updates_within_the_major_version() {
        assert!(update_offered("1.0.0", "1.0.1"));
        assert!(update_offered("0.15.4", "1.2.0"));
        assert!(!update_offered("1.0.0", "1.0.0"));
        assert!(!update_offered("1.3.0", "2.0.0"));
        assert!(update_offered("unknown", "1.0.0"));
    }

    #[test]
    fn reads_progress_events() {
        assert_eq!(
            progress_bytes(r#"{"event":"progress","local":"a.jpg","bytes":1024,"size":2048}"#),
            Some(1024)
        );
        // As the CLI writes it, keys in alphabetical order.
        assert_eq!(
            progress_bytes(r#"{"bytes":8126464,"event":"progress","name":"a","size":9}"#),
            Some(8126464)
        );
        assert_eq!(progress_bytes(r#"{"event":"file","bytes":1}"#), None);
        assert_eq!(progress_bytes("→ Uploading a.jpg"), None);
    }

    #[test]
    fn recognizes_waiting_events() {
        assert!(is_waiting(
            r#"{"event":"waiting","name":"a.jpg","reason":"validating","code":"MED-1017"}"#
        ));
        // As the CLI writes it, keys in alphabetical order.
        assert!(is_waiting(
            r#"{"code":"MED-1017","event":"waiting","name":"a","reason":"validating"}"#
        ));
        assert!(is_waiting(
            r#"{ "event": "waiting", "name": "a.jpg", "reason": "validating" }"#
        ));
        assert!(!is_waiting(r#"{"event":"progress","bytes":1}"#));
        assert!(!is_waiting("→ Uploading a.jpg"));
    }

    #[test]
    fn reads_json_after_progress_text() {
        let output = CliOutput {
            stdout: "warning: something\n{\"id\": 42, \"created\": true}\n".into(),
            stderr: "→ Creating folder…\n".into(),
        };
        #[derive(Deserialize)]
        struct Created {
            id: u64,
            created: bool,
        }
        let created: Created = parse_json(&output).unwrap();
        assert_eq!(created.id, 42);
        assert!(created.created);
    }
}
