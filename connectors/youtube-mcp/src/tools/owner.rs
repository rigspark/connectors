//! Channel-owner tools (OAuth): analytics reads and the write actions.
//! Every write is declared `access = "write"` in connector.toml so the crew asks first.

use connector_kit::time::{format_date, now_unix};
use rmcp::{
    ErrorData as McpError, handler::server::wrapper::Parameters, model::CallToolResult, schemars,
    tool, tool_router,
};
use serde::Deserialize;
use serde_json::json;

use super::{YouTubeServer, date, non_empty, rfc3339};
use crate::api::YouTubeError;
use crate::ids;
use crate::insights;
use crate::owner::{self, AnalyticsQuery, UploadMeta, VideoChanges};

#[derive(Debug, Clone, Copy, Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Privacy {
    Private,
    Unlisted,
    Public,
}

#[derive(Debug, Clone, Copy, Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    Overview,
    Daily,
    TopVideos,
    AudienceRetention,
    TrafficSources,
    SearchTerms,
    AudienceDemographics,
    AudienceGeography,
    Devices,
    SubscribedStatus,
    ShortsVsLongform,
    Revenue,
    RevenuePerVideo,
    /// Thumbnail impressions and click-through rate per video.
    CtrByVideo,
    /// Average view duration and percentage watched per video.
    RetentionByVideo,
    /// Views from other websites, by domain.
    ExternalSources,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AnalyticsArgs {
    /// A ready-made report. Or pass `metrics` (and `dimensions`) for a custom one.
    pub report: Option<Preset>,
    /// Restrict to one video (id or URL). Required for audience_retention.
    pub video: Option<String>,
    /// YYYY-MM-DD, default 28 days ago.
    pub start_date: Option<String>,
    /// YYYY-MM-DD, default today.
    pub end_date: Option<String>,
    /// Custom metrics, e.g. views, engagedViews, estimatedMinutesWatched, averageViewPercentage, subscribersGained, estimatedRevenue.
    pub metrics: Option<Vec<String>>,
    /// Custom dimensions, e.g. day, month, country, video, deviceType, insightTrafficSourceType.
    pub dimensions: Option<Vec<String>>,
    /// Extra filter, e.g. country==US or creatorContentType==shorts (joined to the video filter with ;).
    pub filters: Option<String>,
    /// e.g. -views
    pub sort: Option<String>,
    pub max_results: Option<u32>,
    /// One of your channels (UC…); default: the signed-in channel.
    pub channel_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InsightsArgs {
    /// YYYY-MM-DD, default 28 days ago.
    pub start_date: Option<String>,
    /// YYYY-MM-DD, default today.
    pub end_date: Option<String>,
    /// One of your channels (UC…); default: the signed-in channel.
    pub channel_id: Option<String>,
    /// Skip the one-hour cache and fetch again. Default false.
    pub fresh: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateArgs {
    /// Your video (id or URL).
    pub video: String,
    pub title: Option<String>,
    pub description: Option<String>,
    /// Replaces all tags.
    pub tags: Option<Vec<String>>,
    pub category_id: Option<String>,
    pub privacy: Option<Privacy>,
    /// Schedule publishing (RFC 3339); the video must be private.
    pub publish_at: Option<String>,
    pub default_language: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ThumbArgs {
    /// Your video (id or URL).
    pub video: String,
    /// Path to a .jpg or .png (≤ 2 MB, 1280×720 recommended) inside the host's upload folder.
    pub file: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReplyArgs {
    /// The comment (thread) id to reply to, from video_comments.
    pub comment_id: String,
    pub text: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PostArgs {
    /// Video id or URL.
    pub video: String,
    pub text: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UploadArgs {
    /// Path to the video file inside the host's upload folder.
    pub file: String,
    pub title: String,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    /// Default 22 (People & Blogs).
    pub category_id: Option<String>,
    /// Default private.
    pub privacy: Option<Privacy>,
    /// Schedule publishing (RFC 3339); forces private until then.
    pub publish_at: Option<String>,
    /// Default false.
    pub made_for_kids: Option<bool>,
    /// Default false.
    pub notify_subscribers: Option<bool>,
}

fn text_ok(t: &str, max: usize) -> Result<String, YouTubeError> {
    let t = t.trim();
    if t.is_empty() {
        return Err(YouTubeError::Invalid("text is empty".into()));
    }
    if t.chars().count() > max {
        return Err(YouTubeError::Invalid(format!(
            "text is longer than {max} characters"
        )));
    }
    Ok(t.to_owned())
}

fn title_ok(t: &str) -> Result<String, YouTubeError> {
    let t = t.trim();
    if t.is_empty() || t.chars().count() > 100 || t.contains(['<', '>']) {
        return Err(YouTubeError::Invalid(
            "titles must be 1–100 characters without < or >".into(),
        ));
    }
    Ok(t.to_owned())
}

fn category_ok(c: Option<String>) -> Result<Option<String>, YouTubeError> {
    match non_empty(c) {
        Some(c) if c.chars().all(|x| x.is_ascii_digit()) => Ok(Some(c)),
        Some(_) => Err(YouTubeError::Invalid(
            "`category_id` must be a number such as 22 or 28".into(),
        )),
        None => Ok(None),
    }
}

impl YouTubeServer {
    async fn analytics_inner(&self, a: AnalyticsArgs) -> Result<owner::Report, YouTubeError> {
        let tok = self.token().await?;
        let today = now_unix();
        let end = match a.end_date {
            Some(d) => date("end_date", &d).map_err(YouTubeError::Invalid)?,
            None => format_date(today),
        };
        let start = match a.start_date {
            Some(d) => date("start_date", &d).map_err(YouTubeError::Invalid)?,
            None => format_date(today - 28 * 86_400),
        };
        if start > end {
            return Err(YouTubeError::Invalid("start_date is after end_date".into()));
        }
        let video = a
            .video
            .as_deref()
            .map(ids::video_id)
            .transpose()
            .map_err(YouTubeError::Invalid)?;
        let mut q = AnalyticsQuery {
            channel_id: non_empty(a.channel_id),
            start,
            end,
            ..Default::default()
        };
        if let Some(p) = a.report {
            let name = super::name(p);
            let (m, d, s, n) = owner::preset(&name)
                .ok_or_else(|| YouTubeError::Invalid(format!("unknown report {name}")))?;
            q.metrics = m.iter().map(|x| (*x).to_owned()).collect();
            q.dimensions = d.iter().map(|x| (*x).to_owned()).collect();
            q.sort = s.map(str::to_owned);
            q.max_results = n;
            if name == "audience_retention" && video.is_none() {
                return Err(YouTubeError::Invalid(
                    "audience_retention needs `video`".into(),
                ));
            }
            q.filters = owner::preset_filter(&name).map(str::to_owned);
        }
        if let Some(m) = a.metrics.filter(|m| !m.is_empty()) {
            q.metrics = m;
        }
        if let Some(d) = a.dimensions {
            q.dimensions = d;
        }
        if q.metrics.is_empty() {
            return Err(YouTubeError::Invalid(format!(
                "pass `report` ({}) or `metrics`",
                owner::PRESETS.join(", ")
            )));
        }
        let ok = |s: &str| {
            s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "=;,-_.".contains(c))
        };
        if !q.metrics.iter().chain(q.dimensions.iter()).all(|x| ok(x)) {
            return Err(YouTubeError::Invalid(
                "metrics and dimensions are plain names like views or day".into(),
            ));
        }
        let mut filters: Vec<String> = q.filters.take().into_iter().collect();
        if let Some(v) = video {
            filters.push(format!("video=={v}"));
        }
        if let Some(f) = non_empty(a.filters) {
            filters.push(f);
        }
        q.filters = (!filters.is_empty()).then(|| filters.join(";"));
        q.sort = non_empty(a.sort).or(q.sort);
        q.max_results = a.max_results.or(q.max_results).map(|m| m.clamp(1, 200));
        owner::analytics(&self.yt, &tok, q).await
    }

    async fn report(
        &self,
        tok: &connector_kit::Secret,
        channel: &Option<String>,
        (start, end): (&str, &str),
        preset: &str,
    ) -> Result<Vec<insights::Row>, YouTubeError> {
        let (m, d, s, n) = owner::preset(preset).expect("known preset");
        let q = AnalyticsQuery {
            channel_id: channel.clone(),
            start: start.to_owned(),
            end: end.to_owned(),
            metrics: m.iter().map(|x| (*x).to_owned()).collect(),
            dimensions: d.iter().map(|x| (*x).to_owned()).collect(),
            filters: owner::preset_filter(preset).map(str::to_owned),
            sort: s.map(str::to_owned),
            max_results: n,
            currency: None,
        };
        Ok(owner::analytics(&self.yt, tok, q).await?.rows)
    }

    async fn channel_insights_inner(
        &self,
        a: InsightsArgs,
    ) -> Result<serde_json::Value, YouTubeError> {
        let today = now_unix();
        let end = match a.end_date {
            Some(d) => date("end_date", &d).map_err(YouTubeError::Invalid)?,
            None => format_date(today),
        };
        let start = match a.start_date {
            Some(d) => date("start_date", &d).map_err(YouTubeError::Invalid)?,
            None => format_date(today - 28 * 86_400),
        };
        if start > end {
            return Err(YouTubeError::Invalid("start_date is after end_date".into()));
        }
        let channel = non_empty(a.channel_id);
        let key = format!("{}|{start}|{end}", channel.as_deref().unwrap_or("MINE"));
        if !a.fresh.unwrap_or(false)
            && let Some((at, v)) = self
                .insights_cache
                .lock()
                .ok()
                .and_then(|c| c.get(&key).cloned())
            && today - at < 3600
        {
            let mut v = v;
            v["cached_minutes_ago"] = json!((today - at) / 60);
            return Ok(v);
        }

        let tok = self.token().await?;
        let range = (start.as_str(), end.as_str());
        let (r48s, r48e) = (format_date(today - 2 * 86_400), format_date(today));
        let custom =
            |metrics: &[&str], dims: &[&str], sort: Option<&str>, n: Option<u32>| AnalyticsQuery {
                channel_id: channel.clone(),
                start: r48s.clone(),
                end: r48e.clone(),
                metrics: metrics.iter().map(|x| (*x).to_owned()).collect(),
                dimensions: dims.iter().map(|x| (*x).to_owned()).collect(),
                sort: sort.map(str::to_owned),
                max_results: n,
                ..Default::default()
            };
        let (ctr, ret, traffic, external, demo, geo, days48, vids48) = tokio::join!(
            self.report(&tok, &channel, range, "ctr_by_video"),
            self.report(&tok, &channel, range, "retention_by_video"),
            self.report(&tok, &channel, range, "traffic_sources"),
            self.report(&tok, &channel, range, "external_sources"),
            self.report(&tok, &channel, range, "audience_demographics"),
            self.report(&tok, &channel, range, "audience_geography"),
            owner::analytics(
                &self.yt,
                &tok,
                custom(&["views"], &["day"], Some("day"), None)
            ),
            owner::analytics(
                &self.yt,
                &tok,
                custom(&["views"], &["video"], Some("-views"), Some(10))
            ),
        );

        let mut notes: Vec<String> = Vec::new();
        let mut failed = 0;
        let mut first_err: Option<YouTubeError> = None;
        let mut take = |name: &str, r: Result<Vec<insights::Row>, YouTubeError>| match r {
            Ok(rows) => rows,
            Err(e) => {
                notes.push(format!("{name}: {}", self.clean(&e)));
                failed += 1;
                first_err.get_or_insert(e);
                Vec::new()
            }
        };
        let ctr = take("click-through", ctr);
        let ret = take("retention", ret);
        let traffic = take("traffic sources", traffic);
        let external = take("external sites", external);
        let demo = take("demographics", demo);
        let geo = take("geography", geo);
        let days48 = take("last 48 hours", days48.map(|r| r.rows));
        let vids48 = take("last 48 hours by video", vids48.map(|r| r.rows));
        if failed == 8 {
            return Err(first_err.expect("eight errors"));
        }

        let (mut videos, averages) = insights::videos(&ctr, &ret);
        let traffic = insights::shares(&traffic, "insightTrafficSourceType");
        let external = insights::shares(&external, "insightTrafficSourceDetail");
        let mut countries = insights::shares(&geo, "country");
        countries.truncate(10);
        let audience = insights::audience(&demo);
        let mut recent = insights::recent(&days48, &vids48);

        // Analytics reports carry ids only; one Data API call names up to 50 videos.
        let mut ids: Vec<String> = videos.iter().take(40).map(|v| v.video_id.clone()).collect();
        for v in &recent.top_videos {
            if !ids.contains(&v.video_id) && ids.len() < 50 {
                ids.push(v.video_id.clone());
            }
        }
        match self.yt.videos(&ids).await {
            Ok(found) => {
                self.remember_videos(&found);
                let title = |id: &str| found.iter().find(|f| f.id == id).map(|f| f.title.clone());
                for v in &mut videos {
                    v.title = title(&v.video_id);
                }
                for v in &mut recent.top_videos {
                    v.title = title(&v.video_id);
                }
            }
            Err(e) => notes.push(format!("titles: {}", self.clean(&e))),
        }

        let headlines = insights::headlines(&videos, &averages, &traffic);
        let traffic: Vec<serde_json::Value> = traffic
            .into_iter()
            .map(|s| {
                let mut v = json!(s);
                v["label"] = json!(insights::traffic_name(&s.name));
                v
            })
            .collect();
        notes.push(format!(
            "CTR is compared only for videos with ≥ {} impressions and retention for ≥ {} views; \"below\" and \"above\" mean 15% off the channel average.",
            insights::MIN_IMPRESSIONS as u64,
            insights::MIN_VIEWS as u64
        ));
        notes.push("Analytics lag 1–3 days, so the last 48 hours are partial.".into());
        let out = json!({
            "channel": channel.as_deref().unwrap_or("signed-in channel"),
            "start": start,
            "end": end,
            "headlines": headlines,
            "channel_averages": averages,
            "videos": videos,
            "traffic_sources": traffic,
            "external_sites": external,
            "audience": {
                "age_groups": audience.age_groups,
                "gender": audience.gender,
                "top_countries": countries,
            },
            "last_48_hours": recent,
            "notes": notes,
        });
        if let Ok(mut c) = self.insights_cache.lock() {
            c.retain(|_, (at, _)| today - *at < 3600);
            c.insert(key, (today, out.clone()));
        }
        Ok(out)
    }
}

#[tool_router(router = owner_router, vis = "pub(crate)")]
impl YouTubeServer {
    #[tool(
        name = "my_channels",
        description = "The channels the signed-in creator owns, with stats. Needs OAuth. 1 unit.",
        annotations(title = "My channels", read_only_hint = true, open_world_hint = true)
    )]
    async fn my_channels(&self) -> Result<CallToolResult, McpError> {
        let r = async {
            let tok = self.token().await?;
            let chans = owner::my_channels(&self.yt, &tok).await?;
            for c in &chans {
                self.remember_channel(c);
            }
            Ok(chans)
        }
        .await;
        self.finish(r)
    }

    #[tool(
        name = "channel_analytics",
        description = "YouTube Analytics for your own channel (OAuth): views, watch time, engaged views, subscribers, retention curves, traffic sources, YouTube search terms, demographics, geography, devices, Shorts vs long-form, revenue and RPM. Use a `report` preset or custom metrics/dimensions. Data lags 1–3 days.",
        annotations(
            title = "Channel analytics",
            read_only_hint = true,
            open_world_hint = true
        )
    )]
    async fn channel_analytics(
        &self,
        Parameters(a): Parameters<AnalyticsArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.finish(self.analytics_inner(a).await)
    }

    #[tool(
        name = "channel_insights",
        description = "One-call private health check of your own channel (OAuth), from ViewsFix: per-video thumbnail CTR and retention against your channel averages with a diagnosis (packaging = people skip the thumbnail, content = they click then leave), traffic sources with shares, external sites, age, gender and top countries, and the last 48 hours. Headlines first. Cached for an hour; ~1 Data API unit.",
        annotations(
            title = "Channel insights",
            read_only_hint = true,
            open_world_hint = true
        )
    )]
    async fn channel_insights(
        &self,
        Parameters(a): Parameters<InsightsArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.finish(self.channel_insights_inner(a).await)
    }

    #[tool(
        name = "update_video",
        description = "Change your video's title, description, tags, category, privacy or scheduled publish time (OAuth, write). Returns each field's before and after. Confirm with the user first.",
        annotations(
            title = "Update video",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn update_video(
        &self,
        Parameters(a): Parameters<UpdateArgs>,
    ) -> Result<CallToolResult, McpError> {
        let r = async {
            let id = ids::video_id(&a.video).map_err(YouTubeError::Invalid)?;
            let changes = VideoChanges {
                title: a.title.as_deref().map(title_ok).transpose()?,
                description: a
                    .description
                    .as_deref()
                    .map(|d| text_ok(d, 5000))
                    .transpose()?,
                tags: a.tags,
                category_id: category_ok(a.category_id)?,
                privacy: a.privacy.map(super::name),
                publish_at: rfc3339("publish_at", a.publish_at).map_err(YouTubeError::Invalid)?,
                default_language: non_empty(a.default_language),
            };
            if let Some(t) = &changes.tags
                && t.iter().map(|x| x.chars().count() + 1).sum::<usize>() > 500
            {
                return Err(YouTubeError::Invalid(
                    "tags total more than 500 characters".into(),
                ));
            }
            let tok = self.token().await?;
            let diff = owner::update_video(&self.yt, &tok, &id, changes).await?;
            Ok(json!({
                "video_id": id,
                "changed": diff,
                "unchanged": diff.is_empty(),
                "studio_url": format!("https://studio.youtube.com/video/{id}/edit"),
            }))
        }
        .await;
        self.finish(r)
    }

    #[tool(
        name = "set_thumbnail",
        description = "Upload a new thumbnail for your video from a .jpg/.png in the host's upload folder (OAuth, write). Confirm with the user first.",
        annotations(
            title = "Set thumbnail",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn set_thumbnail(
        &self,
        Parameters(a): Parameters<ThumbArgs>,
    ) -> Result<CallToolResult, McpError> {
        let r = async {
            let id = ids::video_id(&a.video).map_err(YouTubeError::Invalid)?;
            let path = owner::check_path(&a.file, self.upload_dir.as_deref())?;
            let tok = self.token().await?;
            owner::set_thumbnail(&self.yt, &tok, &id, &path).await
        }
        .await;
        self.finish(r)
    }

    #[tool(
        name = "reply_to_comment",
        description = "Reply to a comment as your channel (OAuth, write). Posts publicly: confirm the exact text with the user first.",
        annotations(
            title = "Reply to comment",
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = true
        )
    )]
    async fn reply_to_comment(
        &self,
        Parameters(a): Parameters<ReplyArgs>,
    ) -> Result<CallToolResult, McpError> {
        let r = async {
            let parent = a.comment_id.trim();
            if parent.is_empty()
                || parent.len() > 100
                || !parent
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            {
                return Err(YouTubeError::Invalid(
                    "`comment_id` must be a comment id from video_comments".into(),
                ));
            }
            let text = text_ok(&a.text, 10_000)?;
            let tok = self.token().await?;
            owner::reply_to_comment(&self.yt, &tok, parent, &text).await
        }
        .await;
        self.finish(r)
    }

    #[tool(
        name = "post_comment",
        description = "Post a top-level comment on a video as your channel (OAuth, write). Posts publicly: confirm the exact text with the user first.",
        annotations(
            title = "Post comment",
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = true
        )
    )]
    async fn post_comment(
        &self,
        Parameters(a): Parameters<PostArgs>,
    ) -> Result<CallToolResult, McpError> {
        let r = async {
            let id = ids::video_id(&a.video).map_err(YouTubeError::Invalid)?;
            let text = text_ok(&a.text, 10_000)?;
            let tok = self.token().await?;
            owner::post_comment(&self.yt, &tok, &id, &text).await
        }
        .await;
        self.finish(r)
    }

    #[tool(
        name = "upload_video",
        description = "Upload a video file from the host's upload folder to your channel (OAuth, write). Private by default; set `publish_at` to schedule. ~1,600 quota units. Confirm with the user first.",
        annotations(
            title = "Upload video",
            read_only_hint = false,
            destructive_hint = false,
            open_world_hint = true
        )
    )]
    async fn upload_video(
        &self,
        Parameters(a): Parameters<UploadArgs>,
    ) -> Result<CallToolResult, McpError> {
        let r = async {
            let path = owner::check_path(&a.file, self.upload_dir.as_deref())?;
            let publish_at = rfc3339("publish_at", a.publish_at).map_err(YouTubeError::Invalid)?;
            let privacy = if publish_at.is_some() {
                "private".to_owned()
            } else {
                a.privacy
                    .map(super::name)
                    .unwrap_or_else(|| "private".into())
            };
            let meta = UploadMeta {
                title: title_ok(&a.title)?,
                description: a
                    .description
                    .as_deref()
                    .map(|d| text_ok(d, 5000))
                    .transpose()?
                    .unwrap_or_default(),
                tags: a.tags.unwrap_or_default(),
                category_id: category_ok(a.category_id)?.unwrap_or_else(|| "22".into()),
                privacy,
                publish_at,
                made_for_kids: a.made_for_kids.unwrap_or(false),
                notify_subscribers: a.notify_subscribers.unwrap_or(false),
            };
            let tok = self.token().await?;
            owner::upload_video(&self.yt, &tok, &path, meta).await
        }
        .await;
        self.finish(r)
    }
}
