use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinSet;
use walkdir::WalkDir;

use crate::cache::{CachedListing, ListingCache};
use crate::camera;
use crate::cli::{self, CliManager, SESSION_EXPIRED};
use crate::library::{self, NewUpload, UploadLibrary};
use crate::photos;
use crate::transfer::{self, ExistingFiles, UploadOutcome};
use crate::types::{
    AppSettings, CliInfo, CliRelease, Location, LogicalFile, LogicalListing, LsResponse,
    MkdirOutput, PartRef, RemoteFile, RemoteFolder, SessionStatus, Slide, SongTags, StatusOutput,
    StorageSpace, Track, TransferProgress, VirtualFolder,
};

/// Each `rm` lists the whole account before deleting; a few at a time keeps
/// a chunked file's delete short without flooding O2.
const PARALLEL_DELETES: usize = 3;

/// The whole-account listing (`ls -a --json`), reused for a few minutes.
#[derive(Default)]
struct AllFilesCache {
    at: Option<Instant>,
    files: Vec<RemoteFile>,
}

pub struct AppState {
    pub cli: CliManager,
    pub settings: RwLock<AppSettings>,
    pub library: RwLock<UploadLibrary>,
    listings: ListingCache,
    /// Uploads run one at a time, in the order they were started.
    upload_lock: Mutex<()>,
    /// One library sync at a time.
    library_sync_lock: Mutex<()>,
    all_files: RwLock<AllFilesCache>,
}

impl AppState {
    pub fn new(app: &AppHandle) -> Self {
        let cli = CliManager::discover(app);
        let settings = load_settings(app);
        let listings = transfer::app_cache_dir(app)
            .ok()
            .map(|dir| dir.join("listings.json"));
        Self {
            cli,
            settings: RwLock::new(settings),
            library: RwLock::new(UploadLibrary::load(app)),
            listings: ListingCache::load(listings),
            upload_lock: Mutex::new(()),
            library_sync_lock: Mutex::new(()),
            all_files: RwLock::new(AllFilesCache::default()),
        }
    }
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("settings.json"))
}

fn load_settings(app: &AppHandle) -> AppSettings {
    let Ok(path) = settings_path(app) else {
        return AppSettings::default();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        "/".into()
    } else if trimmed.starts_with('/') {
        trimmed.trim_end_matches('/').to_string()
    } else {
        format!("/{}", trimmed.trim_end_matches('/'))
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{}/{}", parent.trim_end_matches('/'), name)
    }
}

#[tauri::command]
pub fn get_cli_info(state: State<'_, AppState>) -> Result<CliInfo, String> {
    Ok(state.cli.info())
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> AppSettings {
    state.settings.read().unwrap().clone()
}

#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: AppSettings,
) -> Result<AppSettings, String> {
    let raw = serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(settings_path(&app)?, raw).map_err(|e| e.to_string())?;
    *state.settings.write().unwrap() = settings.clone();
    Ok(settings)
}

async fn fetch_session_status(app: &AppHandle, path: &Path) -> Result<SessionStatus, String> {
    let args = vec!["status".to_string(), "--json".to_string()];
    let output = cli::run(app, path, &args).await?;
    let status: StatusOutput = cli::parse_json(&output)?;
    Ok(session_status(status, output.text().to_string()))
}

fn session_status(status: StatusOutput, raw: String) -> SessionStatus {
    SessionStatus {
        logged_in: status.logged_in && status.session.as_deref() == Some("active"),
        raw,
        phone: status.phone,
        since: status.since,
        session: status.session,
        renews_silently: status.renews_silently,
        storage: status.storage,
    }
}

#[tauri::command]
pub async fn get_session_status(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SessionStatus, String> {
    let path = state.cli.path_or_err()?;
    fetch_session_status(&app, &path).await
}

/// Whether the CLI at `cli_path` has a session O2 accepts. `status` checks
/// without renewing it; any other command with an expired session tries to
/// open the CLI's login window, which `cli::run` stops.
async fn session_active(app: &AppHandle, cli_path: &Path) -> Result<bool, String> {
    Ok(fetch_session_status(app, cli_path).await?.logged_in)
}

/// Refuse a CLI command when the session is expired, before the CLI can try
/// to open its own login window and wait there.
async fn ensure_session(app: &AppHandle, cli_path: &Path) -> Result<(), String> {
    if session_active(app, cli_path).await? {
        Ok(())
    } else {
        Err(SESSION_EXPIRED.into())
    }
}

#[tauri::command]
pub async fn login(app: AppHandle, state: State<'_, AppState>) -> Result<SessionStatus, String> {
    let path = state.cli.path_or_err()?;
    let (jsessionid, validationkey) = login_cookies(&app).await?;
    let args = vec!["login".to_string()];
    let env = [
        ("O2CLOUD_JSESSIONID", jsessionid.as_str()),
        ("O2CLOUD_VALIDATIONKEY", validationkey.as_str()),
    ];
    // If the CLI rejects the cookies it announces its own login window; the
    // runner stops it (that window crashes on macOS) and reports the expired
    // session. Here that means the cookies did not work.
    if let Err(error) = cli::run_with_env(&app, &path, &args, &env).await {
        if error == SESSION_EXPIRED {
            return Err("O2 Cloud no ha aceptado el inicio de sesión. Vuelve a intentarlo.".into());
        }
        return Err(error);
    }
    state.listings.clear();
    fetch_session_status(&app, &path).await
}

/// The app's own login window. The CLI's winit login window crashes on macOS
/// when it loses key while the OAuth page is open (seen on macOS 27), which
/// made clicking outside it end the login; this one is a Tauri window.
const LOGIN_WINDOW: &str = "o2cloud-login";
const LOGIN_ERROR_URL: &str = "https://cloud.o2online.es/ui/html/oauth.html";

fn login_device_id() -> String {
    format!("web-{}", uuid::Uuid::new_v4().simple())
}

/// Open the O2 login page and wait until it has stored the session cookies.
/// The redirect at the end of OAuth sets `JSESSIONID` and `validationKey`;
/// the CLI validates and saves them, so its private API handling stays in one
/// place.
async fn login_cookies(app: &AppHandle) -> Result<(String, String), String> {
    let device_id = login_device_id();
    let url = tauri::Url::parse(&format!(
        "https://cloud.o2online.es/sapi/oauth/pkce/authorize?platform=web&deviceid={device_id}"
    ))
    .map_err(|e| e.to_string())?;

    if let Some(existing) = app.get_webview_window(LOGIN_WINDOW) {
        let _ = existing.destroy();
    }
    let mut builder = WebviewWindowBuilder::new(app, LOGIN_WINDOW, WebviewUrl::External(url))
        .title("O2 Cloud — Iniciar sesión")
        .inner_size(480.0, 700.0)
        .center()
        .incognito(true);
    // Keep the login window above the app window: clicking the app while the
    // login is open must not hide it behind the main window.
    if let Some(main) = app.get_webview_window("main") {
        builder = builder.parent(&main).map_err(|e| e.to_string())?;
    }
    let window = builder
        .build()
        .map_err(|e| format!("No se pudo abrir la ventana de inicio de sesión: {e}"))?;

    let result = wait_for_cookies(app, &window).await;
    if let Some(window) = app.get_webview_window(LOGIN_WINDOW) {
        let _ = window.destroy();
    }
    result
}

/// The O2 Cloud cookie `name`, preferring the `cloud.o2online.es` one. The
/// login pages on other Telefónica domains can set cookies with the same name,
/// and the CLI must receive the ones from the O2 Cloud host.
fn cloud_cookie(cookies: &[tauri::webview::Cookie<'static>], name: &str) -> Option<String> {
    let domain_is = |cookie: &&tauri::webview::Cookie<'static>, suffix: &str| {
        cookie
            .domain()
            .is_some_and(|domain| domain.trim_start_matches('.').eq_ignore_ascii_case(suffix))
    };
    let matching = || cookies.iter().filter(|cookie| cookie.name() == name);
    matching()
        .find(|cookie| domain_is(cookie, "cloud.o2online.es"))
        .or_else(|| {
            matching().find(|cookie| {
                cookie.domain().is_some_and(|domain| {
                    let domain = domain.trim_start_matches('.');
                    domain == "o2online.es" || domain.ends_with(".o2online.es")
                })
            })
        })
        .map(|cookie| cookie.value().to_string())
        .filter(|value| !value.is_empty())
}

async fn wait_for_cookies(
    app: &AppHandle,
    window: &tauri::WebviewWindow,
) -> Result<(String, String), String> {
    let mut error_page_polls = 0;
    loop {
        if app.get_webview_window(LOGIN_WINDOW).is_none() {
            return Err("Has cerrado la ventana de inicio de sesión.".into());
        }

        let cookie_window = window.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        app.run_on_main_thread(move || {
            let _ = tx.send(cookie_window.cookies().map_err(|e| e.to_string()));
        })
        .map_err(|e| e.to_string())?;
        let cookies = rx.await.map_err(|_| {
            "La ventana de inicio de sesión se cerró inesperadamente.".to_string()
        })??;

        if let (Some(jsessionid), Some(validationkey)) = (
            cloud_cookie(&cookies, "JSESSIONID"),
            cloud_cookie(&cookies, "validationKey"),
        ) {
            return Ok((jsessionid, validationkey));
        }

        // A rejected login lands on the OAuth error page and never sets the
        // cookies. Give the cookie store a few polls first: a successful
        // login can also pass through this page on its way to O2's UI.
        let on_error_page = window
            .url()
            .map(|url| url.as_str().starts_with(LOGIN_ERROR_URL))
            .unwrap_or(false);
        if on_error_page {
            error_page_polls += 1;
            if error_page_polls >= 8 {
                return Err(
                    "O2 Cloud ha rechazado el inicio de sesión. Inténtalo de nuevo.".into(),
                );
            }
        } else {
            error_page_polls = 0;
        }

        tokio::time::sleep(Duration::from_millis(400)).await;
    }
}

#[tauri::command]
pub async fn logout(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let path = state.cli.path_or_err()?;
    cli::run(&app, &path, &vec!["logout".to_string()]).await?;
    state.listings.clear();
    Ok(())
}

/// List a folder, by ID or else by path. Unless `fresh`, a recent listing
/// of the same folder is reused (see `ListingCache`).
async fn fetch_listing(
    app: &AppHandle,
    cli_path: &Path,
    folder_id: Option<u64>,
    path: Option<&str>,
    fresh: bool,
) -> Result<CachedListing, String> {
    let mut args = vec!["ls".to_string(), "--json".to_string()];
    let key = if let Some(id) = folder_id {
        args.push(id.to_string());
        format!("id:{id}")
    } else {
        let path = normalize_path(path.unwrap_or("/"));
        if path != "/" {
            args.push(path.clone());
        }
        format!("path:{path}")
    };

    let state = app.state::<AppState>();
    if !fresh && let Some(listing) = state.listings.get(&key) {
        return Ok(listing);
    }
    // With an expired session `ls` opens the CLI's own login window, which
    // crashes on macOS. Refuse before spawning it; the frontend then asks for
    // a new login with the app's window.
    ensure_session(app, cli_path).await?;
    let listing: LsResponse = cli::run_json(app, cli_path, &args).await?;
    state.listings.put(key, listing.clone());
    Ok(CachedListing {
        listing,
        stale: false,
    })
}

/// How long a library sync is reused before checking O2 for new snapshots.
const LIBRARY_SYNC_TTL_MS: i64 = 120_000;
/// The whole-account listing takes 5.6 s the first time and 0.3 s after that
/// with CLI 0.12 on a 46k-file account.
const ALL_FILES_TTL: Duration = Duration::from_secs(300);
/// A folder view must not wait forever if the CLI or its pipes hang.
const LISTING_TIMEOUT: Duration = Duration::from_secs(90);
/// After this many own snapshots, publish a full one and drop the deltas.
const SNAPSHOT_COMPACT_AFTER: usize = 8;
/// Leave this much free beyond the original's size when Photos must download it.
const PHOTOS_FREE_SPACE_MARGIN: u64 = 128 * 1024 * 1024;

#[cfg(unix)]
fn free_disk_space(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } == 0 {
        Some(stat.f_bavail as u64 * stat.f_frsize as u64)
    } else {
        None
    }
}

#[cfg(not(unix))]
fn free_disk_space(_path: &Path) -> Option<u64> {
    None
}

async fn fetch_all_files(
    app: &AppHandle,
    state: &AppState,
    cli_path: &Path,
    fresh: bool,
) -> Result<Vec<RemoteFile>, String> {
    if !fresh {
        let cache = state.all_files.read().unwrap();
        if let Some(at) = cache.at
            && at.elapsed() < ALL_FILES_TTL
        {
            return Ok(cache.files.clone());
        }
    }
    let args = vec!["ls".to_string(), "-a".to_string(), "--json".to_string()];
    // Same as `fetch_listing`: never let `ls` open the CLI login window.
    ensure_session(app, cli_path).await?;
    let files: Vec<RemoteFile> = cli::run_json(app, cli_path, &args).await?;
    let files: Vec<RemoteFile> = files
        .into_iter()
        .filter(|file| {
            let path = file.path.as_deref().unwrap_or("");
            !library::is_internal_path(path) && !library::is_internal_name(&file.name)
        })
        .collect();
    *state.all_files.write().unwrap() = AllFilesCache {
        at: Some(Instant::now()),
        files: files.clone(),
    };
    Ok(files)
}

fn library_is_fresh(state: &AppState) -> bool {
    let library = state.library.read().unwrap();
    !library.pending_snapshot()
        && library
            .last_sync()
            .is_some_and(|last| library::now_ms() - last < LIBRARY_SYNC_TTL_MS)
}

async fn ensure_library_synced(
    app: &AppHandle,
    state: &AppState,
    cli_path: &Path,
    force: bool,
) -> Result<(), String> {
    if !force && library_is_fresh(state) {
        return Ok(());
    }
    let _guard = state.library_sync_lock.lock().await;
    if !force && library_is_fresh(state) {
        return Ok(());
    }
    sync_library_inner(app, state, cli_path).await
}

async fn sync_library_inner(
    app: &AppHandle,
    state: &AppState,
    cli_path: &Path,
) -> Result<(), String> {
    let folder_id = library::ensure_folder(app, cli_path).await?;
    let remote_files = library::list_snapshots(app, cli_path, folder_id).await?;
    let known = state.library.read().unwrap().snapshots().clone();
    let mut downloaded = Vec::new();

    for file in &remote_files {
        let size = file.size.unwrap_or(0);
        if known
            .get(&file.name)
            .is_some_and(|state| state.id == file.id && state.size == size)
        {
            continue;
        }
        match library::download_snapshot(app, cli_path, file).await {
            Ok(snapshot) => downloaded.push((
                file.name.clone(),
                library::SnapshotState { id: file.id, size },
                snapshot,
            )),
            Err(_) => {
                // A corrupt or temporarily unavailable snapshot must not stop
                // the rest of the library from loading.
            }
        }
    }

    {
        let mut library = state.library.write().unwrap();
        for (name, snapshot_state, snapshot) in downloaded {
            library.merge_snapshot(snapshot);
            library.remember_snapshot(name, snapshot_state);
        }
        // Snapshots removed by compaction (or from another device) must not
        // stay in the local bookkeeping forever.
        let remote_names: HashSet<String> =
            remote_files.iter().map(|file| file.name.clone()).collect();
        let stale: Vec<String> = library
            .snapshots()
            .keys()
            .filter(|name| !remote_names.contains(*name))
            .cloned()
            .collect();
        for name in stale {
            library.forget_snapshot(&name);
        }
        library.set_folder_id(folder_id);
        library.mark_synced();
        library.save()?;
    }

    if state.library.read().unwrap().pending_snapshot() {
        upload_own_snapshot(app, state, cli_path, folder_id, None).await?;
    }
    Ok(())
}

async fn upload_own_snapshot(
    app: &AppHandle,
    state: &AppState,
    cli_path: &Path,
    folder_id: u64,
    delta: Option<library::LibrarySnapshot>,
) -> Result<(), String> {
    let (full, device_id) = {
        let library = state.library.read().unwrap();
        (library.own_snapshot(), library.device_id().to_string())
    };
    let is_delta = delta.is_some();
    let mut snapshot = delta.unwrap_or_else(|| full.clone());
    if snapshot.entries.is_empty() {
        let mut library = state.library.write().unwrap();
        library.set_folder_id(folder_id);
        library.mark_snapshot_done();
        library.save()?;
        return Ok(());
    }

    // One listing decides both whether the deltas need compacting and which
    // old snapshots to delete if they do.
    let remote_files = library::list_snapshots(app, cli_path, folder_id)
        .await
        .unwrap_or_default();
    let own = library::own_snapshot_files(&remote_files, &device_id);
    let compact = !is_delta || own.len() >= SNAPSHOT_COMPACT_AFTER;
    if compact {
        snapshot = full;
    }

    let (name, reported_id, size) =
        match library::upload_snapshot(app, cli_path, folder_id, &snapshot).await {
            Ok(uploaded) => uploaded,
            Err(error) => {
                let mut library = state.library.write().unwrap();
                library.mark_snapshot_pending();
                let _ = library.save();
                return Err(error);
            }
        };

    let old: Vec<(u64, String)> = if compact {
        own.iter()
            .map(|file| (file.id, file.name.clone()))
            .collect()
    } else {
        Vec::new()
    };
    for (id, _) in &old {
        if *id != reported_id.unwrap_or(0) {
            let _ = library::delete_snapshot(app, cli_path, *id).await;
        }
    }

    let mut library = state.library.write().unwrap();
    library.set_folder_id(folder_id);
    library.remember_snapshot(
        name,
        library::SnapshotState {
            id: reported_id.unwrap_or(0),
            size,
        },
    );
    for (_, old_name) in old {
        library.forget_snapshot(&old_name);
    }
    library.mark_snapshot_done();
    library.save()
}

/// One recording of the library at a time, in the order they came.
static LIBRARY_RECORDING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// `record_library_uploads` after the upload has returned; a failure is
/// told to the page as `library-warning`.
fn record_library_in_background(app: &AppHandle, cli_path: &Path, uploads: Vec<NewUpload>) {
    if uploads.is_empty() {
        return;
    }
    let app = app.clone();
    let cli_path = cli_path.to_path_buf();
    tauri::async_runtime::spawn(async move {
        let _turn = LIBRARY_RECORDING.lock().await;
        let state = app.state::<AppState>();
        if let Err(error) = record_library_uploads(&app, &state, &cli_path, uploads).await {
            let _ = app.emit(
                "library-warning",
                format!(
                    "Los archivos se han subido, pero no se ha podido actualizar la biblioteca: {error}"
                ),
            );
        }
    });
}

async fn record_library_uploads(
    app: &AppHandle,
    state: &AppState,
    cli_path: &Path,
    uploads: Vec<NewUpload>,
) -> Result<(), String> {
    if uploads.is_empty() {
        return Ok(());
    }
    // If a previous snapshot failed, publish a full one: a delta would leave
    // the earlier entries unpublished. Otherwise the batch is enough.
    let delta = {
        let library = state.library.read().unwrap();
        if library.pending_snapshot() {
            None
        } else {
            Some(library.snapshot_from_uploads(&uploads))
        }
    };
    {
        let mut library = state.library.write().unwrap();
        library.add_uploads(uploads);
        library.save()?;
    }
    // One snapshot writer at a time: another sync must not delete the file
    // this upload is about to remember.
    let _guard = state.library_sync_lock.lock().await;
    let folder_id = {
        let library = state.library.read().unwrap();
        library.folder_id()
    };
    let folder_id = match folder_id {
        Some(folder_id) => folder_id,
        None => library::ensure_folder(app, cli_path).await?,
    };
    upload_own_snapshot(app, state, cli_path, folder_id, delta).await
}

struct RemoteIndex {
    ids: HashSet<u64>,
    paths: HashSet<String>,
    sizes: HashSet<u64>,
    by_name_size: HashMap<(String, u64), Vec<RemoteFile>>,
}

impl RemoteIndex {
    fn new(files: &[RemoteFile]) -> Self {
        let mut index = Self {
            ids: HashSet::new(),
            paths: HashSet::new(),
            sizes: HashSet::new(),
            by_name_size: HashMap::new(),
        };
        for file in files {
            let size = file.size.unwrap_or(0);
            index.ids.insert(file.id);
            if let Some(path) = &file.path {
                index.paths.insert(path.clone());
            }
            index.sizes.insert(size);
            index
                .by_name_size
                .entry((file.name.clone(), size))
                .or_default()
                .push(file.clone());
        }
        index.add_whole_files(files);
        index
    }

    /// A file stored in parts (the app's `name.o2part.NNN` + manifest, or
    /// the CLI's `name.001`…) is in the account's listing only as its
    /// parts. Index it whole too, by its own name and size, as a folder view
    /// shows it: otherwise a 6.5 GB video in two parts looks not uploaded.
    fn add_whole_files(&mut self, files: &[RemoteFile]) {
        let mut by_folder: HashMap<&str, Vec<RemoteFile>> = HashMap::new();
        for file in files {
            let name = file.name.as_str();
            let in_parts = name.ends_with(".o2manifest.json")
                || name.ends_with(".o2split.json")
                || transfer::split_part_name(name).is_some()
                || transfer::cli_part_name(name).is_some();
            if in_parts {
                let folder = folder_of(file.path.as_deref().unwrap_or("/"));
                by_folder.entry(folder).or_default().push(file.clone());
            }
        }
        for (folder, group) in by_folder {
            let (logical, _) = transfer::group_logical_files(group);
            for file in logical {
                if !file.chunked || file.incomplete || file.size == 0 {
                    continue;
                }
                let Some(id) = file
                    .manifest_id
                    .or(file.normal_id)
                    .or_else(|| file.parts.first().map(|p| p.id))
                else {
                    continue;
                };
                let path = join_path(folder, &file.name);
                self.paths.insert(path.clone());
                self.sizes.insert(file.size);
                self.by_name_size
                    .entry((file.name.clone(), file.size))
                    .or_default()
                    .push(RemoteFile {
                        id,
                        name: file.name,
                        size: Some(file.size),
                        date: file.date,
                        path: Some(path),
                    });
            }
        }
    }

    fn has_size(&self, size: u64) -> bool {
        self.sizes.contains(&size)
    }
}

fn push_copy(copies: &mut Vec<library::LibraryCopyView>, copy: library::LibraryCopyView) {
    let duplicate = copies
        .iter()
        .any(|existing| match (existing.remote_id, copy.remote_id) {
            (Some(left), Some(right)) => left == right,
            _ => {
                existing.folder_path == copy.folder_path && existing.remote_name == copy.remote_name
            }
        });
    if !duplicate {
        copies.push(copy);
    }
}

fn copies_from_index(index: &RemoteIndex, name: &str, size: u64) -> Vec<library::LibraryCopyView> {
    let mut copies = Vec::new();
    if let Some(files) = index.by_name_size.get(&(name.to_string(), size)) {
        for file in files {
            if let Some(copy) = library::LibraryCopyView::from_remote_file(file) {
                push_copy(&mut copies, copy);
            }
        }
    }
    copies
}

fn prune_library(state: &AppState, index: &RemoteIndex) -> Result<(), String> {
    let mut library = state.library.write().unwrap();
    if library.prune_missing(&index.ids, &index.paths) {
        library.save()?;
    }
    Ok(())
}

#[derive(Serialize)]
pub struct PhotoLibraryMatch {
    pub asset_id: String,
    pub sha256: Option<String>,
    pub copies: Vec<library::LibraryCopyView>,
    pub in_target: bool,
}

#[derive(Serialize)]
pub struct PathCheck {
    pub path: String,
    pub sha256: Option<String>,
    pub copies: Vec<library::LibraryCopyView>,
    pub in_target: bool,
    pub is_file: bool,
}

#[tauri::command]
pub fn library_status(state: State<'_, AppState>) -> library::LibraryStats {
    state.library.read().unwrap().stats()
}

#[tauri::command]
pub async fn library_sync(
    app: AppHandle,
    state: State<'_, AppState>,
    force: Option<bool>,
) -> Result<library::LibraryStats, String> {
    let cli_path = state.cli.path_or_err()?;
    ensure_session(&app, &cli_path).await?;
    ensure_library_synced(&app, &state, &cli_path, force.unwrap_or(false)).await?;
    Ok(state.library.read().unwrap().stats())
}

#[tauri::command]
pub async fn library_reconcile(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<library::LibraryStats, String> {
    let cli_path = state.cli.path_or_err()?;
    ensure_session(&app, &cli_path).await?;
    ensure_library_synced(&app, &state, &cli_path, true).await?;
    let files = fetch_all_files(&app, &state, &cli_path, true).await?;
    let index = RemoteIndex::new(&files);
    prune_library(&state, &index)?;

    // Publish the cleaned state so other devices don't keep merging copies
    // that no longer exist.
    let _guard = state.library_sync_lock.lock().await;
    let folder_id = {
        let library = state.library.read().unwrap();
        library.folder_id()
    };
    if let Some(folder_id) = folder_id {
        upload_own_snapshot(&app, &state, &cli_path, folder_id, None).await?;
    }
    Ok(state.library.read().unwrap().stats())
}

/// Forget this device's library: delete its snapshots from O2 and clear the
/// local hashes. Snapshots from other devices stay and are merged again on
/// the next sync.
#[tauri::command]
pub async fn library_forget(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<library::LibraryStats, String> {
    let cli_path = state.cli.path_or_err()?;
    let _guard = state.library_sync_lock.lock().await;
    let (folder_id, device_id) = {
        let library = state.library.read().unwrap();
        (library.folder_id(), library.device_id().to_string())
    };

    if let Some(folder_id) = folder_id {
        ensure_session(&app, &cli_path).await?;
        let remote = library::list_snapshots(&app, &cli_path, folder_id).await?;
        for file in library::own_snapshot_files(&remote, &device_id) {
            library::delete_snapshot(&app, &cli_path, file.id).await?;
        }
    }

    library::clear_cached_snapshots(&app)?;
    let mut library = state.library.write().unwrap();
    library.forget();
    library.save()?;
    Ok(library.stats())
}

/// The identity fields shared by `.o2manifest.json` and `.o2split.json`.
#[derive(Deserialize)]
struct ManifestIdentity {
    name: String,
    size: u64,
    sha256: String,
}

fn manifest_identity(raw: &str) -> Option<ManifestIdentity> {
    let manifest: ManifestIdentity = serde_json::from_str(raw).ok()?;
    if manifest.name.trim().is_empty() || manifest.sha256.trim().is_empty() {
        return None;
    }
    Some(manifest)
}

/// Read `.o2manifest.json` and `.o2split.json` files and record their
/// whole-file SHA-256 in the shared library, so uploads made before the
/// library existed can match.
#[tauri::command]
pub async fn library_index_manifests(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<library::LibraryStats, String> {
    let cli_path = state.cli.path_or_err()?;
    ensure_session(&app, &cli_path).await?;
    let _guard = state.library_sync_lock.lock().await;
    let files = fetch_all_files(&app, &state, &cli_path, true).await?;
    let cache_root = transfer::app_cache_dir(&app)?.join("manifests");
    std::fs::create_dir_all(&cache_root).map_err(|error| error.to_string())?;

    let mut uploads = Vec::new();
    let mut seen = HashSet::new();
    for file in &files {
        let Some(logical_name) = file
            .name
            .strip_suffix(".o2manifest.json")
            .or_else(|| file.name.strip_suffix(".o2split.json"))
        else {
            continue;
        };
        if logical_name.is_empty() {
            continue;
        }
        let folder_path = file
            .path
            .as_deref()
            .and_then(|path| Path::new(path).parent())
            .map(|path| normalize_path(&path.to_string_lossy()))
            .unwrap_or_else(|| "/".into());
        if !seen.insert((folder_path.clone(), logical_name.to_string())) {
            continue;
        }
        if state
            .library
            .read()
            .unwrap()
            .has_copy(&folder_path, logical_name)
        {
            continue;
        }

        let destination = cache_root.join(format!("{}.json", file.id));
        let args = vec![
            "download".to_string(),
            file.id.to_string(),
            "-o".to_string(),
            destination.to_string_lossy().to_string(),
        ];
        if cli::run(&app, &cli_path, &args).await.is_err() {
            continue;
        }
        let raw = match std::fs::read_to_string(&destination) {
            Ok(raw) => raw,
            Err(_) => continue,
        };
        let _ = std::fs::remove_file(&destination);
        let Some(manifest) = manifest_identity(&raw) else {
            continue;
        };
        uploads.push(NewUpload {
            sha256: manifest.sha256,
            size: manifest.size,
            name: manifest.name.clone(),
            captured_at: None,
            source: "manifest".into(),
            folder_id: None,
            folder_path,
            remote_id: Some(file.id),
            remote_name: manifest.name,
        });
    }

    if !uploads.is_empty() {
        {
            let mut library = state.library.write().unwrap();
            library.add_uploads(uploads);
            library.save()?;
        }
        let folder_id = {
            let library = state.library.read().unwrap();
            library.folder_id()
        };
        let folder_id = match folder_id {
            Some(folder_id) => folder_id,
            None => library::ensure_folder(&app, &cli_path).await?,
        };
        upload_own_snapshot(&app, &state, &cli_path, folder_id, None).await?;
    }

    Ok(state.library.read().unwrap().stats())
}

#[tauri::command]
pub async fn library_match_photos(
    app: AppHandle,
    state: State<'_, AppState>,
    assets: Vec<photos::PhotoAsset>,
    folder_id: Option<u64>,
    folder_path: String,
) -> Result<Vec<PhotoLibraryMatch>, String> {
    if assets.is_empty() {
        return Ok(Vec::new());
    }
    let cli_path = state.cli.path_or_err()?;
    ensure_session(&app, &cli_path).await?;
    ensure_library_synced(&app, &state, &cli_path, false).await?;
    let files = fetch_all_files(&app, &state, &cli_path, false).await?;
    let index = RemoteIndex::new(&files);
    prune_library(&state, &index)?;

    let target = normalize_path(&folder_path);
    let mut matches = Vec::new();
    for asset in assets {
        let mut copies = Vec::new();
        let mut sha256 = None;
        {
            let library = state.library.read().unwrap();
            for entry in library.lookup_fingerprint(&asset.name, asset.size, asset.date) {
                sha256 = Some(entry.sha256.clone());
                for copy in &entry.copies {
                    push_copy(&mut copies, library::LibraryCopyView::from_copy(copy));
                }
            }
        }
        for copy in copies_from_index(&index, &asset.name, asset.size) {
            push_copy(&mut copies, copy);
        }
        if copies.is_empty() {
            continue;
        }
        let in_target = copies.iter().any(|copy| {
            copy.folder_path == target || folder_id.is_some_and(|id| copy.folder_id == Some(id))
        });
        matches.push(PhotoLibraryMatch {
            asset_id: asset.id,
            sha256,
            copies,
            in_target,
        });
    }
    Ok(matches)
}

/// Progress for the duplicate check, shown in the transfers popover while a
/// selection is being hashed before uploading. Without `id` (an older
/// frontend) it does nothing.
fn emit_check_progress(
    app: &AppHandle,
    id: Option<&str>,
    current: u64,
    total: u64,
    message: String,
) {
    let Some(id) = id else {
        return;
    };
    let _ = app.emit(
        "transfer-progress",
        TransferProgress {
            id: id.to_string(),
            kind: "library".into(),
            name: "Comprobando duplicados".into(),
            phase: "hash".into(),
            current,
            total,
            bytes_done: 0,
            bytes_total: 0,
            message,
        },
    );
}

#[tauri::command]
pub async fn library_check_paths(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
    folder_id: Option<u64>,
    folder_path: String,
    progress_id: Option<String>,
) -> Result<Vec<PathCheck>, String> {
    let progress = progress_id.as_deref();
    let total_paths = paths.len() as u64;
    // Folder uploads and non-media files use the normal target-folder check.
    // The whole-account listing is only worth it when media is present; the
    // opt-in non-media hash lookup uses the shared library instead.
    let hash_non_media = state.settings.read().unwrap().hash_non_media;
    let mut file_flags = Vec::with_capacity(paths.len());
    let mut has_media_file = false;
    let mut has_file = false;
    for raw_path in &paths {
        let metadata = std::fs::metadata(raw_path).ok();
        let is_file = metadata.as_ref().is_some_and(|meta| meta.is_file());
        if is_file {
            has_file = true;
            let name = Path::new(raw_path)
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            if camera::media_kind(&name).is_some() {
                has_media_file = true;
            }
        }
        file_flags.push(is_file);
    }
    if !has_file || (!has_media_file && !hash_non_media) {
        return Ok(paths
            .into_iter()
            .zip(file_flags)
            .map(|(path, is_file)| PathCheck {
                path,
                sha256: None,
                copies: Vec::new(),
                in_target: false,
                is_file,
            })
            .collect());
    }

    let cli_path = state.cli.path_or_err()?;
    emit_check_progress(&app, progress, 0, total_paths, "Buscando en O2…".into());
    ensure_session(&app, &cli_path).await?;
    ensure_library_synced(&app, &state, &cli_path, false).await?;
    let index = if has_media_file {
        let files = fetch_all_files(&app, &state, &cli_path, false).await?;
        let index = RemoteIndex::new(&files);
        prune_library(&state, &index)?;
        Some(index)
    } else {
        None
    };

    let target = normalize_path(&folder_path);
    let mut checks = Vec::new();
    for (index_in_batch, raw_path) in paths.into_iter().enumerate() {
        let current = index_in_batch as u64 + 1;
        let path = PathBuf::from(&raw_path);
        let metadata = std::fs::metadata(&path).ok();
        let is_file = metadata.as_ref().is_some_and(|meta| meta.is_file());
        if !is_file {
            checks.push(PathCheck {
                path: raw_path,
                sha256: None,
                copies: Vec::new(),
                in_target: false,
                is_file: false,
            });
            continue;
        }
        let size = metadata.map(|meta| meta.len()).unwrap_or(0);
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        emit_check_progress(
            &app,
            progress,
            current,
            total_paths,
            format!("Comprobando {name} ({current}/{total_paths})…"),
        );
        let mut copies = Vec::new();
        {
            let library = state.library.read().unwrap();
            for entry in library.lookup_fingerprint(&name, size, None) {
                for copy in &entry.copies {
                    push_copy(&mut copies, library::LibraryCopyView::from_copy(copy));
                }
            }
        }
        if let Some(index) = &index {
            for copy in copies_from_index(index, &name, size) {
                push_copy(&mut copies, copy);
            }
        }

        let mut sha256 = None;
        let same_size_exists = index.as_ref().is_some_and(|index| index.has_size(size))
            || state.library.read().unwrap().has_size(size);
        if copies.is_empty()
            && transfer::should_hash_before_upload(size, same_size_exists)
            && (camera::media_kind(&name).is_some() || hash_non_media)
        {
            // Photos and videos are always hashed when their name+size is new.
            // With the opt-in, do the same for any other file type so renamed
            // copies in the shared library are found before uploading.
            let mut last = Instant::now();
            if let Ok(hash) = transfer::sha256_file_with_progress(&path, |done| {
                if done < size && last.elapsed() < Duration::from_millis(500) {
                    return;
                }
                last = Instant::now();
                let percent = done * 100 / size.max(1);
                emit_check_progress(
                    &app,
                    progress,
                    current,
                    total_paths,
                    format!("Comprobando {name} ({current}/{total_paths})… {percent}%"),
                );
            })
            .await
            {
                if let Some(entry) = state.library.read().unwrap().lookup_hash(&hash) {
                    for copy in &entry.copies {
                        push_copy(&mut copies, library::LibraryCopyView::from_copy(copy));
                    }
                }
                sha256 = Some(hash);
            }
        }
        let in_target = copies.iter().any(|copy| {
            copy.folder_path == target || folder_id.is_some_and(|id| copy.folder_id == Some(id))
        });
        checks.push(PathCheck {
            path: raw_path,
            sha256,
            copies,
            in_target,
            is_file: true,
        });
    }
    Ok(checks)
}

fn year_folders(files: &[LogicalFile]) -> Vec<VirtualFolder> {
    use std::collections::BTreeMap;
    let mut counts: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for file in files {
        let year = camera::year_of(file.date.as_deref());
        let entry = counts.entry(year).or_default();
        entry.0 += 1;
        entry.1 += file.size;
    }

    let mut years: Vec<(String, u64, u64)> = counts
        .into_iter()
        .map(|(year, (count, size))| (year, count, size))
        .collect();
    // Newest first, "Sin fecha" at the end.
    years.sort_by(|a, b| {
        if a.0 == "Sin fecha" {
            std::cmp::Ordering::Greater
        } else if b.0 == "Sin fecha" {
            std::cmp::Ordering::Less
        } else {
            b.0.cmp(&a.0)
        }
    });

    let mut folders = Vec::with_capacity(years.len() + 1);
    folders.push(VirtualFolder {
        key: "camera/all".into(),
        name: "Todo".into(),
        files: files.len() as u64,
        size: files.iter().map(|f| f.size).sum(),
        kind: "all".into(),
        thumbnail: None,
        location: None,
        date: None,
        artist: None,
    });
    for (year, count, size) in years {
        let key = if year == "Sin fecha" {
            "camera/unknown".to_string()
        } else {
            format!("camera/{year}")
        };
        folders.push(VirtualFolder {
            key,
            name: year,
            files: count,
            size,
            kind: "year".into(),
            thumbnail: None,
            location: None,
            date: None,
            artist: None,
        });
    }
    folders
}

/// The account's music or documents (`kind`), wherever they are, as
/// files of their own. Música also has its albums and its folders.
async fn media_files(
    app: &AppHandle,
    cli_path: &Path,
    kind: &str,
) -> Result<LogicalListing, String> {
    if !matches!(kind, "music" | "documents") {
        return Err(format!("Vista desconocida: {kind}"));
    }
    if kind == "documents" {
        let args = vec!["media".to_string(), kind.to_string(), "--json".to_string()];
        let files: Vec<RemoteFile> = cli::run_json(app, cli_path, &args).await?;
        return Ok(media_listing(files));
    }
    let songs = shown_songs(cli_songs(app, cli_path).await?);
    let mut folders = music_albums(&songs);
    let files: Vec<RemoteFile> = songs.into_iter().map(|song| song.file).collect();
    folders.extend(music_folders(&files));
    let mut listing = media_listing(files);
    listing.virtual_folders = folders;
    Ok(listing)
}

/// A song as `o2cloud media music --json` lists it: the file, and its tags.
#[derive(serde::Deserialize)]
struct CliSong {
    #[serde(flatten)]
    file: RemoteFile,
    #[serde(flatten)]
    tags: SongTags,
}

async fn cli_songs(app: &AppHandle, cli_path: &Path) -> Result<Vec<CliSong>, String> {
    let args = vec![
        "media".to_string(),
        "music".to_string(),
        "--json".to_string(),
    ];
    cli::run_json(app, cli_path, &args).await
}

/// The songs to show: not those in hidden folders, as `shown_media`.
fn shown_songs(songs: Vec<CliSong>) -> Vec<CliSong> {
    songs
        .into_iter()
        .filter(|song| is_shown(&song.file))
        .collect()
}

/// Between an album's artist and its name in its key.
const ALBUM_SEP: char = '\u{1f}';
/// A compilation's artist in its key.
const VARIOUS_ARTISTS: &str = "varios artistas";

/// Each song's album, as its key: `<artist>\u{1f}<album>`, in lower case.
/// An album is its songs with that album tag and artist, in any folder, so
/// two «Greatest Hits» stay apart. In a folder, an album's songs by several
/// artists with no track number twice are a compilation, by its album
/// alone (`varios artistas\u{1f}<album>`); songs with no artist go with the
/// album's only artist there. Songs with no album are one, «Sin álbum»
/// (`\u{1f}`). Tags have no control characters, so keys can't mix.
fn album_keys(songs: &[CliSong]) -> Vec<String> {
    let fold = |tag: &Option<String>| tag.as_deref().unwrap_or("").to_lowercase();
    let folder = |song: &CliSong| folder_of(song.file.path.as_deref().unwrap_or("/")).to_string();
    // Each album name in each folder: its artists, and whether two of them
    // have the same track number (then they are two albums, not one).
    #[derive(Default)]
    struct Shelf {
        artists: HashSet<String>,
        tracks: HashMap<u32, String>,
        two_albums: bool,
    }
    let mut shelves: HashMap<(String, String), Shelf> = HashMap::new();
    for song in songs {
        let (album, artist) = (fold(&song.tags.album), fold(&song.tags.artist));
        if album.is_empty() || artist.is_empty() {
            continue;
        }
        let shelf = shelves.entry((album, folder(song))).or_default();
        if let Some(track) = song.tags.track {
            let first = shelf.tracks.entry(track).or_insert_with(|| artist.clone());
            shelf.two_albums |= *first != artist;
        }
        shelf.artists.insert(artist);
    }
    songs
        .iter()
        .map(|song| {
            let album = fold(&song.tags.album);
            if album.is_empty() {
                return ALBUM_SEP.to_string();
            }
            let artist = match shelves.get(&(album.clone(), folder(song))) {
                Some(shelf) if shelf.artists.len() > 1 && !shelf.two_albums => {
                    VARIOUS_ARTISTS.to_string()
                }
                Some(shelf) if shelf.artists.len() == 1 => {
                    shelf.artists.iter().next().cloned().unwrap_or_default()
                }
                _ => fold(&song.tags.artist),
            };
            format!("{artist}{ALBUM_SEP}{album}")
        })
        .collect()
}

/// Música by album (see `album_keys`): one entry per album, named as its
/// first song spells it, with how many songs, their size and its artist.
/// By name, «Sin álbum» last. Each opens as `media:music-album:<key>`.
fn music_albums(songs: &[CliSong]) -> Vec<VirtualFolder> {
    use std::collections::BTreeMap;
    struct Album<'a> {
        key: &'a str,
        name: Option<&'a str>,
        artist: Option<&'a str>,
        files: u64,
        size: u64,
    }
    let keys = album_keys(songs);
    let mut albums: BTreeMap<(bool, &str, &str), Album> = BTreeMap::new();
    for (song, key) in songs.iter().zip(&keys) {
        let (artist, album) = key.split_once(ALBUM_SEP).unwrap_or(("", key));
        let entry = albums
            .entry((album.is_empty(), album, artist))
            .or_insert(Album {
                key,
                name: None,
                artist: None,
                files: 0,
                size: 0,
            });
        entry.files += 1;
        entry.size += song.file.size.unwrap_or(0);
        entry.name = entry.name.or(song.tags.album.as_deref());
        entry.artist = entry.artist.or(song.tags.artist.as_deref());
    }
    albums
        .into_iter()
        .map(|((no_album, _, artist), album)| VirtualFolder {
            key: format!("media:music-album:{}", album.key),
            name: album.name.unwrap_or("Sin álbum").to_string(),
            files: album.files,
            size: album.size,
            kind: "music-album".into(),
            thumbnail: None,
            location: None,
            date: None,
            artist: if no_album {
                None
            } else if artist == VARIOUS_ARTISTS {
                Some("Varios artistas".into())
            } else {
                album.artist.map(String::from)
            },
        })
        .collect()
}

/// The songs of one of Música's albums (`key`, as `album_keys` makes it).
async fn music_album_files(
    app: &AppHandle,
    cli_path: &Path,
    key: &str,
) -> Result<LogicalListing, String> {
    let songs = shown_songs(cli_songs(app, cli_path).await?);
    Ok(album_listing(songs, key))
}

/// `music_album_files`' listing: by track number, then by name; those
/// without a number last.
fn album_listing(songs: Vec<CliSong>, key: &str) -> LogicalListing {
    let keys = album_keys(&songs);
    let mut tracks = HashMap::new();
    let in_album: Vec<RemoteFile> = songs
        .into_iter()
        .zip(keys)
        .filter(|(_, song_key)| song_key == key)
        .map(|(song, _)| {
            if let Some(track) = song.tags.track {
                tracks.insert(song.file.id, track);
            }
            song.file
        })
        .collect();
    let mut listing = media_listing(in_album);
    // Already by name: the sort keeps that order for the same number.
    listing.files.sort_by_key(|file| {
        file.normal_id
            .and_then(|id| tracks.get(&id).copied())
            .unwrap_or(u32::MAX)
    });
    listing
}

/// The songs in one of Música's folders (`folder`, as `/a/b`; `/` for the
/// root), not in its subfolders.
async fn music_folder_files(
    app: &AppHandle,
    cli_path: &Path,
    folder: &str,
) -> Result<LogicalListing, String> {
    let args = vec![
        "media".to_string(),
        "music".to_string(),
        "--json".to_string(),
    ];
    let files: Vec<RemoteFile> = cli::run_json(app, cli_path, &args).await?;
    let in_folder = files
        .into_iter()
        .filter(|f| folder_of(f.path.as_deref().unwrap_or("/")) == folder)
        .collect();
    Ok(media_listing(in_folder))
}

/// The folder a file is in, from its path: `/a/b/x.mp3` → `/a/b`,
/// `/x.mp3` → `/`.
fn folder_of(path: &str) -> &str {
    match path.rfind('/') {
        None | Some(0) => "/",
        Some(at) => &path[..at],
    }
}

/// Música by folder: one entry per folder holding songs, with how many and
/// their size. Each opens as `media:music-folder:<folder>`.
fn music_folders(files: &[RemoteFile]) -> Vec<VirtualFolder> {
    use std::collections::BTreeMap;
    let mut folders: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    for file in files {
        let folder = folder_of(file.path.as_deref().unwrap_or("/"));
        let entry = folders.entry(folder).or_default();
        entry.0 += 1;
        entry.1 += file.size.unwrap_or(0);
    }
    folders
        .into_iter()
        .map(|(folder, (files, size))| VirtualFolder {
            key: format!("media:music-folder:{folder}"),
            name: if folder == "/" {
                "O2 Cloud (raíz)".into()
            } else {
                folder.trim_start_matches('/').to_string()
            },
            files,
            size,
            kind: "music-folder".into(),
            thumbnail: None,
            location: None,
            date: None,
            artist: None,
        })
        .collect()
}

/// `media_files`' listing: what's in hidden folders (such as this app's
/// own `.o2cloud-gui`) is left out, as the folder views do.
fn media_listing(files: Vec<RemoteFile>) -> LogicalListing {
    let (files, warnings) = transfer::group_logical_files(shown_media(files));
    LogicalListing {
        path: "/".into(),
        folders: Vec::new(),
        files,
        warnings,
        virtual_folders: Vec::new(),
        stale: false,
    }
}

/// The media files to show: not those in hidden folders.
fn shown_media(files: Vec<RemoteFile>) -> Vec<RemoteFile> {
    files.into_iter().filter(is_shown).collect()
}

fn is_shown(file: &RemoteFile) -> bool {
    !file.name.is_empty() && !in_hidden_folder(file.path.as_deref())
}

/// True for a file in a hidden folder, by its path (its own name doesn't
/// count).
fn in_hidden_folder(path: Option<&str>) -> bool {
    path.unwrap_or("")
        .split('/')
        .rev()
        .skip(1)
        .any(|dir| dir.starts_with('.'))
}

/// The account's music for the player, by name, with their tags and
/// albums. Hidden folders are left out, as in the list.
#[tauri::command]
pub async fn music_tracks(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<Track>, String> {
    let cli_path = state.cli.path_or_err()?;
    let args = vec![
        "media".to_string(),
        "music".to_string(),
        "--json".to_string(),
    ];
    let songs: Vec<serde_json::Value> = cli::run_json(&app, &cli_path, &args).await?;
    Ok(tracks_of(songs))
}

fn tracks_of(songs: Vec<serde_json::Value>) -> Vec<Track> {
    // Each song's album, made from the same songs as in the list.
    let listed = shown_songs(
        songs
            .iter()
            .filter_map(|song| CliSong::deserialize(song).ok())
            .collect(),
    );
    let albums: HashMap<u64, String> = listed
        .iter()
        .map(|song| song.file.id)
        .zip(album_keys(&listed))
        .collect();
    songs
        .into_iter()
        .filter_map(|song| {
            let mut track: Track = serde_json::from_value(song).ok()?;
            let hidden = in_hidden_folder(track.path.as_deref());
            // Only O2's own HTTPS URLs reach the page.
            if hidden || !track.url.starts_with("https://cloud.o2online.es/") {
                return None;
            }
            track.album_key = albums.get(&track.id).cloned().unwrap_or_default();
            Some(track)
        })
        .collect()
}

/// The photos of the moment or place `id`, for its slideshow, in the
/// order they were taken. Videos are left out.
#[tauri::command]
pub async fn label_slides(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<Vec<Slide>, String> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Etiqueta no válida".into());
    }
    let cli_path = state.cli.path_or_err()?;
    let args = vec!["labels".to_string(), id, "--json".to_string()];
    let label: SlideSource = cli::run_json(&app, &cli_path, &args).await?;
    Ok(slides_of(label))
}

#[derive(serde::Deserialize)]
struct SlideSource {
    #[serde(default)]
    files: Vec<SlideFile>,
}

#[derive(serde::Deserialize)]
struct SlideFile {
    id: u64,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    date: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    preview: Option<String>,
}

fn slides_of(label: SlideSource) -> Vec<Slide> {
    label
        .files
        .into_iter()
        .filter(|f| f.kind.as_deref() != Some("video"))
        .filter_map(|f| {
            Some(Slide {
                id: f.id,
                name: f.name.unwrap_or_default(),
                date: f.date,
                // Only O2's own HTTPS URLs reach the page.
                preview: f
                    .preview
                    .filter(|u| u.starts_with("https://cloud.o2online.es/"))?,
            })
        })
        .collect()
}

/// One label with its photos, as `o2cloud labels <id> --json` gives it.
#[derive(serde::Deserialize)]
struct LabelWithFiles {
    #[serde(default)]
    files: Vec<RemoteFile>,
}

/// A label as `o2cloud labels --json` describes it.
#[derive(serde::Deserialize)]
struct CliLabel {
    id: u64,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    count: u64,
    #[serde(default)]
    date: Option<String>,
    #[serde(default)]
    thumbnail: Option<String>,
    #[serde(default)]
    location: Option<Location>,
}

/// The moments, places or people (`kind`) O2 has grouped photos into, as
/// virtual folders: `label:<id>` opens one.
async fn label_folders(
    app: &AppHandle,
    cli_path: &Path,
    kind: &str,
) -> Result<LogicalListing, String> {
    let args = vec![
        "labels".to_string(),
        "--kind".to_string(),
        kind.to_string(),
        "--json".to_string(),
    ];
    let labels: Vec<CliLabel> = cli::run_json(app, cli_path, &args).await?;
    Ok(label_listing(kind, labels))
}

/// `label_folders`' listing from the CLI's labels.
fn label_listing(kind: &str, labels: Vec<CliLabel>) -> LogicalListing {
    let mut warnings = Vec::new();
    if labels.is_empty() && kind == "people" {
        warnings.push(
            "O2 agrupa las caras solo si activas el reconocimiento facial en su web \
             (cloud.o2online.es), y tarda un tiempo en hacerlo."
                .to_string(),
        );
    }
    let virtual_folders = labels
        .into_iter()
        .map(|label| {
            let name = label.name.unwrap_or_else(|| label.id.to_string());
            VirtualFolder {
                key: format!("label:{}", label.id),
                name,
                files: label.count,
                size: 0,
                kind: format!("label-{kind}"),
                // The page shows a moment's day next to its name.
                date: label.date.filter(|_| kind == "moments"),
                // Only O2's own HTTPS URLs reach the page.
                thumbnail: label
                    .thumbnail
                    .filter(|u| u.starts_with("https://cloud.o2online.es/")),
                location: label.location,
                artist: None,
            }
        })
        .collect();
    LogicalListing {
        path: "/".into(),
        folders: Vec::new(),
        files: Vec::new(),
        warnings,
        virtual_folders,
        stale: false,
    }
}

/// The photos and videos of the label `id`, as files of their own (they
/// stay where they are in O2's folders).
async fn label_files(app: &AppHandle, cli_path: &Path, id: &str) -> Result<LogicalListing, String> {
    let args = vec!["labels".to_string(), id.to_string(), "--json".to_string()];
    let label: LabelWithFiles = cli::run_json(app, cli_path, &args).await?;
    let named = label
        .files
        .into_iter()
        .filter(|f| !f.name.is_empty())
        .collect();
    let (files, warnings) = transfer::group_logical_files(named);
    Ok(LogicalListing {
        path: "/".into(),
        folders: Vec::new(),
        files,
        warnings,
        virtual_folders: Vec::new(),
        stale: false,
    })
}

#[tauri::command]
pub async fn list_dir(
    app: AppHandle,
    state: State<'_, AppState>,
    folder_id: Option<u64>,
    path: Option<String>,
    virtual_path: Option<String>,
    fresh: Option<bool>,
) -> Result<LogicalListing, String> {
    let cli_path = state.cli.path_or_err()?;
    let settings = state.settings.read().unwrap().clone();

    // What O2 groups by itself (moments, places, people): asked for to the
    // CLI's `labels`, not taken from the folder listing. Like folders, never
    // with a session that would open the CLI's login window.
    if virtual_path
        .as_deref()
        .is_some_and(|v| v.starts_with("label") || v.starts_with("media:"))
    {
        ensure_session(&app, &cli_path).await?;
    }
    if let Some(kind) = virtual_path
        .as_deref()
        .and_then(|v| v.strip_prefix("labels:"))
    {
        return label_folders(&app, &cli_path, kind).await;
    }
    if let Some(id) = virtual_path
        .as_deref()
        .and_then(|v| v.strip_prefix("label:"))
    {
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
            return Err("Etiqueta no válida".into());
        }
        return label_files(&app, &cli_path, id).await;
    }
    if let Some(folder) = virtual_path
        .as_deref()
        .and_then(|v| v.strip_prefix("media:music-folder:"))
    {
        return music_folder_files(&app, &cli_path, folder).await;
    }
    if let Some(key) = virtual_path
        .as_deref()
        .and_then(|v| v.strip_prefix("media:music-album:"))
    {
        return music_album_files(&app, &cli_path, key).await;
    }
    if let Some(kind) = virtual_path
        .as_deref()
        .and_then(|v| v.strip_prefix("media:"))
    {
        return media_files(&app, &cli_path, kind).await;
    }

    // Virtual views always work on the root listing; nothing is moved in O2.
    let (base_folder, base_path) = if virtual_path.is_some() {
        (None, Some("/".to_string()))
    } else {
        (folder_id, path.clone())
    };
    let fetched = tokio::time::timeout(
        LISTING_TIMEOUT,
        fetch_listing(
            &app,
            &cli_path,
            base_folder,
            base_path.as_deref(),
            fresh.unwrap_or(false),
        ),
    )
    .await
    .map_err(|_| {
        "O2 está tardando demasiado en listar la carpeta. Vuelve a intentarlo.".to_string()
    })??;
    let CachedListing { mut listing, stale } = fetched;
    // The shared upload library lives in a hidden folder in O2; keep it out
    // of the Finder-style listing and the virtual camera view.
    listing
        .folders
        .retain(|folder| !library::is_internal_name(&folder.name));
    listing
        .files
        .retain(|file| !library::is_internal_name(&file.name));
    let (mut files, mut warnings) = transfer::group_logical_files(listing.files);
    let mut virtual_folders = Vec::new();

    match virtual_path.as_deref() {
        Some("camera") => {
            files.retain(camera::is_camera_capture);
            virtual_folders = year_folders(&files);
            files.clear();
        }
        Some(key) if key.starts_with("camera/") => {
            files.retain(camera::is_camera_capture);
            let selected = key.trim_start_matches("camera/");
            if selected != "all" {
                let year = if selected == "unknown" {
                    "Sin fecha".to_string()
                } else {
                    selected.to_string()
                };
                files.retain(|f| camera::year_of(f.date.as_deref()) == year);
            }
        }
        _ => {
            let at_root = base_folder.is_none()
                && base_path
                    .as_deref()
                    .map(normalize_path)
                    .as_deref()
                    .map(|p| p == "/")
                    .unwrap_or(true);
            if at_root && settings.group_camera {
                let (camera_files, other): (Vec<_>, Vec<_>) =
                    files.into_iter().partition(camera::is_camera_capture);
                if !camera_files.is_empty() {
                    virtual_folders.push(VirtualFolder {
                        key: "camera".into(),
                        name: "Cámara".into(),
                        files: camera_files.len() as u64,
                        size: camera_files.iter().map(|f| f.size).sum(),
                        kind: "camera".into(),
                        thumbnail: None,
                        location: None,
                        date: None,
                        artist: None,
                    });
                }
                files = other;
            }
        }
    }

    if virtual_path.is_some() {
        warnings.clear();
    }

    Ok(LogicalListing {
        path: if listing.path.is_empty() {
            path.unwrap_or_else(|| "/".into())
        } else {
            listing.path
        },
        folders: if virtual_path.is_some() {
            Vec::new()
        } else {
            listing.folders
        },
        files,
        warnings,
        virtual_folders,
        stale,
    })
}

/// Create `name` in `parent_path`, or find it if it already exists. The
/// flag says whether it was created (so it is known to be empty).
async fn create_folder_inner(
    app: &AppHandle,
    cli_path: &Path,
    parent_path: &str,
    name: &str,
) -> Result<(RemoteFolder, bool), String> {
    let parent_path = normalize_path(parent_path);
    let target = join_path(&parent_path, name);
    // `-p` reports an existing folder (with its id) instead of failing.
    let args = vec![
        "mkdir".to_string(),
        "-p".to_string(),
        "--json".to_string(),
        target,
    ];
    let output = cli::run(app, cli_path, &args).await?;
    let created: MkdirOutput = cli::parse_json(&output)?;
    let folder = RemoteFolder {
        id: created.id,
        name: name.to_string(),
        files: 0,
        size: 0,
    };
    Ok((folder, created.created))
}

#[derive(Serialize)]
pub struct CreatedFolder {
    pub folder: RemoteFolder,
    /// False when the folder already existed, so it may not be empty.
    pub created: bool,
}

#[tauri::command]
pub async fn create_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    parent_id: Option<u64>,
    parent_path: String,
    name: String,
) -> Result<CreatedFolder, String> {
    let cli_path = state.cli.path_or_err()?;
    ensure_session(&app, &cli_path).await?;
    let (folder, created) = create_folder_inner(&app, &cli_path, &parent_path, &name).await?;
    let parent_path = normalize_path(&parent_path);

    // A folder created by this command is empty, so opening it does not need
    // another whole-account `ls` (up to a minute on a large account).
    let folder_path = join_path(&parent_path, &name);
    if created {
        let empty = LsResponse {
            path: folder_path.clone(),
            folders: Vec::new(),
            files: Vec::new(),
        };
        state
            .listings
            .put(format!("id:{}", folder.id), empty.clone());
        state.listings.put(format!("path:{folder_path}"), empty);
    }

    // Update the parent's cached listing instead of clearing every listing:
    // a new folder only changes the folder it was created in.
    let mut parent_keys = vec![format!("path:{parent_path}")];
    if let Some(id) = parent_id {
        parent_keys.push(format!("id:{id}"));
    }
    for key in parent_keys {
        state.listings.update(&key, |listing| {
            if !listing
                .folders
                .iter()
                .any(|existing| existing.id == folder.id)
            {
                listing.folders.push(folder.clone());
            }
        });
    }

    Ok(CreatedFolder { folder, created })
}

#[derive(Serialize)]
pub struct UploadSummary {
    pub uploaded: u64,
    /// Already in O2 with the same name and size, so not uploaded again.
    pub skipped: u64,
    pub failed: Vec<String>,
    /// Files O2 already has with the same name and another size, as the
    /// CLI described them (`local`, `name`, `size`, `modified`, `cloud`):
    /// not uploaded, for the user to choose to overwrite or skip.
    pub conflicts: Vec<serde_json::Value>,
    /// The uploads succeeded but the shared library could not be updated.
    pub library_warning: Option<String>,
}

impl UploadSummary {
    /// Count a failed upload of `local` (shown as `label`): a conflict if
    /// that's what it was, with the path to upload again if asked.
    fn fail(&mut self, label: &str, local: Option<&str>, err: &str) {
        match (transfer::conflict_of(err), local) {
            (Some(mut conflict), Some(local)) => {
                conflict["local"] = local.into();
                self.conflicts.push(conflict);
            }
            (Some(conflict), None) => self.failed.push(format!(
                "{label}: {}",
                transfer::conflict_message(&conflict)
            )),
            (None, _) => self.failed.push(format!("{label}: {err}")),
        }
    }

    fn add(&mut self, outcome: &UploadOutcome) {
        match outcome {
            UploadOutcome::Uploaded => self.uploaded += 1,
            UploadOutcome::Skipped => self.skipped += 1,
        }
    }
}

fn is_hidden(entry: &walkdir::DirEntry) -> bool {
    if entry
        .file_name()
        .to_str()
        .is_some_and(|s| s.starts_with('.'))
    {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
        if let Ok(metadata) = entry.metadata() {
            let attributes = metadata.file_attributes();
            if attributes & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0 {
                return true;
            }
        }
    }
    false
}

/// A remote folder being uploaded into. `existing` is filled the first
/// time a file goes into it; a folder this upload created starts empty.
/// `logical` groups completed chunks under their logical file name, so a
/// Photos item can be skipped before exporting it from iCloud.
struct UploadTarget {
    id: Option<u64>,
    path: String,
    existing: Option<ExistingFiles>,
    logical: Option<ExistingFiles>,
}

impl UploadTarget {
    async fn existing(&mut self, app: &AppHandle, cli_path: &Path) -> &ExistingFiles {
        if self.existing.is_none() {
            // A stale disk listing can miss files uploaded elsewhere; ask O2
            // before deciding whether a file is already in the target folder.
            let mut listing = fetch_listing(app, cli_path, self.id, Some(&self.path), false).await;
            if listing.as_ref().is_ok_and(|listing| listing.stale) {
                listing = fetch_listing(app, cli_path, self.id, Some(&self.path), true).await;
            }
            // Without a listing nothing is skipped; the upload still goes.
            let files = listing
                .map(|listing| listing.listing.files)
                .unwrap_or_default();
            self.logical = Some(transfer::logical_files_existing(&files));
            self.existing = Some(transfer::existing_files(&files));
        }
        self.existing.as_ref().unwrap()
    }

    fn has_logical(&self, name: &str, size: u64) -> bool {
        self.logical
            .as_ref()
            .is_some_and(|existing| transfer::already_uploaded(existing, name, size))
    }
}

/// Uploads change folder counts in many places, but clearing the whole
/// listing cache makes the next root open pay for the whole-account `ls`
/// again. Keep the saved listings and mark them stale: the UI shows them at
/// once and refreshes in the background.
fn invalidate_after_upload(state: &AppState) {
    state.listings.mark_all_stale();
    state.all_files.write().unwrap().at = None;
}

fn new_upload_from_receipt(
    receipt: &transfer::UploadReceipt,
    name: String,
    captured_at: Option<i64>,
    source: &str,
    folder_id: Option<u64>,
    folder_path: String,
) -> Option<NewUpload> {
    if !matches!(receipt.outcome, UploadOutcome::Uploaded) {
        return None;
    }
    Some(NewUpload {
        sha256: receipt.sha256.clone()?,
        size: receipt.size,
        name,
        captured_at,
        source: source.to_string(),
        folder_id,
        folder_path,
        remote_id: receipt.remote_id,
        remote_name: receipt.remote_name.clone(),
    })
}

fn library_copy_folder(state: &AppState, sha256: &str) -> Option<String> {
    state
        .library
        .read()
        .unwrap()
        .lookup_hash(sha256)
        .and_then(|entry| entry.copies.first().map(|copy| copy.folder_path.clone()))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn upload_paths(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
    folder_id: Option<u64>,
    folder_path: String,
    transfer_ids: Option<Vec<String>>,
    known_hashes: Option<Vec<Option<String>>>,
    skip_known: Option<bool>,
    on_conflict: Option<String>,
) -> Result<UploadSummary, String> {
    // What the user chose for files O2 has with another size: asked about
    // (reported in the summary) unless they said to overwrite or skip.
    let on_conflict = match on_conflict.as_deref() {
        Some("overwrite") => transfer::OnConflict::Overwrite,
        Some("skip") => transfer::OnConflict::Skip,
        _ => transfer::OnConflict::Ask,
    };
    let cli_path = state.cli.path_or_err()?;
    // The frontend already shows each path as queued under these IDs, so
    // the rows are there while this waits for earlier uploads.
    let transfer_ids = transfer_ids.unwrap_or_default();
    ensure_session(&app, &cli_path).await?;
    let _one_at_a_time = state.upload_lock.lock().await;
    let mut summary = UploadSummary {
        uploaded: 0,
        skipped: 0,
        failed: Vec::new(),
        conflicts: Vec::new(),
        library_warning: None,
    };
    let mut library_uploads = Vec::new();
    let skip_known = skip_known.unwrap_or(false);

    let mut target = UploadTarget {
        id: folder_id,
        path: normalize_path(&folder_path),
        existing: None,
        logical: None,
    };

    for (index, raw_path) in paths.into_iter().enumerate() {
        let known_sha256 = known_hashes
            .as_ref()
            .and_then(|hashes| hashes.get(index))
            .and_then(|hash| hash.as_deref());
        let transfer_id = transfer_ids
            .get(index)
            .cloned()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let path = PathBuf::from(&raw_path);
        let display_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| raw_path.clone());
        let meta = match std::fs::metadata(&path) {
            Ok(meta) => meta,
            Err(err) => {
                transfer::emit_failed(
                    &app,
                    &transfer_id,
                    "upload",
                    &display_name,
                    &err.to_string(),
                );
                summary.failed.push(format!("{raw_path}: {err}"));
                continue;
            }
        };

        if meta.is_file() {
            if skip_known
                && let Some(hash) = known_sha256
                && let Some(folder) = library_copy_folder(&state, hash)
            {
                let size = meta.len();
                let _ = app.emit(
                    "transfer-progress",
                    TransferProgress {
                        id: transfer_id.clone(),
                        kind: "upload".into(),
                        name: display_name.clone(),
                        phase: "done".into(),
                        current: 1,
                        total: 1,
                        bytes_done: size,
                        bytes_total: size,
                        message: format!("Ya estaba en O2 en {folder}; no se vuelve a subir"),
                    },
                );
                summary.skipped += 1;
                continue;
            }
            let existing = target.existing(&app, &cli_path).await;
            match transfer::upload_one(
                &app,
                &cli_path,
                &path,
                folder_id,
                transfer::APP_CHUNK_SIZE,
                &transfer_id,
                existing,
                known_sha256,
                on_conflict,
            )
            .await
            {
                Ok(receipt) => {
                    summary.add(&receipt.outcome);
                    if let Some(upload) = new_upload_from_receipt(
                        &receipt,
                        display_name,
                        None,
                        "file",
                        folder_id,
                        target.path.clone(),
                    ) {
                        library_uploads.push(upload);
                    }
                }
                Err(err) => summary.fail(&raw_path, Some(&raw_path), &err),
            }
            continue;
        }

        if !meta.is_dir() {
            let err = "no es un archivo ni una carpeta";
            transfer::emit_failed(&app, &transfer_id, "upload", &display_name, err);
            summary.failed.push(format!("{raw_path}: {err}"));
            continue;
        }

        let root_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("carpeta")
            .to_string();
        let folder_transfer_id = transfer_id;
        let files_in_folder = WalkDir::new(&path)
            .min_depth(1)
            .into_iter()
            .filter_entry(|e| !is_hidden(e))
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .count() as u64;
        let mut folder_done = 0u64;
        let folder_progress = |done: u64, phase: &str, message: String| {
            let _ = app.emit(
                "transfer-progress",
                TransferProgress {
                    id: folder_transfer_id.clone(),
                    kind: "upload".into(),
                    name: root_name.clone(),
                    phase: phase.into(),
                    current: done,
                    total: files_in_folder,
                    bytes_done: 0,
                    bytes_total: 0,
                    message,
                },
            );
        };
        folder_progress(0, "scan", format!("0 de {files_in_folder} archivos"));
        let failed_before = summary.failed.len();
        let (root_folder, created) =
            match create_folder_inner(&app, &cli_path, &target.path, &root_name).await {
                Ok(found) => found,
                Err(err) => {
                    folder_progress(0, "error", err.clone());
                    summary.failed.push(format!("{raw_path}: {err}"));
                    continue;
                }
            };

        let mut remote: HashMap<PathBuf, UploadTarget> = HashMap::new();
        remote.insert(
            path.clone(),
            UploadTarget {
                id: Some(root_folder.id),
                path: join_path(&target.path, &root_name),
                existing: created.then(ExistingFiles::new),
                logical: created.then(ExistingFiles::new),
            },
        );

        for entry in WalkDir::new(&path)
            .min_depth(1)
            .into_iter()
            .filter_entry(|e| !is_hidden(e))
        {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    summary.failed.push(format!("{raw_path}: {err}"));
                    continue;
                }
            };
            let local = entry.path().to_path_buf();
            let Some(parent) = local.parent() else {
                continue;
            };
            let Some(parent_target) = remote.get_mut(parent) else {
                continue;
            };

            if entry.file_type().is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                let parent_path = parent_target.path.clone();
                match create_folder_inner(&app, &cli_path, &parent_path, &name).await {
                    Ok((folder, created)) => {
                        remote.insert(
                            local,
                            UploadTarget {
                                id: Some(folder.id),
                                path: join_path(&parent_path, &name),
                                existing: created.then(ExistingFiles::new),
                                logical: created.then(ExistingFiles::new),
                            },
                        );
                    }
                    Err(err) => summary.failed.push(format!("{}: {err}", local.display())),
                }
            } else if entry.file_type().is_file() {
                // Named after the folder's, so the frontend shows it under it.
                let transfer_id = format!("{folder_transfer_id}/{}", uuid::Uuid::new_v4());
                let parent_id = parent_target.id;
                let parent_path = parent_target.path.clone();
                let existing = parent_target.existing(&app, &cli_path).await;
                match transfer::upload_one(
                    &app,
                    &cli_path,
                    &local,
                    parent_id,
                    transfer::APP_CHUNK_SIZE,
                    &transfer_id,
                    existing,
                    None,
                    on_conflict,
                )
                .await
                {
                    Ok(receipt) => {
                        summary.add(&receipt.outcome);
                        if let Some(upload) = new_upload_from_receipt(
                            &receipt,
                            local
                                .file_name()
                                .map(|name| name.to_string_lossy().to_string())
                                .unwrap_or_default(),
                            None,
                            "file",
                            parent_id,
                            parent_path,
                        ) {
                            library_uploads.push(upload);
                        }
                    }
                    Err(err) => {
                        let label = local.display().to_string();
                        summary.fail(&label, Some(&label), &err)
                    }
                }
                folder_done += 1;
                folder_progress(
                    folder_done,
                    "upload",
                    format!("{folder_done} de {files_in_folder} archivos"),
                );
            }
        }

        let folder_failed = summary.failed.len() - failed_before;
        let (phase, message) = if folder_failed == 0 {
            (
                "done",
                format!("Carpeta completada ({folder_done} archivos)"),
            )
        } else {
            (
                "error",
                format!("{folder_failed} de {files_in_folder} archivos han fallado"),
            )
        };
        folder_progress(folder_done, phase, message);
    }

    // The list shows the new files without waiting for this.
    record_library_in_background(&app, &cli_path, library_uploads);
    invalidate_after_upload(&state);
    Ok(summary)
}

#[derive(Serialize)]
pub struct PhotosAccess {
    pub available: bool,
    pub status: String,
}

#[tauri::command]
pub fn photos_access_status() -> PhotosAccess {
    PhotosAccess {
        available: photos::available(),
        status: photos::authorization_status(),
    }
}

#[tauri::command]
pub async fn photos_request_access() -> Result<PhotosAccess, String> {
    let status = photos::request_access().await?;
    Ok(PhotosAccess {
        available: photos::available(),
        status,
    })
}

#[tauri::command]
pub async fn photos_list(
    media_type: Option<String>,
    min_size: Option<u64>,
    album_id: Option<String>,
    favorites_only: Option<bool>,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
    offset: Option<u32>,
    limit: Option<u32>,
) -> Result<photos::PhotoList, String> {
    photos::list(
        media_type.as_deref().unwrap_or("video"),
        min_size.unwrap_or(0),
        album_id,
        favorites_only.unwrap_or(false),
        start_ms,
        end_ms,
        offset.unwrap_or(0),
        limit.unwrap_or(10_000),
    )
    .await
}

#[tauri::command]
pub async fn photos_albums() -> Result<photos::PhotoAlbums, String> {
    photos::albums().await
}

#[tauri::command]
pub async fn photos_thumbnail(
    asset_id: String,
    size: Option<u32>,
) -> Result<photos::PhotoThumbnail, String> {
    photos::thumbnail(&asset_id, size.unwrap_or(64)).await
}

#[tauri::command]
pub fn photos_cancel_export(transfer_id: String) -> bool {
    photos::cancel_export(&transfer_id)
}

fn safe_component(value: &str) -> String {
    let component: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();
    if component.is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        component
    }
}

/// Export selected Photos originals (including iCloud-only ones) to the
/// app cache and upload them through the same queue and chunking as local
/// files. Files already in the target folder with the same name and size
/// are skipped before downloading them from iCloud.
#[tauri::command]
pub async fn upload_photos(
    app: AppHandle,
    state: State<'_, AppState>,
    asset_ids: Vec<String>,
    folder_id: Option<u64>,
    folder_path: String,
    transfer_ids: Option<Vec<String>>,
    skip_known: Option<bool>,
) -> Result<UploadSummary, String> {
    if !photos::available() {
        return Err("Subir desde Fotos solo está disponible en macOS.".into());
    }
    let cli_path = state.cli.path_or_err()?;
    let transfer_ids = transfer_ids.unwrap_or_default();
    let _one_at_a_time = state.upload_lock.lock().await;
    // Exporting an iCloud original can take a long time; don't start it if
    // the O2 session is already gone.
    ensure_session(&app, &cli_path).await?;
    // Use the latest shared snapshots, but never block an upload when the
    // library cannot be reached.
    let _ = ensure_library_synced(&app, &state, &cli_path, false).await;
    let mut summary = UploadSummary {
        uploaded: 0,
        skipped: 0,
        failed: Vec::new(),
        conflicts: Vec::new(),
        library_warning: None,
    };
    let mut library_uploads = Vec::new();
    let skip_known = skip_known.unwrap_or(false);
    let mut target = UploadTarget {
        id: folder_id,
        path: normalize_path(&folder_path),
        existing: None,
        logical: None,
    };
    let cache_root = transfer::app_cache_dir(&app)?.join("photos");

    for (index, asset_id) in asset_ids.into_iter().enumerate() {
        let transfer_id = transfer_ids
            .get(index)
            .cloned()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let asset = match photos::asset(&asset_id).await {
            Ok(asset) => asset,
            Err(err) => {
                transfer::emit_failed(&app, &transfer_id, "upload", &asset_id, &err);
                summary.failed.push(format!("{asset_id}: {err}"));
                continue;
            }
        };
        let name = photos::safe_file_name(&asset.name);

        if asset.size > 0 {
            let already_uploaded = {
                let existing = target.existing(&app, &cli_path).await;
                transfer::already_uploaded(existing, &name, asset.size)
            };
            if already_uploaded || target.has_logical(&name, asset.size) {
                let _ = app.emit(
                    "transfer-progress",
                    TransferProgress {
                        id: transfer_id.clone(),
                        kind: "upload".into(),
                        name: name.clone(),
                        phase: "done".into(),
                        current: 1,
                        total: 1,
                        bytes_done: asset.size,
                        bytes_total: asset.size,
                        message: "Ya estaba en O2; no se vuelve a subir".into(),
                    },
                );
                summary.skipped += 1;
                continue;
            }
        }

        if asset.size > 0 {
            let needed = asset.size.saturating_add(PHOTOS_FREE_SPACE_MARGIN);
            if let Some(free) = free_disk_space(cache_root.parent().unwrap_or(&cache_root))
                && free < needed
            {
                let message = format!(
                    "Espacio libre insuficiente: se necesitan {} MB y quedan {} MB",
                    needed / (1024 * 1024),
                    free / (1024 * 1024)
                );
                transfer::emit_failed(&app, &transfer_id, "upload", &name, &message);
                summary.failed.push(format!("{name}: {message}"));
                continue;
            }
        }

        let temp_root = cache_root.join(safe_component(&transfer_id));
        if let Err(err) = tokio::fs::create_dir_all(&temp_root).await {
            transfer::emit_failed(&app, &transfer_id, "upload", &name, &err.to_string());
            summary.failed.push(format!("{name}: {err}"));
            continue;
        }
        let destination = temp_root.join(&name);

        if let Err(err) = photos::export(
            app.clone(),
            transfer_id.clone(),
            asset.clone(),
            destination.clone(),
        )
        .await
        {
            transfer::emit_failed(&app, &transfer_id, "upload", &name, &err);
            summary.failed.push(format!("{name}: {err}"));
            let _ = tokio::fs::remove_dir_all(&temp_root).await;
            continue;
        }

        // Hashing is only worth it before the upload for files that read
        // quickly; for a large video the chunked upload computes the same
        // SHA-256 while splitting it anyway.
        let exported_size = std::fs::metadata(&destination)
            .map(|meta| meta.len())
            .unwrap_or(asset.size);
        let same_size_exists = state.library.read().unwrap().has_size(exported_size);
        let sha256 = if transfer::should_hash_before_upload(exported_size, same_size_exists) {
            transfer::sha256_file(&destination).await.ok()
        } else {
            None
        };
        if skip_known
            && let Some(hash) = &sha256
            && let Some(folder) = library_copy_folder(&state, hash)
        {
            let _ = app.emit(
                "transfer-progress",
                TransferProgress {
                    id: transfer_id.clone(),
                    kind: "upload".into(),
                    name: name.clone(),
                    phase: "done".into(),
                    current: 1,
                    total: 1,
                    bytes_done: asset.size,
                    bytes_total: asset.size,
                    message: format!("Ya estaba en O2 en {folder}; no se vuelve a subir"),
                },
            );
            summary.skipped += 1;
            let _ = tokio::fs::remove_dir_all(&temp_root).await;
            continue;
        }

        let existing = target.existing(&app, &cli_path).await;
        match transfer::upload_one(
            &app,
            &cli_path,
            &destination,
            folder_id,
            transfer::APP_CHUNK_SIZE,
            &transfer_id,
            existing,
            sha256.as_deref(),
            // Exported to a temporary file: reported, not offered again.
            transfer::OnConflict::Ask,
        )
        .await
        {
            Ok(receipt) => {
                summary.add(&receipt.outcome);
                if let Some(upload) = new_upload_from_receipt(
                    &receipt,
                    asset.name.clone(),
                    asset.date,
                    "photos",
                    folder_id,
                    target.path.clone(),
                ) {
                    library_uploads.push(upload);
                }
            }
            Err(err) => summary.fail(&name, None, &err),
        }
        let _ = tokio::fs::remove_dir_all(&temp_root).await;
    }

    // The list shows the new files without waiting for this.
    record_library_in_background(&app, &cli_path, library_uploads);
    invalidate_after_upload(&state);
    Ok(summary)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn download_entry(
    app: AppHandle,
    state: State<'_, AppState>,
    id: u64,
    name: String,
    dest: String,
    chunked: bool,
    cli_split: Option<bool>,
    manifest_id: Option<u64>,
    parts: Option<Vec<PartRef>>,
    size: Option<u64>,
    folder_id: Option<u64>,
    folder_path: Option<String>,
    overwrite: Option<bool>,
    transfer_id: Option<String>,
) -> Result<(), String> {
    let cli_path = state.cli.path_or_err()?;
    ensure_session(&app, &cli_path).await?;
    let mut dest = PathBuf::from(dest);
    // O2 accepts names that Windows rejects. Keep the chosen folder, but
    // make the file name safe for this platform.
    let Some(file_name) = dest.file_name().and_then(|name| name.to_str()) else {
        return Err("La ruta de destino no tiene nombre de archivo".into());
    };
    let safe_name = transfer::download_file_name(file_name);
    if safe_name != file_name {
        dest.set_file_name(safe_name);
    }
    // The frontend shows the download as queued under this ID beforehand.
    let transfer_id = transfer_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    // The save dialog asks before replacing; downloads into a chosen folder
    // never replace anything.
    let result = if !overwrite.unwrap_or(false) && dest.exists() {
        Err(format!("{} ya existe en esa carpeta", dest.display()))
    } else if cli_split.unwrap_or(false) {
        let target = match folder_path {
            Some(folder) if !folder.trim().is_empty() => transfer::join_remote_path(&folder, &name),
            _ => name.clone(),
        };
        let parts = parts.unwrap_or_default();
        transfer::download_cli_split(
            &app,
            &cli_path,
            &target,
            manifest_id,
            &parts,
            &name,
            size,
            &dest,
            &transfer_id,
        )
        .await
    } else if chunked {
        match manifest_id {
            Some(manifest_id) => {
                transfer::download_chunked(
                    &app,
                    &cli_path,
                    folder_id,
                    manifest_id,
                    &parts.unwrap_or_default(),
                    &name,
                    &dest,
                    &transfer_id,
                )
                .await
            }
            None => Err("Este archivo no tiene manifiesto".to_string()),
        }
    } else {
        transfer::download_normal(&app, &cli_path, id, size, &dest, &transfer_id, &name).await
    };
    if let Err(err) = &result {
        transfer::emit_failed(&app, &transfer_id, "download", &name, err);
    }
    result
}

/// The thumbnail and preview of one file, from `o2cloud thumbnails`.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct FileThumbnail {
    pub id: u64,
    /// O2's kind: "picture", "video", "file"…
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub thumbnail: Option<String>,
    #[serde(default)]
    pub preview: Option<String>,
}

/// How many files one `thumbnails` call asks for: the rows on screen.
const THUMBNAILS_PER_CALL: usize = 200;

/// Signed links to the thumbnails and previews of the files `ids` (photos,
/// videos and PDFs have them). Files without one come with `null`; files
/// O2 doesn't return (still processing, gone) are left out.
#[tauri::command]
pub async fn file_thumbnails(
    app: AppHandle,
    state: State<'_, AppState>,
    ids: Vec<u64>,
) -> Result<Vec<FileThumbnail>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let cli_path = state.cli.path_or_err()?;
    let mut args = vec!["thumbnails".to_string()];
    args.extend(ids.iter().take(THUMBNAILS_PER_CALL).map(u64::to_string));
    args.push("--json".into());
    let thumbnails: Vec<FileThumbnail> = cli::run_json(&app, &cli_path, &args).await?;
    Ok(only_o2_links(thumbnails))
}

/// Only O2's own HTTPS links reach the page, as for the slideshow.
fn only_o2_links(thumbnails: Vec<FileThumbnail>) -> Vec<FileThumbnail> {
    let o2 = |url: Option<String>| url.filter(|u| u.starts_with("https://cloud.o2online.es/"));
    thumbnails
        .into_iter()
        .map(|t| FileThumbnail {
            thumbnail: o2(t.thumbnail),
            preview: o2(t.preview),
            ..t
        })
        .collect()
}

/// Download the folder `id` (named `name`) with everything in it into the
/// local folder `dest`, where it lands as a folder of its name.
#[tauri::command]
pub async fn download_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    id: u64,
    name: String,
    dest: String,
    transfer_id: Option<String>,
) -> Result<crate::download_dir::FolderDownload, String> {
    let cli_path = state.cli.path_or_err()?;
    let transfer_id = transfer_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let result = async {
        ensure_session(&app, &cli_path).await?;
        crate::download_dir::download_folder(
            &app,
            &cli_path,
            &id.to_string(),
            &name,
            Path::new(&dest),
            &transfer_id,
        )
        .await
    }
    .await;
    if let Err(err) = &result {
        transfer::emit_failed(&app, &transfer_id, "download", &name, err);
    }
    result
}

async fn rm(app: &AppHandle, cli_path: &Path, id: u64, recursive: bool) -> Result<(), String> {
    let mut args = vec!["rm".to_string(), id.to_string()];
    if recursive {
        args.push("-r".into());
        args.push("-y".into());
    }
    cli::run(app, cli_path, &args).await.map(|_| ())
}

/// A file to move to the trash: its ID, or a chunked file's part IDs and
/// manifest.
#[derive(Deserialize)]
pub struct FileToDelete {
    ids: Vec<u64>,
    manifest_id: Option<u64>,
}

/// Move files and folders to the O2 trash. Files go a few at a time; a
/// chunked file's manifest only after all its parts, so if a part fails the
/// file stays listed and the delete can be retried. Folders go last.
#[tauri::command]
pub async fn delete_entries(
    app: AppHandle,
    state: State<'_, AppState>,
    files: Vec<FileToDelete>,
    folders: Vec<u64>,
) -> Result<(), String> {
    let cli_path = state.cli.path_or_err()?;
    let file_ids: HashSet<u64> = files
        .iter()
        .flat_map(|file| file.ids.iter().copied().chain(file.manifest_id))
        .collect();
    let folder_ids: HashSet<u64> = folders.iter().copied().collect();
    let result = delete_entries_inner(&app, &cli_path, files, folders).await;
    if result.is_ok() {
        // Keep the other cached folders: only the deleted entries changed.
        state.listings.remove_entries(&file_ids, &folder_ids);
    } else {
        // A partial delete: don't guess which entries survived.
        state.listings.clear();
    }
    state.all_files.write().unwrap().at = None;
    result
}

async fn delete_entries_inner(
    app: &AppHandle,
    cli_path: &Path,
    files: Vec<FileToDelete>,
    folders: Vec<u64>,
) -> Result<(), String> {
    // With an expired session, `rm` would open a login window and wait.
    ensure_session(app, cli_path).await?;

    let mut errors = Vec::new();
    let ids = files.iter().flat_map(|f| f.ids.iter().copied()).collect();
    let failed = rm_parallel(app, cli_path, ids, &mut errors).await;
    let manifests = match failed {
        Some(failed) => files
            .iter()
            .filter(|f| f.ids.iter().all(|id| !failed.contains(id)))
            .filter_map(|f| f.manifest_id)
            .collect(),
        // A task died without saying which part: keep every manifest.
        None => Vec::new(),
    };
    rm_parallel(app, cli_path, manifests, &mut errors).await;
    for id in folders {
        if let Err(err) = rm(app, cli_path, id, true).await {
            errors.push(err);
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

/// `rm` each of `ids`, `PARALLEL_DELETES` at a time (each `rm` lists the
/// whole account first). Errors go to `errors`; returns the IDs that
/// failed, or `None` if a task died and the failures aren't all known.
async fn rm_parallel(
    app: &AppHandle,
    cli_path: &Path,
    ids: Vec<u64>,
    errors: &mut Vec<String>,
) -> Option<HashSet<u64>> {
    let semaphore = Arc::new(Semaphore::new(PARALLEL_DELETES));
    let mut tasks = JoinSet::new();
    for id in ids {
        let Ok(permit) = semaphore.clone().acquire_owned().await else {
            break;
        };
        let app = app.clone();
        let cli_path = cli_path.to_path_buf();
        tasks.spawn(async move {
            let _permit = permit;
            (id, rm(&app, &cli_path, id, false).await)
        });
    }

    let mut failed = HashSet::new();
    let mut all_known = true;
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((_, Ok(()))) => {}
            Ok((id, Err(err))) => {
                failed.insert(id);
                errors.push(err);
            }
            Err(err) => {
                all_known = false;
                errors.push(err.to_string());
            }
        }
    }
    all_known.then_some(failed)
}

#[tauri::command]
pub async fn get_quota(app: AppHandle, state: State<'_, AppState>) -> Result<StorageSpace, String> {
    let cli_path = state.cli.path_or_err()?;
    ensure_session(&app, &cli_path).await?;
    let args = vec!["quota".to_string(), "--json".to_string()];
    cli::run_json(&app, &cli_path, &args).await
}

#[tauri::command]
pub async fn check_cli_update(state: State<'_, AppState>) -> Result<CliRelease, String> {
    let info = state.cli.info();
    cli::check_update(&info.version).await
}

#[tauri::command]
pub async fn install_cli_update(
    app: AppHandle,
    state: State<'_, AppState>,
    version: String,
    url: String,
) -> Result<CliInfo, String> {
    let info = cli::download_and_install(&app, &version, &url).await?;
    let new_path = PathBuf::from(&info.path);

    // Try the new CLI against this account before switching to it: a bad
    // release must not leave the app unable to list files. Without an
    // active session the listing can't be tried (it would open the login
    // window); the next login exercises it.
    let rejected = |err: String| {
        format!("La versión {version} no ha pasado la prueba; se mantiene la anterior. {err}")
    };
    if session_active(&app, &new_path).await.map_err(rejected)? {
        let test_args = vec!["ls".to_string(), "--json".to_string()];
        match cli::run_json::<LsResponse>(&app, &new_path, &test_args).await {
            Ok(_) => {}
            // The session expired while testing. That says nothing about the
            // new CLI; the next login exercises it.
            Err(err) if err == SESSION_EXPIRED => {}
            Err(err) => return Err(rejected(err)),
        }
    }

    cli::write_current(&app, &new_path, &info.version)?;
    state
        .cli
        .set(new_path, info.version.clone(), info.source.clone());
    state.listings.clear();
    Ok(info)
}

#[cfg(test)]
mod tests {

    #[test]
    fn lists_labels_as_virtual_folders() {
        let labels: Vec<CliLabel> = serde_json::from_str(
            r#"[{"id":7,"name":"Un día como hoy, hace 1 año","kind":"moment","count":4,"date":"2026-11-26T03:41:00Z","items":[1,2,3,4]},
                {"id":8,"name":null,"count":2,"date":null}]"#,
        )
        .unwrap();
        let listing = label_listing("moments", labels);
        let folders = &listing.virtual_folders;
        assert_eq!(folders[0].key, "label:7");
        assert_eq!(folders[0].name, "Un día como hoy, hace 1 año");
        assert_eq!(folders[0].date.as_deref(), Some("2026-11-26T03:41:00Z"));
        assert_eq!(
            (folders[0].files, folders[0].kind.as_str()),
            (4, "label-moments")
        );
        assert_eq!(folders[1].name, "8");
        // The CLI's cover and location come through.
        let places: Vec<CliLabel> = serde_json::from_str(
            r#"[{"id":9,"name":"Vigo","count":3,"thumbnail":"https://cloud.o2online.es/t","location":{"lat":42.2,"lon":-8.7}}]"#,
        )
        .unwrap();
        let place = &label_listing("places", places).virtual_folders[0];
        assert_eq!(
            place.thumbnail.as_deref(),
            Some("https://cloud.o2online.es/t")
        );
        assert_eq!(
            place.location,
            Some(Location {
                lat: 42.2,
                lon: -8.7
            })
        );
        // No people yet: says why.
        let empty = label_listing("people", Vec::new());
        assert!(empty.warnings[0].contains("reconocimiento facial"));
    }

    #[test]
    fn music_tracks_for_the_player() {
        let songs: Vec<serde_json::Value> = serde_json::from_str(
            r#"[{"id":1,"name":"a.mp3","path":"/Música/a.mp3","duration":2.24,"url":"https://cloud.o2online.es/sapi/download/audio?k=1","size":9},
                {"id":2,"name":"b.mp3","path":"/.o2cloud-gui/b.mp3","url":"https://cloud.o2online.es/x"},
                {"id":3,"name":"c.mp3","path":"/c.mp3","url":"https://evil.example/c.mp3"},
                {"id":4,"name":"d.mp3","path":"/d.mp3","url":null},
                {"id":5,"name":"e.mp3","path":"/Mix/e.mp3","duration":180,"url":"https://cloud.o2online.es/e","title":"E","artist":"A","album":"Mix","track":2,"genre":"Pop"},
                {"id":6,"name":"f.mp3","path":"/Mix/f.mp3","url":"https://evil.example/f","artist":"B","album":"Mix","track":1}]"#,
        )
        .unwrap();
        let tracks = tracks_of(songs);
        assert_eq!(tracks.len(), 2);
        assert_eq!((tracks[0].id, tracks[0].duration), (1, Some(2.24)));
        // An older CLI's song: no tags, «Sin álbum».
        assert!(tracks[0].tags.title.is_none());
        assert_eq!(tracks[0].album_key, "\u{1f}");
        // Tags come through, and the album is the list's: a compilation,
        // even though its other song can't be played here.
        let tagged = &tracks[1];
        assert_eq!(tagged.tags.title.as_deref(), Some("E"));
        assert_eq!(tagged.tags.track, Some(2));
        assert_eq!(tagged.duration, Some(180.0));
        assert_eq!(tagged.album_key, "varios artistas\u{1f}mix");
        let json = serde_json::to_value(tagged).unwrap();
        assert_eq!(json["artist"], "A");
        assert_eq!(json["album_key"], "varios artistas\u{1f}mix");
    }

    #[test]
    fn slides_and_media() {
        let label: SlideSource = serde_json::from_str(
            r#"{"files":[
                {"id":1,"name":"a.jpg","type":"picture","preview":"https://cloud.o2online.es/p?k=1"},
                {"id":2,"name":"b.mp4","type":"video","preview":"https://cloud.o2online.es/p?k=2"},
                {"id":3,"name":"c.jpg","type":"picture","preview":null},
                {"id":4,"name":"d.jpg","type":"picture","preview":"https://evil.example/p"}]}"#,
        )
        .unwrap();
        let slides = slides_of(label);
        assert_eq!(slides.iter().map(|s| s.id).collect::<Vec<_>>(), vec![1]);
        let files: Vec<RemoteFile> = serde_json::from_str(
            r#"[{"id":1,"name":"song.mp3","size":10,"path":"/Música/song.mp3"},
                {"id":2,"name":"x.json","size":5,"path":"/.o2cloud-gui/library/x.json"},
                {"id":3,"name":".hidden.pdf","size":5,"path":"/.hidden.pdf"}]"#,
        )
        .unwrap();
        let names: Vec<_> = media_listing(files)
            .files
            .into_iter()
            .map(|f| f.name)
            .collect();
        // Hidden folders are left out; a file's own name is not a folder.
        assert_eq!(names.len(), 2);
        assert!(!names.iter().any(|n| n == "x.json"));
    }

    #[test]
    fn indexes_files_in_parts_whole() {
        // As in /Vídeos grandes: IMG_0112.MOV in two of the app's parts.
        let files: Vec<RemoteFile> = serde_json::from_str(
            r#"[
            {"id":10,"name":"IMG_0112.MOV.o2manifest.json","size":569,"path":"/Vídeos grandes/IMG_0112.MOV.o2manifest.json"},
            {"id":11,"name":"IMG_0112.MOV.o2part.001","size":3670016000,"path":"/Vídeos grandes/IMG_0112.MOV.o2part.001"},
            {"id":12,"name":"IMG_0112.MOV.o2part.002","size":2914064220,"path":"/Vídeos grandes/IMG_0112.MOV.o2part.002"},
            {"id":20,"name":"big.iso.001","size":4194303936,"path":"/isos/big.iso.001"},
            {"id":21,"name":"big.iso.002","size":40,"path":"/isos/big.iso.002"},
            {"id":30,"name":"IMG_0112.MOV.o2part.001","size":5,"path":"/otra/IMG_0112.MOV.o2part.001"},
            {"id":40,"name":"a.jpg","size":7,"path":"/a.jpg"}
            ]"#,
        )
        .unwrap();
        let index = RemoteIndex::new(&files);
        let copies = copies_from_index(&index, "IMG_0112.MOV", 6_584_080_220);
        assert_eq!(copies.len(), 1);
        assert_eq!(copies[0].folder_path, "/Vídeos grandes");
        assert_eq!(copies[0].remote_id, Some(10));
        assert!(index.has_size(6_584_080_220));
        // The CLI's parts (its part size, 4000 MiB less a few bytes), whole.
        assert_eq!(copies_from_index(&index, "big.iso", 4_194_303_976).len(), 1);
        // A lone part elsewhere (no manifest) is not taken for the file, and
        // parts of different folders aren't mixed.
        assert!(copies_from_index(&index, "IMG_0112.MOV", 5).is_empty());
        // Plain files as before.
        assert_eq!(copies_from_index(&index, "a.jpg", 7).len(), 1);
    }

    /// The whole account's listing through `RemoteIndex`: every file the
    /// app stored in parts is found whole. Run with
    /// `O2CLOUD_LIVE_CLI=<o2cloud> cargo test live_whole_files -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_whole_files() {
        let cli = std::env::var("O2CLOUD_LIVE_CLI").expect("O2CLOUD_LIVE_CLI");
        let out = std::process::Command::new(&cli)
            .args(["ls", "-a", "--json"])
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let files: Vec<RemoteFile> = serde_json::from_slice(&out.stdout).unwrap();
        let index = RemoteIndex::new(&files);
        let manifests: Vec<&RemoteFile> = files
            .iter()
            .filter(|f| f.name.ends_with(".o2manifest.json"))
            .collect();
        println!("{} files, {} part manifests", files.len(), manifests.len());
        for manifest in manifests {
            let name = manifest.name.trim_end_matches(".o2manifest.json");
            let found: Vec<_> = index
                .by_name_size
                .iter()
                .filter(|((n, _), list)| n == name && list.iter().any(|f| f.id == manifest.id))
                .map(|((_, size), _)| *size)
                .collect();
            println!("  {name}: whole {found:?}");
            assert_eq!(found.len(), 1, "{name} isn't indexed whole");
        }
    }

    /// `o2cloud thumbnails` against the account, through the app's types:
    /// photos, videos and PDFs from `ls -a`, read as the app reads them.
    /// Run with `O2CLOUD_LIVE_CLI=<o2cloud 1.5+> cargo test live_thumbnails
    /// -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_thumbnails() {
        let cli = std::env::var("O2CLOUD_LIVE_CLI").expect("O2CLOUD_LIVE_CLI");
        let run = |args: &[String]| {
            let out = std::process::Command::new(&cli)
                .args(args)
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            out.stdout
        };
        let files: Vec<RemoteFile> =
            serde_json::from_slice(&run(&["ls".into(), "-a".into(), "--json".into()])).unwrap();
        let pick = |ext: &[&str]| -> Vec<u64> {
            files
                .iter()
                .filter(|f| ext.iter().any(|e| f.name.to_lowercase().ends_with(e)))
                .take(5)
                .map(|f| f.id)
                .collect()
        };
        let (photos, videos, pdfs) = (
            pick(&[".jpg", ".jpeg", ".heic"]),
            pick(&[".mp4", ".mov"]),
            pick(&[".pdf"]),
        );
        let mut args = vec!["thumbnails".to_string()];
        args.extend(
            photos
                .iter()
                .chain(&videos)
                .chain(&pdfs)
                .map(u64::to_string),
        );
        args.push("--json".into());
        let started = std::time::Instant::now();
        let got = only_o2_links(serde_json::from_slice(&run(&args)).unwrap());
        println!(
            "asked {} ({} photos, {} videos, {} PDFs), got {} in {:?}",
            args.len() - 2,
            photos.len(),
            videos.len(),
            pdfs.len(),
            got.len(),
            started.elapsed()
        );
        for t in &got {
            println!(
                "  {} {:?} thumbnail={} preview={}",
                t.id,
                t.kind,
                t.thumbnail.is_some(),
                t.preview.is_some()
            );
        }
        let with = |ids: &[u64]| {
            got.iter()
                .filter(|t| ids.contains(&t.id) && t.thumbnail.is_some())
                .count()
        };
        assert!(with(&photos) > 0, "no photo came with a thumbnail");
        if !videos.is_empty() {
            assert!(with(&videos) > 0, "no video came with a thumbnail");
        }
    }

    #[test]
    fn reads_thumbnails_and_keeps_only_o2_links() {
        let raw = r#"[
            {"id":1,"type":"picture","thumbnail":"https://cloud.o2online.es/sapi/download/picture?k=s","preview":"https://cloud.o2online.es/sapi/download/picture?k=b"},
            {"id":2,"type":"file","thumbnail":null,"preview":null},
            {"id":3,"type":"video","thumbnail":"https://evil.example/t","preview":"http://cloud.o2online.es/p"}
        ]"#;
        let thumbnails = only_o2_links(serde_json::from_str(raw).unwrap());
        assert_eq!(thumbnails.len(), 3);
        assert_eq!(thumbnails[0].kind.as_deref(), Some("picture"));
        assert!(thumbnails[0].thumbnail.as_deref().unwrap().ends_with("k=s"));
        assert_eq!(
            (
                thumbnails[1].thumbnail.as_ref(),
                thumbnails[1].preview.as_ref()
            ),
            (None, None)
        );
        // Not O2's, or not HTTPS: dropped.
        assert_eq!(
            (
                thumbnails[2].thumbnail.as_ref(),
                thumbnails[2].preview.as_ref()
            ),
            (None, None)
        );
        // What the page gets keeps O2's field name.
        let json = serde_json::to_value(&thumbnails[0]).unwrap();
        assert_eq!(json["type"], "picture");
    }

    #[test]
    fn groups_music_by_folder() {
        assert_eq!(folder_of("/musica/Queen/a.mp3"), "/musica/Queen");
        assert_eq!(folder_of("/a.mp3"), "/");
        assert_eq!(folder_of("a.mp3"), "/");
        let files: Vec<RemoteFile> = serde_json::from_str(
            r#"[{"id":1,"name":"a.mp3","size":10,"path":"/musica/Queen/a.mp3"},
                {"id":2,"name":"b.mp3","size":20,"path":"/musica/Queen/b.mp3"},
                {"id":3,"name":"c.mp3","size":5,"path":"/musica/c.mp3"},
                {"id":4,"name":"d.mp3","path":"/d.mp3"},
                {"id":5,"name":"x.mp3","size":1,"path":"/.o2cloud-gui/x.mp3"}]"#,
        )
        .unwrap();
        let folders = music_folders(&shown_media(files));
        let summary: Vec<_> = folders
            .iter()
            .map(|f| (f.key.as_str(), f.name.as_str(), f.files, f.size))
            .collect();
        // By path; the root has a name of its own; hidden folders stay out.
        assert_eq!(
            summary,
            [
                ("media:music-folder:/", "O2 Cloud (raíz)", 1, 0),
                ("media:music-folder:/musica", "musica", 1, 5),
                ("media:music-folder:/musica/Queen", "musica/Queen", 2, 30),
            ]
        );
        assert!(folders.iter().all(|f| f.kind == "music-folder"));
    }

    #[test]
    fn groups_music_by_album() {
        let songs: Vec<CliSong> = serde_json::from_str(
            r#"[
            {"id":1,"name":"b.mp3","size":10,"path":"/musica/Queen/GH/b.mp3","artist":"Queen","album":"Greatest Hits","track":2},
            {"id":2,"name":"c.mp3","size":10,"path":"/musica/Queen/GH/c.mp3","artist":"Queen","album":"Greatest hits","track":1},
            {"id":3,"name":"a.mp3","size":1,"path":"/musica/Queen/GH/a.mp3","album":"Greatest Hits"},
            {"id":4,"name":"abba.mp3","size":5,"path":"/musica/ABBA/GH/abba.mp3","artist":"ABBA","album":"Greatest Hits","track":1},
            {"id":5,"name":"x1.mp3","size":1,"path":"/x1.mp3","artist":"Bee Gees","album":"Best","track":1},
            {"id":6,"name":"x2.mp3","size":1,"path":"/x2.mp3","artist":"Cher","album":"Best","track":1},
            {"id":7,"name":"n1.mp3","size":3,"path":"/Now 50/CD1/n1.mp3","artist":"A","album":"Now 50","track":1},
            {"id":8,"name":"n2.mp3","size":3,"path":"/Now 50/CD1/n2.mp3","artist":"B","album":"Now 50","track":2},
            {"id":9,"name":"n3.mp3","size":3,"path":"/Now 50/CD2/n3.mp3","artist":"C","album":"Now 50","track":1},
            {"id":10,"name":"n4.mp3","size":3,"path":"/Now 50/CD2/n4.mp3","artist":"A","album":"Now 50","track":2},
            {"id":11,"name":"u.mp3","size":2,"path":"/u.mp3","title":null,"artist":null,"album":null,"track":null,"genre":null},
            {"id":12,"name":"v.mp3","size":2,"path":"/v.mp3","artist":"Queen","title":"V"},
            {"id":13,"name":"h.mp3","size":1,"path":"/.o2cloud-gui/h.mp3","artist":"Queen","album":"Greatest Hits","track":3}
            ]"#,
        )
        .unwrap();
        let songs = shown_songs(songs);
        let albums = music_albums(&songs);
        let summary: Vec<_> = albums
            .iter()
            .map(|a| {
                let artist = a.artist.as_deref().unwrap_or("-");
                format!("{} · {artist} · {} · {}", a.name, a.files, a.size)
            })
            .collect();
        // Two «Greatest Hits» stay apart, and so do two «Best» in one
        // folder (both have a track 1); a compilation is one, across its
        // folders; a song with no artist goes with its album's; songs with
        // no album are «Sin álbum», last; hidden folders stay out.
        assert_eq!(
            summary,
            [
                "Best · Bee Gees · 1 · 1",
                "Best · Cher · 1 · 1",
                "Greatest Hits · ABBA · 1 · 5",
                "Greatest Hits · Queen · 3 · 21",
                "Now 50 · Varios artistas · 4 · 12",
                "Sin álbum · - · 2 · 4",
            ]
        );
        let keys: Vec<_> = albums
            .iter()
            .map(|a| a.key.trim_start_matches("media:music-album:"))
            .collect();
        assert_eq!(
            keys,
            [
                "bee gees\u{1f}best",
                "cher\u{1f}best",
                "abba\u{1f}greatest hits",
                "queen\u{1f}greatest hits",
                "varios artistas\u{1f}now 50",
                "\u{1f}",
            ]
        );
        assert!(albums.iter().all(|a| a.kind == "music-album"));

        // An album opens by track number, then by name; those with none last.
        let names: Vec<_> = album_listing(songs, keys[3])
            .files
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert_eq!(names, ["c.mp3", "b.mp3", "a.mp3"]);
    }

    #[test]
    fn albums_without_tags() {
        // An older CLI: no tags, so one «Sin álbum» with every song.
        let songs: Vec<CliSong> = serde_json::from_str(
            r#"[{"id":1,"name":"a.mp3","size":1,"path":"/a.mp3"},
                {"id":2,"name":"b.mp3","size":2,"path":"/m/b.mp3"}]"#,
        )
        .unwrap();
        let albums = music_albums(&shown_songs(songs));
        assert_eq!(albums.len(), 1);
        assert_eq!((albums[0].name.as_str(), albums[0].files), ("Sin álbum", 2));
        // Several artists without track numbers in one folder: a compilation.
        let songs: Vec<CliSong> = serde_json::from_str(
            r#"[{"id":1,"name":"a.mp3","path":"/a.mp3","artist":"A","album":"Mix"},
                {"id":2,"name":"b.mp3","path":"/b.mp3","artist":"B","album":"Mix"},
                {"id":3,"name":"c.mp3","path":"/c.mp3","album":"Mix"}]"#,
        )
        .unwrap();
        assert_eq!(album_keys(&songs), ["varios artistas\u{1f}mix"; 3]);
        // No artist at all: an album of its own, with none shown.
        let songs: Vec<CliSong> =
            serde_json::from_str(r#"[{"id":1,"name":"a.mp3","path":"/a.mp3","album":"Mix"}]"#)
                .unwrap();
        let albums = music_albums(&songs);
        assert_eq!(albums[0].key, "media:music-album:\u{1f}mix");
        assert_eq!(albums[0].artist, None);
    }

    /// The real CLI's `labels` output through the app's types. Run with
    /// `O2CLOUD_LIVE_CLI=<o2cloud 1.1+> cargo test live_labels -- --ignored`.
    #[test]
    #[ignore]
    fn live_labels() {
        let cli = std::env::var("O2CLOUD_LIVE_CLI").expect("O2CLOUD_LIVE_CLI");
        let run = |args: &[&str]| {
            let out = std::process::Command::new(&cli)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap()
        };
        for kind in ["moments", "places", "people"] {
            let labels: Vec<CliLabel> =
                serde_json::from_str(&run(&["labels", "--kind", kind, "--json"])).unwrap();
            let listing = label_listing(kind, labels);
            println!("{kind}: {} folders", listing.virtual_folders.len());
            if let Some(first) = listing.virtual_folders.first() {
                let id = first.key.trim_start_matches("label:");
                let label: LabelWithFiles =
                    serde_json::from_str(&run(&["labels", id, "--json"])).unwrap();
                let (files, _) = transfer::group_logical_files(label.files);
                println!(
                    "  first: {} photos listed, {} files shown",
                    first.files,
                    files.len()
                );
                assert_eq!(first.files as usize, files.len());
                assert!(files.iter().all(|f| f.normal_id.is_some()));
                let label: SlideSource =
                    serde_json::from_str(&run(&["labels", id, "--json"])).unwrap();
                println!("  slides: {}", slides_of(label).len());
            }
        }
        for kind in ["music", "documents"] {
            let files: Vec<RemoteFile> =
                serde_json::from_str(&run(&["media", kind, "--json"])).unwrap();
            println!("{kind}: {} shown", media_listing(files).files.len());
        }
        let songs: Vec<CliSong> =
            serde_json::from_str(&run(&["media", "music", "--json"])).unwrap();
        let songs = shown_songs(songs);
        let untagged = songs.iter().filter(|s| s.tags.album.is_none()).count();
        let albums = music_albums(&songs);
        println!(
            "music: {} albums; {} of {} songs with no album",
            albums.len(),
            untagged,
            songs.len()
        );
        assert_eq!(
            albums.iter().map(|a| a.files as usize).sum::<usize>(),
            songs.len()
        );
    }

    use super::*;

    fn file(name: &str, date: Option<&str>, size: u64) -> LogicalFile {
        LogicalFile {
            name: name.into(),
            size,
            date: date.map(|s| s.to_string()),
            chunked: false,
            cli_split: false,
            manifest_id: None,
            normal_id: Some(1),
            parts: vec![],
            incomplete: false,
        }
    }

    #[test]
    fn manifest_identity_accepts_both_manifest_formats() {
        let old = manifest_identity(
            r#"{"format":"o2cloud-chunks","version":1,"name":"movie.mkv","size":150,"sha256":"abc","chunk_size":100,"parts":[]}"#,
        )
        .unwrap();
        assert_eq!(old.name, "movie.mkv");
        assert_eq!(old.size, 150);
        assert_eq!(old.sha256, "abc");

        let cli = manifest_identity(
            r#"{"format":"o2cloud-cli-split","version":1,"name":"movie.mkv","size":150,"sha256":"def","parts":[]}"#,
        )
        .unwrap();
        assert_eq!(cli.sha256, "def");

        assert!(manifest_identity("{}").is_none());
        assert!(manifest_identity(r#"{"name":"x","size":1,"sha256":" "}"#).is_none());
    }

    #[test]
    fn camera_years_are_sorted_newest_first_with_unknown_last() {
        let files = vec![
            file("IMG_2026.jpg", Some("2026-01-01T00:00:00Z"), 10),
            file("IMG_2025.jpg", Some("2025-01-01T00:00:00Z"), 20),
            file("IMG_2025b.jpg", Some("2025-02-01T00:00:00Z"), 30),
            file("old.jpg", None, 5),
        ];
        let folders = year_folders(&files);
        assert_eq!(folders[0].key, "camera/all");
        assert_eq!(folders[0].files, 4);
        assert_eq!(folders[0].size, 65);
        let names: Vec<&str> = folders.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["Todo", "2026", "2025", "Sin fecha"]);
        assert_eq!(folders[2].files, 2);
        assert_eq!(folders[2].size, 50);
        assert_eq!(folders[3].key, "camera/unknown");
    }

    #[test]
    fn picks_the_o2_cloud_cookie_not_another_jsessionid() {
        let apiseg = tauri::webview::Cookie::build(("JSESSIONID", "apiseg"))
            .domain("apiseg.telefonica.es")
            .build();
        let cloud = tauri::webview::Cookie::build(("JSESSIONID", "cloud"))
            .domain(".cloud.o2online.es")
            .build();
        let cookies = vec![apiseg, cloud];
        assert_eq!(
            cloud_cookie(&cookies, "JSESSIONID").as_deref(),
            Some("cloud")
        );
        assert_eq!(cloud_cookie(&cookies, "validationKey"), None);
    }

    #[test]
    fn session_is_active_only_when_o2_accepts_it() {
        let active: StatusOutput = serde_json::from_str(
            r#"{
                "logged_in": true,
                "phone": "+34600000000",
                "since": "2026-10-30T00:00:00Z",
                "session": "active",
                "renews_silently": true,
                "storage": {
                    "quota": 10995116277760,
                    "used": 1073741824,
                    "free": 10994042535936,
                    "softdeleted": 0,
                    "nolimit": false
                }
            }"#,
        )
        .unwrap();
        let status = session_status(active, String::new());
        assert!(status.logged_in);
        assert_eq!(status.phone.as_deref(), Some("+34600000000"));
        let storage = status.storage.unwrap();
        assert_eq!(storage.used, 1073741824);
        assert_eq!(storage.quota, 10995116277760);

        let expired: StatusOutput = serde_json::from_str(
            r#"{"logged_in": true, "session": "expired", "renews_silently": true}"#,
        )
        .unwrap();
        assert!(!session_status(expired, String::new()).logged_in);

        let not_logged: StatusOutput = serde_json::from_str(r#"{"logged_in": false}"#).unwrap();
        assert!(!session_status(not_logged, String::new()).logged_in);
    }
}
