//! Recorded malformed provider-response shapes (issue #219, roadmap Phase 3
//! "include malformed provider responses"). These are the failure shapes
//! observed live or plausible from real providers, pinned as deterministic
//! offline tests over the public extraction API — the corpus for cases that
//! need no AI call.

use marrow_core::ai::{extract_json_array, extract_json_object};

/// The shape captured live 2026-08-30: the model prefixed prose, opened an
/// array, and got cut off mid-object (output cap). Extraction must fail —
/// pre-#199 this rendered as a clean zero-findings review.
#[test]
fn truncated_mid_array_response_fails_extraction() {
    let recorded = r#"Based on my analysis, here are the notable findings:

[
  {
    "path": "app/src-tauri/crates/core/src/manifest_cache.rs",
    "start_line": 67,
    "end_line": 72,
    "severity": "info",
    "comment": "The lock serializes concurrent writers, but if the second write_atomic (metadata) fails after the manifest write succeeds, you get a new manifest paired with stale metadata."
  },
  {
    "path": "app/src-ta"#;
    let err = extract_json_array(recorded).unwrap_err();
    assert!(err.contains("Could not extract JSON array"), "{err}");
}

/// Prose-wrapped but complete: extraction must succeed (models often narrate
/// before the JSON despite instructions).
#[test]
fn prose_wrapped_complete_array_extracts() {
    let recorded = r#"Sure! Here is the JSON you asked for:

[{"path": "a.rs", "start_line": 1, "end_line": 2, "severity": "info", "comment": "x"}]

Let me know if you need anything else."#;
    let v = extract_json_array(recorded).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1);
}

/// Markdown-fenced JSON: the fence must not defeat extraction.
#[test]
fn fenced_array_extracts() {
    let recorded = "```json\n[{\"path\": \"a.rs\", \"start_line\": 1, \"end_line\": 1, \"severity\": \"info\", \"comment\": \"x\"}]\n```";
    let v = extract_json_array(recorded).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1);
}

/// A refusal / non-answer: must fail extraction, never read as "no findings".
#[test]
fn refusal_text_fails_extraction() {
    let recorded = "I'm sorry, but I can't review this pull request.";
    assert!(extract_json_array(recorded).is_err());
    assert!(extract_json_object(recorded).is_err());
}

/// An object where an array was requested (and vice versa): the caller's
/// parse layer decides — extraction itself must return the payload it finds
/// for arrays, and object extraction must find a braced payload even with
/// trailing prose.
#[test]
fn object_extraction_survives_trailing_prose() {
    let recorded = r#"{"review_order": [], "top_risks": []}

Note: I ordered the files by risk."#;
    let v = extract_json_object(recorded).unwrap();
    assert!(v.get("review_order").is_some());
}
