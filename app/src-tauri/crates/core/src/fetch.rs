use crate::ai::{extract_json_array, extract_json_object, AiBackend};
use crate::config::resolve_github_token;
use crate::dismissed_highlights;
use crate::github::GithubClient;
use crate::manifest_cache;
use crate::pr_parser::parse_pr_ref;
use crate::pr_requirements;
use crate::prompts::{
    build_classification_prompt, build_grouping_prompt, build_highlight_prompt, build_requirements_coverage_prompt,
    build_summary_prompt, build_triage_prompt, extract_test_hunks, has_inline_test_markers, is_test_path, PriorNote,
};
use crate::types::{
    ChangeGroup, FetchProgress, FetchStatus, FileClassification, FileDiff, Highlight, HighlightResult, LinkedIssue,
    PassStatus, RequirementsCoverage, ReviewManifest, ReviewOrderItem, Settings, TopRisk, TriageReport,
};
use futures::stream::{FuturesUnordered, StreamExt};
use sha2::{Sha256, Digest};
use std::collections::{HashMap, HashSet};

/// Sink for fetch progress updates. The Tauri command passes a closure that
/// emits a `fetch-progress` event to the webview; the `marrow` CLI passes one that
/// prints to stderr. Decoupling the core fetch from `tauri::AppHandle` keeps it
/// reusable by any frontend.
pub type ProgressFn<'a> = &'a (dyn Fn(FetchProgress) + Send + Sync);

/// Truncate `s` to at most `max` chars on a char boundary (char-safe, unlike
/// byte slicing). Mirrors the equivalent helpers in prompts.rs/chat.rs —
/// bounds the cached manifest's stored PR body size.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

fn emit_progress(
    progress: ProgressFn,
    step: u8,
    label: &str,
    status: FetchStatus,
    pr_title: Option<&str>,
    files: Option<(u32, u32)>,
) {
    progress(FetchProgress {
        step,
        total_steps: 6,
        label: label.to_string(),
        status,
        pr_title: pr_title.map(|s| s.to_string()),
        files_done: files.map(|(d, _)| d),
        files_total: files.map(|(_, t)| t),
    });
}

/// Re-run ONLY the requirements-coverage pass against the cached manifest —
/// the "Save requirements" flow needs coverage without waiting for a new
/// head commit (a plain Refresh cache-hits on unchanged heads and never
/// re-analyzes). One AI call; everything else comes from the cache. Updates
/// the cached manifest in place and returns it.
pub async fn analyze_requirements_impl(pr_ref: &str, settings: &Settings) -> Result<ReviewManifest, String> {
    if settings.model.is_empty() {
        return Err("No model configured. Set `model` to a Claude model name (e.g. claude-sonnet-4-6) with an Anthropic API key or the `claude` CLI, or to an AWS Bedrock model ARN.".to_string());
    }
    let parsed = parse_pr_ref(pr_ref)?;
    let mut manifest = manifest_cache::load_cached_manifest(&parsed.owner, &parsed.repo, parsed.number)
        .ok_or_else(|| "No cached review for this PR — open it first.".to_string())?;

    let user_requirements_text = pr_requirements::load_pr_requirements(&parsed.owner, &parsed.repo, parsed.number)
        .map(|r| r.text)
        .filter(|t| !t.trim().is_empty());

    // Linked issues participate in the gate below, so they're fetched ahead
    // of it — a short-body PR now costs one (best-effort) API call to learn
    // whether an issue supplies the requirements; that's the feature. Skipped
    // entirely when user text exists: it's authoritative and replaces issues
    // as the extraction source.
    let github = GithubClient::new(resolve_github_token(settings));
    let linked_issues: Vec<LinkedIssue> = if user_requirements_text.is_none() {
        github
            .get_linked_issues(&parsed.owner, &parsed.repo, parsed.number)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    // Same gate as the full pipeline: user text opens it; otherwise the body
    // or a linked issue's body must clear the length bar. Gate closed ⇒
    // coverage becomes absent.
    if !coverage_gate_with_issues(&manifest.body, user_requirements_text.is_some(), &linked_issues) {
        manifest.requirements_coverage = None;
        let _ = manifest_cache::save_cached_manifest(&parsed.owner, &parsed.repo, parsed.number, &manifest);
        return Ok(manifest);
    }

    let test_diffs: Vec<(String, String)> = manifest
        .files
        .iter()
        .filter(|f| is_test_path(&f.path))
        .map(|f| (f.path.clone(), f.unified_diff.clone()))
        .collect();
    let inline_test_diffs: Vec<(String, String)> = manifest
        .files
        .iter()
        .filter(|f| !is_test_path(&f.path) && has_inline_test_markers(&f.unified_diff))
        .map(|f| (f.path.clone(), extract_test_hunks(&f.unified_diff)))
        .collect();
    let changed_paths: Vec<String> = manifest.files.iter().map(|f| f.path.clone()).collect();

    let existing_tests =
        fetch_related_existing_tests(&github, &parsed.owner, &parsed.repo, &manifest.head_sha, &changed_paths).await;

    let (prompt, coverage_truncated) = build_requirements_coverage_prompt(
        &manifest.pr_title,
        &manifest.body,
        &test_diffs,
        &inline_test_diffs,
        &existing_tests,
        &changed_paths,
        user_requirements_text.as_deref(),
        &linked_issues,
    );
    let ai = AiBackend::from_settings(settings).await?;
    let raw = ai.invoke(&prompt).await?;
    let known_tests: HashSet<&str> = test_diffs
        .iter()
        .chain(inline_test_diffs.iter())
        .chain(existing_tests.iter())
        .map(|(p, _)| p.as_str())
        .collect();
    // An unparseable response is an error, not "no requirements" — erroring
    // out here keeps the previously-good coverage in the cache instead of
    // silently wiping it. (finalize returning None — nothing extracted — is
    // a legitimate outcome and does persist.)
    let cov = extract_json_object(&raw)
        .ok()
        .and_then(|v| serde_json::from_value::<RequirementsCoverage>(v).ok())
        .ok_or_else(|| "The analysis response couldn't be parsed; kept the previous coverage.".to_string())?;
    manifest.requirements_coverage = finalize_coverage(cov, &known_tests);
    if !linked_issues.is_empty() {
        if let Some(c) = manifest.requirements_coverage.as_mut() {
            c.source_issues = linked_issues.iter().map(|i| i.number).collect();
        }
    }
    // Asymmetric on purpose: this re-analysis re-runs ONLY the coverage pass,
    // so it can add the truncated flag but never clear it — a prior `true`
    // may be owed to the other passes, which weren't re-built here.
    if coverage_truncated {
        manifest.analysis_truncated = true;
        if !manifest.truncated_passes.iter().any(|p| p == "coverage") {
            manifest.truncated_passes.push("coverage".to_string());
        }
    }
    // Unlike truncation, a coverage FAILURE mark can be cleared here: this
    // path just re-ran exactly that pass and got a usable result (failure
    // would have errored out above).
    manifest.failed_passes.retain(|p| p != "coverage");
    // Upsert the unified per-pass record for the pass this path re-ran; a
    // failure never reaches here, so it's truncated-or-complete.
    let status = if coverage_truncated { "truncated" } else { "complete" };
    upsert_pass(&mut manifest.passes, "coverage", status);

    // The AI call above is slow — a concurrent full Refresh may have written
    // a newer-head manifest to this cache file meanwhile. Only persist onto
    // the same head we analyzed; otherwise our result is stale and the
    // frontend's own head guard drops it too.
    let still_current = manifest_cache::load_cached_manifest(&parsed.owner, &parsed.repo, parsed.number)
        .map(|m| m.head_sha == manifest.head_sha)
        .unwrap_or(true);
    if still_current {
        let _ = manifest_cache::save_cached_manifest(&parsed.owner, &parsed.repo, parsed.number, &manifest);
    }
    Ok(manifest)
}

pub async fn fetch_pr_impl(pr_ref: &str, settings: &Settings, app: ProgressFn<'_>) -> Result<ReviewManifest, String> {
    if settings.model.is_empty() {
        return Err("No model configured. Set `model` to a Claude model name (e.g. claude-sonnet-4-6) with an Anthropic API key or the `claude` CLI, or to an AWS Bedrock model ARN.".to_string());
    }

    let token = resolve_github_token(settings);
    let parsed = parse_pr_ref(pr_ref)?;

    let github = GithubClient::new(token);

    // Step 1: Fetch PR metadata
    emit_progress(app, 1, "Fetching PR metadata", FetchStatus::Running, None, None);
    let metadata = github
        .get_pr_metadata(&parsed.owner, &parsed.repo, parsed.number)
        .await?;
    let pr_title = metadata.title;
    emit_progress(app, 1, "Fetching PR metadata", FetchStatus::Done, Some(&pr_title), None);
    let pr_url = metadata.html_url;
    let pr_number = metadata.number;
    let base_ref = metadata.base.ref_name;
    let head_ref = metadata.head.ref_name;
    let base_sha = metadata.base.sha;
    let head_sha = metadata.head.sha;
    let author = metadata.user.as_ref().map(|u| u.login.clone()).unwrap_or_default();
    let draft = metadata.draft.unwrap_or(false);
    let pr_body = metadata.body.unwrap_or_default();

    // Loaded before this run's manifest overwrites the cache (see the
    // `save_cached_manifest` call at the end of this function) so it's still
    // the *previous* analysis — used below to feed prior triage back into the
    // highlight prompt.
    let previous_manifest = manifest_cache::load_cached_manifest(&parsed.owner, &parsed.repo, parsed.number);
    if let Some(cached) = &previous_manifest {
        if cached.head_sha == head_sha {
            emit_progress(app, 6, "Loaded from cache", FetchStatus::Done, Some(&pr_title), None);
            return Ok(cached.clone());
        }
    }

    // Step 2: Fetch PR file list, diff, and commits in parallel. Commits are
    // best-effort — a failure here must not fail the whole review fetch, so
    // its result is defaulted to empty rather than propagated with `?`.
    emit_progress(app, 2, "Fetching files and diff", FetchStatus::Running, None, None);
    let (files_result, diff_result, commits_result) = tokio::join!(
        github.get_pr_files(&parsed.owner, &parsed.repo, parsed.number),
        github.get_pr_diff(&parsed.owner, &parsed.repo, parsed.number),
        github.get_pr_commits(&parsed.owner, &parsed.repo, parsed.number),
    );

    let pr_files = files_result?;
    let full_diff = diff_result?;
    let commits = commits_result.unwrap_or_default();
    emit_progress(app, 2, "Fetching files and diff", FetchStatus::Done, None, None);

    if pr_files.is_empty() {
        return Err("No changed files found in this PR.".to_string());
    }

    let file_list: Vec<String> = pr_files.iter().map(|f| f.filename.clone()).collect();

    // Index additions/deletions by filename for later use
    let file_stats: HashMap<String, (u64, u64)> = pr_files
        .iter()
        .map(|f| (f.filename.clone(), (f.additions, f.deletions)))
        .collect();

    // Step 3: AI classification
    emit_progress(app, 3, "Classifying files with AI", FetchStatus::Running, None, None);
    let ai = AiBackend::from_settings(settings).await?;

    let (classification_prompt, classification_truncated) =
        build_classification_prompt(&pr_title, &file_list, &full_diff);
    // Every pass's truncation flag this run, by name — ends up on the
    // manifest so the Overview can surface that analysis saw less than the
    // full PR, and WHICH pass was affected (attribution earned its keep the
    // very first time the flag fired: three debugging rounds on #192).
    // Summary/grouping never truncate (see build_file_context_prompt).
    let mut truncated_passes: Vec<String> = Vec::new();
    if classification_truncated {
        truncated_passes.push("classification".to_string());
    }

    let classification_raw = ai.invoke(&classification_prompt).await?;

    let classification_json = extract_json_array(&classification_raw)?;
    let classifications: Vec<FileClassification> = serde_json::from_value(classification_json)
        .map_err(|e| format!("Failed to parse classification: {}", e))?;
    let classifications = validate_classifications(classifications, &file_list);
    emit_progress(app, 3, "Classifying files with AI", FetchStatus::Done, None, None);

    let relevant: Vec<&FileClassification> = classifications
        .iter()
        .filter(|c| c.classification == "RELEVANT")
        .collect();

    // NOT_RELEVANT files still go into the manifest (the UIs surface them behind
    // a relevance filter); only the analysis below is scoped to the relevant
    // subset, and it's skipped entirely when nothing is relevant — so a PR whose
    // changes are all classified NOT_RELEVANT still lists its files instead of
    // coming back empty.
    let per_file_diff_map = build_per_file_diff_map(&full_diff);
    // Coverage judges requirements against test-file diffs, but test files are
    // always classified NOT_RELEVANT (see CLASSIFICATION_PROMPT) — so their
    // diffs must be pulled from the full per-file diff map now, before the
    // relevance filter below would otherwise leave them unavailable.
    let test_diffs: Vec<(String, String)> = file_list
        .iter()
        .filter(|p| is_test_path(p))
        .filter_map(|p| per_file_diff_map.get(p).map(|d| (p.clone(), d.clone())))
        .collect();
    // Implementation files whose diff ADDS inline tests (Rust `#[cfg(test)]`
    // modules live in the file they test, invisible to is_test_path) — extra
    // coverage evidence alongside the changed test files.
    let inline_test_diffs: Vec<(String, String)> = file_list
        .iter()
        .filter(|p| !is_test_path(p))
        .filter_map(|p| per_file_diff_map.get(p).map(|d| (p.clone(), d.clone())))
        .filter(|(_, d)| has_inline_test_markers(d))
        .map(|(p, d)| (p, extract_test_hunks(&d)))
        .collect();
    let mut highlights_by_path: HashMap<String, Vec<Highlight>> = HashMap::new();
    let mut summary = String::new();
    let mut failed_passes: Vec<String> = Vec::new();
    let mut change_groups: Vec<ChangeGroup> = Vec::new();
    // Triage guidance (top risks + contract-first order). Only computed for large
    // PRs (see the gate below); None means the UI falls back to its normal views.
    let mut triage: Option<TriageReport> = None;
    // Requirements-coverage analysis. Gated on PR body length; None means the
    // digest shows nothing for it (see the gate below and the no-fallback
    // parse in the coverage assembly).
    let mut requirements_coverage: Option<RequirementsCoverage> = None;
    // Fetched full contents (base, head), keyed by path — only relevant files,
    // so NOT_RELEVANT files show their diff but not a full-file view.
    let mut content_by_path: HashMap<String, (String, String)> = HashMap::new();

    // Which passes actually ran — feeds the unified per-pass record (#204).
    let mut ran = (false, false, false); // (analysis block, triage, coverage)
    if !relevant.is_empty() {
        // Step 4: AI highlight analysis + summary + grouping (+ triage for large
        // PRs), in parallel. The triage pass (top risks + contract-first order) is
        // gated on size — small PRs don't need a guided path.
        let run_triage = relevant.len() >= TRIAGE_MIN_FILES;
        // Local requirements override (issue #179 phase 2): a reviewer-supplied
        // requirements text (see pr_requirements.rs) is authoritative when
        // present and overrides the body-length gate below — it's the escape
        // hatch for PRs whose description states no real requirements.
        let local_requirements = pr_requirements::load_pr_requirements(&parsed.owner, &parsed.repo, parsed.number);
        let user_requirements_text = local_requirements
            .as_ref()
            .map(|r| r.text.as_str())
            .filter(|t| !t.trim().is_empty());
        // Linked issues (issue #179 phase 3) are an extraction source when the
        // reviewer supplied no local text — user text is authoritative and
        // replaces them, so the (best-effort) fetch is skipped entirely then.
        let linked_issues: Vec<LinkedIssue> = if user_requirements_text.is_none() {
            github
                .get_linked_issues(&parsed.owner, &parsed.repo, parsed.number)
                .await
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        // Coverage needs a PR body substantial enough to state real requirements;
        // a missing/near-empty body means nothing to extract, gate stays closed
        // regardless of whether the PR touched any test files — unless local
        // requirements text or a substantial linked-issue body overrides it.
        let run_coverage =
            coverage_gate_with_issues(&pr_body, user_requirements_text.is_some(), &linked_issues);
        ran = (true, run_triage, run_coverage);
        let ai_total: u32 = 3 + if run_triage { 1 } else { 0 } + if run_coverage { 1 } else { 0 };
        emit_progress(app, 4, "Analyzing highlights, summary, grouping, and coverage", FetchStatus::Running, None, Some((0, ai_total)));
        let per_file_diffs = extract_per_file_diffs(&per_file_diff_map, &relevant);
        let prior_notes = build_prior_notes(&previous_manifest, &parsed.owner, &parsed.repo, parsed.number);
        let (highlight_prompt, highlight_truncated) =
            build_highlight_prompt(&pr_title, &pr_body, &per_file_diffs, &prior_notes);
        if highlight_truncated {
            truncated_passes.push("highlights".to_string());
        }

        let summary_prompt = build_summary_prompt(&pr_title, &relevant);
        let grouping_prompt = build_grouping_prompt(&pr_title, &relevant);
        let triage_prompt = if run_triage {
            let (prompt, truncated) = build_triage_prompt(&pr_title, &relevant, &per_file_diffs);
            if truncated {
                truncated_passes.push("triage".to_string());
            }
            prompt
        } else {
            String::new()
        };
        // Existing unchanged test files related to the changed files — extra
        // coverage evidence fetched only when the coverage pass will run.
        let existing_tests: Vec<(String, String)> = if run_coverage {
            fetch_related_existing_tests(&github, &parsed.owner, &parsed.repo, &head_sha, &file_list).await
        } else {
            Vec::new()
        };
        let coverage_prompt = if run_coverage {
            let (prompt, truncated) = build_requirements_coverage_prompt(
                &pr_title,
                &pr_body,
                &test_diffs,
                &inline_test_diffs,
                &existing_tests,
                &file_list,
                user_requirements_text,
                &linked_issues,
            );
            if truncated {
                truncated_passes.push("coverage".to_string());
            }
            prompt
        } else {
            String::new()
        };

        let mut tasks = vec![
            ("highlights", ai.invoke(&highlight_prompt)),
            ("summary", ai.invoke(&summary_prompt)),
            ("grouping", ai.invoke(&grouping_prompt)),
        ];
        if run_triage {
            tasks.push(("triage", ai.invoke(&triage_prompt)));
        }
        if run_coverage {
            tasks.push(("coverage", ai.invoke(&coverage_prompt)));
        }
        let mut ai_stream: FuturesUnordered<_> =
            tasks.into_iter().map(|(name, fut)| async move { (name, fut.await) }).collect();

        let mut highlights_raw = Err("not started".to_string());
        let mut summary_raw = Err("not started".to_string());
        let mut grouping_raw = Err("not started".to_string());
        let mut triage_raw = Err("not started".to_string());
        let mut coverage_raw = Err("not started".to_string());
        let mut ai_done: u32 = 0;
        while let Some((name, result)) = ai_stream.next().await {
            ai_done += 1;
            emit_progress(app, 4, "Analyzing highlights, summary, grouping, and coverage", FetchStatus::Running, None, Some((ai_done, ai_total)));
            match name {
                "highlights" => highlights_raw = result,
                "summary" => summary_raw = result,
                "grouping" => grouping_raw = result,
                "triage" => triage_raw = result,
                "coverage" => coverage_raw = result,
                _ => {}
            }
        }
        emit_progress(app, 4, "Analyzing highlights, summary, grouping, and coverage", FetchStatus::Done, None, None);

        // The highlights pass is load-bearing: a failure errors the whole
        // fetch (keeping any cached manifest) rather than rendering as a
        // clean review with zero findings (issue #198).
        let highlight_results = validate_highlights(parse_highlights_strict(highlights_raw)?, &file_list);

        // The remaining passes degrade gracefully, but a failure is recorded
        // in `failed_passes` so the Overview can say the analysis is
        // incomplete instead of the section quietly reading as empty.
        match summary_raw {
            Ok(raw) => summary = raw,
            Err(_) => failed_passes.push("summary".to_string()),
        }

        change_groups = match parse_array_pass::<ChangeGroup>(grouping_raw) {
            Ok(groups) => groups,
            Err(_) => {
                failed_passes.push("grouping".to_string());
                Vec::new()
            }
        };

        // Index highlights by file path
        for h in highlight_results {
            highlights_by_path
                .entry(h.path.clone())
                .or_default()
                .push(Highlight {
                    start_line: h.start_line,
                    end_line: h.end_line,
                    severity: h.severity,
                    comment: h.comment,
                });
        }

        // Assemble triage: parse the AI object, else fall back to a deterministic
        // risk-ordered report. Either way, finalize so it references only real
        // files and covers every relevant file.
        if run_triage {
            let parsed = triage_raw
                .ok()
                .and_then(|raw| extract_json_object(&raw).ok())
                .and_then(|json| serde_json::from_value::<TriageReport>(json).ok())
                .filter(|t| !t.review_order.is_empty());
            // The deterministic fallback keeps the UI working, but it is not
            // the AI ordering the user asked for — record the pass as failed.
            if parsed.is_none() {
                failed_passes.push("triage".to_string());
            }
            let mut report = parsed.unwrap_or_else(|| fallback_triage(&relevant, &highlights_by_path));
            finalize_triage(&mut report, &relevant);
            triage = Some(report);
        }

        // Assemble requirements coverage. Unlike triage, there is no fallback on
        // parse failure — absence (field stays `None`) is the correct outcome
        // when the AI pass doesn't return usable JSON.
        if run_coverage {
            let parsed_cov: Option<RequirementsCoverage> = coverage_raw
                .ok()
                .and_then(|raw| extract_json_object(&raw).ok())
                .and_then(|json| serde_json::from_value(json).ok());
            // Absence because the pass errored or returned unusable JSON is a
            // failure; absence from `finalize_coverage` (nothing extracted)
            // is a legitimate result and stays silent.
            if parsed_cov.is_none() {
                failed_passes.push("coverage".to_string());
            }
            let known_tests: HashSet<&str> = test_diffs
                .iter()
                .chain(inline_test_diffs.iter())
                .chain(existing_tests.iter())
                .map(|(p, _)| p.as_str())
                .collect();
            requirements_coverage = parsed_cov.and_then(|cov| finalize_coverage(cov, &known_tests));
            if !linked_issues.is_empty() {
                if let Some(c) = requirements_coverage.as_mut() {
                    c.source_issues = linked_issues.iter().map(|i| i.number).collect();
                }
            }
        }

        // Step 5: Fetch file contents for all relevant files concurrently
        let files_total = relevant.len() as u32;
        emit_progress(app, 5, "Fetching file contents", FetchStatus::Running, None, Some((0, files_total)));
        let content_futures: Vec<_> = relevant
            .iter()
            .map(|f| {
                let path = f.path.clone();
                let owner = parsed.owner.clone();
                let repo = parsed.repo.clone();
                let base = base_sha.clone();
                let head = head_sha.clone();
                let gh = &github;
                async move {
                    let (base_content, head_content) = tokio::join!(
                        gh.get_file_content(&owner, &repo, &path, &base),
                        gh.get_file_content(&owner, &repo, &path, &head),
                    );
                    (path, base_content, head_content)
                }
            })
            .collect();

        // Bounded fan-out (issue #206): at most MAX_CONCURRENT_CONTENT_FILES
        // files in flight (×2 requests each) — an unbounded burst on a large
        // PR is exactly what GitHub's secondary rate limits punish.
        let mut stream = futures::stream::iter(content_futures)
            .buffer_unordered(crate::net::MAX_CONCURRENT_CONTENT_FILES);
        let mut files_done: u32 = 0;
        while let Some((path, base_result, head_result)) = stream.next().await {
            files_done += 1;
            emit_progress(app, 5, "Fetching file contents", FetchStatus::Running, None, Some((files_done, files_total)));
            content_by_path.insert(
                path,
                (
                    base_result.as_deref().unwrap_or("").to_string(),
                    head_result.as_deref().unwrap_or("").to_string(),
                ),
            );
        }
        emit_progress(app, 5, "Fetching file contents", FetchStatus::Done, None, None);
    }

    // Step 6: Build the manifest
    emit_progress(app, 6, "Building review manifest", FetchStatus::Running, None, None);
    let mut file_diffs = Vec::new();

    for pr_file in &pr_files {
        let path = &pr_file.filename;
        // Full content is only fetched for relevant files; others show their
        // diff but fall back to "(full file unavailable)" in the viewer.
        let (base_content, head_content) = content_by_path.get(path).cloned().unwrap_or_default();

        // GitHub's per-file status is authoritative for diff_type; content
        // emptiness is only a fallback — and NOT_RELEVANT files have no fetched
        // content, so the status is what we rely on for them.
        let diff_type = classify_diff_type(
            Some(pr_file.status.as_str()),
            base_content.is_empty(),
            head_content.is_empty(),
        );

        // Get the unified diff for this file, stripping the git header
        // For added/removed files, split large single hunks into smaller chunks
        let unified_diff = per_file_diff_map
            .get(path)
            .map(|d| {
                let stripped = strip_diff_header(d);
                if diff_type == "added" || diff_type == "removed" {
                    split_single_hunk(&stripped)
                } else {
                    stripped
                }
            })
            .unwrap_or_default();

        // The AI's classification, or a NOT_RELEVANT default for any changed
        // file the classifier omitted (so nothing silently disappears).
        let (classification, reason, category, risk_level) = classifications
            .iter()
            .find(|c| c.path == *path)
            .map(|c| {
                (
                    c.classification.clone(),
                    c.reason.clone(),
                    c.category.clone(),
                    c.risk_level.clone(),
                )
            })
            .unwrap_or_else(|| {
                ("NOT_RELEVANT".to_string(), "not classified".to_string(), "N/A".to_string(), "low".to_string())
            });

        let (additions, deletions) = file_stats.get(path).copied().unwrap_or((0, 0));

        // Build hunk_scores using heuristic analysis of each hunk's content
        let hunk_lines = split_diff_into_hunk_lines(&unified_diff);
        let hunk_scores: Vec<String> = hunk_lines
            .iter()
            .map(|lines| heuristic_significance(lines, path))
            .collect();

        let diff_hash = {
            let mut hasher = Sha256::new();
            hasher.update(unified_diff.as_bytes());
            format!("{:x}", hasher.finalize())[..16].to_string()
        };

        file_diffs.push(FileDiff {
            path: path.clone(),
            classification,
            reason,
            category,
            risk_level,
            diff_type: diff_type.to_string(),
            base_content,
            head_content,
            unified_diff,
            additions,
            deletions,
            highlights: highlights_by_path.remove(path.as_str()).unwrap_or_default(),
            hunk_scores,
            diff_hash,
        });
    }

    emit_progress(app, 6, "Building review manifest", FetchStatus::Done, None, None);

    let manifest = ReviewManifest {
        pr_title,
        pr_url,
        pr_number,
        base_ref,
        head_ref,
        base_sha,
        head_sha,
        author,
        draft,
        summary,
        change_groups,
        triage,
        requirements_coverage,
        body: truncate_chars(&pr_body, 10000),
        commits,
        passes: pass_statuses(ran.0, ran.1, ran.2, &truncated_passes, &failed_passes),
        analysis_truncated: !truncated_passes.is_empty(),
        truncated_passes,
        failed_passes,
        analysis_fingerprint: Some(crate::fingerprint::analysis_fingerprint(settings)),
        files: file_diffs,
    };

    let _ = manifest_cache::save_cached_manifest(&parsed.owner, &parsed.repo, parsed.number, &manifest);

    Ok(manifest)
}

/// Minimum number of relevant files before the triage pass runs. Small PRs are
/// fast to review flat and don't need a guided path.
const TRIAGE_MIN_FILES: usize = 5;

/// Rank a risk level for ordering (lower = riskier, comes first).
/// Validate AI classifications against the PR's real file list (issue #210).
/// Hallucinated paths are dropped — a RELEVANT hallucination would otherwise
/// enter the relevant set and pollute prompts, content fetches, and progress
/// counts. Duplicates keep the first entry. Enum-ish fields normalize:
/// anything but RELEVANT becomes NOT_RELEVANT (making today's implicit
/// behavior explicit) and an unknown risk_level becomes "low", matching
/// `risk_rank`'s fallback so the chip renders instead of showing raw noise.
fn validate_classifications(
    raw: Vec<FileClassification>,
    file_list: &[String],
) -> Vec<FileClassification> {
    let known: HashSet<&str> = file_list.iter().map(|s| s.as_str()).collect();
    let mut seen: HashSet<String> = HashSet::new();
    raw.into_iter()
        .filter(|c| known.contains(c.path.as_str()) && seen.insert(c.path.clone()))
        .map(|mut c| {
            if c.classification != "RELEVANT" {
                c.classification = "NOT_RELEVANT".to_string();
            }
            let risk = c.risk_level.to_lowercase();
            c.risk_level = match risk.as_str() {
                "critical" | "high" | "medium" | "low" => risk,
                _ => "low".to_string(),
            };
            c
        })
        .collect()
}

/// Validate AI highlights (issue #210): drop hallucinated paths early (they
/// were previously dropped only implicitly at manifest build), swap inverted
/// line ranges, clamp lines to ≥1, and normalize severity case-insensitively
/// — an unknown severity becomes "info", because severity measures
/// actionability and fabricating urgency from garbage would be dishonest.
/// The note text is untouched. Beyond-EOF anchors remain the frontend
/// reveal-guard's job.
fn validate_highlights(raw: Vec<HighlightResult>, file_list: &[String]) -> Vec<HighlightResult> {
    let known: HashSet<&str> = file_list.iter().map(|s| s.as_str()).collect();
    raw.into_iter()
        .filter(|h| known.contains(h.path.as_str()))
        .map(|mut h| {
            h.start_line = h.start_line.max(1);
            h.end_line = h.end_line.max(1);
            if h.start_line > h.end_line {
                std::mem::swap(&mut h.start_line, &mut h.end_line);
            }
            let sev = h.severity.to_lowercase();
            h.severity = match sev.as_str() {
                "critical" | "warning" | "info" => sev,
                _ => "info".to_string(),
            };
            h
        })
        .collect()
}

/// Derive the unified per-pass record (issue #204) from the signals the
/// pipeline already tracks. `ran_analysis` is false when no files were
/// relevant (the whole step-4 block is skipped). Failed trumps truncated —
/// one honest word per pass. Classification always runs (its failure fails
/// the fetch before any manifest exists).
fn pass_statuses(
    ran_analysis: bool,
    run_triage: bool,
    run_coverage: bool,
    truncated: &[String],
    failed: &[String],
) -> Vec<PassStatus> {
    ["classification", "highlights", "summary", "grouping", "triage", "coverage"]
        .into_iter()
        .map(|pass| {
            let ran = match pass {
                "classification" => true,
                "triage" => ran_analysis && run_triage,
                "coverage" => ran_analysis && run_coverage,
                _ => ran_analysis,
            };
            let status = if !ran {
                "not_run"
            } else if failed.iter().any(|f| f == pass) {
                "failed"
            } else if truncated.iter().any(|t| t == pass) {
                "truncated"
            } else {
                "complete"
            };
            PassStatus { pass: pass.to_string(), status: status.to_string() }
        })
        .collect()
}

/// Replace one pass's entry in the unified record — but only when the
/// record exists. An EMPTY `passes` means the manifest predates per-pass
/// recording (#204); writing a lone entry there would break the
/// one-record-per-pass invariant and read as "the other passes are
/// missing". Empty stays empty until a full fetch rebuilds the record.
fn upsert_pass(passes: &mut Vec<PassStatus>, pass: &str, status: &str) {
    if passes.is_empty() {
        return;
    }
    passes.retain(|p| p.pass != pass);
    passes.push(PassStatus { pass: pass.to_string(), status: status.to_string() });
}

/// Parse the highlights pass output strictly. The highlights pass is the
/// review's core product, so an AI error or unusable JSON is a fetch-level
/// error — "no findings" must mean the analysis succeeded and found nothing
/// (issue #198). The error keeps any previously cached manifest in place.
fn parse_highlights_strict(raw: Result<String, String>) -> Result<Vec<HighlightResult>, String> {
    let raw = raw.map_err(|e| format!("The highlights analysis failed — keeping any previous results. {e}"))?;
    let json = extract_json_array(&raw)
        .map_err(|e| format!("The highlights analysis returned an unusable response — keeping any previous results. {e}"))?;
    let entries = json.as_array().cloned().unwrap_or_default();
    let total = entries.len();
    // Per-element salvage: one malformed entry must not discard the valid
    // findings around it. But an array where NOTHING parses is an unusable
    // response, same as no array at all — that hard-fails.
    let parsed: Vec<HighlightResult> = entries
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect();
    if parsed.is_empty() && total > 0 {
        return Err(
            "The highlights analysis returned an unusable response — keeping any previous results. \
             None of the returned entries matched the expected shape."
                .to_string(),
        );
    }
    Ok(parsed)
}

/// Parse a degradable JSON-array pass (e.g. grouping). `Err` means the pass
/// failed — AI error or unusable JSON — and belongs in `failed_passes`; the
/// caller substitutes a default so the rest of the manifest still ships.
fn parse_array_pass<T: serde::de::DeserializeOwned>(raw: Result<String, String>) -> Result<Vec<T>, String> {
    let raw = raw?;
    let json = extract_json_array(&raw)?;
    serde_json::from_value(json).map_err(|e| e.to_string())
}

fn risk_rank(level: &str) -> u8 {
    match level {
        "critical" => 0,
        "high" => 1,
        "medium" => 2,
        _ => 3,
    }
}

/// Deterministic triage for when the AI pass is unavailable: order relevant files
/// by risk (critical first), and surface the critical/high files as top risks,
/// using each file's first highlight (or its classification reason) as the detail.
fn fallback_triage(
    relevant: &[&FileClassification],
    highlights_by_path: &HashMap<String, Vec<Highlight>>,
) -> TriageReport {
    let mut ordered: Vec<&FileClassification> = relevant.to_vec();
    ordered.sort_by_key(|f| risk_rank(&f.risk_level));
    let review_order = ordered
        .iter()
        .map(|f| ReviewOrderItem { path: f.path.clone(), rationale: String::new() })
        .collect();

    let top_risks = relevant
        .iter()
        .filter(|f| f.risk_level == "critical" || f.risk_level == "high")
        .take(3)
        .map(|f| {
            let first = highlights_by_path.get(&f.path).and_then(|h| h.first());
            TopRisk {
                title: format!("{} ({})", f.path, f.risk_level),
                detail: first.map(|h| h.comment.clone()).unwrap_or_else(|| f.reason.clone()),
                path: f.path.clone(),
                start_line: first.map(|h| h.start_line),
            }
        })
        .collect();

    TriageReport { top_risks, review_order }
}

/// Keep triage output honest regardless of source: drop top_risks / order entries
/// that point at files not in this PR (AI hallucinations), dedupe repeated order
/// entries (first occurrence wins — the prompt asks for each file exactly once,
/// but nothing forces the model to comply), and append any relevant file the AI
/// left out of the order so `[`/`]` navigation still covers everything.
fn finalize_triage(report: &mut TriageReport, relevant: &[&FileClassification]) {
    let known: HashSet<&str> = relevant.iter().map(|f| f.path.as_str()).collect();
    report.top_risks.retain(|r| known.contains(r.path.as_str()));
    let mut seen: HashSet<String> = HashSet::new();
    report.review_order.retain(|r| known.contains(r.path.as_str()) && seen.insert(r.path.clone()));

    let ordered: HashSet<String> = report.review_order.iter().map(|r| r.path.clone()).collect();
    for f in relevant {
        if !ordered.contains(&f.path) {
            report.review_order.push(ReviewOrderItem { path: f.path.clone(), rationale: String::new() });
        }
    }
}

/// The coverage pass runs on any PR whose body plausibly states requirements
/// — deliberately body-length-only, independent of test presence: a
/// requirements-stating PR with zero tests lighting up all-uncovered is the
/// feature's core signal.
fn coverage_gate(pr_body: &str) -> bool {
    pr_body.trim().chars().count() >= 40
}

/// The gate with linked issues in play (issue #179 phase 3): user text always
/// opens it; otherwise the PR body OR any linked issue's body must clear the
/// same 40-char bar — a terse PR whose linked issue states the acceptance
/// criteria is exactly the case linked-issue extraction exists for.
pub(crate) fn coverage_gate_with_issues(
    pr_body: &str,
    has_user_requirements: bool,
    linked_issues: &[LinkedIssue],
) -> bool {
    has_user_requirements
        || coverage_gate(pr_body)
        || linked_issues.iter().any(|i| coverage_gate(&i.body))
}

/// Finalize a parsed `RequirementsCoverage`: drop requirements whose status
/// isn't one the UI understands (a model that strays from the four allowed
/// statuses would otherwise render as neither a row nor an all-clear), drop
/// `TestRef`s (in both `requirements[].tests` and `orphan_tests`) whose path
/// isn't among the test files actually shown to the model (hallucination
/// guard), clamp `requirements` to 8. Returns `None` when no requirements
/// remain — absence is the correct outcome, not an empty-but-present report.
fn finalize_coverage(
    mut cov: RequirementsCoverage,
    known_tests: &HashSet<&str>,
) -> Option<RequirementsCoverage> {
    cov.requirements
        .retain(|r| matches!(r.status.as_str(), "covered" | "partial" | "uncovered" | "untestable"));
    for req in &mut cov.requirements {
        let cited_any = !req.tests.is_empty();
        req.tests.retain(|t| known_tests.contains(t.path.as_str()));
        // A covered/partial verdict whose every citation was hallucinated is
        // unsupported — downgrade it rather than letting it count as covered.
        if cited_any
            && req.tests.is_empty()
            && matches!(req.status.as_str(), "covered" | "partial")
        {
            req.status = "uncovered".to_string();
        }
    }
    cov.orphan_tests.retain(|t| known_tests.contains(t.path.as_str()));
    cov.requirements.truncate(8);

    if cov.requirements.is_empty() {
        None
    } else {
        Some(cov)
    }
}

/// Pick existing test files (from the repo tree at head) related to this
/// PR's changed files: for each changed non-test file's stem (basename minus
/// extension, minus a trailing ".test"/".spec"), a candidate test path scores
/// 1 per stem contained in its basename. Top 5 by (score desc, path asc).
pub(crate) fn pick_related_test_paths(
    tree_paths: &[String],
    changed_paths: &[String],
    already_included: &HashSet<&str>,
) -> Vec<String> {
    let mut stems: Vec<String> = changed_paths
        .iter()
        .filter(|p| !is_test_path(p))
        .filter_map(|p| {
            let base = p.rsplit('/').next().unwrap_or(p);
            let stem = base.rsplit_once('.').map(|(s, _)| s).unwrap_or(base);
            let stem = stem
                .strip_suffix(".test")
                .or_else(|| stem.strip_suffix(".spec"))
                .unwrap_or(stem)
                .to_lowercase();
            // Short stems ("ui", "db") match far too many candidates.
            if stem.chars().count() >= 3 { Some(stem) } else { None }
        })
        .collect();
    stems.sort();
    stems.dedup();

    let mut scored: Vec<(usize, &String)> = tree_paths
        .iter()
        .filter(|p| is_test_path(p) && !already_included.contains(p.as_str()))
        .filter_map(|p| {
            let base = p.rsplit('/').next().unwrap_or(p).to_lowercase();
            let score = stems.iter().filter(|s| base.contains(s.as_str())).count();
            if score > 0 { Some((score, p)) } else { None }
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    scored.into_iter().take(5).map(|(_, p)| p.clone()).collect()
}

/// Fetch the contents (at `head_sha`) of existing test files unchanged in
/// this PR but related to its changed files — coverage evidence for
/// requirements whose tests the PR didn't need to touch. Best-effort by
/// design: any failure yields fewer (or no) files, never a failed pass.
async fn fetch_related_existing_tests(
    github: &GithubClient,
    owner: &str,
    repo: &str,
    head_sha: &str,
    changed_paths: &[String],
) -> Vec<(String, String)> {
    let Ok(tree_paths) = github.get_tree_paths(owner, repo, head_sha).await else {
        return Vec::new();
    };
    // Every changed path is excluded: changed test files already appear as
    // diffs, and a changed file is by definition not an "unchanged" test.
    let already_included: HashSet<&str> = changed_paths.iter().map(|p| p.as_str()).collect();
    let picked = pick_related_test_paths(&tree_paths, changed_paths, &already_included);

    let mut out = Vec::new();
    for path in picked {
        if let Ok(content) = github.get_file_content(owner, repo, &path, head_sha).await {
            if !content.is_empty() {
                out.push((path, content));
            }
        }
    }
    out
}

/// Build the prior-triage context fed back into the highlight prompt: for
/// every highlight in the *previous* cached manifest that the reviewer
/// dismissed, look up its resolution (if any) and carry it forward so
/// re-analysis doesn't re-flag an already-triaged concern. Best-effort —
/// missing previous manifest or dismissed-state file just yields no notes;
/// this must never fail the fetch.
fn build_prior_notes(
    previous_manifest: &Option<ReviewManifest>,
    owner: &str,
    repo: &str,
    pr_number: u64,
) -> Vec<PriorNote> {
    let Some(previous) = previous_manifest else {
        return Vec::new();
    };
    let Some(dismissed) = dismissed_highlights::load_dismissed(owner, repo, pr_number) else {
        return Vec::new();
    };
    let dismissed_keys: HashSet<&str> = dismissed.keys.iter().map(|k| k.as_str()).collect();

    let mut notes = Vec::new();
    for file in &previous.files {
        for h in &file.highlights {
            let key = dismissed_highlights::highlight_key(&file.path, h.start_line, h.end_line, &h.comment);
            if !dismissed_keys.contains(key.as_str()) {
                continue;
            }
            let (state, reason) = dismissed
                .resolutions
                .get(&key)
                .map(|r| (r.state.clone(), r.reason.clone()))
                .unwrap_or_default();
            notes.push(PriorNote {
                path: file.path.clone(),
                comment: h.comment.clone(),
                state,
                reason,
            });
        }
    }
    notes
}

/// Extract per-file diffs for the relevant files from a pre-built diff map.
fn extract_per_file_diffs(
    diff_map: &HashMap<String, String>,
    relevant: &[&FileClassification],
) -> Vec<(String, String)> {
    let mut result = Vec::new();

    for f in relevant {
        if let Some(diff) = diff_map.get(&f.path) {
            result.push((f.path.clone(), diff.clone()));
        }
    }

    result
}

/// Map GitHub's per-file status to the app's `diff_type`. GitHub's status is
/// authoritative ("added"/"removed"/"modified"/"renamed"/"copied"/"changed");
/// renamed/copied/changed all have a base to diff against, so they're "modified".
/// Falls back to content-emptiness only when the status is missing.
fn classify_diff_type(
    github_status: Option<&str>,
    base_empty: bool,
    head_empty: bool,
) -> &'static str {
    match github_status {
        Some("added") => "added",
        Some("removed") => "removed",
        Some(_) => "modified",
        None => {
            if base_empty && !head_empty {
                "added"
            } else if !base_empty && head_empty {
                "removed"
            } else {
                "modified"
            }
        }
    }
}

/// Parse the full unified diff into a map of file_path -> diff_text.
fn build_per_file_diff_map(full_diff: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut current_path: Option<String> = None;
    let mut current_lines: Vec<&str> = Vec::new();

    for line in full_diff.lines() {
        if line.starts_with("diff --git ") {
            // Save previous file
            if let Some(path) = current_path.take() {
                map.insert(path, current_lines.join("\n"));
            }
            current_lines.clear();

            // Extract path from "diff --git a/path b/path"
            if let Some(b_part) = line.split(" b/").last() {
                current_path = Some(b_part.to_string());
            }
        }
        current_lines.push(line);
    }

    // Save the last file
    if let Some(path) = current_path {
        map.insert(path, current_lines.join("\n"));
    }

    map
}

/// For added/removed files with a single large hunk, split into multiple
/// synthetic hunks at blank-line boundaries (targeting ~30 lines per chunk).
/// This lets the AI score each section independently.
fn split_single_hunk(diff: &str) -> String {
    let lines: Vec<&str> = diff.lines().collect();

    // Count @@ lines — only split if there's exactly one hunk
    let hunk_count = lines.iter().filter(|l| l.starts_with("@@")).count();
    if hunk_count != 1 {
        return diff.to_string();
    }

    // Find the content lines (after the @@ header)
    let hunk_start = lines.iter().position(|l| l.starts_with("@@"));
    let hunk_start = match hunk_start {
        Some(i) => i,
        None => return diff.to_string(),
    };

    // Determine the prefix (+ or -)
    let prefix = lines[hunk_start + 1..].iter().find(|l| l.starts_with('+') || l.starts_with('-'));
    let prefix_char = match prefix {
        Some(l) if l.starts_with('+') => '+',
        Some(l) if l.starts_with('-') => '-',
        _ => return diff.to_string(),
    };

    let content_lines = &lines[hunk_start + 1..];
    if content_lines.len() < 40 {
        // Too short to bother splitting
        return diff.to_string();
    }

    // Find split points: blank lines (just the prefix with nothing after)
    let mut split_points: Vec<usize> = Vec::new();
    let target_chunk = 30;
    let mut since_last_split = 0;

    for (i, line) in content_lines.iter().enumerate() {
        since_last_split += 1;
        let is_blank = (line.len() == 1 && line.starts_with(prefix_char)) || line.trim().is_empty();
        if is_blank && since_last_split >= target_chunk {
            split_points.push(i);
            since_last_split = 0;
        }
    }

    if split_points.is_empty() {
        return diff.to_string();
    }

    // Rebuild the diff with synthetic @@ headers at split points
    let mut result = Vec::new();

    // Emit first @@ header with correct chunk size
    let first_chunk_size = split_points[0] + 1;
    if prefix_char == '+' {
        result.push(format!("@@ -0,0 +1,{} @@", first_chunk_size));
    } else {
        result.push(format!("@@ -1,{} +0,0 @@", first_chunk_size));
    }

    let mut current_line_num: u64 = 1;
    let mut chunk_start = 0;

    for (sp_idx, &split_at) in split_points.iter().enumerate() {
        // Emit lines from chunk_start to split_at (inclusive)
        for line in &content_lines[chunk_start..=split_at] {
            result.push(line.to_string());
        }
        current_line_num += (split_at - chunk_start + 1) as u64;
        chunk_start = split_at + 1;

        // Calculate the size of the next chunk
        let next_split = if sp_idx + 1 < split_points.len() {
            split_points[sp_idx + 1] + 1
        } else {
            content_lines.len()
        };
        let next_chunk_size = next_split - chunk_start;

        // Emit synthetic @@ header
        if next_chunk_size > 0 {
            if prefix_char == '+' {
                result.push(format!("@@ -0,0 +{},{} @@", current_line_num, next_chunk_size));
            } else {
                result.push(format!("@@ -{},{} +0,0 @@", current_line_num, next_chunk_size));
            }
        }
    }

    // Emit remaining lines
    for line in &content_lines[chunk_start..] {
        result.push(line.to_string());
    }

    result.join("\n")
}

/// Split a unified diff into per-hunk content lines (excluding @@ headers).
fn split_diff_into_hunk_lines(diff: &str) -> Vec<Vec<String>> {
    let mut hunks: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();

    for line in diff.lines() {
        if line.starts_with("@@") {
            if !current.is_empty() || !hunks.is_empty() {
                hunks.push(current);
            }
            current = Vec::new();
        } else {
            current.push(line.to_string());
        }
    }
    if !current.is_empty() {
        hunks.push(current);
    }
    hunks
}

/// Detect the language family from a file path extension.
fn lang_from_path(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => "js",
        "py" => "python",
        "rs" => "rust",
        "go" => "go",
        "java" | "kt" | "scala" => "jvm",
        "rb" => "ruby",
        "cs" => "csharp",
        "cpp" | "c" | "h" | "hpp" => "cpp",
        "sh" | "bash" | "zsh" => "shell",
        _ => "js", // default to JS patterns since that's the primary codebase
    }
}

/// Check if a line is a structural bracket/paren line (possibly with keywords).
/// Catches patterns like `}: Props)`, `}) => {`, `export default function Foo() {`, etc.
fn is_structural_line(trimmed: &str) -> bool {
    // Pure bracket lines are already caught as exact matches in the trivial check.
    // This catches lines that are *mostly* structural with a bit of decoration.
    let stripped: String = trimmed
        .chars()
        .filter(|c| !matches!(c, '{' | '}' | '(' | ')' | '[' | ']' | ';' | ',' | ' ' | ':'))
        .collect();
    // After removing brackets/punctuation/spaces, if very little remains, it's structural
    stripped.len() <= 3
}

/// Check if a line is JSX/template markup noise.
/// `lower` is the pre-computed lowercase version of `trimmed`.
fn is_jsx_noise(trimmed: &str, lower: &str) -> bool {
    // Lines that are purely JSX tags or props
    if trimmed.starts_with('<') && !trimmed.contains('{') {
        return true; // plain HTML/JSX tag like `<div>`, `</div>`, `<Component />`
    }
    if trimmed.starts_with('<') && trimmed.ends_with('>') {
        return true;
    }
    // JSX prop lines: `className="..."`, `onClick={handler}`, `style={{...}}`
    if lower.starts_with("classname")
        || lower.starts_with("style=")
        || lower.starts_with("aria-")
        || lower.starts_with("data-")
        || lower.starts_with("role=")
        || lower.starts_with("key=")
        || lower.starts_with("ref=")
        || lower.starts_with("id=")
    {
        return true;
    }
    // Closing JSX fragments
    if trimmed == "</>" || trimmed == "<>" || trimmed.starts_with("</") {
        return true;
    }
    false
}

/// Language-specific trivial line detection.
fn is_lang_trivial(trimmed: &str, lower: &str, lang: &str) -> bool {
    match lang {
        "rust" => {
            trimmed.starts_with("use ")
                || trimmed.starts_with("mod ")
                || trimmed.starts_with("pub mod ")
                || trimmed.starts_with("pub use ")
                || trimmed.starts_with("pub(crate)")
                || trimmed.starts_with("#[")
                || trimmed.starts_with("///")
                || trimmed.starts_with("//!")
        }
        "python" => {
            trimmed.starts_with("from ")
                || trimmed.starts_with("import ")
                || trimmed.starts_with("# ")
                || trimmed == "#"
                || trimmed.starts_with("\"\"\"")
                || trimmed.starts_with("'''")
                || lower.starts_with("pass")
                || lower.starts_with("@")
        }
        "go" => {
            trimmed.starts_with("import ")
                || trimmed.starts_with("import (")
                || trimmed == "import ("
                || trimmed.starts_with("// ")
                || trimmed.starts_with("package ")
        }
        "jvm" => {
            trimmed.starts_with("import ")
                || trimmed.starts_with("package ")
                || trimmed.starts_with("@")
                || trimmed.starts_with("//")
                || trimmed.starts_with("/*")
                || trimmed.starts_with("* ")
                || trimmed.starts_with("*/")
        }
        "ruby" => {
            trimmed.starts_with("require ")
                || trimmed.starts_with("require_relative ")
                || trimmed.starts_with("include ")
                || trimmed.starts_with("# ")
                || trimmed == "end"
        }
        "csharp" => {
            trimmed.starts_with("using ")
                || trimmed.starts_with("namespace ")
                || trimmed.starts_with("//")
                || trimmed.starts_with("[")  // attributes like [HttpGet]
        }
        "cpp" => {
            trimmed.starts_with("#include")
                || trimmed.starts_with("#pragma")
                || trimmed.starts_with("#define")
                || trimmed.starts_with("#ifndef")
                || trimmed.starts_with("#endif")
                || trimmed.starts_with("//")
                || trimmed.starts_with("/*")
                || trimmed.starts_with("using namespace")
        }
        "shell" => {
            trimmed.starts_with("# ")
                || trimmed == "#"
                || trimmed.starts_with("#!/")
                || trimmed.starts_with("set ")
                || trimmed.starts_with("export ")
        }
        _ => false, // JS/TS trivial patterns are already in the main check
    }
}

/// Check if `haystack` contains `word` as a standalone word (not as a substring
/// of a larger identifier). A word boundary is any non-alphanumeric, non-underscore
/// character, or the start/end of the string. This prevents "token_count" from
/// matching "token", or "assign(" from matching "sign(".
fn contains_word(haystack: &str, word: &str) -> bool {
    let bytes = haystack.as_bytes();
    let word_bytes = word.as_bytes();
    let wlen = word_bytes.len();
    if bytes.len() < wlen {
        return false;
    }
    for i in 0..=(bytes.len() - wlen) {
        if &bytes[i..i + wlen] == word_bytes {
            let before_ok = i == 0 || !is_ident_char(bytes[i - 1]);
            let after_ok = i + wlen == bytes.len() || !is_ident_char(bytes[i + wlen]);
            if before_ok && after_ok {
                return true;
            }
        }
    }
    false
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Heuristic scoring for a hunk based on content analysis.
///
/// Improvements over a naive keyword check:
/// 1. Two keyword tiers: critical (auth, DB, security) vs normal logic (if, return)
/// 2. Structural/bracket-only lines detected as trivial
/// 3. JSX/template noise detected as trivial
/// 4. Language-aware trivial detection (Rust, Python, Go, etc.)
/// 5. Hunk size as a factor: tiny hunks capped at medium, large hunks bumped up
/// 6. Diff direction: removed critical keywords score double (deleted safety checks)
/// 7. Weighted scoring: ratio of signal lines to non-trivial lines
fn heuristic_significance(lines: &[String], path: &str) -> String {
    let lang = lang_from_path(path);
    let mut total_changes: u64 = 0;
    let mut trivial_changes: u64 = 0;
    let mut critical_score: f64 = 0.0;
    let mut logic_hits: u64 = 0;

    for line in lines {
        // Only look at actual change lines
        let (prefix, rest) = if line.starts_with('+') {
            ('+', &line[1..])
        } else if line.starts_with('-') {
            ('-', &line[1..])
        } else {
            continue;
        };

        total_changes += 1;
        let trimmed = rest.trim();

        // ── Trivial detection ───────────────────────────────────────────────

        // Universal trivial: blank lines, comments, single brackets
        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.starts_with("* ")
            || trimmed.starts_with("*/")
            || trimmed.starts_with("console.")
            || trimmed.starts_with("logger.")
            || trimmed == "}"
            || trimmed == "})"
            || trimmed == "});"
            || trimmed == ");"
            || trimmed == "{"
            || trimmed == "("
            || trimmed == "),"
            || trimmed == "]"
            || trimmed == "],"
            || trimmed == "["
            || trimmed == "else {"
            || trimmed == "} else {"
        {
            trivial_changes += 1;
            continue;
        }

        // JS/TS-specific trivial (always checked since it's the primary codebase)
        if trimmed.starts_with("import ")
            || trimmed.starts_with("import{")
            || trimmed.starts_with("from ")
            || trimmed.starts_with("require(")
            || trimmed.starts_with("export type ")
            || trimmed.starts_with("export interface ")
            || trimmed.starts_with("interface ")
            || trimmed.starts_with("type ")
            || trimmed.starts_with("} from ")
            || trimmed.starts_with("export {")
            || trimmed.starts_with("export default")
            || trimmed.starts_with("export *")
            || trimmed.starts_with("@")
            || trimmed == "} as const;"
            || trimmed == "} as const"
            || trimmed.ends_with("as const;")
        {
            trivial_changes += 1;
            continue;
        }

        let lower = trimmed.to_lowercase();

        // Language-specific trivial
        if is_lang_trivial(trimmed, &lower, lang) {
            trivial_changes += 1;
            continue;
        }

        // Structural bracket lines (e.g. `}: Props)`, `}) => {`)
        if is_structural_line(trimmed) {
            trivial_changes += 1;
            continue;
        }

        // JSX/template noise
        if is_jsx_noise(trimmed, &lower) {
            trivial_changes += 1;
            continue;
        }

        // ── Signal detection ────────────────────────────────────────────────

        // Removed lines with critical keywords are extra dangerous (deleted safety checks)
        let direction_weight: f64 = if prefix == '-' { 1.5 } else { 1.0 };

        // Critical keywords: security, DB mutations, auth, error handling.
        // Use word-boundary-aware matching to avoid false positives like
        // "token_count" matching "token" or "assign(" matching "sign(".
        if lower.contains("authenticate")
            || lower.contains("authorize")
            || lower.contains("password")
            || contains_word(&lower, "secret")
            || lower.contains("permission")
            || contains_word(&lower, "token")
            || lower.contains(".delete(")
            || lower.contains(".remove(")
            || lower.contains(".destroy(")
            || lower.contains(".drop(")
            || lower.contains(".update(")
            || lower.contains(".insert(")
            || lower.contains(".save(")
            || lower.contains(".exec()")
            || lower.contains(".aggregate(")
            || lower.contains("throw ")
            || lower.contains("new error")
            || contains_word(&lower, "migration")
            || contains_word(&lower, "middleware")
            || contains_word(&lower, "cors")
            || lower.contains("csrf")
            || lower.contains("encrypt")
            || lower.contains("decrypt")
            || lower.contains(".hash(")
            || lower.contains(".sign(")
            || lower.contains(".verify(")
        {
            critical_score += direction_weight;
        }
        // Normal logic keywords: common control flow, not inherently risky
        else if lower.contains("if (")
            || lower.contains("if(")
            || lower.contains("return ")
            || lower.contains("await ")
            || lower.contains("async ")
            || lower.contains("switch ")
            || lower.contains("catch ")
            || lower.contains("try {")
            || lower.contains(".then(")
            || lower.contains("promise.")
            || lower.contains(".find(")
            || lower.contains(".findone(")
            || lower.contains(".filter(")
            || lower.contains(".map(")
        {
            logic_hits += 1;
        }
    }

    if total_changes == 0 {
        return "low".to_string();
    }

    let trivial_ratio = trivial_changes as f64 / total_changes as f64;

    // If 80%+ of changes are trivial, it's low regardless
    if trivial_ratio >= 0.8 {
        return "low".to_string();
    }

    let non_trivial = total_changes - trivial_changes;
    let change_lines = total_changes; // for size-based adjustments

    // ── Size-based adjustments ──────────────────────────────────────────

    // Tiny hunks (≤5 change lines): cap at medium even with critical keywords.
    // A single `return token` in a 3-line hunk is trivially reviewable.
    if change_lines <= 5 {
        if critical_score > 0.0 {
            return "medium".to_string();
        }
        // Tiny hunk with only normal logic or plain code → low
        return "low".to_string();
    }

    // ── Score determination ─────────────────────────────────────────────

    // Critical keywords present → high (weighted by direction)
    if critical_score >= 1.0 {
        return "high".to_string();
    }

    // Weighted ratio: what fraction of non-trivial lines are logic keywords?
    let logic_ratio = logic_hits as f64 / non_trivial as f64;

    // Large hunks (50+ change lines) with moderate logic get bumped to medium
    if change_lines >= 50 && logic_ratio >= 0.15 {
        return "medium".to_string();
    }

    if logic_ratio >= 0.3 {
        "medium".to_string()
    } else {
        "low".to_string()
    }
}

/// Strip the git diff header (everything before the first @@ line).
fn strip_diff_header(diff: &str) -> String {
    let mut lines = diff.lines();
    let mut result = Vec::new();
    let mut found_hunk = false;

    for line in &mut lines {
        if line.starts_with("@@") {
            found_hunk = true;
        }
        if found_hunk {
            result.push(line);
        }
    }

    result.join("\n")
}

#[cfg(test)]
mod tests {
    use super::classify_diff_type;
    use super::{parse_array_pass, parse_highlights_strict};
    use crate::types::ChangeGroup;

    #[test]
    fn classifications_drop_hallucinated_and_duplicate_paths() {
        use super::validate_classifications;
        use crate::types::FileClassification;
        let cls = |path: &str, c: &str, risk: &str| FileClassification {
            path: path.to_string(),
            classification: c.to_string(),
            category: "Business Logic".to_string(),
            risk_level: risk.to_string(),
            reason: "r".to_string(),
        };
        let files = vec!["a.rs".to_string(), "b.rs".to_string()];
        let out = validate_classifications(
            vec![
                cls("a.rs", "RELEVANT", "HIGH"),
                cls("ghost.rs", "RELEVANT", "critical"), // hallucinated: dropped
                cls("a.rs", "NOT_RELEVANT", "low"),      // duplicate: first wins
                cls("b.rs", "MAYBE", "extreme"),         // unknown enums: normalized
            ],
            &files,
        );
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].path.as_str(), out[0].classification.as_str(), out[0].risk_level.as_str()), ("a.rs", "RELEVANT", "high"));
        assert_eq!((out[1].classification.as_str(), out[1].risk_level.as_str()), ("NOT_RELEVANT", "low"));
    }

    #[test]
    fn highlights_normalize_ranges_and_severity() {
        use super::validate_highlights;
        use crate::types::HighlightResult;
        let hl = |path: &str, s: u64, e: u64, sev: &str| HighlightResult {
            path: path.to_string(),
            start_line: s,
            end_line: e,
            severity: sev.to_string(),
            comment: "c".to_string(),
        };
        let files = vec!["a.rs".to_string()];
        let out = validate_highlights(
            vec![
                hl("a.rs", 9, 3, "WARNING"),   // inverted range, cased severity
                hl("a.rs", 0, 0, "urgent!!"),  // zero lines, garbage severity
                hl("ghost.rs", 1, 2, "info"),  // hallucinated path: dropped
            ],
            &files,
        );
        assert_eq!(out.len(), 2);
        assert_eq!((out[0].start_line, out[0].end_line, out[0].severity.as_str()), (3, 9, "warning"));
        assert_eq!((out[1].start_line, out[1].end_line, out[1].severity.as_str()), (1, 1, "info"));
        // The note text is never touched.
        assert_eq!(out[1].comment, "c");
    }

    #[test]
    fn pass_statuses_cover_all_four_states() {
        use super::pass_statuses;
        let s = pass_statuses(
            true,
            true,
            false,
            &["highlights".to_string(), "summary".to_string()],
            &["summary".to_string()],
        );
        let get = |name: &str| s.iter().find(|p| p.pass == name).unwrap().status.clone();
        assert_eq!(get("classification"), "complete");
        assert_eq!(get("highlights"), "truncated");
        assert_eq!(get("summary"), "failed", "failed trumps truncated");
        assert_eq!(get("grouping"), "complete");
        assert_eq!(get("triage"), "complete");
        assert_eq!(get("coverage"), "not_run");
        assert_eq!(s.len(), 6, "one record per pass, always");
    }

    #[test]
    fn pass_statuses_when_nothing_relevant() {
        use super::pass_statuses;
        // No relevant files: classification still ran (it decided that);
        // everything downstream is not_run — even with stale flag content.
        let s = pass_statuses(false, true, true, &["classification".to_string()], &[]);
        let get = |name: &str| s.iter().find(|p| p.pass == name).unwrap().status.clone();
        assert_eq!(get("classification"), "truncated");
        for pass in ["highlights", "summary", "grouping", "triage", "coverage"] {
            assert_eq!(get(pass), "not_run", "{pass}");
        }
    }

    #[test]
    fn upsert_pass_replaces_but_never_fabricates_a_record() {
        use super::{pass_statuses, upsert_pass};
        // A populated record: coverage's entry is replaced in place.
        let mut passes = pass_statuses(true, false, true, &[], &["coverage".to_string()]);
        upsert_pass(&mut passes, "coverage", "complete");
        assert_eq!(passes.len(), 6);
        assert_eq!(passes.iter().find(|p| p.pass == "coverage").unwrap().status, "complete");
        // A pre-#204 cache (empty record): upsert must NOT create a lone
        // entry — empty means "predates recording", not "one pass ran".
        let mut empty: Vec<crate::types::PassStatus> = Vec::new();
        upsert_pass(&mut empty, "coverage", "complete");
        assert!(empty.is_empty());
    }

    #[test]
    fn highlights_ai_error_fails_the_fetch_not_empties_it() {
        let err = parse_highlights_strict(Err("connection reset".to_string())).unwrap_err();
        assert!(err.contains("highlights analysis failed"), "{err}");
        assert!(err.contains("keeping any previous results"), "{err}");
        assert!(err.contains("connection reset"), "{err}");
    }

    #[test]
    fn highlights_unusable_json_fails_the_fetch() {
        let err = parse_highlights_strict(Ok("I'm sorry, I can't do that".to_string())).unwrap_err();
        assert!(err.contains("unusable response"), "{err}");
        // Well-formed array where nothing matches the shape is also unusable.
        assert!(parse_highlights_strict(Ok("[{\"nope\": 1}]".to_string())).is_err());
    }

    #[test]
    fn highlights_salvage_valid_entries_around_a_malformed_one() {
        let raw = r#"[
            {"path":"a.rs","start_line":1,"end_line":2,"severity":"info","comment":"x"},
            {"malformed": true},
            {"path":"b.rs","start_line":3,"end_line":4,"severity":"warning","comment":"y"}
        ]"#;
        let ok = parse_highlights_strict(Ok(raw.to_string())).unwrap();
        assert_eq!(ok.len(), 2);
        assert_eq!(ok[0].path, "a.rs");
        assert_eq!(ok[1].path, "b.rs");
    }

    #[test]
    fn highlights_valid_and_empty_responses_parse() {
        let ok = parse_highlights_strict(Ok(
            r#"[{"path":"a.rs","start_line":1,"end_line":2,"severity":"info","comment":"x"}]"#.to_string(),
        ))
        .unwrap();
        assert_eq!(ok.len(), 1);
        // A genuinely empty result is a success — that's the honest "no findings".
        assert!(parse_highlights_strict(Ok("[]".to_string())).unwrap().is_empty());
    }

    #[test]
    fn degradable_array_pass_reports_failure_distinctly_from_empty() {
        assert!(parse_array_pass::<ChangeGroup>(Err("boom".to_string())).is_err());
        assert!(parse_array_pass::<ChangeGroup>(Ok("not json".to_string())).is_err());
        assert!(parse_array_pass::<ChangeGroup>(Ok("[]".to_string())).unwrap().is_empty());
    }

    #[test]
    fn github_status_is_authoritative() {
        assert_eq!(classify_diff_type(Some("added"), true, false), "added");
        assert_eq!(classify_diff_type(Some("removed"), false, true), "removed");
        assert_eq!(classify_diff_type(Some("modified"), false, false), "modified");
        // The bug: a renamed file's new path 404s at base → empty base content,
        // but GitHub says "renamed" → modified, not "added".
        assert_eq!(classify_diff_type(Some("renamed"), true, false), "modified");
        assert_eq!(classify_diff_type(Some("copied"), true, false), "modified");
        assert_eq!(classify_diff_type(Some("changed"), false, false), "modified");
    }

    #[test]
    fn falls_back_to_content_when_status_missing() {
        assert_eq!(classify_diff_type(None, true, false), "added");
        assert_eq!(classify_diff_type(None, false, true), "removed");
        assert_eq!(classify_diff_type(None, false, false), "modified");
    }

    use super::{fallback_triage, finalize_triage};
    use crate::types::{FileClassification, Highlight, ReviewOrderItem, TopRisk, TriageReport};
    use std::collections::HashMap;

    fn fc(path: &str, risk: &str) -> FileClassification {
        FileClassification {
            path: path.to_string(),
            classification: "RELEVANT".to_string(),
            category: "Business Logic".to_string(),
            risk_level: risk.to_string(),
            reason: format!("reason for {path}"),
        }
    }

    #[test]
    fn fallback_triage_orders_by_risk_and_surfaces_top_risks() {
        let files = [fc("a.rs", "low"), fc("b.rs", "critical"), fc("c.rs", "high")];
        let relevant: Vec<&FileClassification> = files.iter().collect();
        let mut highlights = HashMap::new();
        highlights.insert(
            "b.rs".to_string(),
            vec![Highlight { start_line: 42, end_line: 50, severity: "critical".into(), comment: "auth check removed".into() }],
        );

        let report = fallback_triage(&relevant, &highlights);
        // Risk-first ordering: critical, high, low.
        let order: Vec<&str> = report.review_order.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(order, vec!["b.rs", "c.rs", "a.rs"]);
        // Top risks are the critical/high files; the highlight drives detail+line.
        assert_eq!(report.top_risks.len(), 2);
        let b = report.top_risks.iter().find(|r| r.path == "b.rs").unwrap();
        assert_eq!(b.detail, "auth check removed");
        assert_eq!(b.start_line, Some(42));
    }

    #[test]
    fn finalize_triage_drops_unknown_and_appends_missing() {
        let files = [fc("a.rs", "high"), fc("b.rs", "low")];
        let relevant: Vec<&FileClassification> = files.iter().collect();
        // AI returned an order missing b.rs, listing a.rs twice, and a
        // hallucinated ghost.rs, plus a top risk pointing at a file not in the PR.
        let mut report = TriageReport {
            top_risks: vec![TopRisk { title: "x".into(), detail: "y".into(), path: "ghost.rs".into(), start_line: None }],
            review_order: vec![
                ReviewOrderItem { path: "a.rs".into(), rationale: "defines it".into() },
                ReviewOrderItem { path: "ghost.rs".into(), rationale: "nope".into() },
                ReviewOrderItem { path: "a.rs".into(), rationale: "again".into() },
            ],
        };
        finalize_triage(&mut report, &relevant);
        // ghost.rs dropped from both; duplicate a.rs collapsed to its first
        // occurrence; b.rs appended to the order.
        assert!(report.top_risks.is_empty());
        let order: Vec<&str> = report.review_order.iter().map(|r| r.path.as_str()).collect();
        assert_eq!(order, vec!["a.rs", "b.rs"]);
        assert_eq!(report.review_order[0].rationale, "defines it");
    }

    use super::{coverage_gate, coverage_gate_with_issues, finalize_coverage};
    use crate::ai::extract_json_object;
    use crate::types::{LinkedIssue, RequirementsCoverage};
    use std::collections::HashSet;

    #[test]
    fn coverage_json_round_trips_through_extract_and_parse() {
        let raw = r#"{"requirements":[{"text":"Users can log in","status":"covered","tests":[{"path":"tests/login.test.ts","note":"asserts success path"}],"note":null}],"orphan_tests":[{"path":"tests/unrelated.test.ts","note":"tests logout, not login"}]}"#;
        let json = extract_json_object(raw).unwrap();
        let cov: RequirementsCoverage = serde_json::from_value(json).unwrap();
        assert_eq!(cov.requirements.len(), 1);
        assert_eq!(cov.requirements[0].status, "covered");
        assert_eq!(cov.orphan_tests.len(), 1);
    }

    #[test]
    fn coverage_json_parses_through_markdown_fence() {
        let raw = "```json\n{\"requirements\":[],\"orphan_tests\":[]}\n```";
        let json = extract_json_object(raw).unwrap();
        let cov: RequirementsCoverage = serde_json::from_value(json).unwrap();
        assert!(cov.requirements.is_empty());
    }

    #[test]
    fn coverage_malformed_json_fails_to_extract() {
        let raw = "not json at all, sorry";
        assert!(extract_json_object(raw).is_err());
    }

    #[test]
    fn coverage_unknown_status_string_survives_parse() {
        let raw = r#"{"requirements":[{"text":"Something","status":"mystery-status"}],"orphan_tests":[]}"#;
        let json = extract_json_object(raw).unwrap();
        let cov: RequirementsCoverage = serde_json::from_value(json).unwrap();
        assert_eq!(cov.requirements[0].status, "mystery-status");
    }

    #[test]
    fn coverage_missing_optional_fields_default() {
        // No "tests" or "note" on the requirement, no "orphan_tests" key at all.
        let raw = r#"{"requirements":[{"text":"Something","status":"uncovered"}]}"#;
        let json = extract_json_object(raw).unwrap();
        let cov: RequirementsCoverage = serde_json::from_value(json).unwrap();
        assert!(cov.requirements[0].tests.is_empty());
        assert_eq!(cov.requirements[0].note, None);
        assert!(cov.orphan_tests.is_empty());
    }

    #[test]
    fn finalize_coverage_drops_hallucinated_test_paths() {
        let cov = RequirementsCoverage {
            requirements: vec![crate::types::RequirementEntry {
                text: "Does the thing".to_string(),
                status: "covered".to_string(),
                tests: vec![
                    crate::types::TestRef { path: "tests/real.test.ts".to_string(), note: None },
                    crate::types::TestRef { path: "tests/made_up.test.ts".to_string(), note: None },
                ],
                note: None,
            }],
            orphan_tests: vec![
                crate::types::TestRef { path: "tests/real.test.ts".to_string(), note: None },
                crate::types::TestRef { path: "tests/made_up.test.ts".to_string(), note: None },
            ],
            source_issues: vec![],
        };
        let known: HashSet<&str> = ["tests/real.test.ts"].into_iter().collect();
        let result = finalize_coverage(cov, &known).unwrap();
        assert_eq!(result.requirements[0].tests.len(), 1);
        assert_eq!(result.requirements[0].tests[0].path, "tests/real.test.ts");
        assert_eq!(result.orphan_tests.len(), 1);
        assert_eq!(result.orphan_tests[0].path, "tests/real.test.ts");
    }

    #[test]
    fn finalize_coverage_empty_requirements_yields_none() {
        let cov = RequirementsCoverage { requirements: vec![], orphan_tests: vec![], source_issues: vec![] };
        let known: HashSet<&str> = HashSet::new();
        assert!(finalize_coverage(cov, &known).is_none());
    }

    #[test]
    fn coverage_gate_requires_40_trimmed_chars() {
        assert!(!coverage_gate(""));
        assert!(!coverage_gate("   \n\t  "));
        assert!(!coverage_gate("short body"));
        assert!(!coverage_gate(&format!("  {}  ", "x".repeat(39))));
        assert!(coverage_gate(&"x".repeat(40)));
        assert!(coverage_gate(&format!("  {}  ", "x".repeat(40))));
    }

    #[test]
    fn coverage_gate_with_issues_opens_on_substantial_issue_body() {
        let issue = |body: &str| LinkedIssue {
            number: 1,
            title: "t".to_string(),
            body: body.to_string(),
        };
        // Short PR body + a linked issue that clears the 40-char bar → open.
        assert!(coverage_gate_with_issues("short", false, &[issue(&"x".repeat(40))]));
        // Any one substantial issue among trivial ones is enough.
        assert!(coverage_gate_with_issues("short", false, &[issue(""), issue(&"x".repeat(40))]));
        // Short PR body + only trivial issue bodies → closed.
        assert!(!coverage_gate_with_issues("short", false, &[issue(""), issue("tiny")]));
        assert!(!coverage_gate_with_issues("short", false, &[issue(&format!("  {}  ", "x".repeat(39)))]));
        assert!(!coverage_gate_with_issues("short", false, &[]));
        // User requirements always open the gate, issues or not.
        assert!(coverage_gate_with_issues("", true, &[]));
        // A substantial PR body still opens it on its own.
        assert!(coverage_gate_with_issues(&"x".repeat(40), false, &[]));
    }

    #[test]
    fn finalize_coverage_clamps_to_eight_requirements() {
        let req = |n: usize| crate::types::RequirementEntry {
            text: format!("Requirement {n}"),
            status: "uncovered".to_string(),
            tests: vec![],
            note: None,
        };
        let cov = RequirementsCoverage {
            requirements: (0..10).map(req).collect(),
            orphan_tests: vec![],
            source_issues: vec![],
        };
        let known: HashSet<&str> = HashSet::new();
        assert_eq!(finalize_coverage(cov, &known).unwrap().requirements.len(), 8);
    }

    #[test]
    fn finalize_coverage_downgrades_verdicts_with_only_hallucinated_tests() {
        let req = |status: &str, tests: Vec<&str>| crate::types::RequirementEntry {
            text: "Does the thing".to_string(),
            status: status.to_string(),
            tests: tests
                .into_iter()
                .map(|p| crate::types::TestRef { path: p.to_string(), note: None })
                .collect(),
            note: None,
        };
        let cov = RequirementsCoverage {
            requirements: vec![
                req("covered", vec!["tests/made_up.test.ts"]),
                req("partial", vec!["tests/real.test.ts", "tests/made_up.test.ts"]),
                // Uncited "covered" is left alone — nothing was hallucinated.
                req("covered", vec![]),
            ],
            orphan_tests: vec![],
            source_issues: vec![],
        };
        let known: HashSet<&str> = ["tests/real.test.ts"].into_iter().collect();
        let result = finalize_coverage(cov, &known).unwrap();
        assert_eq!(result.requirements[0].status, "uncovered");
        assert_eq!(result.requirements[1].status, "partial");
        assert_eq!(result.requirements[1].tests.len(), 1);
        assert_eq!(result.requirements[2].status, "covered");
    }

    #[test]
    fn manifest_without_coverage_field_deserializes_to_none() {
        let json = r#"{
            "pr_title": "t", "pr_url": "u", "pr_number": 1,
            "base_ref": "main", "head_ref": "f", "base_sha": "a", "head_sha": "b",
            "author": "o", "draft": false, "summary": "", "change_groups": [],
            "body": "", "commits": [], "files": []
        }"#;
        let m: crate::types::ReviewManifest = serde_json::from_str(json).unwrap();
        assert!(m.requirements_coverage.is_none());
        assert!(m.triage.is_none());
    }

    #[test]
    fn finalize_coverage_drops_unknown_statuses() {
        let req = |status: &str| crate::types::RequirementEntry {
            text: "Does the thing".to_string(),
            status: status.to_string(),
            tests: vec![],
            note: None,
        };
        let cov = RequirementsCoverage {
            requirements: vec![req("covered"), req("mystery"), req("partial")],
            orphan_tests: vec![],
            source_issues: vec![],
        };
        let known: HashSet<&str> = HashSet::new();
        let result = finalize_coverage(cov, &known).unwrap();
        assert_eq!(result.requirements.len(), 2);
        assert!(result.requirements.iter().all(|r| r.status != "mystery"));

        // All statuses unknown → the pass reports nothing at all.
        let cov = RequirementsCoverage { requirements: vec![req("mystery")], orphan_tests: vec![], source_issues: vec![] };
        assert!(finalize_coverage(cov, &known).is_none());
    }

    use super::pick_related_test_paths;

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn pick_related_test_paths_matches_by_stem() {
        let tree = strs(&["src/digest.ts", "src/digest.test.ts", "tests/parser.rs", "README.md"]);
        let changed = strs(&["src/digest.ts"]);
        let already = HashSet::new();
        let picked = pick_related_test_paths(&tree, &changed, &already);
        assert_eq!(picked, vec!["src/digest.test.ts".to_string()]);
    }

    #[test]
    fn pick_related_test_paths_skips_unrelated_and_already_included() {
        let tree = strs(&["src/digest.test.ts", "tests/parser.rs", "tests/unrelated.rs"]);
        let changed = strs(&["src/digest.ts", "src/parser.rs"]);
        let already: HashSet<&str> = ["src/digest.test.ts"].into_iter().collect();
        let picked = pick_related_test_paths(&tree, &changed, &already);
        assert_eq!(picked, vec!["tests/parser.rs".to_string()]);
    }

    #[test]
    fn pick_related_test_paths_caps_at_five_deterministically() {
        let tree: Vec<String> = (0..9).map(|i| format!("tests/digest_{}.rs", i)).collect();
        let changed = strs(&["src/digest.rs"]);
        let already = HashSet::new();
        let picked = pick_related_test_paths(&tree, &changed, &already);
        // All score 1 → tie broken by path asc, capped at 5.
        assert_eq!(
            picked,
            strs(&[
                "tests/digest_0.rs",
                "tests/digest_1.rs",
                "tests/digest_2.rs",
                "tests/digest_3.rs",
                "tests/digest_4.rs",
            ])
        );
    }

    #[test]
    fn pick_related_test_paths_ranks_by_score_then_path() {
        let tree = strs(&["tests/z_digest_parser.rs", "tests/digest_only.rs", "tests/parser_only.rs"]);
        let changed = strs(&["src/digest.rs", "src/parser.rs"]);
        let already = HashSet::new();
        let picked = pick_related_test_paths(&tree, &changed, &already);
        // Two-stem match outranks single-stem matches despite its later path.
        assert_eq!(
            picked,
            strs(&["tests/z_digest_parser.rs", "tests/digest_only.rs", "tests/parser_only.rs"])
        );
    }

    #[test]
    fn pick_related_test_paths_ignores_short_stems_and_changed_test_files() {
        // "db.rs" stem is too short to seed matches; changed test files don't
        // seed stems either.
        let tree = strs(&["tests/db_backup.rs", "tests/digest.rs"]);
        let changed = strs(&["src/db.rs", "tests/digest.rs"]);
        let already = HashSet::new();
        let picked = pick_related_test_paths(&tree, &changed, &already);
        assert!(picked.is_empty());
    }
}
