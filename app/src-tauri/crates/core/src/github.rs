use crate::types::{CheckAnnotation, CheckFailures, CheckRunInfo, CommentAuthor, CommitDiff, CommitDiffFile, LinkedIssue, MyReviewState, PrChecksStatus, PrCommit, PrConversationComment, PrFile, PrLabel, PrMetadata, ReactionGroup, ReviewComment, ReviewRequestItem, ReviewThread};
use std::collections::HashMap;
use base64::Engine;
use reqwest::header::{ACCEPT, AUTHORIZATION, USER_AGENT};
use reqwest::Client;
use serde::Deserialize;

pub struct GithubClient {
    client: Client,
    token: String,
}

#[derive(Deserialize)]
struct ContentsResponse {
    content: Option<String>,
    encoding: Option<String>,
}

#[derive(Deserialize)]
struct TreeResponse {
    #[serde(default)]
    tree: Vec<TreeEntry>,
}

#[derive(Deserialize)]
struct TreeEntry {
    path: String,
    #[serde(rename = "type")]
    entry_type: String,
}

/// One entry of a directory listing from the contents API.
pub struct DirEntry {
    pub name: String,
    pub entry_type: String,
    pub size: u64,
}

#[derive(Deserialize)]
struct RawDirEntry {
    name: String,
    #[serde(rename = "type")]
    entry_type: String,
    #[serde(default)]
    size: u64,
}

/// One hit from repo-scoped code search: the file path plus up to a few
/// text-match fragments (GitHub only indexes the default branch).
pub struct CodeSearchHit {
    pub path: String,
    pub fragments: Vec<String>,
}

#[derive(Deserialize)]
struct CodeSearchResponse {
    #[serde(default)]
    total_count: u32,
    items: Vec<CodeSearchItem>,
}

#[derive(Deserialize)]
struct CodeSearchItem {
    path: String,
    #[serde(default)]
    text_matches: Vec<CodeSearchTextMatch>,
}

#[derive(Deserialize)]
struct CodeSearchTextMatch {
    #[serde(default)]
    fragment: String,
}

#[derive(Deserialize)]
struct GithubUser {
    login: String,
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    total_count: u32,
    items: Vec<SearchItem>,
}

#[derive(Deserialize)]
struct SearchItem {
    number: u64,
    title: String,
    html_url: String,
    user: GithubUser,
    created_at: String,
    updated_at: String,
    draft: Option<bool>,
    pull_request: Option<SearchPullRequest>,
}

#[derive(Deserialize)]
struct SearchPullRequest {
    merged_at: Option<String>,
}

fn parse_reaction_groups(c: &serde_json::Value) -> Vec<ReactionGroup> {
    c.get("reactionGroups")
        .and_then(|v| v.as_array())
        .map(|groups| {
            groups
                .iter()
                .filter_map(|g| {
                    let content = g.get("content")?.as_str()?.to_string();
                    let total_count = g
                        .pointer("/users/totalCount")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0) as u32;
                    let viewer_has_reacted = g
                        .get("viewerHasReacted")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if total_count > 0 || viewer_has_reacted {
                        Some(ReactionGroup {
                            content,
                            total_count,
                            viewer_has_reacted,
                        })
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a single review comment from a GraphQL JSON node.
fn parse_review_comment(c: &serde_json::Value) -> Option<ReviewComment> {
    Some(ReviewComment {
        id: c.get("id")?.as_str()?.to_string(),
        body: c.get("body")?.as_str()?.to_string(),
        author: CommentAuthor {
            login: c
                .pointer("/author/login")
                .and_then(|v| v.as_str())
                .unwrap_or("ghost")
                .to_string(),
            avatar_url: c
                .pointer("/author/avatarUrl")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        },
        created_at: c.get("createdAt")?.as_str()?.to_string(),
        updated_at: c.get("updatedAt")?.as_str()?.to_string(),
        url: c.get("url")?.as_str()?.to_string(),
        reactions: parse_reaction_groups(c),
    })
}

/// Parse a review thread from a GraphQL JSON node.
fn parse_review_thread(node: &serde_json::Value) -> ReviewThread {
    let diff_hunk = node
        .pointer("/comments/nodes/0/diffHunk")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let comments: Vec<ReviewComment> = node
        .pointer("/comments/nodes")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(parse_review_comment).collect())
        .unwrap_or_default();

    ReviewThread {
        id: node.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        is_resolved: node.get("isResolved").and_then(|v| v.as_bool()).unwrap_or(false),
        is_outdated: node.get("isOutdated").and_then(|v| v.as_bool()).unwrap_or(false),
        path: node.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        line: node.get("line").and_then(|v| v.as_u64()),
        original_line: node.get("originalLine").and_then(|v| v.as_u64()),
        diff_hunk,
        comments,
    }
}

/// Check whether `username` appears as a requested reviewer in a
/// `reviewRequests { nodes { requestedReviewer { login } } }` JSON array.
fn is_user_in_review_requests(nodes: &[serde_json::Value], username: &str) -> bool {
    nodes.iter().any(|node| {
        node.pointer("/requestedReviewer/login")
            .and_then(|l| l.as_str())
            .map(|l| l.eq_ignore_ascii_case(username))
            .unwrap_or(false)
    })
}

/// Determine the latest decisive review status for `username` from a
/// `reviews { nodes { author { login } state } }` JSON array.
/// Returns one of: "pending", "approved", "changes_requested", "dismissed", "commented".
fn resolve_user_review_status(reviews: &[serde_json::Value], username: &str) -> String {
    let mut status = "pending".to_string();
    for review in reviews {
        let author = review
            .pointer("/author/login")
            .and_then(|l| l.as_str())
            .unwrap_or("");
        if !author.eq_ignore_ascii_case(username) {
            continue;
        }
        if let Some(state) = review.get("state").and_then(|s| s.as_str()) {
            match state {
                "APPROVED" | "CHANGES_REQUESTED" | "DISMISSED" => {
                    status = state.to_lowercase();
                }
                "COMMENTED" => {
                    if status == "pending" {
                        status = "commented".to_string();
                    }
                }
                _ => {}
            }
        }
    }
    status
}

/// Compute the set of reviewers whose latest opinionated review is APPROVED,
/// mirroring GitHub's "latest review per reviewer" semantics used by
/// `resolve_user_review_status`: per-author (case-insensitive identity, but
/// original casing preserved for display), track the latest state among
/// APPROVED / CHANGES_REQUESTED / DISMISSED (COMMENTED never clears a prior
/// approval; DISMISSED does). Returns authors with a final APPROVED state, in
/// first-review order.
fn resolve_approvers(reviews: &[serde_json::Value]) -> Vec<String> {
    let mut order: Vec<String> = Vec::new();
    let mut latest: HashMap<String, (String, String)> = HashMap::new(); // key(lower) -> (display, state)

    for review in reviews {
        let author = match review.pointer("/author/login").and_then(|l| l.as_str()) {
            Some(a) => a,
            None => continue,
        };
        let state = match review.get("state").and_then(|s| s.as_str()) {
            Some(s) => s,
            None => continue,
        };
        if !matches!(state, "APPROVED" | "CHANGES_REQUESTED" | "DISMISSED") {
            continue;
        }
        let key = author.to_lowercase();
        if !latest.contains_key(&key) {
            order.push(key.clone());
        }
        latest.insert(key, (author.to_string(), state.to_string()));
    }

    order
        .into_iter()
        .filter_map(|key| latest.get(&key))
        .filter(|(_, state)| state == "APPROVED")
        .map(|(display, _)| display.clone())
        .collect()
}

/// Parse the viewer's `(review_status, is_re_requested)` from a `pullRequest`
/// GraphQL node carrying `reviews { nodes { author{login} state } }` and
/// `reviewRequests { nodes { requestedReviewer { ... on User { login } } } }`.
/// Shared by `get_my_review_state` and the activity-feed enrichment so the two
/// paths can't disagree on the viewer's review state.
fn resolve_review_state(pr: &serde_json::Value, viewer: &str) -> (String, bool) {
    let is_re_requested = pr
        .pointer("/reviewRequests/nodes")
        .and_then(|v| v.as_array())
        .map(|nodes| is_user_in_review_requests(nodes, viewer))
        .unwrap_or(false);
    let status = pr
        .pointer("/reviews/nodes")
        .and_then(|v| v.as_array())
        .map(|reviews| resolve_user_review_status(reviews, viewer))
        .unwrap_or_else(|| "pending".to_string());
    (status, is_re_requested)
}

/// Extract a commit message's headline: its first line, with no trailing
/// newline (GitHub's REST commit payloads always include the full multi-line
/// message here).
fn first_line(message: &str) -> String {
    message.lines().next().unwrap_or("").to_string()
}

/// Parse one commit from GitHub's `GET .../pulls/{n}/commits` REST response.
/// `author_login`/`author_avatar` come from the top-level `author` (GitHub's
/// linked-account association), which is `null` when the commit's email isn't
/// linked to a GitHub account — fall back to the raw `commit.author.name` for
/// the login in that case (there's no avatar to fall back to).
fn parse_pr_commit(c: &serde_json::Value) -> PrCommit {
    let message = c
        .pointer("/commit/message")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let author_login = c
        .pointer("/author/login")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or_else(|| {
            c.pointer("/commit/author/name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });

    let author_avatar = c
        .pointer("/author/avatar_url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Committer date, not author date: rebases and cherry-picks preserve the
    // author date, so only the committer date reflects when a commit actually
    // landed on the branch — which is what the "since your last review"
    // fallback compares against after a force push.
    let committed_at = c
        .pointer("/commit/committer/date")
        .or_else(|| c.pointer("/commit/author/date"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    PrCommit {
        sha: c.get("sha").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        message_headline: first_line(message),
        author_login,
        author_avatar,
        committed_at,
    }
}

/// Parse one file entry from a commit's `files` array (REST `GET
/// /repos/{owner}/{repo}/commits/{sha}`).
fn parse_commit_diff_file(f: &serde_json::Value) -> CommitDiffFile {
    CommitDiffFile {
        path: f.get("filename").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        status: f.get("status").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        additions: f.get("additions").and_then(|v| v.as_u64()).unwrap_or(0),
        deletions: f.get("deletions").and_then(|v| v.as_u64()).unwrap_or(0),
        patch: f.get("patch").and_then(|v| v.as_str()).map(|s| s.to_string()),
        previous_path: f
            .get("previous_filename")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    }
}

/// Conclusions whose runs are worth scanning for annotations: outright
/// failures plus the attention states a step can reach after emitting
/// annotations (`timed_out`, `action_required`). `cancelled` is deliberately
/// excluded — a user-initiated stop's partial annotations are noise.
fn failing_conclusion(conclusion: &str) -> bool {
    conclusion.eq_ignore_ascii_case("failure")
        || conclusion.eq_ignore_ascii_case("timed_out")
        || conclusion.eq_ignore_ascii_case("action_required")
}

/// Parse one annotation from a check run's `GET
/// .../check-runs/{id}/annotations` REST response. Only "failure" and
/// "warning" levels are surfaced; "notice" is dropped. Missing or malformed
/// required fields skip the annotation rather than erroring.
fn parse_check_annotation(v: &serde_json::Value, check_name: &str) -> Option<CheckAnnotation> {
    let path = v.get("path").and_then(|v| v.as_str())?.to_string();
    let start_line = v.get("start_line").and_then(|v| v.as_u64())?;
    let end_line = v.get("end_line").and_then(|v| v.as_u64())?;
    let annotation_level = v.get("annotation_level").and_then(|v| v.as_str())?.to_string();
    if annotation_level != "failure" && annotation_level != "warning" {
        return None;
    }
    let message = v.get("message").and_then(|v| v.as_str())?.to_string();
    let title = v.get("title").and_then(|v| v.as_str()).map(|s| s.to_string());

    Some(CheckAnnotation {
        path,
        start_line,
        end_line,
        annotation_level,
        message,
        title,
        check_name: check_name.to_string(),
    })
}

/// Find the viewer's most recent submitted review (by `submittedAt`) from a
/// `reviews { nodes { author{login} submittedAt commit{oid} } }` JSON array.
/// Returns `(sha, submitted_at)`; `None` if the viewer has no submitted
/// review (pending/unsubmitted reviews have a `null` `submittedAt` and are
/// skipped).
fn latest_viewer_review(reviews: &[serde_json::Value], username: &str) -> Option<(String, String)> {
    reviews
        .iter()
        .filter(|r| {
            r.pointer("/author/login")
                .and_then(|l| l.as_str())
                .map(|l| l.eq_ignore_ascii_case(username))
                .unwrap_or(false)
        })
        .filter_map(|r| {
            let submitted_at = r.get("submittedAt").and_then(|v| v.as_str())?.to_string();
            let sha = r.pointer("/commit/oid").and_then(|v| v.as_str())?.to_string();
            Some((sha, submitted_at))
        })
        .max_by(|a, b| a.1.cmp(&b.1))
}

/// Lowercase a `statusCheckRollup.state` GraphQL enum
/// (`SUCCESS`/`FAILURE`/`ERROR`/`PENDING`/`EXPECTED`) to the activity feed's
/// `ci_state` convention. `EXPECTED` (checks queued but not yet reported)
/// reads the same as `PENDING` to callers — both mean "not settled yet".
fn normalize_ci_state(state: &str) -> String {
    match state {
        "SUCCESS" => "success".to_string(),
        "FAILURE" => "failure".to_string(),
        "ERROR" => "error".to_string(),
        "PENDING" | "EXPECTED" => "pending".to_string(),
        other => other.to_lowercase(),
    }
}

/// Whether a GraphQL payload gets mutation retry semantics (connect-only).
/// Fails SAFE: only text that provably starts a query document — `query` or
/// the shorthand `{` — gets query semantics; anything unrecognized (leading
/// comments, fragment-first documents, missing text) is treated as a
/// mutation, because over-classifying merely loses retries while
/// under-classifying risks duplicate posts.
fn graphql_is_mutation(body: &serde_json::Value) -> bool {
    !body
        .get("query")
        .and_then(|q| q.as_str())
        .is_some_and(|q| {
            let q = q.trim_start();
            q.starts_with("query") || q.starts_with('{')
        })
}

impl GithubClient {
    pub fn new(token: Option<String>) -> Self {
        Self {
            client: crate::net::http_client().clone(),
            token: token.unwrap_or_default(),
        }
    }

    fn request(&self, url: &str, accept: &str) -> reqwest::RequestBuilder {
        let mut req = self
            .client
            .get(url)
            .header(USER_AGENT, "relevant-reviews")
            .header(ACCEPT, accept);

        if !self.token.is_empty() {
            req = req.header(AUTHORIZATION, format!("Bearer {}", self.token));
        }

        req
    }

    async fn send_checked(&self, url: &str, accept: &str) -> Result<reqwest::Response, String> {
        let mut attempt = 1u32;
        loop {
            let sent = self
                .request(url, accept)
                .timeout(crate::net::GITHUB_REQUEST_TIMEOUT)
                .send()
                .await;
            let resp = match sent {
                Ok(resp) => resp,
                Err(e) if crate::net::transient_transport_error(&e) && attempt < crate::net::MAX_ATTEMPTS => {
                    tokio::time::sleep(crate::net::backoff_delay(attempt)).await;
                    attempt += 1;
                    continue;
                }
                Err(e) => return Err(format!("GitHub API request failed: {}", e)),
            };

            let status = resp.status();
            if status.is_success() {
                return Ok(resp);
            }
            if attempt < crate::net::MAX_ATTEMPTS {
                if let Some(delay) = crate::net::retryable_response_delay(status, resp.headers(), attempt) {
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                    continue;
                }
            }
            if crate::net::primary_rate_limited(status, resp.headers()) {
                return Err(crate::net::rate_limit_message(resp.headers()));
            }
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("GitHub API error ({}): {}", status, body));
        }
    }

    /// Send a GraphQL request and return the parsed JSON response.
    /// Handles POST, headers, status check, and GraphQL-level error checking.
    ///
    /// Retry policy keys off the operation itself: a mutation retries only
    /// when the request provably never left (connect error) — anything later
    /// is ambiguous and a retry could post a duplicate comment/review.
    /// Queries retry the full transient set (connect/timeout, 5xx, 429,
    /// secondary rate limits). Classification fails SAFE: only text that
    /// provably starts a query document gets query semantics; anything
    /// unrecognized (comments, fragments-first, missing text) is treated as
    /// a mutation, because the worst case of over-classifying is lost
    /// retries, while under-classifying risks duplicate posts.
    async fn graphql_request(&self, body: serde_json::Value) -> Result<serde_json::Value, String> {
        let is_mutation = graphql_is_mutation(&body);
        let mut attempt = 1u32;
        loop {
            let mut req = self
                .client
                .post("https://api.github.com/graphql")
                .header(USER_AGENT, "relevant-reviews")
                .timeout(crate::net::GITHUB_REQUEST_TIMEOUT);

            if !self.token.is_empty() {
                req = req.header(AUTHORIZATION, format!("Bearer {}", self.token));
            }

            let sent = req.json(&body).send().await;
            let resp = match sent {
                Ok(resp) => resp,
                Err(e) => {
                    let transient = if is_mutation {
                        crate::net::unsent_transport_error(&e)
                    } else {
                        crate::net::transient_transport_error(&e)
                    };
                    if transient && attempt < crate::net::MAX_ATTEMPTS {
                        tokio::time::sleep(crate::net::backoff_delay(attempt)).await;
                        attempt += 1;
                        continue;
                    }
                    return Err(format!("GraphQL request failed: {}", e));
                }
            };

            let status = resp.status();
            if !status.is_success() {
                if !is_mutation && attempt < crate::net::MAX_ATTEMPTS {
                    if let Some(delay) = crate::net::retryable_response_delay(status, resp.headers(), attempt) {
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                        continue;
                    }
                }
                if crate::net::primary_rate_limited(status, resp.headers()) {
                    return Err(crate::net::rate_limit_message(resp.headers()));
                }
                let body = resp.text().await.unwrap_or_default();
                return Err(format!("GraphQL error ({}): {}", status, body));
            }

            let result: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| format!("Failed to parse GraphQL response: {}", e))?;

            if let Some(errors) = result.get("errors") {
                return Err(format!("GraphQL errors: {}", errors));
            }

            return Ok(result);
        }
    }

    /// Lightweight check: returns (head_sha, review_comment_count, merged) from a
    /// single REST call. `merged` lets the GUI's existing update-poll flip the
    /// "Merged" badge without a separate GraphQL round-trip per tab.
    pub async fn get_pr_status(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<(String, u32, bool), String> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls/{}",
            owner, repo, pr_number
        );

        let resp = self.send_checked(&url, "application/vnd.github.v3+json").await?;

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse PR status: {}", e))?;

        let head_sha = json
            .pointer("/head/sha")
            .and_then(|v| v.as_str())
            .ok_or("Missing head SHA in PR response")?
            .to_string();

        let comment_count = json
            .get("review_comments")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;

        let merged = json
            .get("merged")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        Ok((head_sha, comment_count, merged))
    }

    pub async fn is_pr_open(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<bool, String> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls/{}",
            owner, repo, pr_number
        );

        let resp = self
            .client
            .get(&url)
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(USER_AGENT, "relevant-reviews")
            .header(ACCEPT, "application/vnd.github.v3+json")
            .send()
            .await
            .map_err(|e| format!("GitHub API request failed: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("GitHub API error ({}): {}", status, body));
        }

        let json: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse PR state: {}", e))?;

        let state = json
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        Ok(state == "open")
    }

    pub async fn get_pr_metadata(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<PrMetadata, String> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls/{}",
            owner, repo, pr_number
        );

        let resp = self.send_checked(&url, "application/vnd.github.v3+json").await?;

        resp.json::<PrMetadata>()
            .await
            .map_err(|e| format!("Failed to parse PR metadata: {}", e))
    }

    pub async fn get_pr_files(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<Vec<PrFile>, String> {
        let mut all_files = Vec::new();
        let mut page = 1u32;

        loop {
            let url = format!(
                "https://api.github.com/repos/{}/{}/pulls/{}/files?per_page=100&page={}",
                owner, repo, pr_number, page
            );

            let resp = self.send_checked(&url, "application/vnd.github.v3+json").await?;

            let files: Vec<PrFile> = resp
                .json()
                .await
                .map_err(|e| format!("Failed to parse PR files: {}", e))?;

            if files.is_empty() {
                break;
            }

            all_files.extend(files);
            page += 1;

            // Safety: don't paginate forever
            if page > 30 {
                break;
            }
        }

        Ok(all_files)
    }

    /// This PR's commits, oldest first (GitHub's returned order).
    pub async fn get_pr_commits(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<Vec<PrCommit>, String> {
        let mut all_commits = Vec::new();
        let mut page = 1u32;

        loop {
            let url = format!(
                "https://api.github.com/repos/{}/{}/pulls/{}/commits?per_page=100&page={}",
                owner, repo, pr_number, page
            );

            let resp = self.send_checked(&url, "application/vnd.github.v3+json").await?;

            let commits: Vec<serde_json::Value> = resp
                .json()
                .await
                .map_err(|e| format!("Failed to parse PR commits: {}", e))?;

            if commits.is_empty() {
                break;
            }

            all_commits.extend(commits.iter().map(parse_pr_commit));
            page += 1;

            // Safety: don't paginate forever
            if page > 30 {
                break;
            }
        }

        Ok(all_commits)
    }

    /// A single commit's diff. GitHub caps this endpoint's file listing at
    /// 300 files; `truncated` is set when that cap is hit.
    pub async fn get_commit_diff(
        &self,
        owner: &str,
        repo: &str,
        sha: &str,
    ) -> Result<CommitDiff, String> {
        let mut all_files: Vec<CommitDiffFile> = Vec::new();
        let mut message_headline = String::new();
        let mut truncated = false;
        let mut page = 1u32;

        loop {
            let url = format!(
                "https://api.github.com/repos/{}/{}/commits/{}?per_page=100&page={}",
                owner, repo, sha, page
            );

            let resp = self.send_checked(&url, "application/vnd.github.v3+json").await?;

            let commit: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| format!("Failed to parse commit: {}", e))?;

            if page == 1 {
                let message = commit
                    .pointer("/commit/message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                message_headline = first_line(message);
            }

            let files = commit
                .get("files")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let page_len = files.len();

            for f in &files {
                if all_files.len() >= 300 {
                    truncated = true;
                    break;
                }
                all_files.push(parse_commit_diff_file(f));
            }

            // Last page: fewer than a full page (or empty) means no more pages.
            if truncated || page_len < 100 {
                break;
            }

            page += 1;

            // Safety: don't paginate forever
            if page > 30 {
                break;
            }
        }

        Ok(CommitDiff {
            sha: sha.to_string(),
            message_headline,
            files: all_files,
            truncated,
        })
    }

    /// Inline annotations from the head commit's failed check runs. Runs with
    /// an attention conclusion (see `failing_conclusion`: failure, timed_out,
    /// action_required) and at least one reported
    /// annotation are fetched. Each qualifying run's annotations are capped
    /// at 50 (one page); total accumulation is capped at 200, with
    /// `truncated` set when that cap is hit or a qualifying run couldn't be
    /// fetched because the cap was already reached.
    pub async fn get_check_annotations(
        &self,
        owner: &str,
        repo: &str,
        head_sha: &str,
    ) -> Result<CheckFailures, String> {
        const MAX_ANNOTATIONS: usize = 200;

        let mut failed_runs: Vec<(u64, String, u64)> = Vec::new();
        let mut run_list_truncated = false;
        let mut page = 1u32;

        loop {
            let url = format!(
                "https://api.github.com/repos/{}/{}/commits/{}/check-runs?per_page=100&page={}",
                owner, repo, head_sha, page
            );

            let resp = self.send_checked(&url, "application/vnd.github.v3+json").await?;

            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| format!("Failed to parse check runs: {}", e))?;

            let runs = body
                .get("check_runs")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            if runs.is_empty() {
                break;
            }

            for r in &runs {
                let conclusion = r.get("conclusion").and_then(|v| v.as_str()).unwrap_or("");
                if !failing_conclusion(conclusion) {
                    continue;
                }

                let annotations_count = r
                    .pointer("/output/annotations_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                if annotations_count == 0 {
                    continue;
                }

                let id = match r.get("id").and_then(|v| v.as_u64()) {
                    Some(id) => id,
                    None => continue,
                };
                let name = r.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                failed_runs.push((id, name, annotations_count));
            }

            page += 1;

            // Safety: don't paginate forever. Runs past this point are
            // dropped, so the result must say so.
            if page > 30 {
                run_list_truncated = true;
                break;
            }
        }

        let mut annotations: Vec<CheckAnnotation> = Vec::new();
        let mut truncated = run_list_truncated;

        for (id, name, annotations_count) in &failed_runs {
            if annotations.len() >= MAX_ANNOTATIONS {
                truncated = true;
                break;
            }
            // One page of 50 per run is a deliberate cap — but a run reporting
            // more must not let the result claim completeness.
            if *annotations_count > 50 {
                truncated = true;
            }

            let url = format!(
                "https://api.github.com/repos/{}/{}/check-runs/{}/annotations?per_page=50",
                owner, repo, id
            );

            let resp = self.send_checked(&url, "application/vnd.github.v3+json").await?;

            let items: Vec<serde_json::Value> = resp
                .json()
                .await
                .map_err(|e| format!("Failed to parse check annotations: {}", e))?;

            for item in &items {
                if annotations.len() >= MAX_ANNOTATIONS {
                    truncated = true;
                    break;
                }
                if let Some(a) = parse_check_annotation(item, name) {
                    annotations.push(a);
                }
            }
        }

        Ok(CheckFailures {
            head_sha: head_sha.to_string(),
            annotations,
            truncated,
        })
    }

    pub async fn get_pr_diff(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<String, String> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/pulls/{}",
            owner, repo, pr_number
        );

        let resp = self.send_checked(&url, "application/vnd.github.v3.diff").await?;

        resp.text()
            .await
            .map_err(|e| format!("Failed to read PR diff: {}", e))
    }

    pub async fn get_file_content(
        &self,
        owner: &str,
        repo: &str,
        path: &str,
        ref_sha: &str,
    ) -> Result<String, String> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/contents/{}?ref={}",
            owner, repo, path, ref_sha
        );

        let resp = self
            .request(&url, "application/vnd.github.v3+json")
            .send()
            .await
            .map_err(|e| format!("GitHub API request failed: {}", e))?;

        if resp.status().as_u16() == 404 {
            // File doesn't exist at this ref (added or deleted)
            return Ok(String::new());
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("GitHub API error ({}): {}", status, body));
        }

        let contents: ContentsResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse file contents: {}", e))?;

        match contents.content {
            Some(encoded) if contents.encoding.as_deref() == Some("base64") => {
                let cleaned: String = encoded.chars().filter(|c| !c.is_whitespace()).collect();
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(&cleaned)
                    .map_err(|e| format!("Failed to decode base64 content: {}", e))?;
                String::from_utf8(decoded)
                    .map_err(|e| format!("File content is not valid UTF-8: {}", e))
            }
            Some(content) => Ok(content),
            None => Ok(String::new()),
        }
    }

    /// List all blob (file) paths in the repo tree at `ref_sha`. GitHub
    /// truncates very large trees (`"truncated": true` in the response) —
    /// callers get whatever was returned; best-effort by design.
    pub async fn get_tree_paths(
        &self,
        owner: &str,
        repo: &str,
        ref_sha: &str,
    ) -> Result<Vec<String>, String> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/git/trees/{}?recursive=1",
            owner, repo, ref_sha
        );

        let resp = self.send_checked(&url, "application/vnd.github.v3+json").await?;

        let tree: TreeResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse git tree: {}", e))?;

        Ok(tree
            .tree
            .into_iter()
            .filter(|e| e.entry_type == "blob")
            .map(|e| e.path)
            .collect())
    }

    /// Search this repository's code (GitHub only indexes the default
    /// branch, so results are approximate for a PR's head — the chat agent's
    /// tool prompt tells the model to confirm with `get_file_content`).
    /// Returns (hits, server-reported total).
    pub async fn search_code(
        &self,
        owner: &str,
        repo: &str,
        query: &str,
    ) -> Result<(Vec<CodeSearchHit>, u32), String> {
        let url = format!(
            "https://api.github.com/search/code?q={}&per_page=10",
            urlencoding::encode(&format!("{query} repo:{owner}/{repo}"))
        );
        let resp = self
            .send_checked(&url, "application/vnd.github.text-match+json")
            .await?;
        let search: CodeSearchResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse code search results: {}", e))?;
        let hits = search
            .items
            .into_iter()
            .map(|item| CodeSearchHit {
                path: item.path,
                fragments: item.text_matches.into_iter().map(|m| m.fragment).collect(),
            })
            .collect();
        Ok((hits, search.total_count))
    }

    /// List a directory's entries at `ref_sha` via the contents API. `path`
    /// empty means the repo root. Mirrors `get_file_content`'s URL building —
    /// paths pass through unencoded.
    pub async fn list_dir(
        &self,
        owner: &str,
        repo: &str,
        path: &str,
        ref_sha: &str,
    ) -> Result<Vec<DirEntry>, String> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/contents/{}?ref={}",
            owner, repo, path, ref_sha
        );

        let resp = self.send_checked(&url, "application/vnd.github.v3+json").await?;
        let value: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse directory listing: {}", e))?;

        let entries: Vec<RawDirEntry> = match value {
            serde_json::Value::Array(_) => serde_json::from_value(value)
                .map_err(|e| format!("Failed to parse directory listing: {}", e))?,
            _ => return Err(format!("not a directory: {}", path)),
        };

        Ok(entries
            .into_iter()
            .map(|e| DirEntry { name: e.name, entry_type: e.entry_type, size: e.size })
            .collect())
    }

    pub async fn get_authenticated_user(&self) -> Result<String, String> {
        let resp = self
            .client
            .get("https://api.github.com/user")
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(USER_AGENT, "relevant-reviews")
            .header(ACCEPT, "application/vnd.github.v3+json")
            .send()
            .await
            .map_err(|e| format!("GitHub API request failed: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("GitHub API error ({}): {}", status, body));
        }

        let user: GithubUser = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse user: {}", e))?;
        Ok(user.login)
    }

    /// Run a `/search/issues` query, returning (items, server-reported total).
    async fn search_issues(
        &self,
        query: &str,
        sort: &str,
        order: &str,
    ) -> Result<(Vec<SearchItem>, u32), String> {
        let url = format!(
            "https://api.github.com/search/issues?q={}&sort={}&order={}&per_page=100",
            urlencoding::encode(query),
            urlencoding::encode(sort),
            urlencoding::encode(order)
        );
        let resp = self
            .send_checked(&url, "application/vnd.github.v3+json")
            .await?;
        let search: SearchResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse search results: {}", e))?;
        Ok((search.items, search.total_count))
    }

    async fn search_prs(&self, query: &str) -> Result<Vec<SearchItem>, String> {
        Ok(self.search_issues(query, "created", "asc").await?.0)
    }

    pub async fn get_review_requests(
        &self,
        username: &str,
        cutoff_date: &str,
        fetch_recent: bool,
    ) -> Result<Vec<ReviewRequestItem>, String> {
        let date_filter = if fetch_recent {
            format!("created:>={}", cutoff_date)
        } else {
            format!("created:<{}", cutoff_date)
        };

        // Search for PRs where user is a requested reviewer, has reviewed, or has commented
        // This covers: pending requests, submitted reviews, and pending reviews with comments
        let requested_query = format!(
            "is:pr is:open review-requested:{} {}",
            username, date_filter
        );
        let reviewed_query = format!(
            "is:pr is:open reviewed-by:{} -author:{} {}",
            username, username, date_filter
        );
        let commented_query = format!(
            "is:pr is:open commenter:{} -author:{} {}",
            username, username, date_filter
        );

        // Run all three searches concurrently
        let (requested_result, reviewed_result, commented_result) = tokio::join!(
            self.search_prs(&requested_query),
            self.search_prs(&reviewed_query),
            self.search_prs(&commented_query),
        );

        let requested_items = requested_result?;
        let reviewed_items = reviewed_result.unwrap_or_default();
        let commented_items = commented_result.unwrap_or_default();

        // Merge and deduplicate by URL
        let mut seen = std::collections::HashSet::new();
        let mut items: Vec<ReviewRequestItem> = Vec::new();

        for item in requested_items.into_iter().chain(reviewed_items.into_iter()).chain(commented_items.into_iter()) {
            if !seen.insert(item.html_url.clone()) {
                continue;
            }
            if let Some(ref pr) = item.pull_request {
                if pr.merged_at.is_some() {
                    continue;
                }
            }

            let parsed = crate::pr_parser::parse_pr_ref(&item.html_url)?;

            items.push(ReviewRequestItem {
                owner: parsed.owner,
                repo: parsed.repo,
                number: item.number,
                title: item.title,
                html_url: item.html_url,
                author: item.user.login,
                created_at: item.created_at,
                updated_at: item.updated_at,
                draft: item.draft.unwrap_or(false),
                direct_request: false,
                my_review_status: "pending".to_string(),
                unresolved_thread_count: 0,
                approval_count: 0,
            });
        }

        // Enrich all PRs in a single GraphQL call
        if !items.is_empty() {
            let _ = self.enrich_review_requests(&mut items, username).await;
        }

        // Sort: direct requests first, then by created_at ascending (oldest first)
        items.sort_by(|a, b| {
            b.direct_request
                .cmp(&a.direct_request)
                .then_with(|| a.created_at.cmp(&b.created_at))
        });

        Ok(items)
    }

    async fn enrich_review_requests(
        &self,
        items: &mut [ReviewRequestItem],
        username: &str,
    ) -> Result<(), String> {
        // Build a batched GraphQL query with one alias per PR
        let pr_fragment = r#"
            reviewRequests(first: 20) {
                nodes {
                    requestedReviewer {
                        ... on User { login }
                    }
                }
            }
            reviews(last: 100) {
                nodes {
                    author { login }
                    state
                }
            }
            reviewThreads(first: 100) {
                nodes { isResolved }
            }
        "#;

        let mut query_parts = Vec::new();
        for (i, item) in items.iter().enumerate() {
            query_parts.push(format!(
                "pr{}: repository(owner: \"{}\", name: \"{}\") {{ pullRequest(number: {}) {{ {} }} }}",
                i, item.owner, item.repo, item.number, pr_fragment
            ));
        }

        let query = format!("{{ {} }}", query_parts.join("\n"));
        let body = serde_json::json!({ "query": query });

        let result = self.graphql_request(body).await?;

        let data = match result.get("data") {
            Some(d) => d,
            None => return Err("No data in GraphQL response".to_string()),
        };

        for (i, item) in items.iter_mut().enumerate() {
            let key = format!("pr{}", i);
            let pr = match data.pointer(&format!("/{}/pullRequest", key)) {
                Some(pr) => pr,
                None => continue,
            };

            if let Some(nodes) = pr.pointer("/reviewRequests/nodes").and_then(|v| v.as_array()) {
                item.direct_request = is_user_in_review_requests(nodes, username);
            }

            if let Some(reviews) = pr.pointer("/reviews/nodes").and_then(|v| v.as_array()) {
                item.my_review_status = resolve_user_review_status(reviews, username);
                item.approval_count = resolve_approvers(reviews).len() as u32;
            }

            // Unresolved thread count
            if let Some(threads) = pr.pointer("/reviewThreads/nodes").and_then(|v| v.as_array()) {
                item.unresolved_thread_count = threads
                    .iter()
                    .filter(|node| {
                        node.get("isResolved")
                            .and_then(|v| v.as_bool())
                            .map(|r| !r)
                            .unwrap_or(false)
                    })
                    .count() as u32;
            }
        }

        Ok(())
    }

    pub async fn get_review_threads(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<Vec<ReviewThread>, String> {
        let mut all_threads = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let after_clause = match &cursor {
                Some(c) => format!(", after: \"{}\"", c),
                None => String::new(),
            };

            let query = format!(
                r#"{{
                    repository(owner: "{}", name: "{}") {{
                        pullRequest(number: {}) {{
                            reviewThreads(first: 100{}) {{
                                pageInfo {{ hasNextPage endCursor }}
                                nodes {{
                                    id
                                    isResolved
                                    isOutdated
                                    path
                                    line
                                    originalLine
                                    diffSide
                                    comments(first: 100) {{
                                        nodes {{
                                            id
                                            body
                                            author {{ login avatarUrl }}
                                            createdAt
                                            updatedAt
                                            url
                                            diffHunk
                                            reactionGroups {{
                                                content
                                                viewerHasReacted
                                                users {{ totalCount }}
                                            }}
                                        }}
                                    }}
                                }}
                            }}
                        }}
                    }}
                }}"#,
                owner, repo, pr_number, after_clause
            );

            let body = serde_json::json!({ "query": query });
            let result = self.graphql_request(body).await?;

            let threads_data = result
                .pointer("/data/repository/pullRequest/reviewThreads")
                .ok_or("Missing reviewThreads in response")?;

            let nodes = threads_data
                .pointer("/nodes")
                .and_then(|v| v.as_array())
                .ok_or("Missing nodes in reviewThreads")?;

            for node in nodes {
                all_threads.push(parse_review_thread(node));
            }

            let has_next = threads_data
                .pointer("/pageInfo/hasNextPage")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if has_next {
                cursor = threads_data
                    .pointer("/pageInfo/endCursor")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            } else {
                break;
            }
        }

        Ok(all_threads)
    }

    /// Issues this PR closes ("Fixes #N" references / manually linked) —
    /// GraphQL `closingIssuesReferences`. Callers treat a failure as "no
    /// linked issues" (best-effort).
    pub async fn get_linked_issues(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<Vec<LinkedIssue>, String> {
        let query = format!(
            r#"{{
                repository(owner: "{}", name: "{}") {{
                    pullRequest(number: {}) {{
                        closingIssuesReferences(first: 5) {{
                            nodes {{ number title body }}
                        }}
                    }}
                }}
            }}"#,
            owner, repo, pr_number
        );

        let body = serde_json::json!({ "query": query });
        let result = self.graphql_request(body).await?;

        let nodes = result
            .pointer("/data/repository/pullRequest/closingIssuesReferences/nodes")
            .and_then(|v| v.as_array())
            .ok_or("Missing closingIssuesReferences in response")?;

        Ok(nodes
            .iter()
            // A node without a real number is malformed — skip it rather than
            // stamping a placeholder 0 into source_issues.
            .filter_map(|node| {
                let number = node.get("number").and_then(|v| v.as_u64())?;
                Some(LinkedIssue {
                number,
                title: node
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                // body is null on issues created empty — treat as no text.
                body: node
                    .get("body")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            })})
            .collect())
    }

    pub async fn reply_to_review_thread(
        &self,
        pull_request_id: &str,
        comment_node_id: &str,
        body: &str,
    ) -> Result<ReviewComment, String> {
        let query = r#"mutation($prId: ID!, $inReplyTo: ID!, $body: String!) {
            addPullRequestReviewComment(input: {
                pullRequestId: $prId,
                inReplyTo: $inReplyTo,
                body: $body
            }) {
                comment {
                    id
                    body
                    author { login avatarUrl }
                    createdAt
                    updatedAt
                    url
                    reactionGroups {
                        content
                        viewerHasReacted
                        users { totalCount }
                    }
                }
            }
        }"#;

        let payload = serde_json::json!({
            "query": query,
            "variables": {
                "prId": pull_request_id,
                "inReplyTo": comment_node_id,
                "body": body,
            }
        });

        let result = self.graphql_request(payload).await?;

        let c = result
            .pointer("/data/addPullRequestReviewComment/comment")
            .ok_or("Missing comment in response")?;

        parse_review_comment(c).ok_or_else(|| "Failed to parse comment from response".to_string())
    }

    pub async fn resolve_review_thread(
        &self,
        thread_id: &str,
        resolve: bool,
    ) -> Result<bool, String> {
        let mutation_name = if resolve {
            "resolveReviewThread"
        } else {
            "unresolveReviewThread"
        };

        let query = format!(
            r#"mutation($threadId: ID!) {{
                {}(input: {{ threadId: $threadId }}) {{
                    thread {{ isResolved }}
                }}
            }}"#,
            mutation_name
        );

        let payload = serde_json::json!({
            "query": query,
            "variables": { "threadId": thread_id }
        });

        let result = self.graphql_request(payload).await?;

        let is_resolved = result
            .pointer(&format!("/data/{}/thread/isResolved", mutation_name))
            .and_then(|v| v.as_bool())
            .unwrap_or(resolve);

        Ok(is_resolved)
    }

    pub async fn update_review_comment(
        &self,
        comment_node_id: &str,
        body: &str,
    ) -> Result<ReviewComment, String> {
        let query = r#"mutation($commentId: ID!, $body: String!) {
            updatePullRequestReviewComment(input: {
                pullRequestReviewCommentId: $commentId,
                body: $body
            }) {
                pullRequestReviewComment {
                    id body author { login avatarUrl } createdAt updatedAt url
                    reactionGroups {
                        content
                        viewerHasReacted
                        users { totalCount }
                    }
                }
            }
        }"#;

        let payload = serde_json::json!({
            "query": query,
            "variables": {
                "commentId": comment_node_id,
                "body": body,
            }
        });

        let result = self.graphql_request(payload).await?;

        let c = result
            .pointer("/data/updatePullRequestReviewComment/pullRequestReviewComment")
            .ok_or("Missing comment in response")?;

        parse_review_comment(c).ok_or_else(|| "Failed to parse comment from response".to_string())
    }

    pub async fn create_review_thread(
        &self,
        pull_request_id: &str,
        body: &str,
        path: &str,
        line: u64,
        side: &str,
        start_line: Option<u64>,
        start_side: Option<&str>,
    ) -> Result<ReviewThread, String> {
        // Build mutation dynamically based on whether start_line is provided
        let (vars_decl, input_extra) = if start_line.is_some() {
            (
                ", $startLine: Int!, $startSide: DiffSide!",
                "\n                startLine: $startLine\n                startSide: $startSide",
            )
        } else {
            ("", "")
        };

        let query = format!(
            r#"mutation($prId: ID!, $body: String!, $path: String!, $line: Int!, $side: DiffSide!{vars_decl}) {{
            addPullRequestReviewThread(input: {{
                pullRequestId: $prId
                body: $body
                path: $path
                line: $line
                side: $side{input_extra}
            }}) {{"#
        );

        let query = format!(
            r#"{}
                thread {{
                    id
                    isResolved
                    isOutdated
                    path
                    line
                    originalLine
                    comments(first: 100) {{
                        nodes {{
                            id body author {{ login avatarUrl }} createdAt updatedAt url diffHunk
                            reactionGroups {{
                                content
                                viewerHasReacted
                                users {{ totalCount }}
                            }}
                        }}
                    }}
                }}
            }}
        }}"#,
            query
        );

        let mut variables = serde_json::json!({
            "prId": pull_request_id,
            "body": body,
            "path": path,
            "line": line,
            "side": side,
        });

        if let (Some(sl), Some(ss)) = (start_line, start_side) {
            variables["startLine"] = serde_json::json!(sl);
            variables["startSide"] = serde_json::json!(ss);
        }

        let payload = serde_json::json!({
            "query": query,
            "variables": variables,
        });

        let result = self.graphql_request(payload).await?;

        let thread = result
            .pointer("/data/addPullRequestReviewThread/thread")
            .ok_or("Missing thread in response")?;

        Ok(parse_review_thread(thread))
    }

    /// Fetch the most recent top-level conversation comments on a PR (GitHub
    /// "issue comments") — the ones `add_pr_comment` creates, not review
    /// threads (issue #185). Newest 50, oldest first.
    ///
    /// The 50-comment cap is deliberate and unpaginated: it bounds the query
    /// for the panel's supplementary section, and 50 recent comments covers
    /// any sanely-sized PR conversation. Add pagination only when a real PR
    /// demonstrates the need (decision on PR #187).
    pub async fn get_pr_conversation(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<Vec<PrConversationComment>, String> {
        let query = format!(
            r#"{{
                repository(owner: "{}", name: "{}") {{
                    pullRequest(number: {}) {{
                        comments(last: 50) {{
                            nodes {{
                                id
                                author {{ login }}
                                body
                                createdAt
                                url
                            }}
                        }}
                    }}
                }}
            }}"#,
            owner, repo, pr_number
        );

        let body = serde_json::json!({ "query": query });
        let result = self.graphql_request(body).await?;

        let nodes = result
            .pointer("/data/repository/pullRequest/comments/nodes")
            .and_then(|v| v.as_array())
            .ok_or("Missing comments in response")?;

        Ok(nodes
            .iter()
            .filter_map(|c| {
                Some(PrConversationComment {
                    id: c.get("id")?.as_str()?.to_string(),
                    author: c
                        .pointer("/author/login")
                        .and_then(|v| v.as_str())
                        .unwrap_or("ghost")
                        .to_string(),
                    body: c.get("body")?.as_str()?.to_string(),
                    created_at: c.get("createdAt")?.as_str()?.to_string(),
                    url: c.get("url")?.as_str()?.to_string(),
                })
            })
            .collect())
    }

    /// Add a top-level conversation comment to a PR (a GitHub "issue comment").
    /// This is not a review and is not anchored to a diff line. Returns the URL
    /// of the created comment.
    pub async fn add_pr_comment(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
        body: &str,
    ) -> Result<String, String> {
        let subject_id = self.get_pull_request_id(owner, repo, pr_number).await?;

        let query = r#"mutation($subjectId: ID!, $body: String!) {
            addComment(input: { subjectId: $subjectId, body: $body }) {
                commentEdge { node { url } }
            }
        }"#;

        let payload = serde_json::json!({
            "query": query,
            "variables": {
                "subjectId": subject_id,
                "body": body,
            }
        });

        let result = self.graphql_request(payload).await?;

        let url = result
            .pointer("/data/addComment/commentEdge/node/url")
            .and_then(|v| v.as_str())
            .ok_or("Missing comment URL in response")?
            .to_string();

        Ok(url)
    }

    /// Submit a pending review, or create a new review with the given event.
    /// `event` must be one of: APPROVE, REQUEST_CHANGES, COMMENT
    pub async fn submit_review(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
        event: &str,
        body: &str,
    ) -> Result<String, String> {
        // Find the viewer's pending review using the viewer query
        let query = format!(
            r#"{{
                viewer {{ login }}
                repository(owner: "{}", name: "{}") {{
                    pullRequest(number: {}) {{
                        id
                        reviews(last: 10, states: PENDING) {{
                            nodes {{ id author {{ login }} }}
                        }}
                    }}
                }}
            }}"#,
            owner, repo, pr_number
        );

        let result = self.graphql_request(serde_json::json!({ "query": query })).await?;

        let viewer_login = result
            .pointer("/data/viewer/login")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let pr_id = result
            .pointer("/data/repository/pullRequest/id")
            .and_then(|v| v.as_str())
            .ok_or("Could not find PR node ID")?
            .to_string();

        let pending_review_id = result
            .pointer("/data/repository/pullRequest/reviews/nodes")
            .and_then(|v| v.as_array())
            .and_then(|nodes| {
                nodes.iter().find(|n| {
                    n.pointer("/author/login")
                        .and_then(|l| l.as_str())
                        .map(|l| l.eq_ignore_ascii_case(viewer_login))
                        .unwrap_or(false)
                })
            })
            .and_then(|n| n.get("id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        if let Some(review_id) = pending_review_id {
            // Submit existing pending review
            let mutation = r#"mutation($reviewId: ID!, $event: PullRequestReviewEvent!, $body: String) {
                submitPullRequestReview(input: {
                    pullRequestReviewId: $reviewId
                    event: $event
                    body: $body
                }) {
                    pullRequestReview { state }
                }
            }"#;

            let payload = serde_json::json!({
                "query": mutation,
                "variables": {
                    "reviewId": review_id,
                    "event": event,
                    "body": body,
                }
            });

            let result = self.graphql_request(payload).await?;
            let state = result
                .pointer("/data/submitPullRequestReview/pullRequestReview/state")
                .and_then(|v| v.as_str())
                .unwrap_or("UNKNOWN")
                .to_string();

            Ok(state)
        } else {
            // No pending review — create a new one directly
            let mutation = r#"mutation($prId: ID!, $event: PullRequestReviewEvent!, $body: String) {
                addPullRequestReview(input: {
                    pullRequestId: $prId
                    event: $event
                    body: $body
                }) {
                    pullRequestReview { state }
                }
            }"#;

            let payload = serde_json::json!({
                "query": mutation,
                "variables": {
                    "prId": pr_id,
                    "event": event,
                    "body": body,
                }
            });

            let result = self.graphql_request(payload).await?;
            let state = result
                .pointer("/data/addPullRequestReview/pullRequestReview/state")
                .and_then(|v| v.as_str())
                .unwrap_or("UNKNOWN")
                .to_string();

            Ok(state)
        }
    }

    pub async fn toggle_reaction(
        &self,
        subject_id: &str,
        content: &str,
        add: bool,
    ) -> Result<(), String> {
        let mutation_name = if add { "addReaction" } else { "removeReaction" };

        let query = format!(
            r#"mutation($subjectId: ID!, $content: ReactionContent!) {{
                {mutation_name}(input: {{ subjectId: $subjectId, content: $content }}) {{
                    reaction {{ content }}
                }}
            }}"#
        );

        let payload = serde_json::json!({
            "query": query,
            "variables": {
                "subjectId": subject_id,
                "content": content,
            }
        });

        self.graphql_request(payload).await?;
        Ok(())
    }

    pub async fn get_pull_request_id(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<String, String> {
        let query = format!(
            r#"{{
                repository(owner: "{}", name: "{}") {{
                    pullRequest(number: {}) {{ id }}
                }}
            }}"#,
            owner, repo, pr_number
        );

        let body = serde_json::json!({ "query": query });
        let result = self.graphql_request(body).await?;

        result
            .pointer("/data/repository/pullRequest/id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "Could not find PR node ID".to_string())
    }

    /// Get the combined status of all checks on the PR's head commit.
    pub async fn get_pr_checks(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<PrChecksStatus, String> {
        let query = format!(
            r#"{{
                repository(owner: "{}", name: "{}") {{
                    pullRequest(number: {}) {{
                        commits(last: 1) {{
                            nodes {{
                                commit {{
                                    statusCheckRollup {{
                                        state
                                        contexts(first: 100) {{
                                            nodes {{
                                                ... on CheckRun {{
                                                    __typename
                                                    name
                                                    status
                                                    conclusion
                                                    detailsUrl
                                                }}
                                                ... on StatusContext {{
                                                    __typename
                                                    context
                                                    state
                                                    targetUrl
                                                }}
                                            }}
                                        }}
                                    }}
                                }}
                            }}
                        }}
                    }}
                }}
            }}"#,
            owner, repo, pr_number
        );

        let result = self
            .graphql_request(serde_json::json!({ "query": query }))
            .await?;

        let commit = result
            .pointer("/data/repository/pullRequest/commits/nodes/0/commit");

        let rollup = commit.and_then(|c| c.get("statusCheckRollup"));

        // If there's no statusCheckRollup, the PR has no checks configured
        let overall_state = rollup
            .and_then(|r| r.get("state"))
            .and_then(|v| v.as_str())
            .unwrap_or("SUCCESS")
            .to_uppercase();

        let contexts = rollup
            .and_then(|r| r.pointer("/contexts/nodes"))
            .and_then(|v| v.as_array());

        let mut check_runs = Vec::new();
        if let Some(nodes) = contexts {
            for node in nodes {
                let typename = node.get("__typename").and_then(|v| v.as_str()).unwrap_or("");
                match typename {
                    "CheckRun" => {
                        let name = node.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let status = node.get("status").and_then(|v| v.as_str()).unwrap_or("QUEUED").to_string();
                        let conclusion = node.get("conclusion").and_then(|v| v.as_str()).map(|s| s.to_string());
                        let details_url = node.get("detailsUrl").and_then(|v| v.as_str()).map(|s| s.to_string());
                        check_runs.push(CheckRunInfo { name, status, conclusion, details_url });
                    }
                    "StatusContext" => {
                        let name = node.get("context").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let state = node.get("state").and_then(|v| v.as_str()).unwrap_or("PENDING").to_string();
                        let conclusion = match state.as_str() {
                            "SUCCESS" => Some("SUCCESS".to_string()),
                            "FAILURE" | "ERROR" => Some("FAILURE".to_string()),
                            _ => None,
                        };
                        let status = match state.as_str() {
                            "PENDING" | "EXPECTED" => "IN_PROGRESS".to_string(),
                            _ => "COMPLETED".to_string(),
                        };
                        let details_url = node.get("targetUrl").and_then(|v| v.as_str()).map(|s| s.to_string());
                        check_runs.push(CheckRunInfo { name, status, conclusion, details_url });
                    }
                    _ => {}
                }
            }
        }

        // Normalize overall state to lowercase for frontend consistency
        let overall_state = match overall_state.as_str() {
            "SUCCESS" => "success".to_string(),
            "FAILURE" | "ERROR" => "failure".to_string(),
            "PENDING" | "EXPECTED" => "pending".to_string(),
            other => other.to_lowercase(),
        };

        Ok(PrChecksStatus {
            overall_state,
            check_runs,
        })
    }

    /// Mark or unmark a file as viewed on a pull request.
    pub async fn mark_file_viewed(
        &self,
        pull_request_id: &str,
        path: &str,
        viewed: bool,
    ) -> Result<(), String> {
        let mutation_name = if viewed {
            "markFileAsViewed"
        } else {
            "unmarkFileAsViewed"
        };

        let query = format!(
            r#"mutation($prId: ID!, $path: String!) {{
                {mutation_name}(input: {{ pullRequestId: $prId, path: $path }}) {{
                    pullRequest {{ id }}
                }}
            }}"#
        );

        let payload = serde_json::json!({
            "query": query,
            "variables": {
                "prId": pull_request_id,
                "path": path,
            }
        });

        self.graphql_request(payload).await?;
        Ok(())
    }

    /// Fetch the viewer's viewed state for all files in a pull request.
    /// Returns a map of file path → "VIEWED" | "UNVIEWED" | "DISMISSED".
    pub async fn get_files_viewed_state(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<HashMap<String, String>, String> {
        let mut all_files = HashMap::new();
        let mut cursor: Option<String> = None;

        loop {
            let after_clause = match &cursor {
                Some(c) => format!(", after: \"{}\"", c),
                None => String::new(),
            };

            let query = format!(
                r#"{{
                    repository(owner: "{}", name: "{}") {{
                        pullRequest(number: {}) {{
                            files(first: 100{}) {{
                                pageInfo {{ hasNextPage endCursor }}
                                nodes {{
                                    path
                                    viewerViewedState
                                }}
                            }}
                        }}
                    }}
                }}"#,
                owner, repo, pr_number, after_clause
            );

            let body = serde_json::json!({ "query": query });
            let result = self.graphql_request(body).await?;

            let files_data = result
                .pointer("/data/repository/pullRequest/files")
                .ok_or("Missing files in response")?;

            let nodes = files_data
                .pointer("/nodes")
                .and_then(|v| v.as_array())
                .ok_or("Missing nodes in files")?;

            for node in nodes {
                let path = node.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let state = node
                    .get("viewerViewedState")
                    .and_then(|v| v.as_str())
                    .unwrap_or("UNVIEWED");
                if !path.is_empty() {
                    all_files.insert(path.to_string(), state.to_string());
                }
            }

            let has_next = files_data
                .pointer("/pageInfo/hasNextPage")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if has_next {
                cursor = files_data
                    .pointer("/pageInfo/endCursor")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            } else {
                break;
            }
        }

        Ok(all_files)
    }

    /// Get the current user's review state on a PR, including whether they've
    /// been re-requested for review.
    pub async fn get_my_review_state(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<MyReviewState, String> {
        let query = format!(
            r#"{{
                viewer {{ login }}
                repository(owner: "{}", name: "{}") {{
                    pullRequest(number: {}) {{
                        merged
                        isDraft
                        mergeable
                        author {{ login }}
                        labels(first: 10) {{
                            nodes {{ name color }}
                        }}
                        reviewRequests(first: 20) {{
                            nodes {{
                                requestedReviewer {{
                                    ... on User {{ login }}
                                }}
                            }}
                        }}
                        reviews(last: 100) {{
                            nodes {{
                                author {{ login }}
                                state
                                submittedAt
                                commit {{ oid }}
                            }}
                        }}
                    }}
                }}
            }}"#,
            owner, repo, pr_number
        );

        let result = self
            .graphql_request(serde_json::json!({ "query": query }))
            .await?;

        let viewer_login = result
            .pointer("/data/viewer/login")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let pr = result
            .pointer("/data/repository/pullRequest")
            .ok_or("Could not find PR data")?;

        let is_merged = pr
            .get("merged")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let author = pr
            .pointer("/author/login")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let draft = pr
            .get("isDraft")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let (status, is_re_requested) = resolve_review_state(pr, viewer_login);

        let approved_by = pr
            .pointer("/reviews/nodes")
            .and_then(|v| v.as_array())
            .map(|reviews| resolve_approvers(reviews))
            .unwrap_or_default();

        let mergeable = pr
            .get("mergeable")
            .and_then(|v| v.as_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_default();

        let labels = pr
            .pointer("/labels/nodes")
            .and_then(|v| v.as_array())
            .map(|nodes| {
                nodes
                    .iter()
                    .filter_map(|n| {
                        let name = n.get("name").and_then(|v| v.as_str())?.to_string();
                        let color = n
                            .get("color")
                            .and_then(|v| v.as_str())
                            .unwrap_or_default()
                            .to_string();
                        Some(PrLabel { name, color })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let (last_reviewed_sha, last_reviewed_at) = pr
            .pointer("/reviews/nodes")
            .and_then(|v| v.as_array())
            .and_then(|reviews| latest_viewer_review(reviews, viewer_login))
            .map(|(sha, at)| (Some(sha), Some(at)))
            .unwrap_or((None, None));

        Ok(MyReviewState {
            status,
            is_re_requested,
            is_merged,
            author,
            draft,
            approved_by,
            mergeable,
            labels,
            last_reviewed_sha,
            last_reviewed_at,
        })
    }
}

/// GitHub serves an avatar (with a redirect) at `https://github.com/<login>.png`.
/// Using it avoids an extra API call just to render a face in the activity feed.
fn avatar_for(login: &str) -> String {
    if login.is_empty() {
        String::new()
    } else {
        format!("https://github.com/{}.png?size=40", login)
    }
}

/// Convert a notifications `subject.url`
/// (`https://api.github.com/repos/{owner}/{repo}/pulls/{n}`) into the canonical
/// HTML PR url so it keys the same as search/review-request results.
fn pulls_api_to_html(api_url: &str) -> Option<String> {
    let rest = api_url.strip_prefix("https://api.github.com/repos/")?;
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() == 4 && parts[2] == "pulls" {
        Some(format!(
            "https://github.com/{}/{}/pull/{}",
            parts[0], parts[1], parts[3]
        ))
    } else {
        None
    }
}

fn search_item_to_observed(
    item: SearchItem,
    reason: String,
) -> Option<crate::activity::ObservedPr> {
    if let Some(pr) = &item.pull_request {
        if pr.merged_at.is_some() {
            return None;
        }
    }
    let parsed = crate::pr_parser::parse_pr_ref(&item.html_url).ok()?;
    let avatar_url = avatar_for(&item.user.login);
    Some(crate::activity::ObservedPr {
        pr_url: item.html_url,
        owner: parsed.owner,
        repo: parsed.repo,
        number: item.number,
        title: item.title,
        author: item.user.login,
        avatar_url,
        draft: item.draft.unwrap_or(false),
        reasons: vec![reason],
        observed: crate::activity::Observed {
            updated_at: item.updated_at,
            ..Default::default()
        },
        last_actor: None,
        is_re_requested: false,
    })
}

fn review_item_to_observed(
    item: crate::types::ReviewRequestItem,
    reason: &str,
) -> crate::activity::ObservedPr {
    let avatar_url = avatar_for(&item.author);
    crate::activity::ObservedPr {
        pr_url: item.html_url,
        owner: item.owner,
        repo: item.repo,
        number: item.number,
        title: item.title,
        author: item.author,
        avatar_url,
        draft: item.draft,
        reasons: vec![reason.to_string()],
        observed: crate::activity::Observed {
            updated_at: item.updated_at,
            review_state: Some(item.my_review_status),
            unresolved_threads: Some(item.unresolved_thread_count),
            ..Default::default()
        },
        last_actor: None,
        is_re_requested: false,
    }
}

/// Merge an observation into the de-duplicated map keyed by PR url: union the
/// `reasons`, keep the freshest `updated_at`, and fill any observable field the
/// existing entry is still missing (so a rich review-request entry isn't
/// clobbered by a bare watch-search hit, or vice-versa).
fn merge_observation(
    map: &mut HashMap<String, crate::activity::ObservedPr>,
    incoming: crate::activity::ObservedPr,
) {
    match map.get_mut(&incoming.pr_url) {
        Some(existing) => {
            for r in incoming.reasons {
                if !existing.reasons.contains(&r) {
                    existing.reasons.push(r);
                }
            }
            let o = &mut existing.observed;
            let i = incoming.observed;
            if i.updated_at > o.updated_at {
                o.updated_at = i.updated_at;
            }
            o.review_state = o.review_state.take().or(i.review_state);
            o.unresolved_threads = o.unresolved_threads.or(i.unresolved_threads);
            o.head_sha = o.head_sha.take().or(i.head_sha);
            o.comment_count = o.comment_count.or(i.comment_count);
            o.ci_state = o.ci_state.take().or(i.ci_state);
            if existing.author.is_empty() {
                existing.author = incoming.author;
                existing.avatar_url = incoming.avatar_url;
            }
            if existing.title.is_empty() {
                existing.title = incoming.title;
            }
        }
        None => {
            map.insert(incoming.pr_url.clone(), incoming);
        }
    }
}

impl GithubClient {
    /// Like `search_prs` but ordered by recency and returning the server total
    /// so callers can report truncation honestly when a watch matches more PRs
    /// than one page (or the feed cap) shows.
    async fn search_prs_total(&self, query: &str) -> Result<(Vec<SearchItem>, u32), String> {
        self.search_issues(query, "updated", "desc").await
    }

    /// Fetch the viewer's PR notifications (only PR-type, unread). Each maps to
    /// an observation with reason `notification:<reason>`.
    ///
    /// Honors conditional requests: pass the previous response's `Last-Modified`
    /// as `if_modified_since` and a `304 Not Modified` (no items, free against
    /// the rate limit) comes back. The returned `poll_interval` is GitHub's
    /// `X-Poll-Interval` — the minimum seconds to wait before polling again.
    pub async fn get_notifications(
        &self,
        if_modified_since: Option<&str>,
    ) -> Result<NotificationsResult, String> {
        let mut req = self.request(
            "https://api.github.com/notifications?all=false&per_page=50",
            "application/vnd.github.v3+json",
        );
        if let Some(ims) = if_modified_since {
            req = req.header(reqwest::header::IF_MODIFIED_SINCE, ims);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| format!("Notifications request failed: {}", e))?;

        let poll_interval = resp
            .headers()
            .get("x-poll-interval")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());
        let last_modified = resp
            .headers()
            .get(reqwest::header::LAST_MODIFIED)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(NotificationsResult {
                items: Vec::new(),
                poll_interval,
                last_modified,
            });
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("GitHub API error ({}): {}", status, body));
        }

        let arr: Vec<serde_json::Value> = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse notifications: {}", e))?;

        let mut out = Vec::new();
        for n in arr {
            if n.pointer("/subject/type").and_then(|v| v.as_str()) != Some("PullRequest") {
                continue;
            }
            let api_url = n
                .pointer("/subject/url")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let Some(html_url) = pulls_api_to_html(api_url) else {
                continue;
            };
            let Ok(parsed) = crate::pr_parser::parse_pr_ref(&html_url) else {
                continue;
            };
            let title = n
                .pointer("/subject/title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let updated_at = n
                .get("updated_at")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let reason = n.get("reason").and_then(|v| v.as_str()).unwrap_or("notification");
            out.push(crate::activity::ObservedPr {
                pr_url: html_url,
                owner: parsed.owner,
                repo: parsed.repo,
                number: parsed.number,
                title,
                author: String::new(),
                avatar_url: String::new(),
                draft: false,
                reasons: vec![format!("notification:{}", reason)],
                observed: crate::activity::Observed {
                    updated_at,
                    ..Default::default()
                },
                last_actor: None,
                is_re_requested: false,
            });
        }
        Ok(NotificationsResult {
            items: out,
            poll_interval,
            last_modified,
        })
    }

    /// Enrich the merged feed with per-PR data the search/notification sources
    /// can't give us, in batched GraphQL queries (best-effort):
    ///  - the viewer's own review status + whether a re-review is currently
    ///    requested → `review_state` / `is_re_requested`, so `compute_activity`
    ///    can hide approved PRs while keeping re-requested ones visible;
    ///  - the latest comment's author → `last_actor`, so the viewer's own comment
    ///    doesn't read as an "updated" delta;
    ///  - `merged` → drop PRs merged out from under a stale notification.
    ///
    /// Chunks run concurrently (the upstream sources already do; don't regress to
    /// serial here).
    async fn enrich_observations(
        &self,
        observations: &mut Vec<crate::activity::ObservedPr>,
        viewer: &str,
    ) {
        if observations.is_empty() {
            return;
        }
        const CHUNK: usize = 40;
        let pr_fragment = r#"
            merged
            reviewRequests(first: 20) { nodes { requestedReviewer { ... on User { login } } } }
            reviews(last: 100) { nodes { author { login } state } }
            comments(last: 1) { nodes { author { login } } }
            commits(last: 1) { nodes { commit { statusCheckRollup { state } } } }
        "#;

        // Build one query per chunk from immutable reads, then fire them all at once.
        let queries: Vec<String> = observations
            .chunks(CHUNK)
            .map(|chunk| {
                let parts: Vec<String> = chunk
                    .iter()
                    .enumerate()
                    .map(|(i, o)| {
                        format!(
                            "pr{}: repository(owner: \"{}\", name: \"{}\") {{ pullRequest(number: {}) {{ {} }} }}",
                            i, o.owner, o.repo, o.number, pr_fragment
                        )
                    })
                    .collect();
                format!("{{ {} }}", parts.join("\n"))
            })
            .collect();

        let results = futures::future::join_all(
            queries
                .into_iter()
                .map(|q| self.graphql_request(serde_json::json!({ "query": q }))),
        )
        .await;

        // Apply each chunk's response to its own slice of observations — the chunk
        // *is* the slice, so positional aliases (pr0..) map 1:1 with no index
        // math. Collect merged PRs to drop after (can't remove from a &mut slice
        // mid-iteration).
        let mut merged_urls: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (chunk, result) in observations.chunks_mut(CHUNK).zip(results) {
            let Ok(result) = result else { continue };
            let Some(data) = result.get("data") else { continue };
            for (i, o) in chunk.iter_mut().enumerate() {
                let Some(pr) = data.pointer(&format!("/pr{}/pullRequest", i)) else {
                    continue;
                };
                if pr.get("merged").and_then(|v| v.as_bool()).unwrap_or(false) {
                    merged_urls.insert(o.pr_url.clone());
                    continue;
                }
                let (status, is_re_requested) = resolve_review_state(pr, viewer);
                o.observed.review_state = Some(status);
                o.is_re_requested = is_re_requested;
                if let Some(login) = pr
                    .pointer("/comments/nodes/0/author/login")
                    .and_then(|v| v.as_str())
                {
                    o.last_actor = Some(login.to_string());
                }
                if let Some(state) = pr
                    .pointer("/commits/nodes/0/commit/statusCheckRollup/state")
                    .and_then(|v| v.as_str())
                {
                    o.observed.ci_state = Some(normalize_ci_state(state));
                }
            }
        }
        observations.retain(|o| !merged_urls.contains(&o.pr_url));
    }

    /// Gather the full activity set for the mini-player: involved PRs (review
    /// requests, enriched), notifications, and each saved watch's search. Merges
    /// and de-duplicates by PR url, and returns per-watch truncation counts plus
    /// the notification poll-interval / last-modified for adaptive scheduling.
    ///
    /// Best-effort: a failure of any one source is swallowed so the others still
    /// produce a feed. `notif_since` is forwarded as the conditional
    /// `If-Modified-Since` for the notifications fetch.
    pub async fn collect_activity(
        &self,
        watches: &[crate::watches::Watch],
        per_watch_cap: usize,
        notif_since: Option<&str>,
    ) -> CollectedActivity {
        let mut merged: HashMap<String, crate::activity::ObservedPr> = HashMap::new();
        let mut truncated: HashMap<String, u32> = HashMap::new();
        let mut notif_poll_interval = None;
        let mut notif_last_modified = None;

        // Involved PRs + notifications require a known viewer. They're
        // independent, so fetch them concurrently once we have the username.
        let viewer = self.get_authenticated_user().await.ok();
        if let Some(username) = viewer.as_deref() {
            let cutoff = (chrono::Utc::now() - chrono::Duration::days(30))
                .format("%Y-%m-%d")
                .to_string();
            let (review_requests, notifications) = tokio::join!(
                self.get_review_requests(username, &cutoff, true),
                self.get_notifications(notif_since),
            );
            if let Ok(items) = review_requests {
                for it in items {
                    let reason = if it.direct_request {
                        "review-requested"
                    } else {
                        "involved"
                    };
                    merge_observation(&mut merged, review_item_to_observed(it, reason));
                }
            }
            if let Ok(res) = notifications {
                notif_poll_interval = res.poll_interval;
                notif_last_modified = res.last_modified;
                for pr in res.items {
                    merge_observation(&mut merged, pr);
                }
            }
        }

        // Saved watches (work even where the viewer isn't a reviewer).
        for w in watches {
            if let Ok((items, total)) = self.search_prs_total(&w.query).await {
                let returned = items.len();
                let label = format!("watching:{}", w.label);
                for item in items.into_iter().take(per_watch_cap) {
                    if let Some(pr) = search_item_to_observed(item, label.clone()) {
                        merge_observation(&mut merged, pr);
                    }
                }
                let shown = per_watch_cap.min(returned);
                let dropped = (total as usize).saturating_sub(shown);
                if dropped > 0 {
                    truncated.insert(w.label.clone(), dropped as u32);
                }
            }
        }

        // Enrich the deduped set in one batched query (review status, latest
        // comment author, merged-state) so features that need per-PR detail work
        // across all sources, not just review-requested PRs.
        let mut observations: Vec<crate::activity::ObservedPr> = merged.into_values().collect();
        if let Some(username) = viewer.as_deref() {
            self.enrich_observations(&mut observations, username).await;
        }

        CollectedActivity {
            observations,
            truncated,
            notif_poll_interval,
            notif_last_modified,
            viewer,
        }
    }
}

/// Result of a notifications fetch, carrying the conditional-request metadata
/// callers need to poll politely.
pub struct NotificationsResult {
    pub items: Vec<crate::activity::ObservedPr>,
    pub poll_interval: Option<u64>,
    pub last_modified: Option<String>,
}

/// The merged activity set plus notification scheduling metadata.
pub struct CollectedActivity {
    pub observations: Vec<crate::activity::ObservedPr>,
    pub truncated: HashMap<String, u32>,
    pub notif_poll_interval: Option<u64>,
    pub notif_last_modified: Option<String>,
    /// The authenticated user's login (when a token resolved), so the app layer
    /// can suppress the viewer's own-comment "updated" deltas in compute_activity.
    pub viewer: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{failing_conclusion, first_line, graphql_is_mutation, latest_viewer_review, parse_check_annotation, parse_pr_commit};

    #[test]
    fn graphql_mutation_classification_fails_safe() {
        let q = |s: &str| serde_json::json!({ "query": s });
        // The two shapes every in-repo query uses get query semantics.
        assert!(!graphql_is_mutation(&q("query($id: ID!) { node(id: $id) { id } }")));
        assert!(!graphql_is_mutation(&q("{ viewer { login } }")));
        assert!(!graphql_is_mutation(&q("  \n{ viewer { login } }")));
        // Mutations are mutations.
        assert!(graphql_is_mutation(&q("mutation($id: ID!) { addComment(input: {}) { id } }")));
        // Anything unrecognized fails SAFE to mutation semantics: leading
        // comments, fragment-first documents, or a missing/odd query field.
        assert!(graphql_is_mutation(&q("# a comment\nmutation { x }")));
        assert!(graphql_is_mutation(&q("fragment F on PR { id } query { ...F }")));
        assert!(graphql_is_mutation(&serde_json::json!({})));
        assert!(graphql_is_mutation(&serde_json::json!({ "query": 42 })));
    }

    #[test]
    fn first_line_extracts_headline_from_multi_line_message() {
        assert_eq!(first_line("fix bug\n\nLonger explanation here."), "fix bug");
    }

    #[test]
    fn first_line_handles_trailing_newline() {
        assert_eq!(first_line("fix bug\n"), "fix bug");
    }

    #[test]
    fn first_line_handles_single_line_message() {
        assert_eq!(first_line("fix bug"), "fix bug");
    }

    #[test]
    fn latest_viewer_review_picks_most_recent_submitted_at() {
        let reviews: Vec<serde_json::Value> = serde_json::from_str(
            r#"[
                {"author": {"login": "alice"}, "submittedAt": "2026-07-01T00:00:00Z", "commit": {"oid": "sha1"}},
                {"author": {"login": "alice"}, "submittedAt": "2026-07-15T00:00:00Z", "commit": {"oid": "sha2"}},
                {"author": {"login": "bob"}, "submittedAt": "2026-07-20T00:00:00Z", "commit": {"oid": "sha3"}}
            ]"#,
        )
        .unwrap();

        let result = latest_viewer_review(&reviews, "alice");
        assert_eq!(result, Some(("sha2".to_string(), "2026-07-15T00:00:00Z".to_string())));
    }

    #[test]
    fn latest_viewer_review_none_when_viewer_has_no_review() {
        let reviews: Vec<serde_json::Value> = serde_json::from_str(
            r#"[{"author": {"login": "bob"}, "submittedAt": "2026-07-20T00:00:00Z", "commit": {"oid": "sha3"}}]"#,
        )
        .unwrap();

        assert_eq!(latest_viewer_review(&reviews, "alice"), None);
    }

    #[test]
    fn latest_viewer_review_skips_pending_unsubmitted_reviews() {
        // A pending (draft) review has no submittedAt yet.
        let reviews: Vec<serde_json::Value> = serde_json::from_str(
            r#"[{"author": {"login": "alice"}, "submittedAt": null, "commit": {"oid": "sha1"}}]"#,
        )
        .unwrap();

        assert_eq!(latest_viewer_review(&reviews, "alice"), None);
    }

    #[test]
    fn parse_pr_commit_prefers_committer_date_over_author_date() {
        // Rebases keep the author date but stamp a fresh committer date; the
        // "since your last review" fallback needs the latter.
        let c: serde_json::Value = serde_json::from_str(
            r#"{"sha": "abc", "commit": {"message": "m", "author": {"name": "a", "date": "2026-01-01T00:00:00Z"}, "committer": {"date": "2026-02-02T00:00:00Z"}}}"#,
        )
        .unwrap();

        assert_eq!(parse_pr_commit(&c).committed_at, "2026-02-02T00:00:00Z");
    }

    #[test]
    fn failing_conclusion_covers_attention_states_but_not_cancelled() {
        assert!(failing_conclusion("failure"));
        assert!(failing_conclusion("FAILURE"));
        assert!(failing_conclusion("timed_out"));
        assert!(failing_conclusion("action_required"));
        assert!(!failing_conclusion("cancelled"));
        assert!(!failing_conclusion("success"));
        assert!(!failing_conclusion("neutral"));
        assert!(!failing_conclusion(""));
    }

    #[test]
    fn parse_pr_commit_falls_back_to_author_date() {
        let c: serde_json::Value = serde_json::from_str(
            r#"{"sha": "abc", "commit": {"message": "m", "author": {"name": "a", "date": "2026-01-01T00:00:00Z"}}}"#,
        )
        .unwrap();

        assert_eq!(parse_pr_commit(&c).committed_at, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn parse_check_annotation_parses_all_fields() {
        let a: serde_json::Value = serde_json::from_str(
            r#"{"path": "src/main.rs", "start_line": 10, "end_line": 12, "annotation_level": "failure", "message": "unused variable", "title": "clippy"}"#,
        )
        .unwrap();

        let parsed = parse_check_annotation(&a, "clippy-check").unwrap();
        assert_eq!(parsed.path, "src/main.rs");
        assert_eq!(parsed.start_line, 10);
        assert_eq!(parsed.end_line, 12);
        assert_eq!(parsed.annotation_level, "failure");
        assert_eq!(parsed.message, "unused variable");
        assert_eq!(parsed.title, Some("clippy".to_string()));
        assert_eq!(parsed.check_name, "clippy-check");
    }

    #[test]
    fn parse_check_annotation_null_title_becomes_none() {
        let a: serde_json::Value = serde_json::from_str(
            r#"{"path": "src/main.rs", "start_line": 10, "end_line": 12, "annotation_level": "warning", "message": "msg", "title": null}"#,
        )
        .unwrap();

        assert_eq!(parse_check_annotation(&a, "check").unwrap().title, None);
    }

    #[test]
    fn parse_check_annotation_missing_start_line_is_skipped() {
        let a: serde_json::Value = serde_json::from_str(
            r#"{"path": "src/main.rs", "end_line": 12, "annotation_level": "failure", "message": "msg"}"#,
        )
        .unwrap();

        assert!(parse_check_annotation(&a, "check").is_none());
    }

    #[test]
    fn parse_check_annotation_drops_notice_level() {
        let a: serde_json::Value = serde_json::from_str(
            r#"{"path": "src/main.rs", "start_line": 1, "end_line": 1, "annotation_level": "notice", "message": "msg"}"#,
        )
        .unwrap();

        assert!(parse_check_annotation(&a, "check").is_none());
    }

    #[test]
    fn parse_check_annotation_keeps_failure_and_warning_levels() {
        let failure: serde_json::Value = serde_json::from_str(
            r#"{"path": "a.rs", "start_line": 1, "end_line": 1, "annotation_level": "failure", "message": "msg"}"#,
        )
        .unwrap();
        let warning: serde_json::Value = serde_json::from_str(
            r#"{"path": "a.rs", "start_line": 1, "end_line": 1, "annotation_level": "warning", "message": "msg"}"#,
        )
        .unwrap();

        assert!(parse_check_annotation(&failure, "check").is_some());
        assert!(parse_check_annotation(&warning, "check").is_some());
    }
}
