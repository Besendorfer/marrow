//! Read-only repo tools shared by the chat agent (issue #150) and the
//! review pass (issue #232): the `marrow-tool` call shapes, the rules that
//! keep model-controlled arguments inside their allowed scope, and the
//! backends that execute them — GitHub, a local clone, or an in-memory
//! snapshot (tests and the corpus eval).
//!
//! Scope is enforced HERE, never trusted from the model: chat stays on the
//! PR's repo at its head; the review may also read the base commit and any
//! repo owned by the PR's owner (user decision on #232), and nothing else.

use crate::chat::truncate;
use crate::github::GithubClient;
use crate::local_repo::LocalClone;
use std::collections::HashMap;
use std::sync::Mutex;

/// Per-result char cap — matches chat.rs PER_FILE_CONTENT_BUDGET.
pub const TOOL_RESULT_BUDGET: usize = 8000;
const LIST_DIR_MAX_ENTRIES: usize = 200;

/// One read-only repo tool call the model can request via a ```marrow-tool
/// fence. The optional fields are review-only extensions (issue #232);
/// chat's protocol text documents only the base shapes, and chat's scope
/// rejects the extensions at execution time.
// Kept in sync BY HAND with CHAT_REPO_TOOLS (chat.rs) and the ChatToolCall
// union (app/src/types.ts) — edit all three together.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case", deny_unknown_fields)]
pub enum ToolCall {
    ReadFile {
        path: String,
        /// Another repo under the PR's owner: `name` or `owner/name`.
        #[serde(default)]
        repo: Option<String>,
        /// "head" (default) or "base" — PR repo only.
        #[serde(default, rename = "ref")]
        rev: Option<String>,
    },
    SearchCode {
        query: String,
        /// "repo" (default) or "org" — every repo under the PR's owner.
        #[serde(default)]
        scope: Option<String>,
    },
    ListDir {
        #[serde(default)]
        path: String,
        #[serde(default)]
        repo: Option<String>,
    },
}

impl ToolCall {
    /// Transient status shown via `StreamUpdate::Status` while the tool runs.
    pub fn status_label(&self) -> String {
        match self {
            ToolCall::ReadFile { path, repo, .. } => match repo {
                Some(r) => format!("Reading {r}/{path}…"),
                None => format!("Reading {path}…"),
            },
            ToolCall::SearchCode { query, .. } => format!("Searching code for \u{201c}{query}\u{201d}…"),
            ToolCall::ListDir { path, .. } => {
                let where_ = if path.is_empty() { "repo root" } else { path.as_str() };
                format!("Listing {where_}…")
            }
        }
    }
}

/// Parse a ```marrow-tool fence body into a [`ToolCall`]. Serde's
/// `deny_unknown_fields` rejects unknown tools/fields; this additionally
/// rejects an empty `path`/`query`, which the documented protocol never
/// emits but a model could still produce.
pub fn parse_tool_call(json: &str) -> Result<ToolCall, String> {
    let call: ToolCall = serde_json::from_str(json).map_err(|e| e.to_string())?;
    match &call {
        ToolCall::ReadFile { path, .. } if path.trim().is_empty() => {
            Err("read_file requires a non-empty path".to_string())
        }
        ToolCall::SearchCode { query, .. } if query.trim().is_empty() => {
            Err("search_code requires a non-empty query".to_string())
        }
        _ => Ok(call),
    }
}

/// Strip GitHub search qualifiers that would widen a model-supplied query
/// (`repo:`, `org:`, `user:`) — the executor appends its own scope, and model
/// input must not be able to add more. Search qualifiers OR together, so a
/// prompt-injected `repo:other/repo` would otherwise reach anything the
/// token can read.
pub fn sanitize_search_query(q: &str) -> String {
    q.split_whitespace()
        .filter(|t| {
            let t = t.to_ascii_lowercase();
            !(t.starts_with("repo:") || t.starts_with("org:") || t.starts_with("user:"))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Reject model-supplied repo paths that could rewrite the API request:
/// absolute paths, `.`/`..` segments (endpoint traversal), backslashes, and
/// the URL metacharacters `?`/`#` (which would displace the `ref` query that
/// pins reads to a commit). Also `:`, which would re-address a local
/// `git show <sha>:<path>` spec.
pub fn validate_repo_path(path: &str) -> Result<(), String> {
    if path.starts_with('/')
        || path.contains('\\')
        || path.contains('?')
        || path.contains('#')
        || path.contains(':')
        || path.split('/').any(|seg| seg == "." || seg == "..")
    {
        return Err(format!("invalid path: {path}"));
    }
    Ok(())
}

/// Which repo (and commits) the tools serve — always the PR under review.
pub struct RepoToolTarget {
    pub owner: String,
    pub repo: String,
    pub head_sha: String,
    /// Empty for chat (base reads are review-only).
    pub base_sha: String,
}

/// What a caller may reach beyond the PR repo at its head.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToolScope {
    /// `ref: "base"` reads.
    pub base_ref: bool,
    /// `repo:` reads/listings and `scope: "org"` searches under the PR's owner.
    pub same_owner: bool,
}

impl ToolScope {
    pub const CHAT: ToolScope = ToolScope { base_ref: false, same_owner: false };
    pub const REVIEW: ToolScope = ToolScope { base_ref: true, same_owner: true };
}

/// Resolve a model-supplied `repo` argument against the scope rule. Returns
/// the repo NAME to read (`None` = the PR's own repo). Accepts `name` or
/// `owner/name`; the owner must be the PR's owner (case-insensitive).
pub fn resolve_repo(target: &RepoToolTarget, scope: ToolScope, repo: Option<&str>) -> Result<Option<String>, String> {
    let Some(raw) = repo.map(str::trim).filter(|r| !r.is_empty()) else { return Ok(None) };
    let (owner, name) = match raw.split_once('/') {
        Some((o, n)) => (o, n),
        None => (target.owner.as_str(), raw),
    };
    let name_ok = !name.is_empty()
        && name != "."
        && name != ".."
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !owner.eq_ignore_ascii_case(&target.owner) || !name_ok {
        return Err(format!(
            "repo \"{raw}\" is out of scope — only repos owned by {} are readable",
            target.owner
        ));
    }
    if name.eq_ignore_ascii_case(&target.repo) {
        return Ok(None);
    }
    if !scope.same_owner {
        return Err("reading other repositories isn't available here — only this PR's repo".to_string());
    }
    Ok(Some(name.to_string()))
}

/// One thing the review read or searched (issue #232) — persisted on the
/// manifest as "Context used" so conclusions stay auditable.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ContextRead {
    /// `owner/name`.
    pub repo: String,
    /// File or directory path; the query text for searches.
    pub path: String,
    /// "head" | "base" | "default" (another repo's default branch) | "owner"
    /// (owner-wide search).
    pub rev: String,
    /// "read_file" | "search_code" | "list_dir".
    pub tool: String,
}

/// An in-memory repo snapshot: `files[repo_name][rev][path] = content`,
/// where `rev` is "head" | "base" for the PR repo and "default" for others.
/// Serves tests and the corpus eval's fixture `repo/` snapshots.
#[derive(Debug, Default, Clone)]
pub struct SnapshotRepo {
    pub pr_repo: String,
    pub files: HashMap<String, HashMap<String, HashMap<String, String>>>,
}

impl SnapshotRepo {
    fn files_at(&self, repo: &str, rev: &str) -> Option<&HashMap<String, String>> {
        self.files.get(repo).and_then(|r| r.get(rev))
    }

    /// Like `files_at`, but a repo/rev the snapshot doesn't carry is an
    /// error: "not captured here" must never read as "doesn't exist" — the
    /// model would treat an empty search as evidence.
    fn require(&self, repo: &str, rev: &str) -> Result<&HashMap<String, String>, String> {
        self.files_at(repo, rev)
            .ok_or_else(|| format!("contents of {repo} ({rev}) are unavailable in this environment — don't treat this as the code not existing"))
    }
}

/// Where tool calls are executed.
pub enum ToolBackend<'a> {
    Github(&'a GithubClient),
    /// A local clone serving the PR repo at its SHAs (see local_repo.rs);
    /// other repos still go to GitHub.
    Local { clone: LocalClone, github: &'a GithubClient },
    Snapshot(&'a SnapshotRepo),
}

/// Executes tool calls under a scope and records what was read.
pub struct ToolExecutor<'a> {
    pub backend: ToolBackend<'a>,
    pub target: RepoToolTarget,
    pub scope: ToolScope,
    reads: Mutex<Vec<ContextRead>>,
    owner_qualifier: Mutex<Option<String>>,
}

impl<'a> ToolExecutor<'a> {
    pub fn new(backend: ToolBackend<'a>, target: RepoToolTarget, scope: ToolScope) -> Self {
        ToolExecutor { backend, target, scope, reads: Mutex::new(Vec::new()), owner_qualifier: Mutex::new(None) }
    }

    /// Everything read so far, in call order, de-duplicated.
    pub fn reads(&self) -> Vec<ContextRead> {
        self.reads.lock().unwrap().clone()
    }

    fn record(&self, repo: &str, path: &str, rev: &str, tool: &str) {
        let r = ContextRead {
            repo: format!("{}/{}", self.target.owner, repo),
            path: path.to_string(),
            rev: rev.to_string(),
            tool: tool.to_string(),
        };
        let mut reads = self.reads.lock().unwrap();
        if !reads.contains(&r) {
            reads.push(r);
        }
    }

    fn github(&self) -> Option<&GithubClient> {
        match &self.backend {
            ToolBackend::Github(g) => Some(g),
            ToolBackend::Local { github, .. } => Some(github),
            ToolBackend::Snapshot(_) => None,
        }
    }

    /// `org:<owner>` or `user:<owner>`, looked up once per executor. A
    /// failed lookup is an error, not a guess: the wrong qualifier returns
    /// zero hits, which the model would read as "no consumers exist". The
    /// failure isn't cached, so a later call retries.
    async fn owner_qualifier(&self, gh: &GithubClient) -> Result<String, String> {
        if let Some(q) = self.owner_qualifier.lock().unwrap().clone() {
            return Ok(q);
        }
        let kind = gh.get_owner_type(&self.target.owner).await.map_err(|e| {
            format!("couldn't determine whether {} is an organization or a user ({e}) — retry, or search one repo by reading it directly", self.target.owner)
        })?;
        let q = owner_qualifier_for(&kind, &self.target.owner);
        *self.owner_qualifier.lock().unwrap() = Some(q.clone());
        Ok(q)
    }

    /// Run one call. Never returns `Err` — failures become text the model
    /// can read and recover from, same as any other tool output.
    pub async fn execute(&self, call: &ToolCall) -> String {
        let result = match call {
            ToolCall::ReadFile { path, repo, rev } => self.read_file(path, repo.as_deref(), rev.as_deref()).await,
            ToolCall::SearchCode { query, scope } => self.search_code(query, scope.as_deref()).await,
            ToolCall::ListDir { path, repo } => self.list_dir(path, repo.as_deref()).await,
        };
        truncate(&result.unwrap_or_else(|e| format!("Tool error: {e}")), TOOL_RESULT_BUDGET)
    }

    async fn read_file(&self, path: &str, repo: Option<&str>, rev: Option<&str>) -> Result<String, String> {
        validate_repo_path(path)?;
        let other = resolve_repo(&self.target, self.scope, repo)?;
        let rev = rev.map(|r| r.trim().to_lowercase()).filter(|r| !r.is_empty()).unwrap_or_else(|| "head".to_string());
        if rev != "head" && rev != "base" {
            return Err(format!("unknown ref \"{rev}\" (use \"head\" or \"base\")"));
        }
        if rev == "base" && (!self.scope.base_ref || self.target.base_sha.is_empty()) {
            return Err("base reads aren't available here".to_string());
        }

        let (repo_name, rev_label, content) = match &other {
            Some(name) => {
                if rev == "base" {
                    return Err("ref applies to this PR's repo only; other repos are read at their default branch".to_string());
                }
                let content = match &self.backend {
                    ToolBackend::Snapshot(s) => s.require(name, "default")?.get(path).cloned().unwrap_or_default(),
                    _ => self.github().unwrap().get_file_content(&self.target.owner, name, path, "").await?,
                };
                (name.clone(), "default", content)
            }
            None => {
                let sha = if rev == "base" { &self.target.base_sha } else { &self.target.head_sha };
                let content = match &self.backend {
                    ToolBackend::Github(g) => g.get_file_content(&self.target.owner, &self.target.repo, path, sha).await?,
                    ToolBackend::Local { clone, .. } => clone.read_file(sha, path).await?,
                    ToolBackend::Snapshot(s) => s.require(&s.pr_repo, &rev)?.get(path).cloned().unwrap_or_default(),
                };
                (self.target.repo.clone(), if rev == "base" { "base" } else { "head" }, content)
            }
        };
        self.record(&repo_name, path, rev_label, "read_file");
        let where_ = match rev_label {
            "base" => "at PR base (before this PR)".to_string(),
            "head" => "at PR head".to_string(),
            _ => format!("in {}/{} (default branch)", self.target.owner, repo_name),
        };
        Ok(if content.is_empty() {
            format!("File not found (or empty) {where_}: {path}")
        } else {
            format!("Contents of {path} {where_}:\n{content}")
        })
    }

    async fn search_code(&self, query: &str, scope: Option<&str>) -> Result<String, String> {
        let query = sanitize_search_query(query);
        if query.is_empty() {
            return Err("the query was empty after removing repo/org/user qualifiers (scope is set by the tool, not the query)".to_string());
        }
        let owner_wide = match scope.map(|s| s.trim().to_lowercase()) {
            None => false,
            Some(s) if s.is_empty() || s == "repo" => false,
            Some(s) if s == "org" => {
                if !self.scope.same_owner {
                    return Err("owner-wide search isn't available here — only this PR's repo".to_string());
                }
                true
            }
            Some(s) => return Err(format!("unknown scope \"{s}\" (use \"repo\" or \"org\")")),
        };

        if owner_wide {
            self.record("*", &query, "owner", "search_code");
            return match &self.backend {
                ToolBackend::Snapshot(s) => {
                    if s.files.is_empty() {
                        return Err(format!("contents of {}'s repos are unavailable in this environment — don't treat this as the code not existing", self.target.owner));
                    }
                    let mut rows = Vec::new();
                    let mut repos: Vec<&String> = s.files.keys().collect();
                    repos.sort();
                    for repo in repos {
                        let rev = if *repo == s.pr_repo { "head" } else { "default" };
                        rows.extend(grep_files(s.files_at(repo, rev), &query).into_iter().map(|r| format!("{repo}: {r}")));
                    }
                    Ok(format_rows(&format!("Search results for \"{query}\" across {}'s repos", self.target.owner), rows))
                }
                _ => {
                    let gh = self.github().unwrap();
                    let q = self.owner_qualifier(gh).await?;
                    let (hits, total) = gh.search_code_qualified(&query, &q).await?;
                    Ok(format_hits(&query, &format!("{}'s repos", self.target.owner), &hits, total, true))
                }
            };
        }

        self.record(&self.target.repo.clone(), &query, "head", "search_code");
        match &self.backend {
            ToolBackend::Github(g) => {
                let (hits, total) = g.search_code(&self.target.owner, &self.target.repo, &query).await?;
                Ok(format_hits(&query, &format!("{}/{}", self.target.owner, self.target.repo), &hits, total, false))
            }
            ToolBackend::Local { clone, .. } => {
                let (rows, more) = clone.grep(&self.target.head_sha, &query).await?;
                let mut out = format_rows(&format!("Search results for \"{query}\" at PR head (local clone, exact)"), rows);
                if more {
                    out.push_str("... (more matches; refine the query)\n");
                }
                Ok(out)
            }
            ToolBackend::Snapshot(s) => {
                let rows = grep_files(Some(s.require(&s.pr_repo, "head")?), &query);
                Ok(format_rows(&format!("Search results for \"{query}\" at PR head"), rows))
            }
        }
    }

    async fn list_dir(&self, path: &str, repo: Option<&str>) -> Result<String, String> {
        if !path.is_empty() {
            validate_repo_path(path)?;
        }
        let other = resolve_repo(&self.target, self.scope, repo)?;
        let repo_name = other.clone().unwrap_or_else(|| self.target.repo.clone());
        let rev_label = if other.is_some() { "default" } else { "head" };
        let entries: Vec<(String, String, u64)> = match (&self.backend, &other) {
            (ToolBackend::Snapshot(s), _) => {
                let rev = if other.is_some() { "default" } else { "head" };
                snapshot_list(Some(s.require(&repo_name, rev)?), path)
            }
            (ToolBackend::Local { clone, .. }, None) => clone.list_dir(&self.target.head_sha, path).await?,
            (_, Some(name)) => self
                .github()
                .unwrap()
                .list_dir(&self.target.owner, name, path, "")
                .await?
                .into_iter()
                .map(|e| (e.name, e.entry_type, e.size))
                .collect(),
            (_, None) => self
                .github()
                .unwrap()
                .list_dir(&self.target.owner, &self.target.repo, path, &self.target.head_sha)
                .await?
                .into_iter()
                .map(|e| (e.name, e.entry_type, e.size))
                .collect(),
        };
        self.record(&repo_name, path, rev_label, "list_dir");
        let where_ = if path.is_empty() { "repo root".to_string() } else { path.to_string() };
        let at = if other.is_some() { format!("in {}/{repo_name}", self.target.owner) } else { "at PR head".to_string() };
        let mut out = format!("Contents of {where_} {at}:\n");
        let total = entries.len();
        for (name, kind, size) in entries.iter().take(LIST_DIR_MAX_ENTRIES) {
            if kind == "dir" {
                out.push_str(&format!("{name}  (dir)\n"));
            } else {
                out.push_str(&format!("{name}  ({kind}, {size} B)\n"));
            }
        }
        if total > LIST_DIR_MAX_ENTRIES {
            out.push_str(&format!("... ({} more entries)\n", total - LIST_DIR_MAX_ENTRIES));
        }
        Ok(out)
    }
}

/// The code-search qualifier for an owner of GitHub type `kind`.
fn owner_qualifier_for(kind: &str, owner: &str) -> String {
    if kind == "Organization" {
        format!("org:{owner}")
    } else {
        format!("user:{owner}")
    }
}

fn grep_files(files: Option<&HashMap<String, String>>, query: &str) -> Vec<String> {
    let Some(files) = files else { return Vec::new() };
    let needle = query.to_lowercase();
    let mut paths: Vec<&String> = files.keys().collect();
    paths.sort();
    let mut rows = Vec::new();
    for p in paths {
        for (i, line) in files[p].lines().enumerate() {
            if line.to_lowercase().contains(&needle) {
                rows.push(format!("{p}:{}:{}", i + 1, line.trim()));
            }
        }
    }
    rows
}

fn snapshot_list(files: Option<&HashMap<String, String>>, dir: &str) -> Vec<(String, String, u64)> {
    let Some(files) = files else { return Vec::new() };
    let prefix = if dir.is_empty() { String::new() } else { format!("{}/", dir.trim_end_matches('/')) };
    let mut out: Vec<(String, String, u64)> = Vec::new();
    for (p, content) in files {
        let Some(rest) = p.strip_prefix(&prefix) else { continue };
        let entry = match rest.split_once('/') {
            Some((d, _)) => (d.to_string(), "dir".to_string(), 0),
            None => (rest.to_string(), "file".to_string(), content.len() as u64),
        };
        if !out.contains(&entry) {
            out.push(entry);
        }
    }
    out.sort();
    out
}

fn format_rows(header: &str, rows: Vec<String>) -> String {
    if rows.is_empty() {
        return format!("{header}: no matches.\n");
    }
    let mut out = format!("{header} ({} matches):\n", rows.len());
    for r in rows {
        out.push_str(&format!("- {r}\n"));
    }
    out
}

fn format_hits(query: &str, where_: &str, hits: &[crate::github::CodeSearchHit], total: u32, show_repo: bool) -> String {
    if hits.is_empty() {
        return format!("No code-search results for \"{query}\" in {where_}.");
    }
    let mut out = format!(
        "Code search results for \"{query}\" in {where_} ({total} total, showing {}; GitHub indexes default branches only — confirm with read_file):\n",
        hits.len()
    );
    for hit in hits {
        if show_repo && !hit.repo.is_empty() {
            out.push_str(&format!("- {} {}\n", hit.repo, hit.path));
        } else {
            out.push_str(&format!("- {}\n", hit.path));
        }
        for frag in hit.fragments.iter().take(2) {
            out.push_str(&format!("  {}\n", frag));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> RepoToolTarget {
        RepoToolTarget { owner: "acme".into(), repo: "api".into(), head_sha: "h".into(), base_sha: "b".into() }
    }

    fn snapshot() -> SnapshotRepo {
        let mut s = SnapshotRepo { pr_repo: "api".into(), ..Default::default() };
        let mut put = |repo: &str, rev: &str, path: &str, content: &str| {
            s.files
                .entry(repo.into())
                .or_default()
                .entry(rev.into())
                .or_default()
                .insert(path.into(), content.into());
        };
        put("api", "head", "src/money.rs", "pub fn to_cents(d: f64) -> u64 { (d * 100.0) as u64 }\n");
        put("api", "head", "src/billing/charge.rs", "let c = to_cents(amount);\n");
        put("api", "base", "src/money.rs", "pub fn to_cents(d: f64) -> u64 { (d * 100.0).round() as u64 }\n");
        put("web", "default", "src/client.ts", "const id = resp.user_id;\n");
        s
    }

    #[test]
    fn repo_scope_is_enforced_by_the_executor() {
        let t = target();
        assert_eq!(resolve_repo(&t, ToolScope::REVIEW, None).unwrap(), None);
        assert_eq!(resolve_repo(&t, ToolScope::REVIEW, Some("api")).unwrap(), None);
        assert_eq!(resolve_repo(&t, ToolScope::REVIEW, Some("ACME/API")).unwrap(), None);
        assert_eq!(resolve_repo(&t, ToolScope::REVIEW, Some("web")).unwrap(), Some("web".into()));
        assert_eq!(resolve_repo(&t, ToolScope::REVIEW, Some("Acme/web")).unwrap(), Some("web".into()));
        for bad in ["evil/web", "web/../x", "..", "we b", "a/b/c", "web?x=1"] {
            assert!(resolve_repo(&t, ToolScope::REVIEW, Some(bad)).is_err(), "{bad} must be rejected");
        }
        // Chat never leaves the PR repo.
        assert!(resolve_repo(&t, ToolScope::CHAT, Some("web")).is_err());
        assert_eq!(resolve_repo(&t, ToolScope::CHAT, Some("api")).unwrap(), None);
    }

    #[test]
    fn owner_qualifier_matches_the_owner_type() {
        assert_eq!(owner_qualifier_for("Organization", "acme"), "org:acme");
        assert_eq!(owner_qualifier_for("User", "tj"), "user:tj");
    }

    #[test]
    fn colon_paths_are_rejected_so_git_specs_cant_be_readdressed() {
        assert!(validate_repo_path("HEAD:secret").is_err());
        assert!(validate_repo_path("src/lib.rs").is_ok());
    }

    #[tokio::test]
    async fn snapshot_executor_reads_head_base_and_sibling_repos_and_records_them() {
        let s = snapshot();
        let ex = ToolExecutor::new(ToolBackend::Snapshot(&s), target(), ToolScope::REVIEW);
        let head = ex.execute(&parse_tool_call(r#"{"tool":"read_file","path":"src/money.rs"}"#).unwrap()).await;
        assert!(head.contains("at PR head") && head.contains("as u64 }"));
        let base = ex.execute(&parse_tool_call(r#"{"tool":"read_file","path":"src/money.rs","ref":"base"}"#).unwrap()).await;
        assert!(base.contains("before this PR") && base.contains(".round()"));
        let web = ex.execute(&parse_tool_call(r#"{"tool":"read_file","path":"src/client.ts","repo":"web"}"#).unwrap()).await;
        assert!(web.contains("acme/web (default branch)") && web.contains("user_id"));
        let out = ex.execute(&parse_tool_call(r#"{"tool":"read_file","path":"x","repo":"evil/web"}"#).unwrap()).await;
        assert!(out.starts_with("Tool error:") && out.contains("out of scope"));

        let found = ex.execute(&parse_tool_call(r#"{"tool":"search_code","query":"TO_CENTS("}"#).unwrap()).await;
        assert!(found.contains("src/billing/charge.rs:1:") && found.contains("src/money.rs:1:"));
        let owner_wide = ex.execute(&parse_tool_call(r#"{"tool":"search_code","query":"user_id","scope":"org"}"#).unwrap()).await;
        assert!(owner_wide.contains("web: src/client.ts:1:"));
        // Injected qualifiers are stripped, scope comes from the tool.
        let injected = ex.execute(&parse_tool_call(r#"{"tool":"search_code","query":"repo:evil/x"}"#).unwrap()).await;
        assert!(injected.starts_with("Tool error:"));

        let listing = ex.execute(&parse_tool_call(r#"{"tool":"list_dir","path":"src"}"#).unwrap()).await;
        assert!(listing.contains("billing  (dir)") && listing.contains("money.rs  (file"));

        let reads = ex.reads();
        let tags: Vec<String> = reads.iter().map(|r| format!("{} {} {} {}", r.tool, r.repo, r.rev, r.path)).collect();
        assert_eq!(
            tags,
            vec![
                "read_file acme/api head src/money.rs",
                "read_file acme/api base src/money.rs",
                "read_file acme/web default src/client.ts",
                "search_code acme/api head TO_CENTS(",
                "search_code acme/* owner user_id",
                "list_dir acme/api head src",
            ]
        );
    }

    #[tokio::test]
    async fn uncaptured_snapshot_repos_error_instead_of_looking_empty() {
        let empty = SnapshotRepo { pr_repo: "api".into(), ..Default::default() };
        let ex = ToolExecutor::new(ToolBackend::Snapshot(&empty), target(), ToolScope::REVIEW);
        for call in [
            r#"{"tool":"search_code","query":"MAX_ATTEMPTS"}"#,
            r#"{"tool":"search_code","query":"MAX_ATTEMPTS","scope":"org"}"#,
            r#"{"tool":"read_file","path":"src/lib.rs"}"#,
            r#"{"tool":"list_dir","path":""}"#,
        ] {
            let out = ex.execute(&parse_tool_call(call).unwrap()).await;
            assert!(out.contains("unavailable in this environment"), "{call} → {out}");
        }
        // A captured repo still answers "no matches" honestly.
        let s = snapshot();
        let ex = ToolExecutor::new(ToolBackend::Snapshot(&s), target(), ToolScope::REVIEW);
        let out = ex.execute(&parse_tool_call(r#"{"tool":"search_code","query":"zzz"}"#).unwrap()).await;
        assert!(out.contains("no matches"), "{out}");
        let out = ex.execute(&parse_tool_call(r#"{"tool":"read_file","path":"x.ts","repo":"mobile"}"#).unwrap()).await;
        assert!(out.contains("unavailable"), "{out}");
    }

    #[tokio::test]
    async fn chat_scope_rejects_review_only_extensions() {
        let s = snapshot();
        let mut t = target();
        t.base_sha = String::new();
        let ex = ToolExecutor::new(ToolBackend::Snapshot(&s), t, ToolScope::CHAT);
        for call in [
            r#"{"tool":"read_file","path":"src/money.rs","ref":"base"}"#,
            r#"{"tool":"read_file","path":"src/client.ts","repo":"web"}"#,
            r#"{"tool":"search_code","query":"user_id","scope":"org"}"#,
            r#"{"tool":"list_dir","path":"","repo":"web"}"#,
        ] {
            let out = ex.execute(&parse_tool_call(call).unwrap()).await;
            assert!(out.starts_with("Tool error:"), "{call} → {out}");
        }
        assert!(ex.reads().is_empty(), "rejected calls read nothing");
    }
}
