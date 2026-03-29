mod ai;
mod bedrock;
mod checks_dismiss;
mod commands;
mod config;
mod fetch;
mod github;
mod manifest_cache;
mod pr_parser;
mod prompts;
pub mod types;
mod viewed_state;

use commands::AppState;
use std::env;
use std::sync::Mutex;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let args: Vec<String> = env::args().collect();
    let manifest_path = args.get(1).cloned();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(AppState {
            manifest_path: Mutex::new(manifest_path),
        })
        .invoke_handler(tauri::generate_handler![
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
            commands::get_settings,
            commands::save_settings,
            commands::load_viewed_files,
            commands::save_viewed_files,
            commands::list_cached_prs,
            commands::get_pr_checks,
            commands::dismiss_checks_warning,
            commands::is_checks_dismissed,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
