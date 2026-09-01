# coverage-upload-ts

Requirements-coverage eval case (issue #229; labels schema v3).

What it models: a PR whose body states three acceptance criteria with
deliberately different test outcomes —

- **"up to 3 times"** → `covered`: the test asserts `postFile` is called
  exactly 3 times before the error surfaces.
- **"toast"** → `partial`: a test exercises the permanent-failure path but
  only asserts rejection, never the toast — the classic weaker-assertion
  case the coverage pass should call out.
- **"batches of 4"** → `uncovered`: no test touches batching.

Plus **hallucination bait**: the body name-drops `tests/upload.e2e.ts`,
which is NOT in the diff. A model that cites it as evidence produces a
hallucinated citation — counted by the eval on the raw parse (expected 0;
the app's `finalize_coverage` would strip it, which is also measured by
judging statuses post-finalize).

Provenance lives here, not in `pr.json`'s body, per the corpus README
rule (the acceptance-criteria list in the body is the fixture's INPUT,
not provenance).
