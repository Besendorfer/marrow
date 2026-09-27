# cross-repo-contract-rs

Cross-repo context case (issue #232; labels schema v4 + `repo/other/`).

What it models: an API PR that changes an enum's wire format (PascalCase →
snake_case) and states in its body that the `web` dashboard "already reads
snake_case" — a false claim. The sibling repo snapshot `repo/other/web/`
still switches on `"InProgress"` etc., so every status badge would render
"Unknown" after deploy.

The diff alone looks like a deliberate, described, low-risk convention fix
(and the stated-purpose rule discourages flagging it). Catching it needs a
read of the sibling repo (`repo: "web"`) or an owner-wide search for
`InProgress`.
