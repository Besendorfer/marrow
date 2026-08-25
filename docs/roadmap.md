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
- [ ] Resolve or narrow the parent requirements issue (#179).
- [ ] Review older open PR #178 and either finish or close it.

## Phase 2: Reliability Foundation

- [ ] Add explicit network connection and request timeouts.
- [ ] Add bounded retries with jitter for transient GitHub and AI-provider failures.
- [ ] Handle GitHub rate limits explicitly and communicate retry timing.
- [ ] Record each AI pass as complete, truncated, failed, or not run.
- [ ] Ensure pass failures cannot silently become empty successful results.
- [ ] Fingerprint cached analysis using pipeline version, model/provider, prompts,
      budgets, relevant settings, and PR metadata.
- [ ] Make local state writes atomic and synchronize concurrent updates.
- [x] Make diagnostic and provider-response truncation Unicode-safe.
- [ ] Bound concurrent GitHub file-content requests.
- [ ] Reduce unnecessary diff/content duplication for large PRs.
- [ ] Fix Claude CLI streaming argument construction and concurrent stderr draining.
- [ ] Route review-body generation through the provider-neutral AI abstraction.
- [ ] Validate and sanitize every owner/repository persistence key.
- [ ] URL-encode GitHub repository paths and query values correctly.
- [ ] Merge old and new manifest-cache discovery instead of hiding old entries.
- [ ] Validate AI classifications, files, severities, and line ranges.
- [ ] Fix critical-risk ranking in the CLI/TUI.
- [ ] Reconcile the published CLI/core dependency versions.
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
