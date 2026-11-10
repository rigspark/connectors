//! `youtube-mcp`: RigSpark's MCP connector for public YouTube data (Data API v3).
//!
//! Read-only. Needs `YOUTUBE_API_KEY` in its environment; see `connector.toml`.

pub mod api;
pub mod ids;
pub mod tools;

pub use api::YouTube;
pub use tools::YouTubeServer;

/// This connector's manifest, compiled in so `--manifest` can print it.
pub const MANIFEST: &str = include_str!("../connector.toml");

/// Env var holding the API key.
pub const KEY_ENV: &str = "YOUTUBE_API_KEY";
/// Optional API base override (tests, proxies).
pub const BASE_ENV: &str = "YOUTUBE_API_BASE";
