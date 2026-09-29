# looks-scary-correct-ts

Hard fixture (corpus v7). A change that looks risky and isn't: a null
check is removed, but the only caller (in the same diff) returns early on a
missing invoice and the parameter type now excludes null. The right review
is "ship" with nothing flagged. Tests false alarms, where a weaker model
might over-warn.

Corpus v8 (issue #243) adds a `repo/` snapshot: renderInvoice's
not-found check is outside the diff's context lines, so a review
clearing the "missing invoice" risk on evidence has to read it.
