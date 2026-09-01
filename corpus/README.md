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

Runs the real classification pass with your configured provider/model over
every fixture and reports precision/recall for RELEVANT plus per-file
mismatches. Results are provider- and model-dependent by design — that is
the point: run before/after a prompt or model change and compare.

Deterministic cases that need no AI (malformed provider responses) live as
regular tests in `crates/core/tests/` instead of here.
