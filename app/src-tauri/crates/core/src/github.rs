use crate::types::{CheckRunInfo, CommentAuthor, MyReviewState, PrChecksStatus, PrFile, PrMetadata, ReactionGroup, ReviewComment, ReviewRequestItem, ReviewThread};
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

impl GithubClient {
    pub fn new(token: Option<String>) -> Self {
        Self {
            client: Client::new(),
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
        let resp = self
            .request(url, accept)
            .send()
            .await
            .map_err(|e| format!("GitHub API request failed: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("GitHub API error ({}): {}", status, body));
        }

        Ok(resp)
    }

    /// Send a GraphQL request and return the parsed JSON response.
    /// Handles POST, headers, status check, and GraphQL-level error checking.
    async fn graphql_request(&self, body: serde_json::Value) -> Result<serde_json::Value, String> {
        let mut req = self
            .client
            .post("https://api.github.com/graphql")
            .header(USER_AGENT, "relevant-reviews");

        if !self.token.is_empty() {
            req = req.header(AUTHORIZATION, format!("Bearer {}", self.token));
        }

        let resp = req
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("GraphQL request failed: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
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

        Ok(result)
    }

    /// Lightweight check: returns (head_sha, review_comment_count) from a single REST call.
    pub async fn get_pr_status(
        &self,
        owner: &str,
        repo: &str,
        pr_number: u64,
    ) -> Result<(String, u32), String> {
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

        Ok((head_sha, comment_count))
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
            "is:pr is:open review-requested:{} -is:draft {}",
            username, date_filter
        );
        let reviewed_query = format!(
            "is:pr is:open reviewed-by:{} -author:{} -is:draft {}",
            username, username, date_filter
        );
        let commented_query = format!(
            "is:pr is:open commenter:{} -author:{} -is:draft {}",
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

        let is_re_requested = pr
            .pointer("/reviewRequests/nodes")
            .and_then(|v| v.as_array())
            .map(|nodes| is_user_in_review_requests(nodes, viewer_login))
            .unwrap_or(false);

        let status = pr
            .pointer("/reviews/nodes")
            .and_then(|v| v.as_array())
            .map(|reviews| resolve_user_review_status(reviews, viewer_login))
            .unwrap_or_else(|| "pending".to_string());

        Ok(MyReviewState {
            status,
            is_re_requested,
            is_merged,
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
            });
        }
        Ok(NotificationsResult {
            items: out,
            poll_interval,
            last_modified,
        })
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
        if let Ok(username) = self.get_authenticated_user().await {
            let cutoff = (chrono::Utc::now() - chrono::Duration::days(30))
                .format("%Y-%m-%d")
                .to_string();
            let (review_requests, notifications) = tokio::join!(
                self.get_review_requests(&username, &cutoff, true),
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

        CollectedActivity {
            observations: merged.into_values().collect(),
            truncated,
            notif_poll_interval,
            notif_last_modified,
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
}
