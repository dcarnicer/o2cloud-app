mod cache;
mod camera;
mod cli;
mod commands;
mod download_dir;
mod library;
mod photos;
mod transfer;
mod types;

use tauri::Manager;

/// macOS may put a background app into App Nap. That suspends timers and can
/// leave the window on «Cargando…» after a listing has already finished.
/// Keep a user-initiated activity for the whole process lifetime; system sleep
/// is still allowed.
#[cfg(target_os = "macos")]
fn prevent_app_nap() {
    use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};

    let reason = NSString::from_str("O2 Cloud listing and transfers");
    let activity = NSProcessInfo::processInfo().beginActivityWithOptions_reason(
        NSActivityOptions::UserInitiatedAllowingIdleSystemSleep,
        &reason,
    );
    std::mem::forget(activity);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "macos")]
    prevent_app_nap();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // Updates of the app itself, from the GitHub release's latest.json,
        // checked against the public key in tauri.conf.json; and the restart
        // after one.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            let state = commands::AppState::new(app.handle());
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_cli_info,
            commands::get_settings,
            commands::save_settings,
            commands::get_session_status,
            commands::login,
            commands::logout,
            commands::list_dir,
            commands::label_slides,
            commands::music_tracks,
            commands::create_folder,
            commands::upload_paths,
            commands::photos_access_status,
            commands::photos_request_access,
            commands::photos_list,
            commands::photos_albums,
            commands::photos_thumbnail,
            commands::photos_cancel_export,
            commands::upload_photos,
            commands::library_status,
            commands::library_sync,
            commands::library_reconcile,
            commands::library_forget,
            commands::library_index_manifests,
            commands::library_match_photos,
            commands::library_check_paths,
            commands::download_entry,
            commands::download_folder,
            commands::file_thumbnails,
            commands::delete_entries,
            commands::get_quota,
            commands::check_cli_update,
            commands::install_cli_update,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
