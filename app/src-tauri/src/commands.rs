use marrow_core::bedrock::{region_from_arn, BedrockClient};
use marrow_core::config::{load_settings, resolve_github_token, save_settings_to_disk};
use marrow_core::fetch::fetch_pr_impl;
use marrow_core::github::GithubClient;
use marrow_core::types::{MyReviewState, PrChecksStatus, PrUpdateStatus, ReviewComment, ReviewManifest, ReviewRequestItem, ReviewThread, Settings};
use marrow_core::manifest_cache::{self, CachedPrInfo};
use marrow_core::session::{self, SessionState};
use marrow_core::dismissed_highlights::{self, DismissedHighlights};
use marrow_core::viewed_state::{self, ViewedFileState};
use marrow_core::activity::{self, Observed};
use marrow_core::watches::{self, Watch};
use std::collections::HashMap;
use std::fs;
use std::sync::Mutex;
use tauri::{command, State};

pub struct AppState {
    pub manifest_path: Mutex<Option<String>>,
    pub pending_deep_link: Mutex<Option<String>>,
    pub pr_node_ids: Mutex<HashMap<String, String>>,
    // True once the frontend has finished its init handshake. Until then,
    // deep-link events get buffered into pending_deep_link for cold-start
    // replay; after that point, hot-open emits suffice and we skip buffering
    // to avoid replaying stale URLs on the next cold-start.
    pub frontend_ready: Mutex<bool>,
}

fn github_client() -> GithubClient {
    let settings = load_settings();
    let token = resolve_github_token(&settings);
    GithubClient::new(token)
}

#[command]
pub fn get_settings() -> Settings {
    load_settings()
}

#[command]
pub fn save_settings(settings: Settings) -> Result<(), String> {
    save_settings_to_disk(&settings)
}

#[command]
pub fn load_manifest(path: String) -> Result<ReviewManifest, String> {
    let content =
        fs::read_to_string(&path).map_err(|e| format!("Failed to read manifest: {}", e))?;
    let manifest: ReviewManifest =
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse manifest: {}", e))?;
    Ok(manifest)
}

#[command]
pub fn get_initial_manifest_path(state: State<AppState>) -> Option<String> {
    state.manifest_path.lock().unwrap().clone()
}

#[command]
pub fn get_pending_deep_link(state: State<AppState>) -> Option<String> {
    state.pending_deep_link.lock().ok()?.take()
}

#[command]
pub fn signal_frontend_ready(state: State<AppState>) {
    if let Ok(mut ready) = state.frontend_ready.lock() {
        *ready = true;
    }
}

#[command]
pub async fn fetch_pr(app: tauri::AppHandle, pr_ref: String) -> Result<ReviewManifest, String> {
    use tauri::Emitter;
    let settings = load_settings();
    let report = move |p: marrow_core::types::FetchProgress| {
        let _ = app.emit("fetch-progress", p);
    };
    fetch_pr_impl(&pr_ref, &settings, &report).await
}

#[command]
pub async fn check_pr_updates(
    pr_url: String,
    current_head_sha: String,
    current_comment_count: u32,
) -> Result<PrUpdateStatus, String> {
    let github = github_client();
    let parsed = marrow_core::pr_parser::parse_pr_ref(&pr_url)?;
    let (new_head_sha, new_comment_count, merged) = github
        .get_pr_status(&parsed.owner, &parsed.repo, parsed.number)
        .await?;

    let head_sha_changed = new_head_sha != current_head_sha;
    let comment_count_changed = new_comment_count != current_comment_count;

    Ok(PrUpdateStatus {
        has_changes: head_sha_changed || comment_count_changed,
        head_sha_changed,
        comment_count_changed,
        new_head_sha: if head_sha_changed { Some(new_head_sha) } else { None },
        new_comment_count: if comment_count_changed { Some(new_comment_count) } else { None },
        merged,
    })
}

#[command]
pub async fn fetch_review_requests(
    cutoff_date: String,
    fetch_recent: bool,
) -> Result<Vec<ReviewRequestItem>, String> {
    let github = github_client();
    let username = github.get_authenticated_user().await?;
    github
        .get_review_requests(&username, &cutoff_date, fetch_recent)
        .await
}

// ---- Mini-player: PR activity widget ----

#[command]
pub fn get_watches() -> Vec<Watch> {
    watches::load_watches()
}

#[command]
pub fn save_watches(watches: Vec<Watch>) -> Result<(), String> {
    marrow_core::watches::save_watches(&watches)
}

/// Acknowledge a PR in the activity feed: store its current observable state so
/// future polls diff against it (clearing the unread badge). The frontend sends
/// the fields it has from the feed item; sha/comment-count it doesn't track stay
/// `None` and simply don't contribute to future diffs.
#[command]
pub fn mark_pr_seen(pr_url: String, observed: Observed) -> Result<(), String> {
    let mut store = activity::load_activity_store();
    activity::mark_seen(&mut store, &pr_url, observed, activity::now_rfc3339());
    activity::save_activity_store(&store)
}

/// Create the floating mini-player window, left HIDDEN. Built eagerly at
/// startup (and on enable) so that the first show is never a window-creation —
/// creating a webview window activates the app, which would yank you back to
/// Marrow on your first Cmd+Tab away. The caller reveals it via
/// `set_activity_window_visible`.
pub(crate) fn build_activity_window(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder};
    use tauri_plugin_window_state::{StateFlags, WindowExt};
    let win = WebviewWindowBuilder::new(
        app,
        "activity-widget",
        WebviewUrl::App("widget.html".into()),
    )
    .title("Marrow Activity")
    .inner_size(340.0, 460.0)
    .min_inner_size(210.0, 132.0)
    .resizable(true)
    .decorations(false)
    .transparent(true)
    // macOS draws a rectangular shadow around the full (square) window
    // bounds, which shows as a square contour behind the rounded panel.
    // Disable it and let the panel's own CSS box-shadow provide the look.
    .shadow(false)
    .always_on_top(true)
    .visible(false)
    .build()
    .map_err(|e| format!("Failed to open activity window: {}", e))?;

    // Restore the last-saved size & position so it reveals at the right geometry.
    let _ = win.restore_state(StateFlags::POSITION | StateFlags::SIZE);

    // Make it a non-activating NSPanel so clicking/dragging/resizing the widget
    // doesn't activate Marrow (so interacting with it never pulls focus from the
    // app you're in). It stays hidden here; the caller orders it front.
    #[cfg(target_os = "macos")]
    {
        use tauri_nspanel::WebviewWindowExt;
        // NSWindowStyleMask bits: Resizable (1<<3) keeps edge-resize; Borderless
        // (0) = frameless; NonactivatingPanel (1<<7) is the key bit.
        const NONACTIVATING_RESIZABLE: i32 = (1 << 3) | (1 << 7); // 8 | 128 = 136
        if let Ok(panel) = win.to_panel() {
            panel.set_style_mask(NONACTIVATING_RESIZABLE);
            // NSPanels are released-when-closed by default; turn that off so a
            // stray close() can't deallocate it out from under Tauri (which
            // aborts with a foreign-exception runtime error).
            panel.set_released_when_closed(false);
        }
    }

    Ok(())
}

/// Show or hide the floating mini-player. This is the single actuator; callers
/// decide *when*: the macOS app-active poll shows it when you leave Marrow, and
/// the main window hides it when it regains focus (see App.tsx). The panel is
/// pre-created at startup, so the lazy build here is only a fallback.
#[command]
pub fn set_activity_window_visible(app: tauri::AppHandle, visible: bool) -> Result<(), String> {
    // Respect the user's opt-out: never auto-show when the feature is disabled.
    if visible && !load_settings().activity_mini_player {
        return Ok(());
    }

    // macOS: drive the NSPanel directly. order_front_regardless/order_out never
    // make the panel key, so showing it while you're in another app can't pull
    // activation back to Marrow (which made the app-active poll flip-flop).
    #[cfg(target_os = "macos")]
    {
        use tauri_nspanel::ManagerExt;
        // Normally pre-created at startup; build lazily only as a fallback.
        if visible && app.get_webview_panel("activity-widget").is_err() {
            build_activity_window(&app)?;
        }
        if let Ok(panel) = app.get_webview_panel("activity-widget") {
            if visible {
                panel.order_front_regardless();
            } else {
                panel.order_out(None);
            }
        }
        return Ok(());
    }

    #[cfg(not(target_os = "macos"))]
    {
        use tauri::Manager;
        if visible && app.get_webview_window("activity-widget").is_none() {
            build_activity_window(&app)?;
        }
        if let Some(win) = app.get_webview_window("activity-widget") {
            if visible {
                win.show().map_err(|e| e.to_string())?;
            } else {
                win.hide().map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
}

/// Toggle whether the floating mini-player auto-shows when Marrow is in the
/// background (the in-app dock's toggle writes this).
#[command]
pub fn set_mini_player_enabled(enabled: bool) -> Result<(), String> {
    let mut settings = load_settings();
    settings.activity_mini_player = enabled;
    save_settings_to_disk(&settings)
}

/// Dismiss the floating mini-player from its own ✕: persistently disable
/// auto-show (so navigating away won't bring it back) and hide the window. On
/// macOS, hide the app too so focus returns to whatever the user was in rather
/// than the main Marrow window. Re-enable via the dock toggle.
///
/// We HIDE the panel (order_out), not close()/destroy it: an NSPanel is released
/// when closed by default, and destroying it while Tauri still holds a reference
/// raises an Objective-C exception that aborts the process. The disabled setting
/// keeps it from reappearing; the hidden panel is reused if re-enabled.
#[command]
pub fn dismiss_mini_player(app: tauri::AppHandle) -> Result<(), String> {
    // Same persistent opt-out as the dock toggle, plus hide the window.
    let _ = set_mini_player_enabled(false);

    #[cfg(target_os = "macos")]
    {
        use tauri_nspanel::ManagerExt;
        if let Ok(panel) = app.get_webview_panel("activity-widget") {
            panel.order_out(None);
        }
        let _ = app.hide();
    }
    #[cfg(not(target_os = "macos"))]
    {
        use tauri::Manager;
        if let Some(win) = app.get_webview_window("activity-widget") {
            let _ = win.hide();
        }
    }
    Ok(())
}

/// Route a PR-open from the floating widget back to the main window: the main
/// window already listens for `deep-link-open` (the same channel the browser
/// deep link uses), so we just re-emit and focus it.
#[command]
pub fn open_pr_in_main(app: tauri::AppHandle, pr_ref: String) -> Result<(), String> {
    use tauri::{Emitter, Manager};
    let _ = app.emit("deep-link-open", &pr_ref);
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.set_focus();
    }
    Ok(())
}

#[command]
pub async fn fetch_review_comments(pr_url: String) -> Result<Vec<ReviewThread>, String> {
    let github = github_client();
    let parsed = marrow_core::pr_parser::parse_pr_ref(&pr_url)?;
    github.get_review_threads(&parsed.owner, &parsed.repo, parsed.number).await
}

#[command]
pub async fn reply_to_thread(
    pr_url: String,
    comment_id: String,
    body: String,
) -> Result<ReviewComment, String> {
    let github = github_client();
    let parsed = marrow_core::pr_parser::parse_pr_ref(&pr_url)?;
    let pr_node_id = github.get_pull_request_id(&parsed.owner, &parsed.repo, parsed.number).await?;
    github
        .reply_to_review_thread(&pr_node_id, &comment_id, &body)
        .await
}

#[command]
pub async fn create_review_comment(
    pr_url: String,
    body: String,
    path: String,
    line: u64,
    side: String,
    start_line: Option<u64>,
    start_side: Option<String>,
) -> Result<ReviewThread, String> {
    let github = github_client();
    let parsed = marrow_core::pr_parser::parse_pr_ref(&pr_url)?;
    let pr_node_id = github.get_pull_request_id(&parsed.owner, &parsed.repo, parsed.number).await?;
    github
        .create_review_thread(
            &pr_node_id,
            &body,
            &path,
            line,
            &side,
            start_line,
            start_side.as_deref(),
        )
        .await
}

#[command]
pub async fn update_review_comment(
    comment_id: String,
    body: String,
) -> Result<ReviewComment, String> {
    let github = github_client();
    github.update_review_comment(&comment_id, &body).await
}

#[command]
pub async fn get_my_review_state(pr_url: String) -> Result<MyReviewState, String> {
    let github = github_client();
    let parsed = marrow_core::pr_parser::parse_pr_ref(&pr_url)?;
    github
        .get_my_review_state(&parsed.owner, &parsed.repo, parsed.number)
        .await
}

#[command]
pub async fn submit_review(
    pr_url: String,
    event: String,
    body: String,
) -> Result<String, String> {
    let github = github_client();
    let parsed = marrow_core::pr_parser::parse_pr_ref(&pr_url)?;
    github
        .submit_review(&parsed.owner, &parsed.repo, parsed.number, &event, &body)
        .await
}

#[command]
pub async fn generate_review_body(
    threads_json: String,
    pr_title: String,
    has_unresolved: bool,
) -> Result<String, String> {
    let settings = load_settings();
    let region = region_from_arn(&settings.model)?;
    let bedrock = BedrockClient::new(&region, &settings.aws_profile).await?;

    let prompt = if has_unresolved {
        format!(
            r#"You are a code reviewer writing a brief review summary for a pull request titled "{}".

Here are the unresolved review comment threads (JSON):
{}

Write a concise 1-3 sentence summary of the changes you're requesting. Focus on the key themes across the comments, not individual details. Write in first person as the reviewer. Do not use markdown. Do not include a greeting or sign-off."#,
            pr_title, threads_json
        )
    } else {
        format!(
            r#"You are a code reviewer approving a pull request titled "{}".

Write a short, fun, nerdy LGTM message (1-2 sentences). Be creative — reference sci-fi, programming culture, memes, or geek humor. Vary your style. Do not use markdown. Do not include a greeting or sign-off."#,
            pr_title
        )
    };

    bedrock.invoke_model(&settings.model, &prompt).await
}

#[command]
pub async fn toggle_thread_resolved(
    thread_id: String,
    resolve: bool,
) -> Result<bool, String> {
    let github = github_client();
    github.resolve_review_thread(&thread_id, resolve).await
}

#[command]
pub async fn toggle_reaction(
    comment_id: String,
    content: String,
    add: bool,
) -> Result<(), String> {
    let github = github_client();
    github.toggle_reaction(&comment_id, &content, add).await
}

#[command]
pub async fn sync_file_viewed_to_github(
    state: State<'_, AppState>,
    pr_url: String,
    path: String,
    viewed: bool,
) -> Result<(), String> {
    let cached = state.pr_node_ids.lock().unwrap().get(&pr_url).cloned();
    let github = github_client();
    let pr_node_id = match cached {
        Some(id) => id,
        None => {
            let parsed = marrow_core::pr_parser::parse_pr_ref(&pr_url)?;
            let id = github
                .get_pull_request_id(&parsed.owner, &parsed.repo, parsed.number)
                .await?;
            state.pr_node_ids.lock().unwrap().insert(pr_url, id.clone());
            id
        }
    };
    github.mark_file_viewed(&pr_node_id, &path, viewed).await
}

#[command]
pub async fn fetch_gh_viewed_state(pr_url: String) -> Result<HashMap<String, String>, String> {
    let github = github_client();
    let parsed = marrow_core::pr_parser::parse_pr_ref(&pr_url)?;
    github
        .get_files_viewed_state(&parsed.owner, &parsed.repo, parsed.number)
        .await
}

#[command]
pub fn load_viewed_files(owner: String, repo: String, pr_number: u64) -> Option<ViewedFileState> {
    viewed_state::load_viewed_state(&owner, &repo, pr_number)
}

#[command]
pub fn save_viewed_files(
    owner: String,
    repo: String,
    pr_number: u64,
    state: ViewedFileState,
) -> Result<(), String> {
    viewed_state::save_viewed_state(&owner, &repo, pr_number, &state)
}

#[command]
pub fn load_dismissed_highlights(
    owner: String,
    repo: String,
    pr_number: u64,
) -> Option<DismissedHighlights> {
    dismissed_highlights::load_dismissed(&owner, &repo, pr_number)
}

#[command]
pub fn save_dismissed_highlights(
    owner: String,
    repo: String,
    pr_number: u64,
    state: DismissedHighlights,
) -> Result<(), String> {
    dismissed_highlights::save_dismissed(&owner, &repo, pr_number, &state)
}

#[command]
pub async fn get_pr_checks(pr_url: String) -> Result<PrChecksStatus, String> {
    let github = github_client();
    let parsed = marrow_core::pr_parser::parse_pr_ref(&pr_url)?;
    github
        .get_pr_checks(&parsed.owner, &parsed.repo, parsed.number)
        .await
}

#[command]
pub fn dismiss_checks_warning(pr_url: String) -> Result<(), String> {
    let parsed = marrow_core::pr_parser::parse_pr_ref(&pr_url)?;
    marrow_core::checks_dismiss::set_dismissed(&parsed.owner, &parsed.repo, parsed.number)
}

#[command]
pub fn is_checks_dismissed(pr_url: String) -> Result<bool, String> {
    let parsed = marrow_core::pr_parser::parse_pr_ref(&pr_url)?;
    Ok(marrow_core::checks_dismiss::is_dismissed(&parsed.owner, &parsed.repo, parsed.number))
}

#[command]
pub async fn list_cached_prs() -> Vec<CachedPrInfo> {
    let all = manifest_cache::list_cached_manifests();
    let github = github_client();

    let mut open_prs = Vec::new();
    for pr in all {
        match github.is_pr_open(&pr.owner, &pr.repo, pr.pr_number).await {
            Ok(true) => open_prs.push(pr),
            Ok(false) => {
                manifest_cache::delete_cached_manifest(&pr.owner, &pr.repo, pr.pr_number);
            }
            Err(_) => open_prs.push(pr), // keep on API failure
        }
    }
    open_prs
}

#[command]
pub fn load_cached_manifest_by_pr(pr_url: String) -> Result<Option<ReviewManifest>, String> {
    let parsed = marrow_core::pr_parser::parse_pr_ref(&pr_url)?;
    Ok(manifest_cache::load_cached_manifest(
        &parsed.owner,
        &parsed.repo,
        parsed.number,
    ))
}

#[command]
pub fn save_session(state: SessionState) -> Result<(), String> {
    session::save_session_state(&state)
}

#[command]
pub fn load_session() -> Option<SessionState> {
    session::load_session_state()
}

