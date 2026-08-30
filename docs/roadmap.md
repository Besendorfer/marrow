# Marrow Roadmap

This is the shared product and engineering roadmap for Marrow. It records the
repository review performed in August 2026 so future contributors and coding
agents can continue the work without reconstructing the analysis.

Marrow's strongest product position is broader than AI diff filtering:

> Marrow assembles the evidence needed to make a review decision, then directs
> human attention to what still requires judgment.

The app already has substantial product depth. Near-term work should favor
trustworthiness, recovery, accessibility, and measurable AI quality over adding
more surfaces.

## Working Principles

- "No findings" must mean analysis succeeded and found nothing.
- AI output must remain advisory, inspectable, and editable before mutation.
- Prefer fewer, more actionable findings over more AI output.
- Measure prompt and model changes against stable examples before shipping.
- Keep failures local and preserve the reviewer's work.
- Treat private source, prompts, tokens, and local state as sensitive data.
- Avoid broad feature expansion until reliability and quality are measurable.

## Phase 1: Finish Active Work

- [x] Ship linked-issue requirements coverage (#189 / PR #190). Shipped in v0.36.0.
- [x] Ship model-scale analysis budgets and visible truncation (#191 / PR #192). Shipped in v0.36.0.
- [x] Resolve or narrow the parent requirements issue (#179). Closed as
      shipped + narrowed 2026-08-25: remaining scope split into #194
      (hunk-level linkage + untested-hunks filter), #195 (traceability map),
      and #196 (test-quality lens).
- [x] Review older open PR #178 and either finish or close it. Finished:
      rebased, converged, and merged 2026-08-25 (prefix fix + search reveal
      + search perf).

## Phase 2: Reliability Foundation

- [x] Add explicit network connection and request timeouts. (#198 / PR #199,
      merged 2026-08-28: shared client, 10s connect, 30s/120s/600s deadlines.)
- [x] Add bounded retries with jitter for transient GitHub and AI-provider
      failures. (#198 / PR #199: 3 attempts, jittered backoff; mutations
      connect-only; timed-out inference never retried.)
- [x] Handle GitHub rate limits explicitly and communicate retry timing.
      (#198 / PR #199: Retry-After honored ≤30s, every declined wait names
      its timing.)
- [x] Record each AI pass as complete, truncated, failed, or not run.
      (#204 / PR #205, merged 2026-08-29: unified `passes` record per
      manifest; skipped passes visible for the first time.)
- [x] Ensure pass failures cannot silently become empty successful results.
      (#198 / PR #199: highlights failure fails the fetch; other passes record
      `failed_passes` + Overview "analysis incomplete" notice.)
- [x] Fingerprint cached analysis using pipeline version, model/provider, prompts,
      budgets, relevant settings, and PR metadata. (#202 / PR #203, merged
      2026-08-29: analysis_fingerprint stamped per manifest, Overview flags
      stale caches; head_sha remains the PR-content leg.)
- [x] Make local state writes atomic and synchronize concurrent updates.
      (#200 / PR #201, merged 2026-08-28: state_io atomic replace for all 11
      state modules, locked manifest+meta pair, migration lint-test.)
- [x] Make diagnostic and provider-response truncation Unicode-safe.
- [x] Bound concurrent GitHub file-content requests. (#206 / PR #207, merged
      2026-08-29: 5 files ×2 requests ≤10 in flight, peak-concurrency tested.)
- [ ] Reduce unnecessary diff/content duplication for large PRs.
- [x] Fix Claude CLI streaming argument construction and concurrent stderr
      draining. (#208 / PR #209, merged 2026-08-29: empty-model guard +
      concurrent capped stderr drain + kill_on_drop parity.)
- [x] ~~Route review-body generation through the provider-neutral AI
      abstraction.~~ Narrowed 2026-08-29: audit found no review-body
      generation code outside the AI abstraction — review submission posts
      user-written text only. Re-open with specifics if the referenced code
      is found.
- [x] Validate and sanitize every owner/repository persistence key.
      (#208 / PR #209: shared `state_io::sanitize_key`, all 7 keyed modules
      wired, private copies deduped.)
- [x] URL-encode GitHub repository paths and query values correctly.
      (#208 / PR #209: per-segment path encoding + ref encoding on both
      contents endpoints; search endpoints already encoded.)
- [ ] Merge old and new manifest-cache discovery instead of hiding old entries.
- [x] Validate AI classifications, files, severities, and line ranges.
      (#210 / PR #211, merged 2026-08-30: ingestion validators drop
      hallucinated paths, dedupe, and normalize enums/ranges; files already
      could not vanish — manifest iterates GitHub's list.)
- [ ] Fix critical-risk ranking in the CLI/TUI.
- [x] Reconcile the published CLI/core dependency versions. (#208 / PR #209:
      marrow-core path-dep pin brought to lockstep with the workspace
      version; crates.io publish remains the manual release-time step.)
- [ ] Fix or reassess Python barrel-file classification (#71).

## Phase 3: Quality Measurement

- [ ] Create a versioned corpus of representative PR fixtures.
- [ ] Score relevant-file classification precision and recall.
- [ ] Score important findings, missed findings, and low-value findings.
- [ ] Evaluate requirements coverage and hallucinated test evidence.
- [ ] Include malformed provider responses, giant PRs, and adversarial cases.
- [ ] Compare prompts, models, and providers against the same corpus.
- [ ] Report quality regressions in pull requests.
- [ ] Add mocked HTTP tests for providers and GitHub.
- [ ] Test timeouts, retries, pagination, streaming boundaries, and rate limits.
- [ ] Test persistence corruption and concurrent writes.
- [ ] Add Tauri command-boundary tests.
- [ ] Add frontend interaction tests for the primary review flows.

## Phase 4: Reviewer Trust and Personalization

- [ ] Semantically match new findings to previously resolved findings (#128).
- [ ] Suppress strong matches marked intentional or noise.
- [ ] Explain suppressed findings and allow restoration.
- [ ] Conservatively learn recurring rejected finding categories.
- [ ] Add rationale and a suggested action to findings (#130).
- [ ] Rank findings by confidence and actionability.
- [ ] Hide descriptive or low-confidence notes by default without deleting them.
- [ ] Avoid unsupported precision in displayed confidence scores.
- [ ] Bound chat history context or compact long conversations.

## Phase 5: Resilient Desktop UX

- [ ] Keep review-submission failures local instead of replacing the PR screen.
- [ ] Preserve review and comment drafts after failures.
- [ ] Standardize pending, failure, retry, and success mutation behavior.
- [ ] Add visible settings load/save errors.
- [ ] Add scoped retry to checks, commits, conversations, queue sections, and caches.
- [ ] Give every PR tab an independent loading lifecycle.
- [ ] Queue deep links received while another PR is loading.
- [ ] Normalize progress around one clearly defined review-file set.
- [ ] Prevent negative initial loading progress.
- [ ] Reconsider hard-blocking review submission while CI is pending.
- [ ] Make sidebars and right-side docks collapsible or resizable.
- [ ] Add layouts for constrained desktop widths.
- [ ] Improve meaningful muted-text contrast and critical hit-target sizes.

## Phase 6: Accessibility

- [ ] Introduce a shared accessible dialog primitive with focus management.
- [ ] Use proper tab semantics for PR tabs and review lenses.
- [ ] Add consistent visible focus styling.
- [ ] Add expanded, pressed, current, progress, and live-region semantics.
- [ ] Make statuses understandable without color alone.
- [ ] Make thread headers, comment controls, and collapsible groups keyboard usable.
- [ ] Respect reduced-motion preferences during automatic scrolling.
- [ ] Improve screen-reader structure in the diff viewer.

## Phase 7: Onboarding, Security, and Diagnostics

- [ ] Add a provider selector and provider-specific fields to first-run setup.
- [ ] Replace provider-biased labels such as "Claude Model."
- [ ] Explain provider data flow and local CLI options.
- [ ] Add optional OS Keychain storage (#108).
- [ ] Retain environment-variable support for CLI and advanced users.
- [ ] Add a local redacted diagnostics/support bundle.
- [ ] Include versions, provider type, failed stage, timings, truncation, rate-limit
      state, and sanitized errors in diagnostics.
- [ ] Never include code, prompts, repository identifiers, or credentials.
- [ ] Consider only explicit, privacy-preserving remote telemetry after local
      diagnostics exist.

## Phase 8: Review Workflow Improvements

- [ ] Add a local per-PR reviewer scratchpad (#64).
- [ ] Let reviewers collect questions, findings, and draft feedback there.
- [ ] Help assemble scratchpad items into a final review without auto-posting.
- [ ] Expand unchanged lines incrementally around hunks (#59).
- [ ] Add regex, scoped, and base-content search (#62).
- [ ] Add keyboard commit walking (#172).
- [ ] Show when an overview regenerated in the background (#173).
- [ ] Add Copy PR URL (#53).
- [ ] Add organization filters to the review queue (#57).

## Phase 9: Deeper Differentiation

- [ ] Build cross-file impact navigation (#63).
- [ ] Show changed and unchanged call sites on demand.
- [ ] Connect symbols to tests, registration points, and configuration.
- [ ] Detect likely unmodified callers after behavior or signature changes.
- [ ] Extend requirements coverage toward requirement-to-test-to-hunk traceability.
- [ ] Add focused review profiles such as security, infrastructure, API
      compatibility, and migrations.
- [ ] Prefer review profiles over a matrix of independent feature flags (#58).
- [ ] Consider custom approval-message guidance (#54).
- [ ] Expand chat answer cards (#174) only for concrete review questions.

## Phase 10: CLI/TUI Parity

- [ ] Surface triage, requirements coverage, truncation, and failures in the TUI.
- [ ] Add commit-lens capabilities and line-level CI annotations.
- [ ] Consider chat and read-only repository tools in terminal workflows.
- [ ] Add conversation reading, reactions, and editing where they fit the TUI.
- [ ] Sign and notarize CLI release binaries (#111).
- [ ] Define and finish the remaining CLI production gaps (#78).

## Phase 11: Engineering Maintainability

- [ ] Add ordinary PR CI for frontend build/tests and Rust checks/tests.
- [ ] Gradually extract per-tab loading, review mutations, conversations, analysis
      lifecycle, and session persistence from `App.tsx`; do not rewrite wholesale.
- [ ] Generate or validate duplicated frontend/backend chat protocols.
- [ ] Centralize the duplicated PR-reference parser contract and fixtures.
- [ ] Centralize risk/severity types instead of comparing free-form strings.
- [ ] Correct tiny-PR grouping instructions.
- [ ] Add formatting and package-consistency checks to CI.

## Phase 12: Product Story and Distribution

- [ ] Position Marrow as the attention-and-evidence layer for review decisions.
- [ ] Stop describing tests and UI categorically as noise.
- [ ] Make user-facing AI-provider language provider-neutral.
- [ ] Document GitHub token permissions and provider setup recipes.
- [ ] Explain AI data flow, privacy, likely cost, and large-PR truncation.
- [ ] Document requirements-coverage semantics, troubleshooting, and diagnostics.
- [ ] Complete the browser-extension Marrow rebrand while preserving compatible
      deep links unless a deliberate migration is designed.
- [ ] Document extension installation or distribute through browser stores.
- [ ] Test the extension against GitHub layout fixtures.
- [ ] Consider Intel macOS and broader desktop support only after measuring demand.

## Phase 13: Repository and Release Hygiene

- [ ] Close or narrow issues already substantially implemented.
- [ ] Reassess #68 (resolve/unresolve already exists).
- [ ] Narrow #77 to remaining extension/compatibility rebrand work.
- [ ] Narrow #78 to remaining CLI production gaps.
- [ ] Narrow #18 to unsupported packaging targets.
- [ ] Reconcile README, security policy, publishing docs, workspace metadata, and
      released CLI versions.
- [ ] Prepare release-note drafts automatically while preserving manual editing.
- [ ] Keep final release publishing manual because signing and updater artifacts
      require care.

## Lower Priority Until Foundations Improve

- Additional chart/card vocabulary that does not answer a concrete review question.
- Broad feature-flag matrices that multiply UI states.
- New desktop platforms without demonstrated demand.
- Remote analytics before redacted local diagnostics and clear privacy controls.
