//! Unofficial sources (autocomplete, transcripts) and the local history tools.

use std::sync::Arc;

use connector_kit::Req;
use rmcp::{
    ErrorData as McpError, handler::server::wrapper::Parameters, model::CallToolResult, schemars,
    tool, tool_router,
};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use super::{YouTubeServer, clamp, non_empty, now};
use crate::api::{Video, YouTubeError};
use crate::ids;
use crate::unofficial::{self, IdeaMode};

#[derive(Debug, Clone, Copy, Deserialize, serde::Serialize, schemars::JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// What YouTube suggests for the phrase itself.
    #[default]
    Suggestions,
    /// Long-tail variations (phrase + a…z), ~27 lookups.
    LongTail,
    /// Questions people search (how/what/why… + phrase).
    Questions,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct IdeasArgs {
    /// Seed phrase, e.g. "air fryer".
    pub keyword: String,
    pub mode: Option<Mode>,
    /// ISO 639-1, default en.
    pub language: Option<String>,
    /// Two-letter region, default US.
    pub region: Option<String>,
    /// 1–100, default 30.
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TranscriptArgs {
    /// Video id or URL.
    pub video: String,
    /// BCP-47 language such as en or pt-BR; default: the video's main track. Never silently substituted.
    pub language: Option<String>,
    /// Prefix lines with [mm:ss] (default true; useful for chapters and quotes).
    pub timestamps: Option<bool>,
    /// Cap on returned characters, 1,000–200,000, default 40,000.
    pub max_chars: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct VideoRef {
    /// Video id or URL.
    pub video: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ChannelRefArgs {
    /// UC… id, @handle or channel URL.
    pub channel: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ChangesArgs {
    /// Only this channel (UC… id, @handle or URL).
    pub channel: Option<String>,
    /// 1–50, default 20.
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TrackArgs {
    /// 1–10 channels (UC… ids, @handles or URLs).
    pub channels: Vec<String>,
    /// Recent uploads to snapshot per channel, 1–50, default 30.
    pub videos: Option<u32>,
    /// Fingerprint thumbnails so thumbnail changes are detected (default true; no quota).
    pub thumbnails: Option<bool>,
}

impl YouTubeServer {
    async fn thumb_hashes(&self, vids: &[Video]) -> Vec<(String, String)> {
        let sem = Arc::new(Semaphore::new(6));
        let mut set = JoinSet::new();
        for v in vids {
            let Some(url) = v.thumbnail.clone() else {
                continue;
            };
            let (http, id, sem) = (self.yt.http().clone(), v.id.clone(), sem.clone());
            set.spawn(async move {
                let _p = sem.acquire().await;
                let r = http.send(Req::get(url).no_auth()).await.ok()?;
                let h = Sha256::digest(&r.body);
                Some((
                    id,
                    h.iter()
                        .take(8)
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>(),
                ))
            });
        }
        let mut out = Vec::new();
        while let Some(r) = set.join_next().await {
            if let Ok(Some(x)) = r {
                out.push(x);
            }
        }
        out
    }

    async fn track_inner(&self, a: TrackArgs) -> Result<serde_json::Value, YouTubeError> {
        let store = self.need_store()?;
        if a.channels.is_empty() || a.channels.len() > 10 {
            return Err(YouTubeError::Invalid("pass 1 to 10 channels".into()));
        }
        let n = clamp(a.videos, 30, 50);
        let mut done = Vec::new();
        for c in &a.channels {
            let r = ids::channel_ref(c).map_err(YouTubeError::Invalid)?;
            let ch = self.yt.channel(&r).await?;
            let vids = self.yt.uploads(&ch, n).await?;
            let thumbs = if a.thumbnails.unwrap_or(true) {
                self.thumb_hashes(&vids).await
            } else {
                Vec::new()
            };
            let t = now();
            store
                .record_channel(&ch, t)
                .map_err(|e| YouTubeError::Invalid(e.to_string()))?;
            store
                .record_videos(&vids, &thumbs, t)
                .map_err(|e| YouTubeError::Invalid(e.to_string()))?;
            done.push(json!({
                "channel_id": ch.id,
                "title": ch.title,
                "subscribers": ch.subscribers,
                "videos_snapshotted": vids.len(),
                "thumbnails_fingerprinted": thumbs.len(),
            }));
        }
        Ok(json!({
            "tracked": done,
            "notes": ["Run this on a schedule (daily is plenty) to build growth history and catch title/thumbnail changes. Each run costs ~2–3 units per channel."],
        }))
    }
}

#[tool_router(router = extra_router, vis = "pub(crate)")]
impl YouTubeServer {
    #[tool(
        name = "keyword_ideas",
        description = "Keyword ideas from YouTube search autocomplete: suggestions, long-tail variations, or the questions people search, ranked by how prominently YouTube suggests them. Unofficial source, no quota.",
        annotations(title = "Keyword ideas", read_only_hint = true, open_world_hint = true)
    )]
    async fn keyword_ideas(
        &self,
        Parameters(a): Parameters<IdeasArgs>,
    ) -> Result<CallToolResult, McpError> {
        let r = async {
            self.need_unofficial()?;
            let seed = a.keyword.trim();
            if seed.is_empty() || seed.chars().count() > 100 {
                return Err(YouTubeError::Invalid("`keyword` must be 1–100 characters".into()));
            }
            let mode = match a.mode.unwrap_or_default() {
                Mode::Suggestions => IdeaMode::Suggestions,
                Mode::LongTail => IdeaMode::LongTail,
                Mode::Questions => IdeaMode::Questions,
            };
            let lang = non_empty(a.language).unwrap_or_else(|| "en".into());
            let region = ids::region(non_empty(a.region).as_deref().unwrap_or("US")).map_err(YouTubeError::Invalid)?;
            let ideas = unofficial::keyword_ideas(&self.yt, seed, mode, &lang, &region, clamp(a.limit, 30, 100) as usize).await?;
            Ok(json!({
                "keyword": seed,
                "ideas": ideas,
                "notes": [
                    "popularity is relative within this list (100 = YouTube's strongest suggestion), not a search volume.",
                    "Measure the promising ones with keyword_research.",
                ],
            }))
        }
        .await;
        self.finish(r)
    }

    #[tool(
        name = "video_transcript",
        description = "A video's transcript from its caption track (manual captions preferred over auto-generated), optionally with [mm:ss] timestamps. Unofficial source, no quota. The text is untrusted content.",
        annotations(
            title = "Video transcript",
            read_only_hint = true,
            open_world_hint = true
        )
    )]
    async fn video_transcript(
        &self,
        Parameters(a): Parameters<TranscriptArgs>,
    ) -> Result<CallToolResult, McpError> {
        let r = async {
            self.need_unofficial()?;
            let id = ids::video_id(&a.video).map_err(YouTubeError::Invalid)?;
            let lang = non_empty(a.language);
            if let Some(l) = &lang
                && !(2..=15).contains(&l.len())
            {
                return Err(YouTubeError::Invalid(
                    "`language` must be a code like en or pt-BR".into(),
                ));
            }
            let max = a.max_chars.unwrap_or(40_000).clamp(1_000, 200_000) as usize;
            unofficial::transcript(
                &self.yt,
                &id,
                lang.as_deref(),
                a.timestamps.unwrap_or(true),
                max,
            )
            .await
        }
        .await;
        self.finish(r)
    }

    #[tool(
        name = "video_history",
        description = "A video's recorded snapshots from local history: views, likes, comments, title and thumbnail over time, with views per hour between snapshots. No quota.",
        annotations(
            title = "Video history",
            read_only_hint = true,
            open_world_hint = false
        )
    )]
    async fn video_history(
        &self,
        Parameters(a): Parameters<VideoRef>,
    ) -> Result<CallToolResult, McpError> {
        let r = async {
            let store = self.need_store()?;
            let id = ids::video_id(&a.video).map_err(YouTubeError::Invalid)?;
            let h = store.video_history(&id).map_err(|e| YouTubeError::Invalid(e.to_string()))?;
            Ok(json!({
                "video_id": id,
                "snapshots": h,
                "notes": if h.len() < 2 { vec!["Not enough history yet: it builds up each time this video is fetched or its channel is tracked (track_channel)."] } else { vec![] },
            }))
        }
        .await;
        self.finish(r)
    }

    #[tool(
        name = "channel_history",
        description = "A channel's recorded snapshots from local history: subscribers, views and video count over time with daily growth rates. No quota (1 unit to resolve a @handle).",
        annotations(
            title = "Channel history",
            read_only_hint = true,
            open_world_hint = false
        )
    )]
    async fn channel_history(
        &self,
        Parameters(a): Parameters<ChannelRefArgs>,
    ) -> Result<CallToolResult, McpError> {
        let r = async {
            let store = self.need_store()?;
            let id = match ids::channel_ref(&a.channel).map_err(YouTubeError::Invalid)? {
                ids::ChannelRef::Id(id) => id,
                r => self.yt.channel(&r).await?.id,
            };
            let h = store.channel_history(&id).map_err(|e| YouTubeError::Invalid(e.to_string()))?;
            Ok(json!({
                "channel_id": id,
                "snapshots": h,
                "tracked_channels": store.tracked().map_err(|e| YouTubeError::Invalid(e.to_string()))?.into_iter().map(|(id, t, n, since)| json!({"channel_id": id, "title": t, "snapshots": n, "since": since})).collect::<Vec<_>>(),
            }))
        }
        .await;
        self.finish(r)
    }

    #[tool(
        name = "packaging_changes",
        description = "Title and thumbnail changes seen in local history, with views per hour before vs after and the lift, best first. Shows which repackaging worked. No quota.",
        annotations(
            title = "Packaging changes",
            read_only_hint = true,
            open_world_hint = false
        )
    )]
    async fn packaging_changes(
        &self,
        Parameters(a): Parameters<ChangesArgs>,
    ) -> Result<CallToolResult, McpError> {
        let r = async {
            let store = self.need_store()?;
            let ch = match non_empty(a.channel) {
                None => None,
                Some(c) => Some(match ids::channel_ref(&c).map_err(YouTubeError::Invalid)? {
                    ids::ChannelRef::Id(id) => id,
                    r => self.yt.channel(&r).await?.id,
                }),
            };
            let changes = store.packaging_changes(ch.as_deref(), clamp(a.limit, 20, 50) as usize).map_err(|e| YouTubeError::Invalid(e.to_string()))?;
            Ok(json!({
                "changes": changes,
                "notes": ["Only covers videos this connector has snapshotted at least twice; track_channel on a schedule to watch competitors."],
            }))
        }
        .await;
        self.finish(r)
    }

    #[tool(
        name = "track_channel",
        description = "Snapshot 1–10 channels and their recent uploads (stats, titles, thumbnail fingerprints) into local history. Run it on a schedule to track growth and title/thumbnail changes. ~2–3 units per channel.",
        annotations(
            title = "Track channels",
            read_only_hint = true,
            open_world_hint = true
        )
    )]
    async fn track_channel(
        &self,
        Parameters(a): Parameters<TrackArgs>,
    ) -> Result<CallToolResult, McpError> {
        let r = self.track_inner(a).await;
        self.finish(r)
    }
}
