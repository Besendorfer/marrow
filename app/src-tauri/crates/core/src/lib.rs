//! marrow-core — the frontend-agnostic core shared by the desktop app and the
//! `marrow` CLI: GitHub client, fetch/diff pipeline, AI classification, config,
//! and local caching. No Tauri, no UI.

pub mod activity;
pub mod ai;
pub mod bedrock;
pub mod chat;
pub mod chat_agent;
pub mod chat_history;
pub mod checks_dismiss;
pub mod config;
pub mod dismissed_highlights;
pub mod fetch;
pub mod github;
pub mod manifest_cache;
pub mod pr_parser;
pub mod prompts;
pub mod session;
pub mod types;
pub mod viewed_state;
pub mod watches;
