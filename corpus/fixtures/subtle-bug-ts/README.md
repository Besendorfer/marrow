# subtle-bug-ts

Synthetic subtle-bug review case (issue #223).

What it models: a well-intentioned DRY refactor that silently changes
semantics — the extracted `pageCount()` helper uses `Math.floor` where
both inline call sites used `Math.ceil`, so the last partial page of any
listing/export is dropped. No hint appears in `pr.json`'s body (see the
corpus README's provenance rule). The expected-finding region spans the
helper and the first call site (new lines 3–11) so a review that flags
either location scores as found. The CHANGELOG edit is the
classification not-relevant signal.
