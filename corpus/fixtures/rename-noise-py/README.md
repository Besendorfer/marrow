# rename-noise-py

Synthetic rename-noise bait case (issue #223).

What it models: a "mechanical, no behavior change" rename PR where almost
every hunk really is mechanical (`fetch_user` → `load_user`; those are
`should_not_flag` regions), but one swap is poisoned: in
`svc/jobs.py`'s nightly fan-out, `fetch_user_cached` was swept to
`load_user`, silently bypassing the cache — the planted important
finding. `loaders.py` keeps `fetch_user_cached` visible as a context
line so the bug is discoverable from the diff alone.

Measures two things at once: noise discipline (do mechanical hunks get
flagged? → low-value) and buried-bug recall (is the one real change
found among lookalikes?). Provenance lives here, not in `pr.json`'s
body, per the corpus README rule.
