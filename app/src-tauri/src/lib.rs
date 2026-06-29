mod commands;
#[cfg(target_os = "macos")]
mod menu;

use commands::AppState;
use marrow_core::pr_parser;
use std::collections::HashMap;
use std::env;
use std::sync::Mutex;
use tauri::{Emitter, Manager};
use tauri_plugin_deep_link::DeepLinkExt;

/// Default/min seconds between activity polls. GitHub's notifications API floor
/// is 60s; we honor its `X-Poll-Interval` header above this when it asks for more.
const ACTIVITY_POLL_SECS: u64 = 60;
/// Fallback per-watch cap when the configured value is 0/unset.
const DEFAULT_PER_WATCH_CAP: usize = 50;

/// One activity poll: build a GitHub client from the saved token, collect
/// involved PRs + watch results, diff against the seen-state, and broadcast the
/// `pr-activity` event. Returns the notifications scheduling metadata
/// `(poll_interval, last_modified)`, or `None` when no token is configured.
async fn poll_activity_once(
    handle: &tauri::AppHandle,
    notif_since: Option<String>,
) -> Option<(Option<u64>, Option<String>)> {
    let settings = marrow_core::config::load_settings();
    let token = marrow_core::config::resolve_github_token(&settings)?;
    let client = marrow_core::github::GithubClient::new(Some(token));
    let watches = marrow_core::watches::load_watches();
    let cap = match settings.activity_per_watch_cap {
        0 => DEFAULT_PER_WATCH_CAP,
        n => n as usize,
    };
    let collected = client
        .collect_activity(&watches, cap, notif_since.as_deref())
        .await;
    let store = marrow_core::activity::load_activity_store();
    let payload = marrow_core::activity::compute_activity(
        collected.observations,
        &store,
        collected.truncated,
        marrow_core::activity::now_rfc3339(),
    );
    let _ = handle.emit("pr-activity", payload);
    Some((collected.notif_poll_interval, collected.notif_last_modified))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let args: Vec<String> = env::args().collect();
    let manifest_path = args.get(1).cloned();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_deep_link::init())
        // Persist/restore the floating mini-player's size & position. Scoped to
        // the activity window only (the main window keeps its config default).
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_denylist(&["main"])
                .build(),
        )
        .on_menu_event(|app, event| {
            // Cmd+W / Cmd+T are routed to the frontend, which owns the tab model.
            // The frontend skips closing when a text field is focused. Cmd+Q has no
            // accelerator (see menu.rs), so the only quit path is this explicit item.
            match event.id().as_ref() {
                "close_tab" => { let _ = app.emit("menu-close-tab", ()); }
                "new_tab" => { let _ = app.emit("menu-new-tab", ()); }
                "quit_request" => { let _ = app.emit("menu-quit-request", ()); }
                _ => {}
            }
        })
        .manage(AppState {
            manifest_path: Mutex::new(manifest_path),
            pending_deep_link: Mutex::new(None),
            pr_node_ids: Mutex::new(HashMap::new()),
            frontend_ready: Mutex::new(false),
        })
        .setup(|app| {
            // Custom menu intercepts Ctrl+W / Ctrl+Q at the native layer (see menu.rs).
            #[cfg(target_os = "macos")]
            {
                let menu = menu::build_menu(app.handle())?;
                app.set_menu(menu)?;
            }

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

            // Mini-player background watcher: poll the user's involved PRs +
            // saved watches, diff against the persisted seen-state, and emit a
            // `pr-activity` event to every window. Runs for the app's lifetime.
            let activity_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                // Conditional-request state for notifications: `Last-Modified`
                // makes the next poll cheap (304), and `X-Poll-Interval` sets
                // the cadence GitHub asks us to keep.
                let mut notif_since: Option<String> = None;
                let mut poll_secs = ACTIVITY_POLL_SECS;
                loop {
                    if let Some((poll_interval, last_modified)) =
                        poll_activity_once(&activity_handle, notif_since.clone()).await
                    {
                        if last_modified.is_some() {
                            notif_since = last_modified;
                        }
                        if let Some(pi) = poll_interval {
                            poll_secs = pi.max(ACTIVITY_POLL_SECS);
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(poll_secs)).await;
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
            commands::load_dismissed_highlights,
            commands::save_dismissed_highlights,
            commands::list_cached_prs,
            commands::get_pr_checks,
            commands::dismiss_checks_warning,
            commands::is_checks_dismissed,
            commands::load_cached_manifest_by_pr,
            commands::save_session,
            commands::load_session,
            commands::get_watches,
            commands::save_watches,
            commands::mark_pr_seen,
            commands::set_activity_window_visible,
            commands::set_mini_player_enabled,
            commands::dismiss_mini_player,
            commands::open_pr_in_main,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
