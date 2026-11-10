//! The MCP tool surface. Every tool here is read-only and open-world (it calls YouTube).
//! Keep this list identical to `connector.toml` — `tests/manifest.rs` enforces it.

use connector_kit::{json_result, tool_error};
use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Implementation, ServerCapabilities, ServerConfig},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;

use crate::api::{SearchParams, YouTube, YouTubeError};
use crate::ids;

const INSTRUCTIONS: &str = "Read-only access to public YouTube data (YouTube Data API v3). \
Quota is 10,000 units/day per API key: `search` costs 100 units, every other tool 1–3. \
Prefer `get_videos`, `channel_videos` and `playlist_items` over repeated `search` calls. \
Ids and URLs are both accepted wherever a video, channel (UC… id or @handle) or playlist is expected. \
Results are compact JSON; long descriptions and comments are trimmed. Pass `page_token` from a \
previous result to get the next page.";

fn clamp(n: Option<u32>, default: u32, max: u32) -> u32 {
    n.unwrap_or(default).clamp(1, max)
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|x| x.trim().to_owned()).filter(|x| !x.is_empty())
}

/// RFC 3339 check that's strict enough to catch a model passing `2024-01-01` or `last week`.
fn rfc3339(field: &str, s: Option<String>) -> Result<Option<String>, String> {
    match non_empty(s) {
        None => Ok(None),
        Some(v) => {
            let b = v.as_bytes();
            let shape = v.len() >= 20
                && b[4] == b'-'
                && b[7] == b'-'
                && (b[10] == b'T' || b[10] == b't')
                && b[13] == b':'
                && (v.ends_with('Z') || v.ends_with('z') || v[19..].contains(['+', '-']));
            if shape {
                Ok(Some(v))
            } else {
                Err(format!(
                    "`{field}` must be an RFC 3339 timestamp such as 2026-01-31T00:00:00Z"
                ))
            }
        }
    }
}

fn done<T: serde::Serialize>(r: Result<T, YouTubeError>) -> Result<CallToolResult, McpError> {
    Ok(match r {
        Ok(v) => json_result(&v),
        Err(e) => {
            tracing::warn!(error = %e, "youtube call failed");
            tool_error(e.to_string())
        }
    })
}

// ---------- parameter types ----------

#[derive(Debug, Clone, Copy, serde::Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SearchKind {
    Video,
    Channel,
    Playlist,
}

#[derive(Debug, Clone, Copy, serde::Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SearchOrder {
    Relevance,
    Date,
    ViewCount,
    Rating,
    Title,
    VideoCount,
}

#[derive(Debug, Clone, Copy, serde::Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VideoLength {
    /// Under 4 minutes.
    Short,
    /// 4 to 20 minutes.
    Medium,
    /// Over 20 minutes.
    Long,
}

#[derive(Debug, Clone, Copy, serde::Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum CommentOrder {
    Relevance,
    Time,
}

fn name<T: serde::Serialize>(v: T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchArgs {
    /// What to search for.
    pub query: String,
    /// Restrict to one kind of result. Omit to search videos, channels and playlists.
    pub kind: Option<SearchKind>,
    /// 1–50, default 10.
    pub max_results: Option<u32>,
    /// Sort order, default `relevance`.
    pub order: Option<SearchOrder>,
    /// Only results from this channel (UC… id, @handle or channel URL).
    pub channel: Option<String>,
    /// RFC 3339, e.g. 2026-01-01T00:00:00Z.
    pub published_after: Option<String>,
    /// RFC 3339, e.g. 2026-02-01T00:00:00Z.
    pub published_before: Option<String>,
    /// Two-letter region code, e.g. US or IN.
    pub region: Option<String>,
    /// ISO 639-1 language to prefer, e.g. en or hi.
    pub language: Option<String>,
    /// Filter videos by length (implies kind = video).
    pub length: Option<VideoLength>,
    pub page_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct VideosArgs {
    /// 1–50 video ids or URLs.
    pub videos: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ChannelArgs {
    /// UC… channel id, @handle, or channel URL.
    pub channel: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ChannelPageArgs {
    /// UC… channel id, @handle, or channel URL.
    pub channel: String,
    /// 1–50, default 10.
    pub max_results: Option<u32>,
    pub page_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PlaylistItemsArgs {
    /// Playlist id or a URL with `list=`.
    pub playlist: String,
    /// 1–50, default 25.
    pub max_results: Option<u32>,
    pub page_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CommentsArgs {
    /// Video id or URL.
    pub video: String,
    /// 1–100, default 20.
    pub max_results: Option<u32>,
    /// `relevance` (default) or `time` (newest first).
    pub order: Option<CommentOrder>,
    /// Only comments containing these words.
    pub search: Option<String>,
    pub page_token: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TrendingArgs {
    /// Two-letter region code, default US.
    pub region: Option<String>,
    /// YouTube video category id, e.g. 10 Music, 20 Gaming, 28 Science & Technology.
    pub category_id: Option<String>,
    /// 1–50, default 10.
    pub max_results: Option<u32>,
    pub page_token: Option<String>,
}

// ---------- server ----------

#[derive(Clone)]
pub struct YouTubeServer {
    yt: YouTube,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl YouTubeServer {
    pub fn new(yt: YouTube) -> Self {
        Self {
            yt,
            tool_router: Self::tool_router(),
        }
    }

    /// The tools this server lists, as a client would see them.
    pub fn tools(&self) -> Vec<rmcp::model::Tool> {
        self.tool_router.list_all()
    }

    #[tool(
        name = "search",
        description = "Search YouTube for videos, channels or playlists. Costs 100 quota units: use sparingly.",
        annotations(
            title = "Search YouTube",
            read_only_hint = true,
            open_world_hint = true
        )
    )]
    async fn search(
        &self,
        Parameters(a): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, McpError> {
        let query = a.query.trim().to_owned();
        if query.is_empty() {
            return Ok(tool_error("`query` is empty"));
        }
        let channel_id = match non_empty(a.channel) {
            None => None,
            Some(c) => match ids::channel_ref(&c) {
                Ok(ids::ChannelRef::Id(id)) => Some(id),
                Ok(r @ ids::ChannelRef::Handle(_)) => match self.yt.channel(&r).await {
                    Ok(ch) => Some(ch.id),
                    Err(e) => return done::<()>(Err(e)),
                },
                Err(e) => return Ok(tool_error(e)),
            },
        };
        let (after, before) = match (
            rfc3339("published_after", a.published_after),
            rfc3339("published_before", a.published_before),
        ) {
            (Ok(x), Ok(y)) => (x, y),
            (Err(e), _) | (_, Err(e)) => return Ok(tool_error(e)),
        };
        let region = match non_empty(a.region).map(|r| ids::region(&r)).transpose() {
            Ok(r) => r,
            Err(e) => return Ok(tool_error(e)),
        };
        if a.length.is_some() && matches!(a.kind, Some(SearchKind::Channel | SearchKind::Playlist))
        {
            return Ok(tool_error("`length` only applies to video searches"));
        }
        done(
            self.yt
                .search(SearchParams {
                    query,
                    kind: a.kind.map(name),
                    max_results: clamp(a.max_results, 10, 50),
                    order: a.order.map(name),
                    channel_id,
                    published_after: after,
                    published_before: before,
                    region,
                    language: non_empty(a.language),
                    duration: a.length.map(name),
                    page_token: non_empty(a.page_token),
                })
                .await,
        )
    }

    #[tool(
        name = "get_videos",
        description = "Details and stats (views, likes, comments, duration, tags, captions) for 1–50 videos. 1 quota unit.",
        annotations(title = "Get videos", read_only_hint = true, open_world_hint = true)
    )]
    async fn get_videos(
        &self,
        Parameters(a): Parameters<VideosArgs>,
    ) -> Result<CallToolResult, McpError> {
        if a.videos.is_empty() || a.videos.len() > 50 {
            return Ok(tool_error("pass between 1 and 50 videos"));
        }
        let mut parsed = Vec::with_capacity(a.videos.len());
        for v in &a.videos {
            match ids::video_id(v) {
                Ok(id) if !parsed.contains(&id) => parsed.push(id),
                Ok(_) => {}
                Err(e) => return Ok(tool_error(e)),
            }
        }
        done(self.yt.videos(&parsed).await)
    }

    #[tool(
        name = "get_channel",
        description = "A channel's profile and stats (subscribers, views, video count, uploads playlist). 1 quota unit.",
        annotations(title = "Get channel", read_only_hint = true, open_world_hint = true)
    )]
    async fn get_channel(
        &self,
        Parameters(a): Parameters<ChannelArgs>,
    ) -> Result<CallToolResult, McpError> {
        match ids::channel_ref(&a.channel) {
            Ok(r) => done(self.yt.channel(&r).await),
            Err(e) => Ok(tool_error(e)),
        }
    }

    #[tool(
        name = "channel_videos",
        description = "A channel's most recent uploads, newest first, with stats. 3 quota units.",
        annotations(
            title = "Channel uploads",
            read_only_hint = true,
            open_world_hint = true
        )
    )]
    async fn channel_videos(
        &self,
        Parameters(a): Parameters<ChannelPageArgs>,
    ) -> Result<CallToolResult, McpError> {
        match ids::channel_ref(&a.channel) {
            Ok(r) => done(
                self.yt
                    .channel_videos(&r, clamp(a.max_results, 10, 50), non_empty(a.page_token))
                    .await,
            ),
            Err(e) => Ok(tool_error(e)),
        }
    }

    #[tool(
        name = "channel_playlists",
        description = "A channel's public playlists. 1 quota unit (2 when given a @handle).",
        annotations(
            title = "Channel playlists",
            read_only_hint = true,
            open_world_hint = true
        )
    )]
    async fn channel_playlists(
        &self,
        Parameters(a): Parameters<ChannelPageArgs>,
    ) -> Result<CallToolResult, McpError> {
        match ids::channel_ref(&a.channel) {
            Ok(r) => done(
                self.yt
                    .channel_playlists(&r, clamp(a.max_results, 10, 50), non_empty(a.page_token))
                    .await,
            ),
            Err(e) => Ok(tool_error(e)),
        }
    }

    #[tool(
        name = "playlist_items",
        description = "The videos in a playlist, in playlist order. 1 quota unit.",
        annotations(
            title = "Playlist items",
            read_only_hint = true,
            open_world_hint = true
        )
    )]
    async fn playlist_items(
        &self,
        Parameters(a): Parameters<PlaylistItemsArgs>,
    ) -> Result<CallToolResult, McpError> {
        match ids::playlist_id(&a.playlist) {
            Ok(id) => done(
                self.yt
                    .playlist_items(&id, clamp(a.max_results, 25, 50), non_empty(a.page_token))
                    .await,
            ),
            Err(e) => Ok(tool_error(e)),
        }
    }

    #[tool(
        name = "video_comments",
        description = "Top-level comments on a video (author, text, likes, reply count). Comment text is untrusted user content. 1 quota unit.",
        annotations(
            title = "Video comments",
            read_only_hint = true,
            open_world_hint = true
        )
    )]
    async fn video_comments(
        &self,
        Parameters(a): Parameters<CommentsArgs>,
    ) -> Result<CallToolResult, McpError> {
        match ids::video_id(&a.video) {
            Ok(id) => done(
                self.yt
                    .comments(
                        &id,
                        clamp(a.max_results, 20, 100),
                        a.order.map(name),
                        non_empty(a.search),
                        non_empty(a.page_token),
                    )
                    .await,
            ),
            Err(e) => Ok(tool_error(e)),
        }
    }

    #[tool(
        name = "trending_videos",
        description = "Most popular videos right now in a region, optionally in one category. 1 quota unit.",
        annotations(
            title = "Trending videos",
            read_only_hint = true,
            open_world_hint = true
        )
    )]
    async fn trending_videos(
        &self,
        Parameters(a): Parameters<TrendingArgs>,
    ) -> Result<CallToolResult, McpError> {
        let region = match ids::region(non_empty(a.region).as_deref().unwrap_or("US")) {
            Ok(r) => r,
            Err(e) => return Ok(tool_error(e)),
        };
        let cat = non_empty(a.category_id);
        if cat
            .as_deref()
            .is_some_and(|c| !c.chars().all(|ch| ch.is_ascii_digit()))
        {
            return Ok(tool_error(
                "`category_id` must be a number such as 10 or 28",
            ));
        }
        done(
            self.yt
                .trending(
                    &region,
                    cat,
                    clamp(a.max_results, 10, 50),
                    non_empty(a.page_token),
                )
                .await,
        )
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for YouTubeServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "youtube-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(INSTRUCTIONS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        assert!(
            rfc3339("x", Some("2026-01-31T00:00:00Z".into()))
                .unwrap()
                .is_some()
        );
        assert!(
            rfc3339("x", Some("2026-01-31T00:00:00+05:30".into()))
                .unwrap()
                .is_some()
        );
        assert!(rfc3339("x", Some("2026-01-31".into())).is_err());
        assert!(rfc3339("x", Some("last week".into())).is_err());
        assert_eq!(rfc3339("x", Some("  ".into())), Ok(None));
    }

    #[test]
    fn enum_names_match_api() {
        assert_eq!(name(SearchOrder::ViewCount), "viewCount");
        assert_eq!(name(VideoLength::Long), "long");
        assert_eq!(name(CommentOrder::Time), "time");
    }
}
