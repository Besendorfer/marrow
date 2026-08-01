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
    // Settings default to 50 and the UI clamps to >= 1; .max(1) is a cheap guard
    // against a hand-edited 0.
    let cap = (settings.activity_per_watch_cap as usize).max(1);
    let collected = client
        .collect_activity(&watches, cap, notif_since.as_deref())
        .await;
    let mut store = marrow_core::activity::load_activity_store();
    let viewer = collected.viewer.unwrap_or_default();
    let payload = marrow_core::activity::compute_activity(
        collected.observations,
        &mut store,
        collected.truncated,
        marrow_core::activity::now_rfc3339(),
        &viewer,
        settings.show_approved_prs,
    );
    // Persist only when compute_activity actually mutated the store (a delta
    // woke a snoozed PR). Skipping quiet polls keeps this loop from being a
    // periodic writer racing mark_pr_seen/snooze_pr on the unlocked,
    // last-write-wins activity.json.
    if store.dirty {
        let _ = marrow_core::activity::save_activity_store(&store);
    }
    let _ = handle.emit("pr-activity", payload);
    Some((collected.notif_poll_interval, collected.notif_last_modified))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let args: Vec<String> = env::args().collect();
    let manifest_path = args.get(1).cloned();

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default()
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
        );
    // Lets the activity window become a non-activating NSPanel (macOS only).
    #[cfg(target_os = "macos")]
    {
        builder = builder.plugin(tauri_nspanel::init());
    }

    builder
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
            chat_cancels: Mutex::new(HashMap::new()),
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
                        // A 304 (None) means "unchanged" — keep the prior value.
                        notif_since = last_modified.or(notif_since);
                        if let Some(pi) = poll_interval {
                            poll_secs = pi.max(ACTIVITY_POLL_SECS);
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(poll_secs)).await;
                }
            });

            // macOS: drive the floating mini-player from the app-active state.
            // App (re)activation (Cmd+Tab / dock) delivers no webview focus event
            // and Page Visibility doesn't fire for occlusion, so focus-based
            // logic can't catch it. Polling NSApplication.isActive on the main
            // thread is the one reliable signal: the widget shows while Marrow is
            // NOT the active app and hides once it is. We act only on a CHANGE in
            // active-state and delegate the actual show/hide/build (and the
            // enabled-setting gate) to `set_activity_window_visible`, so the disk
            // read and window mutation happen on transitions, not every tick.
            #[cfg(target_os = "macos")]
            {
                use std::sync::atomic::{AtomicU8, Ordering};
                let poll_handle = app.handle().clone();
                let was_active = std::sync::Arc::new(AtomicU8::new(0)); // 1 active, 2 inactive
                tauri::async_runtime::spawn(async move {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        let h = poll_handle.clone();
                        let wa = was_active.clone();
                        let _ = poll_handle.run_on_main_thread(move || {
                            use objc2::MainThreadMarker;
                            use objc2_app_kit::NSApplication;

                            let Some(mtm) = MainThreadMarker::new() else {
                                return;
                            };
                            let active = NSApplication::sharedApplication(mtm).isActive();

                            // SHOW the panel when you leave Marrow (active→inactive).
                            // HIDING is driven by the main window gaining focus
                            // (see App.tsx) — NOT by app-active — so clicking/dragging
                            // the panel (which activates the app but does not focus the
                            // main window) never hides it.
                            let acode = if active { 1 } else { 2 };
                            if wa.swap(acode, Ordering::Relaxed) == 1 && acode == 2 {
                                let _ = commands::set_activity_window_visible(h, true);
                            }
                        });
                    }
                });
            }

            // Pre-create the mini-player panel (hidden) at startup if enabled, so
            // the first show isn't a window creation — creating a webview window
            // activates the app and would yank you back to Marrow on your first
            // Cmd+Tab away. Runs on the main thread (setup), where it's safe.
            if marrow_core::config::load_settings().activity_mini_player {
                let _ = commands::build_activity_window(app.handle());
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_pending_deep_link,
            commands::signal_frontend_ready,
            commands::needs_setup,
            commands::get_viewer_login,
            commands::check_pr_ref,
            commands::validate_github_token,
            commands::validate_ai_provider,
            commands::load_manifest,
            commands::get_initial_manifest_path,
            commands::fetch_pr,
            commands::check_pr_updates,
            commands::fetch_review_requests,
            commands::fetch_review_comments,
            commands::reply_to_thread,
            commands::toggle_thread_resolved,
            commands::get_my_review_state,
            commands::get_commit_diff,
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
            commands::snooze_pr,
            commands::unsnooze_pr,
            commands::set_activity_window_visible,
            commands::set_mini_player_enabled,
            commands::dismiss_mini_player,
            commands::open_pr_in_main,
            commands::resume_review_in_main,
            commands::chat_send,
            commands::chat_cancel,
            commands::load_chat_history,
            commands::save_chat_history,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
