# looks-scary-correct-ts

Hard fixture (corpus v7). A change that looks risky and isn't: a null
check is removed, but the only caller (in the same diff) returns early on a
missing invoice and the parameter type now excludes null. The right review
is "ship" with nothing flagged. Tests false alarms, where a weaker model
might over-warn.
