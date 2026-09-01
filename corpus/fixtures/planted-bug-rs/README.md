# planted-bug-rs

Synthetic planted-bug review case (labels schema v2, issue #221).

What it models: a token-refresh refactor that silently DROPS the
`expires_at` check before reusing a cached token — the important finding a
reviewer must catch (`src/auth/refresh.rs` L20–30). The removal of
`refresh_legacy` is the PR's stated purpose and must NOT be flagged
(`should_not_flag`), and `do_token_thing` is a deliberately vague helper
name planted as the minor naming nit (L60–65).

Provenance lives HERE, not in `pr.json`'s body: the body is fed to the
model verbatim, so describing the planted bug there would hand the review
its answer and inflate the findings score. Keep `pr.json`'s body written
in the (oblivious) PR author's voice.
