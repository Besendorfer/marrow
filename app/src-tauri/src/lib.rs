mod ai;
mod bedrock;
mod checks_dismiss;
mod commands;
pub mod config;
pub mod fetch;
pub mod github;
pub mod manifest_cache;
pub mod pr_parser;
mod prompts;
mod session;
pub mod types;
mod viewed_state;

use commands::AppState;
use std::collections::HashMap;
use std::env;
use std::sync::Mutex;
use tauri::{Emitter, Manager};
use tauri_plugin_deep_link::DeepLinkExt;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let args: Vec<String> = env::args().collect();
    let manifest_path = args.get(1).cloned();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_deep_link::init())
        .manage(AppState {
            manifest_path: Mutex::new(manifest_path),
            pending_deep_link: Mutex::new(None),
            pr_node_ids: Mutex::new(HashMap::new()),
            frontend_ready: Mutex::new(false),
        })
        .setup(|app| {
            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |event: tauri_plugin_deep_link::OpenUrlEvent| {
                if let Some(url) = event.urls().first() {
                    let url_str: &str = url.as_str();

                    // Tauri normalizes scheme to `relevantreviews://` on most platforms,
                    // but some shells/OSes hand off `relevantreviews:` without the slashes.
                    let pr_ref = url_str
                        .strip_prefix("relevantreviews://")
                        .or_else(|| url_str.strip_prefix("relevantreviews:"));

                    if let Some(pr_ref) = pr_ref {
                        // Reject anything that isn't a real PR reference before forwarding.
                        if pr_parser::parse_pr_ref(pr_ref).is_err() {
                            return;
                        }

                        // Hot-open: frontend already running
                        let _ = handle.emit("deep-link-open", pr_ref);

                        // Cold-start: only buffer for replay if the frontend
                        // hasn't completed init — otherwise the next cold-start
                        // would replay this URL after the frontend already
                        // handled it via the emit above.
                        let state = handle.state::<AppState>();
                        let frontend_ready = state
                            .frontend_ready
                            .lock()
                            .map(|g| *g)
                            .unwrap_or(false);
                        if !frontend_ready {
                            if let Ok(mut pending) = state.pending_deep_link.lock() {
                                *pending = Some(pr_ref.to_string());
                            }
                        }

                        if let Some(window) = handle.get_webview_window("main") {
                            let _ = window.set_focus();
                        }
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_pending_deep_link,
            commands::signal_frontend_ready,
            commands::load_manifest,
            commands::get_initial_manifest_path,
            commands::fetch_pr,
            commands::check_pr_updates,
            commands::fetch_review_requests,
            commands::fetch_review_comments,
            commands::reply_to_thread,
            commands::toggle_thread_resolved,
            commands::get_my_review_state,
            commands::submit_review,
            commands::generate_review_body,
            commands::update_review_comment,
            commands::create_review_comment,
            commands::toggle_reaction,
            commands::get_settings,
            commands::save_settings,
            commands::sync_file_viewed_to_github,
            commands::fetch_gh_viewed_state,
            commands::load_viewed_files,
            commands::save_viewed_files,
            commands::list_cached_prs,
            commands::get_pr_checks,
            commands::dismiss_checks_warning,
            commands::is_checks_dismissed,
            commands::load_cached_manifest_by_pr,
            commands::save_session,
            commands::load_session,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
