use crate::bedrock::{region_from_arn, BedrockClient};
use crate::config::{load_settings, resolve_github_token, save_settings_to_disk};
use crate::fetch::fetch_pr_impl;
use crate::github::GithubClient;
use crate::types::{PrUpdateStatus, ReviewComment, ReviewManifest, ReviewRequestItem, ReviewThread, Settings};
use std::fs;
use std::sync::Mutex;
use tauri::{command, State};

pub struct AppState {
    pub manifest_path: Mutex<Option<String>>,
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
pub async fn fetch_pr(app: tauri::AppHandle, pr_ref: String) -> Result<ReviewManifest, String> {
    let settings = load_settings();
    fetch_pr_impl(&pr_ref, &settings, &app).await
}

#[command]
pub async fn check_pr_updates(
    pr_url: String,
    current_head_sha: String,
    current_comment_count: u32,
) -> Result<PrUpdateStatus, String> {
    let github = github_client()?;
    let parsed = crate::pr_parser::parse_pr_ref(&pr_url)?;
    let (new_head_sha, new_comment_count) = github
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

#[command]
pub async fn fetch_review_comments(pr_url: String) -> Result<Vec<ReviewThread>, String> {
    let github = github_client();
    let parsed = crate::pr_parser::parse_pr_ref(&pr_url)?;
    github.get_review_threads(&parsed.owner, &parsed.repo, parsed.number).await
}

#[command]
pub async fn reply_to_thread(
    pr_url: String,
    comment_id: String,
    body: String,
) -> Result<ReviewComment, String> {
    let github = github_client();
    let parsed = crate::pr_parser::parse_pr_ref(&pr_url)?;
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
    let parsed = crate::pr_parser::parse_pr_ref(&pr_url)?;
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
pub async fn submit_review(
    pr_url: String,
    event: String,
    body: String,
) -> Result<String, String> {
    let github = github_client();
    let parsed = crate::pr_parser::parse_pr_ref(&pr_url)?;
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
