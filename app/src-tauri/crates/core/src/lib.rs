//! marrow-core — the frontend-agnostic core shared by the desktop app and the
//! `marrow` CLI: GitHub client, fetch/diff pipeline, AI classification, config,
//! and local caching. No Tauri, no UI.

pub mod ai;
pub mod bedrock;
pub mod checks_dismiss;
pub mod config;
pub mod fetch;
pub mod github;
pub mod manifest_cache;
pub mod pr_parser;
pub mod prompts;
pub mod session;
pub mod types;
pub mod viewed_state;
