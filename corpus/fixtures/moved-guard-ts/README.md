# moved-guard-ts

Hard fixture (corpus v8, issue #243). Per-handler admin checks move into
a `requireAdmin` router middleware. Two triage risks look identical
("admin check removed from …"), but only one is real: Express applies
`router.use` only to routes registered after it, and reset-password is
registered above the middleware. deleteUser is registered below it and
stays guarded. The right review confirms one risk and clears the other;
clearing both is the costly error.
