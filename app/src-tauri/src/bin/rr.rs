//! `rr` — a thin terminal frontend over the Relevant Reviews Rust core.
//!
//! This is a prototype to evaluate a TUI/CLI direction. It deliberately reuses
//! the exact same modules the desktop app's Tauri commands call — `github.rs`,
//! `fetch.rs`, `config.rs` — so it proves how much of the app is already
//! frontend-agnostic. No webview, no Tauri runtime: just the core + stdout.

use std::io::{IsTerminal, Write};

use clap::{Parser, Subcommand};

use relevant_reviews_lib::config::{load_settings, resolve_github_token};
use relevant_reviews_lib::fetch::fetch_pr_impl;
use relevant_reviews_lib::github::GithubClient;
use relevant_reviews_lib::types::{FetchProgress, FetchStatus, ReviewManifest, ReviewThread};

#[derive(Parser)]
#[command(
    name = "rr",
    about = "Relevant Reviews in your terminal (prototype CLI over the shared Rust core)",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Skip the confirmation prompt for mutating commands (reply, resolve,
    /// approve, …). Required when stdin is not a terminal.
    #[arg(short = 'y', long, global = true)]
    yes: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Fetch + classify a PR and print the review manifest
    Review {
        /// PR reference: URL, owner/repo/pull/N, or owner/repo#N
        pr: String,
        /// Emit the full manifest as JSON instead of formatted text
        #[arg(long)]
        json: bool,
        /// Also print the unified diff for each file
        #[arg(long)]
        diffs: bool,
    },
    /// List PRs awaiting your review
    Requests {
        /// How many days back to search
        #[arg(long, default_value_t = 60)]
        days: i64,
        #[arg(long)]
        json: bool,
    },
    /// Show review comment threads on a PR
    Comments {
        pr: String,
        /// Only show unresolved threads
        #[arg(long)]
        unresolved: bool,
        #[arg(long)]
        json: bool,
    },
    /// Show CI check status for a PR
    Checks {
        pr: String,
        #[arg(long)]
        json: bool,
    },
    /// Reply to a review comment thread
    Reply {
        pr: String,
        /// Comment node ID to reply to (shown by `rr comments`)
        comment_id: String,
        /// Reply body
        body: String,
    },
    /// Mark a review thread resolved
    Resolve {
        /// Thread node ID (shown by `rr comments`)
        thread_id: String,
    },
    /// Reopen a resolved review thread
    Unresolve {
        thread_id: String,
    },
    /// Approve a PR
    Approve {
        pr: String,
        /// Optional approval message
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Request changes on a PR
    #[command(name = "request-changes")]
    RequestChanges {
        pr: String,
        /// Review body explaining the requested changes
        #[arg(short, long)]
        message: String,
    },
    /// Submit a comment-only review (no approval state)
    #[command(name = "comment-review")]
    CommentReview {
        pr: String,
        #[arg(short, long)]
        message: String,
    },
    /// Start a new inline review comment on a file line
    Comment {
        pr: String,
        /// File path as it appears in the diff
        path: String,
        /// Line number to attach the comment to
        line: u64,
        /// Comment body
        body: String,
        /// Which side of the diff: RIGHT (new/head) or LEFT (old/base)
        #[arg(long, default_value = "RIGHT")]
        side: String,
        /// For a multi-line comment, the first line of the range
        #[arg(long)]
        start_line: Option<u64>,
        /// Side for the start line (defaults to --side)
        #[arg(long)]
        start_side: Option<String>,
    },
    /// Add a top-level conversation comment to a PR (not a review, not anchored to a line)
    #[command(name = "pr-comment")]
    PrComment {
        pr: String,
        #[arg(short, long)]
        message: String,
    },
    /// Print resolved settings (token source is masked)
    Settings,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli.command, cli.yes).await {
        eprintln!("{}", paint(&format!("error: {e}"), RED));
        std::process::exit(1);
    }
}

fn github_client() -> GithubClient {
    let settings = load_settings();
    GithubClient::new(resolve_github_token(&settings))
}

async fn run(command: Command, yes: bool) -> Result<(), String> {
    match command {
        Command::Review { pr, json, diffs } => review(&pr, json, diffs).await,
        Command::Requests { days, json } => requests(days, json).await,
        Command::Comments { pr, unresolved, json } => comments(&pr, unresolved, json).await,
        Command::Checks { pr, json } => checks(&pr, json).await,
        Command::Reply { pr, comment_id, body } => {
            confirm(&format!("Reply to a thread on {pr}?"), yes)?;
            reply(&pr, &comment_id, &body).await
        }
        Command::Resolve { thread_id } => {
            confirm(&format!("Resolve thread {thread_id}?"), yes)?;
            set_resolved(&thread_id, true).await
        }
        Command::Unresolve { thread_id } => {
            confirm(&format!("Reopen thread {thread_id}?"), yes)?;
            set_resolved(&thread_id, false).await
        }
        Command::Approve { pr, message } => {
            confirm(&format!("Approve {pr}?"), yes)?;
            submit(&pr, "APPROVE", message.as_deref().unwrap_or("")).await
        }
        Command::RequestChanges { pr, message } => {
            confirm(&format!("Request changes on {pr}?"), yes)?;
            submit(&pr, "REQUEST_CHANGES", &message).await
        }
        Command::CommentReview { pr, message } => {
            confirm(&format!("Submit a comment review on {pr}?"), yes)?;
            submit(&pr, "COMMENT", &message).await
        }
        Command::Comment { pr, path, line, body, side, start_line, start_side } => {
            confirm(&format!("Comment on {pr} {path}:{line}?"), yes)?;
            comment(&pr, &path, line, &body, &side, start_line, start_side.as_deref()).await
        }
        Command::PrComment { pr, message } => {
            confirm(&format!("Comment on the {pr} conversation?"), yes)?;
            pr_comment(&pr, &message).await
        }
        Command::Settings => {
            settings_cmd();
            Ok(())
        }
    }
}

/// Gate a mutating action behind confirmation. With `--yes`, proceeds silently.
/// Interactively, prompts on stderr. When stdin is not a terminal and `--yes`
/// was not given, refuses rather than mutating a real PR unattended.
fn confirm(action: &str, yes: bool) -> Result<(), String> {
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        return Err(format!(
            "{action} refusing to run without --yes (stdin is not a terminal)"
        ));
    }
    eprint!("{} {} ", paint(action, BOLD), paint("[y/N]", DIM));
    let _ = std::io::stderr().flush();
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .map_err(|e| e.to_string())?;
    match input.trim().to_lowercase().as_str() {
        "y" | "yes" => Ok(()),
        _ => Err("aborted".to_string()),
    }
}

// ── mutations ────────────────────────────────────────────────────────────────

async fn reply(pr: &str, comment_id: &str, body: &str) -> Result<(), String> {
    let github = github_client();
    let parsed = relevant_reviews_lib::pr_parser::parse_pr_ref(pr)?;
    let pr_node_id = github
        .get_pull_request_id(&parsed.owner, &parsed.repo, parsed.number)
        .await?;
    let c = github.reply_to_review_thread(&pr_node_id, comment_id, body).await?;
    println!("{} replied as @{}  {}", paint("✓", GREEN), c.author.login, paint(&c.url, DIM));
    Ok(())
}

async fn set_resolved(thread_id: &str, resolve: bool) -> Result<(), String> {
    let github = github_client();
    let now_resolved = github.resolve_review_thread(thread_id, resolve).await?;
    let label = if now_resolved { paint("resolved", GREEN) } else { paint("reopened", YELLOW) };
    println!("{} thread {label}", paint("✓", GREEN));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn comment(
    pr: &str,
    path: &str,
    line: u64,
    body: &str,
    side: &str,
    start_line: Option<u64>,
    start_side: Option<&str>,
) -> Result<(), String> {
    let github = github_client();
    let parsed = relevant_reviews_lib::pr_parser::parse_pr_ref(pr)?;
    let pr_node_id = github
        .get_pull_request_id(&parsed.owner, &parsed.repo, parsed.number)
        .await?;
    let side = side.to_uppercase();
    // A range comment needs a start side too; default it to the same side.
    let eff_start_side: Option<String> = start_line
        .map(|_| start_side.map(|s| s.to_uppercase()).unwrap_or_else(|| side.clone()));
    let thread = github
        .create_review_thread(&pr_node_id, body, path, line, &side, start_line, eff_start_side.as_deref())
        .await?;
    let url = thread.comments.first().map(|c| c.url.as_str()).unwrap_or("");
    println!("{} comment posted on {path}:{line}  {}", paint("✓", GREEN), paint(url, DIM));
    Ok(())
}

async fn pr_comment(pr: &str, body: &str) -> Result<(), String> {
    let github = github_client();
    let parsed = relevant_reviews_lib::pr_parser::parse_pr_ref(pr)?;
    let url = github
        .add_pr_comment(&parsed.owner, &parsed.repo, parsed.number, body)
        .await?;
    println!("{} comment posted  {}", paint("✓", GREEN), paint(&url, DIM));
    Ok(())
}

async fn submit(pr: &str, event: &str, body: &str) -> Result<(), String> {
    let github = github_client();
    let parsed = relevant_reviews_lib::pr_parser::parse_pr_ref(pr)?;
    let state = github
        .submit_review(&parsed.owner, &parsed.repo, parsed.number, event, body)
        .await?;
    let colored = match state.as_str() {
        "APPROVED" => paint(&state, GREEN),
        "CHANGES_REQUESTED" => paint(&state, RED),
        _ => paint(&state, YELLOW),
    };
    println!("{} review submitted: {colored}", paint("✓", GREEN));
    Ok(())
}

// ── review ──────────────────────────────────────────────────────────────────

async fn review(pr: &str, json: bool, show_diffs: bool) -> Result<(), String> {
    let settings = load_settings();

    // Progress goes to stderr so stdout stays clean for piping/--json.
    let report = move |p: FetchProgress| {
        if matches!(p.status, FetchStatus::Running) {
            let mut line = format!("[{}/{}] {}", p.step, p.total_steps, p.label);
            if let (Some(d), Some(t)) = (p.files_done, p.files_total) {
                line.push_str(&format!(" ({d}/{t})"));
            }
            eprintln!("{}", paint(&line, DIM));
        }
    };

    let manifest = fetch_pr_impl(pr, &settings, &report).await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?);
        return Ok(());
    }

    print_manifest(&manifest, show_diffs);
    Ok(())
}

fn print_manifest(m: &ReviewManifest, show_diffs: bool) {
    println!();
    println!("{}", paint(&format!("{} #{}", m.pr_title, m.pr_number), BOLD));
    println!("{}", paint(&m.pr_url, DIM));
    println!("{}", paint(&format!("{} ← {}", m.base_ref, m.head_ref), DIM));

    if !m.summary.is_empty() {
        println!();
        for line in wrap(&m.summary, 88) {
            println!("  {line}");
        }
    }

    if !m.change_groups.is_empty() {
        println!("\n{}", paint("Change groups", BOLD));
        for g in &m.change_groups {
            println!("  {} {}", paint("●", CYAN), paint(&g.label, BOLD));
            for line in wrap(&g.description, 84) {
                println!("      {}", paint(&line, DIM));
            }
            println!("      {}", paint(&format!("{} file(s)", g.file_paths.len()), DIM));
        }
    }

    println!("\n{} {}", paint("Files", BOLD), paint(&format!("({})", m.files.len()), DIM));
    for f in &m.files {
        let risk = match f.risk_level.as_str() {
            "high" => paint("HIGH", RED),
            "low" => paint("low ", GREEN),
            _ => paint("med ", YELLOW),
        };
        let churn = paint(&format!("+{} -{}", f.additions, f.deletions), DIM);
        println!("  {risk} {}  {churn}", f.path);
        let meta = format!("{} · {}", f.classification, f.category);
        println!("        {}", paint(&meta, DIM));
        if !f.reason.is_empty() {
            for line in wrap(&f.reason, 80) {
                println!("        {}", paint(&line, DIM));
            }
        }
        for h in &f.highlights {
            let sev = match h.severity.as_str() {
                "high" | "warning" => RED,
                "medium" => YELLOW,
                _ => CYAN,
            };
            let loc = if h.start_line == h.end_line {
                format!("L{}", h.start_line)
            } else {
                format!("L{}-{}", h.start_line, h.end_line)
            };
            println!("        {} {} {}", paint("▸", sev), paint(&loc, sev), h.comment);
        }
        if show_diffs && !f.unified_diff.is_empty() {
            println!();
            print_diff(&f.unified_diff);
            println!();
        }
    }
    println!();
}

fn print_diff(diff: &str) {
    for line in diff.lines() {
        let colored = if line.starts_with("@@") {
            paint(line, CYAN)
        } else if line.starts_with('+') {
            paint(line, GREEN)
        } else if line.starts_with('-') {
            paint(line, RED)
        } else {
            line.to_string()
        };
        println!("        {colored}");
    }
}

// ── requests ────────────────────────────────────────────────────────────────

async fn requests(days: i64, json: bool) -> Result<(), String> {
    let github = github_client();
    let username = github.get_authenticated_user().await?;
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(days))
        .format("%Y-%m-%d")
        .to_string();
    let items = github.get_review_requests(&username, &cutoff, true).await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?);
        return Ok(());
    }

    if items.is_empty() {
        println!("No review requests in the last {days} days.");
        return Ok(());
    }

    println!("{}", paint(&format!("Review requests for {username} ({} found)", items.len()), BOLD));
    for it in &items {
        let flag = if it.direct_request { paint("@you", CYAN) } else { paint("team", DIM) };
        let status = match it.my_review_status.as_str() {
            "APPROVED" => paint("approved", GREEN),
            "CHANGES_REQUESTED" => paint("changes", RED),
            "" | "PENDING" => paint("pending", YELLOW),
            other => paint(other, DIM),
        };
        println!(
            "  {} {}  {}",
            flag,
            status,
            paint(&format!("{}/{} #{}", it.owner, it.repo, it.number), BOLD)
        );
        println!("      {}", it.title);
        let mut tail = format!("by {}", it.author);
        if it.unresolved_thread_count > 0 {
            tail.push_str(&format!(" · {} unresolved", it.unresolved_thread_count));
        }
        println!("      {}", paint(&tail, DIM));
    }
    Ok(())
}

// ── comments ────────────────────────────────────────────────────────────────

async fn comments(pr: &str, unresolved_only: bool, json: bool) -> Result<(), String> {
    let github = github_client();
    let parsed = relevant_reviews_lib::pr_parser::parse_pr_ref(pr)?;
    let mut threads = github
        .get_review_threads(&parsed.owner, &parsed.repo, parsed.number)
        .await?;

    if unresolved_only {
        threads.retain(|t| !t.is_resolved);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&threads).map_err(|e| e.to_string())?);
        return Ok(());
    }

    print_threads(&threads);
    Ok(())
}

fn print_threads(threads: &[ReviewThread]) {
    if threads.is_empty() {
        println!("No threads.");
        return;
    }
    for t in threads {
        let state = if t.is_resolved {
            paint("✓ resolved", GREEN)
        } else {
            paint("○ open", YELLOW)
        };
        let loc = t.line.map(|l| format!(":{l}")).unwrap_or_default();
        let outdated = if t.is_outdated { paint(" (outdated)", DIM) } else { String::new() };
        println!("\n{state} {}{}{}", paint(&t.path, BOLD), loc, outdated);
        println!("  {}", paint(&format!("thread {}", t.id), DIM));
        for c in &t.comments {
            println!(
                "  {}  {}",
                paint(&format!("@{}", c.author.login), CYAN),
                paint(&format!("reply-to {}", c.id), DIM)
            );
            for line in wrap(&c.body, 84) {
                println!("    {line}");
            }
        }
    }
    println!();
}

// ── checks ──────────────────────────────────────────────────────────────────

async fn checks(pr: &str, json: bool) -> Result<(), String> {
    let github = github_client();
    let parsed = relevant_reviews_lib::pr_parser::parse_pr_ref(pr)?;
    let status = github
        .get_pr_checks(&parsed.owner, &parsed.repo, parsed.number)
        .await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&status).map_err(|e| e.to_string())?);
        return Ok(());
    }

    let overall = match status.overall_state.as_str() {
        "SUCCESS" => paint(&status.overall_state, GREEN),
        "FAILURE" | "ERROR" => paint(&status.overall_state, RED),
        _ => paint(&status.overall_state, YELLOW),
    };
    println!("Overall: {overall}");
    for c in &status.check_runs {
        let mark = match c.conclusion.as_deref() {
            Some("SUCCESS") => paint("✓", GREEN),
            Some("FAILURE") | Some("TIMED_OUT") | Some("CANCELLED") => paint("✗", RED),
            Some("SKIPPED") | Some("NEUTRAL") => paint("•", DIM),
            _ => paint("◌", YELLOW),
        };
        let detail = c.conclusion.clone().unwrap_or_else(|| c.status.clone());
        println!("  {mark} {}  {}", c.name, paint(&detail, DIM));
    }
    Ok(())
}

// ── settings ────────────────────────────────────────────────────────────────

fn settings_cmd() {
    let s = load_settings();
    let token = resolve_github_token(&s);
    let token_status = match token {
        Some(t) if !t.is_empty() => paint(&format!("set ({}…)", &t.chars().take(4).collect::<String>()), GREEN),
        _ => paint("not set", RED),
    };
    println!("model:        {}", if s.model.is_empty() { paint("(none)", RED) } else { s.model.clone() });
    println!("github_token: {token_status}");
    println!("aws_profile:  {}", if s.aws_profile.is_empty() { paint("(none)", DIM) } else { s.aws_profile.clone() });
    println!("view_mode:    {}", s.view_mode);
    println!("hunk_filter:  {}", s.hunk_filter);
}

// ── tiny ANSI + wrapping helpers (no extra deps) ─────────────────────────────

const BOLD: &str = "1";
const DIM: &str = "2";
const RED: &str = "31";
const GREEN: &str = "32";
const YELLOW: &str = "33";
const CYAN: &str = "36";

fn color_enabled() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

fn paint(s: &str, code: &str) -> String {
    if color_enabled() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// Naive word wrap at `width` columns. Good enough for a prototype; a real TUI
/// would use a layout engine.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            if !current.is_empty() && current.len() + 1 + word.len() > width {
                lines.push(std::mem::take(&mut current));
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
        lines.push(current);
    }
    // Drop a trailing empty line artifact but keep intentional blank paragraphs.
    let _ = std::io::stdout().flush();
    lines
}
