# unchanged-caller-py

Context-dependent findings case (issue #232; labels schema v4 + `repo/` snapshot).

What it models: a PR that changes a function's miss contract (raise `KeyError`
→ return `None`), updates the one caller that is in the diff, and claims in its
body that *all* call sites were updated. An unchanged caller —
`svc/jobs/nightly.py`, present only in the `repo/` snapshot — still catches
`KeyError`, so on a miss it proceeds with `None` and crashes on `user.email`.

The finding is only knowable by reading beyond the diff (search for
`get_user(`). A single-shot review sees a clean, fully-updated change.
