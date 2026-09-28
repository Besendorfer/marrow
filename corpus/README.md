# Marrow quality corpus

Versioned PR fixtures for measuring analysis quality (roadmap Phase 3,
issue #219). The corpus is the stable yardstick the working principles
demand: prompt and model changes are measured against these fixtures, not
shipped on faith.

## Format

- `VERSION` — bumped whenever any fixture's inputs or labels change, so eval
  results always name the corpus they were scored against.
- One directory per fixture under `fixtures/`:
  - `pr.json` — frozen inputs: `{ "title", "body", "files": [{ "path", "diff" }] }`.
    Diffs are unified-diff bodies (no `diff --git` header; the runner adds
    them when assembling the whole-PR diff).
  - `labels.json` — expected outcomes:
    `{ "relevant": [paths…], "not_relevant": [paths…] }`. Every path in
    `pr.json` must appear in exactly one list. Two optional lists (schema
    v2) drive findings scoring:
    - `expected_findings`: regions a good review MUST flag —
      `{ path, start_line, end_line, importance: "important"|"minor", note }`.
    - `should_not_flag`: regions a good review should NOT flag (e.g. the
      PR's stated purpose); flagging one counts as a low-value finding.
    A model highlight matches a region when paths are equal and line ranges
    overlap. Highlights matching no label report neutrally as "extra".
    One optional list (schema v3) drives requirements-coverage scoring:
    - `expected_coverage`: `{ "requirement_contains": <case-insensitive
      substring>, "status": "covered"|"partial"|"uncovered"|"untestable" }`.
      Substring matching because requirement text is model-extracted. The
      eval also counts hallucinated citations — test paths cited in the raw
      output that were never shown to the model (expected 0).
    One optional field (schema v4, issue #231) drives verdict scoring:
    - `expected_verdict`: `"fix_first"|"ship"|"needs_discussion"` — the
      review's expected one-line verdict. A fixture with only this label
      still runs the findings pass. The report also counts "complete"
      findings: bug/behavior/test_gap findings carrying both a scenario and
      a fix.
  - `repo/` (optional, issue #232) — a snapshot the agentic review's repo
    tools read instead of GitHub: `repo/head/**` and `repo/base/**` are the
    PR's repo at its head/base commits; `repo/other/<name>/**` is sibling
    repo `<name>` (same owner) at its default branch. Fixtures whose
    findings are only knowable from outside the diff carry one; without it
    the tools find nothing.

## Jev probes

`jev-probes.json` (issue #249) holds reviewer-style claims about fixture
diffs, each labeled `real` or `counterfeit`, with a `kind` (real /
invented / wrong_line / intended / trivial) and `outside_diff` for real
claims only verifiable from code beyond the diff. Optional per-fixture
`evidence` lists repo-snapshot files an agentic review would have read.
`marrow eval --corpus ../../corpus --jev-probe` measures how well Jev's
P(real) separates them (AUC, accuracy@0.5), with no review calls. Claims
are hand-written like a reviewer's; don't copy label notes, which speak to
labelers.

`jev-dedupe.json` (issue #249) holds pairs of findings on one fixture,
labeled `same` (one problem said twice: merge), `related` (one root cause,
different actions, e.g. a bug and its missing test: group) or `different`.
`marrow eval --corpus ../../corpus --jev-dedupe` scores Jev's call on each
pair; merging two `different` findings is the costly error.

## Labeling rules

- Labels encode the CLASSIFICATION_PROMPT's *intent*, decided by a human at
  fixture-creation time — they are the spec, not a model's past output.
- Provenance: note what real case a fixture models in a fixture-local
  README. For fixtures carrying findings labels it must NOT go in
  `pr.json`'s `body` — the body is fed to the model verbatim, so
  describing the planted finding there hands the review its answer.
  Synthetic content is fine; secrets and private code are not.

## Running

```bash
cargo run -p marrow-cli -- eval --corpus ../../corpus   # from app/src-tauri
```

Add `--single-shot` to run the review as one prompt without repo tools
(the pre-#232 pipeline) for before/after comparison; the default is the
agentic review the app runs, and the report shows each fixture's tool-call
count and any single-shot fallback ("DEGRADED").

Runs the real classification pass with your configured provider/model over
every fixture and reports precision/recall for RELEVANT plus per-file
mismatches. Results are provider- and model-dependent by design — that is
the point: run before/after a prompt or model change and compare.

Deterministic cases that need no AI (malformed provider responses) live as
regular tests in `crates/core/tests/` instead of here.
