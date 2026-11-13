//! `youtube-mcp`: RigSpark's MCP connector for YouTube.
//!
//! Public data and research with an API key; the creator's own analytics and channel edits
//! with OAuth; keyword ideas and transcripts from unofficial endpoints (switchable); and a
//! local history database for growth and packaging-change tracking. See `connector.toml`.

pub mod analysis;
pub mod api;
pub mod config;
pub mod earnings;
pub mod ids;
pub mod insights;
pub mod oauth;
pub mod owner;
pub mod pii;
pub mod store;
pub mod tools;
pub mod unofficial;

pub use api::YouTube;
pub use config::{BASE_ENV, Config, Endpoints, KEY_ENV};
pub use tools::YouTubeServer;

/// This connector's manifest, compiled in so `--manifest` can print it.
pub const MANIFEST: &str = include_str!("../connector.toml");
