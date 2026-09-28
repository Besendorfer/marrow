# zero-means-unlimited-rs

Hard fixture (corpus v7; `repo/` snapshot). The default connection cap
becomes 0, which the PR says means "no limit". Nothing implements that:
`pool/builder.rs` passes the value to `Pool::new`, and `pool/mod.rs`
refuses to acquire when `in_use >= max`, so every acquire fails. Needs
following the value through two unchanged files.
