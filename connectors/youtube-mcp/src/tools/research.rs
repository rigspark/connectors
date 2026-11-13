//! Research tools: vidIQ-style analysis computed from public data.

use std::collections::HashSet;

use connector_kit::text::trim;
use connector_kit::time::format_rfc3339;
use rmcp::{
    ErrorData as McpError, handler::server::wrapper::Parameters, model::CallToolResult, schemars,
    tool, tool_router,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{YouTubeServer, clamp, non_empty, now};
use crate::analysis::{self, Outlier, SeoInput};
use crate::api::{Channel, CommentTarget, SearchParams, Video, YouTubeError};
use crate::earnings;
use crate::ids;
use crate::unofficial::{self, IdeaMode};

#[derive(
    Debug, Clone, Copy, Deserialize, Serialize, schemars::JsonSchema, PartialEq, Eq, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// Long-form only (over 3 minutes).
    Long,
    /// Shorts only (3 minutes or less).
    Short,
    #[default]
    Any,
}

impl Format {
    fn keep(self, v: &Video) -> bool {
        match self {
            Format::Any => true,
            Format::Short => analysis::is_short(v),
            Format::Long => !analysis::is_short(v),
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct KeywordArgs {
    /// The search phrase to measure, e.g. "sourdough for beginners".
    pub keyword: String,
    /// Two-letter region, default US.
    pub region: Option<String>,
    /// ISO 639-1 language of the audience, default en.
    pub language: Option<String>,
    /// Add autocomplete-based related keywords (unofficial source; default true).
    pub include_ideas: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OutlierArgs {
    /// Find outliers on this topic (search, ~102 units).
    pub keyword: Option<String>,
    /// Or: find this channel's own outliers vs its recent median (~3 units).
    pub channel: Option<String>,
    /// Or: find videos like this one from other channels (~103 units).
    pub seed_video: Option<String>,
    /// Only videos published in the last N days (keyword/seed mode).
    pub published_within_days: Option<u32>,
    /// Minimum breakout (views ÷ channel's typical views). Default 3 for keyword/seed, 1.5 for channel.
    pub min_breakout: Option<f64>,
    pub format: Option<Format>,
    pub min_subscribers: Option<u64>,
    pub max_subscribers: Option<u64>,
    pub region: Option<String>,
    pub language: Option<String>,
    /// 1–50, default 20.
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct HotArgs {
    /// Topic. Omit for YouTube's overall most-popular chart.
    pub query: Option<String>,
    /// Published within the last N days, default 7 (topic mode).
    pub within_days: Option<u32>,
    pub format: Option<Format>,
    pub region: Option<String>,
    pub language: Option<String>,
    pub min_subscribers: Option<u64>,
    pub max_subscribers: Option<u64>,
    /// 1–50, default 20.
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PerformanceArgs {
    /// UC… id, @handle or channel URL.
    pub channel: String,
    /// Recent uploads to analyse, 10–200, default 50.
    pub videos: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SimilarArgs {
    /// UC… id, @handle or channel URL.
    pub channel: String,
    /// 1–50, default 20.
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct InsightArgs {
    /// Analyse one video's comments…
    pub video: Option<String>,
    /// …or comments across a channel's videos.
    pub channel: Option<String>,
    /// A video idea to test against the comments.
    pub idea: Option<String>,
    /// Comments to read, 20–1000, default 300 (1 unit per 100).
    pub max_comments: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TitleArgs {
    /// 1–10 candidate titles.
    pub titles: Vec<String>,
    /// The search phrase the video targets.
    pub keyword: Option<String>,
    /// `long` (default) or `short`.
    pub format: Option<Format>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SeoArgs {
    /// Video id or URL.
    pub video: String,
    /// The search phrase the video targets.
    pub keyword: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EarningsArgs {
    pub monthly_views: u64,
    /// One of: Finance, Business, Technology, Education, Autos & Vehicles, Health, Howto & Style, Travel, Food, Lifestyle, Sports, News & Politics, Pets & Animals, Nonprofits & Activism, People & Blogs, Entertainment, Film & Animation, Gaming, Comedy, Religion, Music.
    pub category: String,
    /// Main audience country (two letters). Omit if mixed or unknown.
    pub country: Option<String>,
    /// Fraction of views that are Shorts, 0–1 (default 0).
    pub shorts_share: Option<f64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct VideoEarningsArgs {
    /// Video id or URL.
    pub video: String,
    /// Main audience country; defaults to the channel's country.
    pub country: Option<String>,
}

#[derive(Serialize)]
struct Compact<'a> {
    title: &'a str,
    channel: &'a str,
    views: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    channel_subscribers: Option<u64>,
    published_at: &'a str,
    url: &'a str,
}

fn region_or(r: Option<String>) -> Result<String, YouTubeError> {
    ids::region(non_empty(r).as_deref().unwrap_or("US")).map_err(YouTubeError::Invalid)
}

fn lang_or(l: Option<String>) -> Result<String, YouTubeError> {
    let l = non_empty(l).unwrap_or_else(|| "en".into()).to_lowercase();
    if (2..=3).contains(&l.len()) && l.chars().all(|c| c.is_ascii_lowercase()) {
        Ok(l)
    } else {
        Err(YouTubeError::Invalid(
            "`language` must be a code like en, hi or es".into(),
        ))
    }
}

fn subs_ok(c: Option<&Channel>, min: Option<u64>, max: Option<u64>) -> bool {
    if min.is_none() && max.is_none() {
        return true;
    }
    match c.and_then(|c| c.subscribers) {
        Some(s) => min.is_none_or(|m| s >= m) && max.is_none_or(|m| s <= m),
        None => false,
    }
}

impl YouTubeServer {
    async fn resolve(&self, input: &str) -> Result<Channel, YouTubeError> {
        let r = ids::channel_ref(input).map_err(YouTubeError::Invalid)?;
        let c = self.yt.channel(&r).await?;
        self.remember_channel(&c);
        Ok(c)
    }

    /// Search → videos with stats → their channels.
    async fn search_videos(
        &self,
        p: SearchParams,
    ) -> Result<(Vec<Video>, Vec<Channel>), YouTubeError> {
        let hits = self.yt.search(p).await?;
        let ids: Vec<String> = hits
            .items
            .iter()
            .filter(|h| h.kind == "video")
            .map(|h| h.id.clone())
            .collect();
        let vids = self.yt.videos(&ids).await?;
        self.remember_videos(&vids);
        let mut chans: Vec<String> = vids.iter().map(|v| v.channel_id.clone()).collect();
        chans.sort();
        chans.dedup();
        let channels = self.yt.channels(&chans).await?;
        Ok((vids, channels))
    }

    async fn keyword_research_inner(&self, a: KeywordArgs) -> Result<Value, YouTubeError> {
        let keyword = a.keyword.trim().to_owned();
        if keyword.is_empty() {
            return Err(YouTubeError::Invalid("`keyword` is empty".into()));
        }
        let region = region_or(a.region)?;
        let language = lang_or(a.language)?;
        let (vids, channels) = self
            .search_videos(SearchParams {
                query: keyword.clone(),
                kind: Some("video".into()),
                max_results: 25,
                region: Some(region.clone()),
                language: Some(language.clone()),
                ..Default::default()
            })
            .await?;
        let scores = analysis::competition(&keyword, &vids, &channels, now());
        let texts: Vec<String> = vids
            .iter()
            .map(|v| format!("{} {}", v.title, v.tags.join(" ")))
            .collect();
        let terms: Vec<String> = analysis::top_terms(&texts, 15)
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        let mut notes = vec![
            "Scores are computed from the top 25 results for this keyword; they are not YouTube search volumes (YouTube doesn't publish those).".to_owned(),
            "Opportunity is high when people watch this topic but results come from small or off-target channels.".into(),
        ];
        let ideas = if a.include_ideas.unwrap_or(true) && self.unofficial {
            match unofficial::keyword_ideas(
                &self.yt,
                &keyword,
                IdeaMode::Suggestions,
                &language,
                &region,
                15,
            )
            .await
            {
                Ok(i) => Some(i),
                Err(e) => {
                    notes.push(format!("Related keywords unavailable: {e}"));
                    None
                }
            }
        } else {
            None
        };
        let subs = |id: &str| {
            channels
                .iter()
                .find(|c| c.id == id)
                .and_then(|c| c.subscribers)
        };
        let top: Vec<Compact> = vids
            .iter()
            .take(10)
            .map(|v| Compact {
                title: &v.title,
                channel: &v.channel_title,
                views: v.views,
                channel_subscribers: subs(&v.channel_id),
                published_at: &v.published_at,
                url: &v.url,
            })
            .collect();
        Ok(json!({
            "keyword": keyword,
            "region": region,
            "language": language,
            "scores": scores,
            "related_keywords": ideas,
            "terms_in_top_results": terms,
            "top_results": top,
            "notes": notes,
        }))
    }

    async fn outliers_inner(&self, a: OutlierArgs) -> Result<Value, YouTubeError> {
        let format = a.format.unwrap_or_default();
        let limit = clamp(a.limit, 20, 50) as usize;
        let t = now();
        let modes = [
            a.keyword.is_some(),
            a.channel.is_some(),
            a.seed_video.is_some(),
        ]
        .iter()
        .filter(|x| **x)
        .count();
        if modes != 1 {
            return Err(YouTubeError::Invalid(
                "pass exactly one of `keyword`, `channel` or `seed_video`".into(),
            ));
        }
        if let Some(c) = &a.channel {
            let ch = self.resolve(c).await?;
            let vids: Vec<Video> = self
                .yt
                .uploads(&ch, 50)
                .await?
                .into_iter()
                .filter(|v| format.keep(v))
                .collect();
            self.remember_videos(&vids);
            let views: Vec<f64> = vids.iter().map(|v| v.views.unwrap_or(0) as f64).collect();
            let base = analysis::median(&views).unwrap_or(0.0);
            let min = a.min_breakout.unwrap_or(1.5);
            let mut out: Vec<Outlier> = vids
                .into_iter()
                .map(|v| analysis::outlier(v, base, "recent_median", ch.subscribers, t))
                .filter(|o| o.breakout >= min)
                .collect();
            out.sort_by(|x, y| y.breakout.total_cmp(&x.breakout));
            out.truncate(limit);
            return Ok(json!({
                "mode": "channel",
                "channel": ch.title,
                "baseline_views": base.round(),
                "outliers": out,
                "notes": ["Baseline is the median of the channel's last 50 uploads (same format). Older videos have had longer to collect views."],
            }));
        }
        let (keyword, exclude_channel) = match (&a.keyword, &a.seed_video) {
            (Some(k), _) => (k.trim().to_owned(), None),
            (_, Some(s)) => {
                let id = ids::video_id(s).map_err(YouTubeError::Invalid)?;
                let seed = self
                    .yt
                    .videos(std::slice::from_ref(&id))
                    .await?
                    .into_iter()
                    .next()
                    .ok_or_else(|| YouTubeError::NotFound(format!("video {id}")))?;
                let toks: Vec<String> = analysis::tokens(&seed.title).into_iter().take(5).collect();
                if toks.is_empty() {
                    return Err(YouTubeError::Invalid(
                        "the seed video's title has no searchable words".into(),
                    ));
                }
                (toks.join(" "), Some(seed.channel_id))
            }
            _ => unreachable!(),
        };
        if keyword.is_empty() {
            return Err(YouTubeError::Invalid("`keyword` is empty".into()));
        }
        let after = a
            .published_within_days
            .map(|d| format_rfc3339(t - d.clamp(1, 3650) as i64 * 86_400));
        let (vids, channels) = self
            .search_videos(SearchParams {
                query: keyword.clone(),
                kind: Some("video".into()),
                max_results: 50,
                published_after: after,
                duration: (format == Format::Short).then(|| "short".into()),
                region: Some(region_or(a.region)?),
                language: Some(lang_or(a.language)?),
                ..Default::default()
            })
            .await?;
        let min = a.min_breakout.unwrap_or(3.0);
        let mut out: Vec<Outlier> = vids
            .into_iter()
            .filter(|v| format.keep(v) && Some(&v.channel_id) != exclude_channel.as_ref())
            .filter_map(|v| {
                let ch = channels.iter().find(|c| c.id == v.channel_id);
                if !subs_ok(ch, a.min_subscribers, a.max_subscribers) {
                    return None;
                }
                let base = ch.and_then(analysis::channel_avg_views)?;
                Some(analysis::outlier(
                    v,
                    base,
                    "lifetime_average",
                    ch.and_then(|c| c.subscribers),
                    t,
                ))
            })
            .filter(|o| o.breakout >= min)
            .collect();
        out.sort_by(|x, y| y.breakout.total_cmp(&x.breakout));
        out.truncate(limit);
        Ok(json!({
            "mode": if a.seed_video.is_some() { "seed_video" } else { "keyword" },
            "keyword": keyword,
            "outliers": out,
            "notes": [
                "Breakout = video views ÷ the channel's lifetime average views per video. 3+ means the video did far better than that channel usually does.",
                "From the top 50 search results; lower min_breakout or widen published_within_days for more.",
            ],
        }))
    }

    async fn hot_inner(&self, a: HotArgs) -> Result<Value, YouTubeError> {
        let format = a.format.unwrap_or_default();
        let limit = clamp(a.limit, 20, 50) as usize;
        let region = region_or(a.region)?;
        let t = now();
        let (vids, channels) = match non_empty(a.query) {
            None => {
                let p = self.yt.trending(&region, None, 50, None).await?;
                self.remember_videos(&p.items);
                let need = a.min_subscribers.is_some() || a.max_subscribers.is_some();
                let mut ch: Vec<String> = if need {
                    p.items.iter().map(|v| v.channel_id.clone()).collect()
                } else {
                    vec![]
                };
                ch.sort();
                ch.dedup();
                (p.items, self.yt.channels(&ch).await?)
            }
            Some(q) => {
                let days = a.within_days.unwrap_or(7).clamp(1, 90) as i64;
                self.search_videos(SearchParams {
                    query: q,
                    kind: Some("video".into()),
                    max_results: 50,
                    order: Some("viewCount".into()),
                    published_after: Some(format_rfc3339(t - days * 86_400)),
                    duration: (format == Format::Short).then(|| "short".into()),
                    region: Some(region.clone()),
                    language: Some(lang_or(a.language)?),
                    ..Default::default()
                })
                .await?
            }
        };
        #[derive(Serialize)]
        struct Hot {
            #[serde(flatten)]
            video: Video,
            vph: f64,
            hours_since_publish: f64,
            #[serde(skip_serializing_if = "Option::is_none")]
            engagement: Option<f64>,
            #[serde(skip_serializing_if = "Option::is_none")]
            channel_subscribers: Option<u64>,
        }
        let mut out: Vec<Hot> = vids
            .into_iter()
            .filter(|v| format.keep(v))
            .filter(|v| {
                subs_ok(
                    channels.iter().find(|c| c.id == v.channel_id),
                    a.min_subscribers,
                    a.max_subscribers,
                )
            })
            .map(|v| Hot {
                vph: (analysis::vph(&v, t) * 10.0).round() / 10.0,
                hours_since_publish: analysis::age_hours(&v, t).round(),
                engagement: analysis::engagement(&v).map(|e| (e * 1000.0).round() / 1000.0),
                channel_subscribers: channels
                    .iter()
                    .find(|c| c.id == v.channel_id)
                    .and_then(|c| c.subscribers),
                video: v,
            })
            .collect();
        out.sort_by(|x, y| y.vph.total_cmp(&x.vph));
        out.truncate(limit);
        Ok(json!({
            "region": region,
            "videos": out,
            "notes": ["vph = lifetime views ÷ hours since publish. track_channel + video_history give current (not lifetime) velocity."],
        }))
    }

    async fn similar_inner(&self, a: SimilarArgs) -> Result<Value, YouTubeError> {
        let limit = clamp(a.limit, 20, 50) as usize;
        let ch = self.resolve(&a.channel).await?;
        let vids = self.yt.uploads(&ch, 25).await?;
        self.remember_videos(&vids);
        let texts: Vec<String> = vids
            .iter()
            .map(|v| format!("{} {}", v.title, v.tags.join(" ")))
            .collect();
        let own = analysis::tokens(&ch.title);
        let terms: Vec<String> = analysis::top_terms(&texts, 12)
            .into_iter()
            .map(|(t, _)| t)
            .filter(|t| !own.iter().any(|o| t.contains(o.as_str())))
            .collect();
        if terms.is_empty() {
            return Err(YouTubeError::Invalid(
                "not enough recent uploads to tell what this channel is about".into(),
            ));
        }
        let query = terms.iter().take(3).cloned().collect::<Vec<_>>().join(" ");
        let hits = self
            .yt
            .search(SearchParams {
                query: query.clone(),
                kind: Some("channel".into()),
                max_results: 50,
                ..Default::default()
            })
            .await?;
        let ids: Vec<String> = hits
            .items
            .iter()
            .map(|h| h.id.clone())
            .filter(|id| *id != ch.id)
            .collect();
        let cands = self.yt.channels(&ids).await?;
        let term_set: HashSet<String> = terms
            .iter()
            .flat_map(|t| t.split(' ').map(str::to_owned))
            .collect();
        let ref_subs = (ch.subscribers.unwrap_or(1000) as f64).max(1.0).log10();
        #[derive(Serialize)]
        struct Similar {
            #[serde(flatten)]
            channel: Channel,
            similarity: f64,
            topic_overlap: f64,
            size_ratio: Option<f64>,
        }
        let mut out: Vec<Similar> = cands
            .into_iter()
            .map(|c| {
                let toks: HashSet<String> =
                    analysis::tokens(&format!("{} {}", c.title, c.description))
                        .into_iter()
                        .collect();
                let overlap = term_set.iter().filter(|t| toks.contains(*t)).count() as f64
                    / term_set.len().max(1) as f64;
                let size = c
                    .subscribers
                    .map(|s| 1.0 - (((s as f64).max(1.0).log10() - ref_subs).abs() / 3.0).min(1.0));
                let sim = 0.65 * overlap + 0.35 * size.unwrap_or(0.3);
                Similar {
                    size_ratio: match (c.subscribers, ch.subscribers) {
                        (Some(a), Some(b)) if b > 0 => {
                            Some(((a as f64 / b as f64) * 100.0).round() / 100.0)
                        }
                        _ => None,
                    },
                    similarity: (sim * 1000.0).round() / 1000.0,
                    topic_overlap: (overlap * 1000.0).round() / 1000.0,
                    channel: c,
                }
            })
            .collect();
        out.sort_by(|x, y| y.similarity.total_cmp(&x.similarity));
        out.truncate(limit);
        Ok(json!({
            "channel": ch.title,
            "topic_terms": terms,
            "searched_for": query,
            "similar": out,
            "notes": ["Found by searching channels for this channel's most frequent title/tag terms, then ranking by topic overlap and subscriber band. Not audience-overlap data."],
        }))
    }

    async fn insights_inner(&self, a: InsightArgs) -> Result<Value, YouTubeError> {
        let max = a.max_comments.unwrap_or(300).clamp(20, 1000);
        let (target_id, is_video) = match (&a.video, &a.channel) {
            (Some(v), None) => (ids::video_id(v).map_err(YouTubeError::Invalid)?, true),
            (None, Some(c)) => (self.resolve(c).await?.id, false),
            _ => {
                return Err(YouTubeError::Invalid(
                    "pass exactly one of `video` or `channel`".into(),
                ));
            }
        };
        let mut comments = Vec::new();
        let mut token = None;
        while (comments.len() as u32) < max {
            let target = if is_video {
                CommentTarget::Video(&target_id)
            } else {
                CommentTarget::Channel(&target_id)
            };
            let page = self
                .yt
                .comments(target, 100, Some("relevance".into()), None, token)
                .await?;
            comments.extend(page.items);
            token = page.next_page_token;
            if token.is_none() {
                break;
            }
        }
        comments.truncate(max as usize);
        let ins = analysis::insights(&comments, a.idea.as_deref());
        Ok(serde_json::to_value(ins).unwrap_or(Value::Null))
    }

    async fn seo_inner(&self, a: SeoArgs) -> Result<Value, YouTubeError> {
        let id = ids::video_id(&a.video).map_err(YouTubeError::Invalid)?;
        let raw = self
            .yt
            .videos_raw(
                std::slice::from_ref(&id),
                "snippet,contentDetails,status,statistics",
            )
            .await?;
        let v = raw
            .first()
            .ok_or_else(|| YouTubeError::NotFound(format!("video {id}")))?;
        let sn = &v["snippet"];
        let tags: Vec<String> = sn["tags"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let desc = sn["description"].as_str().unwrap_or_default();
        let title = sn["title"].as_str().unwrap_or_default();
        let dur = v["contentDetails"]["duration"]
            .as_str()
            .and_then(connector_kit::text::iso8601_duration_secs);
        let seo = analysis::seo_audit(&SeoInput {
            title,
            description: desc,
            tags: &tags,
            duration_secs: dur,
            has_captions: v["contentDetails"]["caption"].as_str().map(|c| c == "true"),
            default_language: sn["defaultLanguage"]
                .as_str()
                .or(sn["defaultAudioLanguage"].as_str()),
            made_for_kids: v["status"]["madeForKids"].as_bool(),
            keyword: a.keyword.as_deref().filter(|k| !k.trim().is_empty()),
        });
        self.remember_videos(&[crate::api::video_from(v)]);
        Ok(json!({
            "video_id": id,
            "title": title,
            "tags": tags,
            "description_preview": trim(desc, 400),
            "seo": seo,
        }))
    }

    async fn video_earnings_inner(&self, a: VideoEarningsArgs) -> Result<Value, YouTubeError> {
        let id = ids::video_id(&a.video).map_err(YouTubeError::Invalid)?;
        let v = self
            .yt
            .videos(std::slice::from_ref(&id))
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| YouTubeError::NotFound(format!("video {id}")))?;
        self.remember_videos(std::slice::from_ref(&v));
        let country = match non_empty(a.country) {
            Some(c) => Some(ids::region(&c).map_err(YouTubeError::Invalid)?),
            None => self
                .yt
                .channels(std::slice::from_ref(&v.channel_id))
                .await?
                .first()
                .and_then(|c| c.country.clone()),
        };
        let cat = earnings::category_for_id(v.category_id.as_deref().unwrap_or(""));
        let short = analysis::is_short(&v);
        let months = (analysis::age_hours(&v, now()) / (24.0 * 30.4)).max(1.0);
        let views = v.views.unwrap_or(0);
        let lifetime = earnings::estimate(
            views,
            cat,
            country.as_deref(),
            if short { 1.0 } else { 0.0 },
        );
        let monthly = earnings::estimate(
            (views as f64 / months).round() as u64,
            cat,
            country.as_deref(),
            if short { 1.0 } else { 0.0 },
        );
        Ok(json!({
            "video_id": id,
            "title": v.title,
            "views": views,
            "format": if short { "short" } else { "long" },
            "category": cat,
            "lifetime_usd": { "low": lifetime.monthly_usd_low, "mid": lifetime.monthly_usd_mid, "high": lifetime.monthly_usd_high },
            "average_month_usd": { "low": monthly.monthly_usd_low, "mid": monthly.monthly_usd_mid, "high": monthly.monthly_usd_high },
            "rpm_range": [lifetime.rpm_low, lifetime.rpm_high],
            "audience": lifetime.audience,
            "notes": lifetime.notes,
        }))
    }
}

#[tool_router(router = research_router, vis = "pub(crate)")]
impl YouTubeServer {
    #[tool(
        name = "keyword_research",
        description = "Measure a YouTube search keyword: competition, demand and opportunity scores (0–100) from its top 25 results, related keywords, and the terms top videos use. ~102 quota units.",
        annotations(
            title = "Keyword research",
            read_only_hint = true,
            open_world_hint = true
        )
    )]
    async fn keyword_research(
        &self,
        Parameters(a): Parameters<KeywordArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.finish(self.keyword_research_inner(a).await)
    }

    #[tool(
        name = "outliers",
        description = "Find breakout videos that beat their channel's typical views: by topic (`keyword`), like a given video (`seed_video`), or within one channel (`channel`). Keyword/seed ~102 units, channel ~3.",
        annotations(
            title = "Outlier videos",
            read_only_hint = true,
            open_world_hint = true
        )
    )]
    async fn outliers(
        &self,
        Parameters(a): Parameters<OutlierArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.finish(self.outliers_inner(a).await)
    }

    #[tool(
        name = "hot_videos",
        description = "Fastest-growing videos by views per hour: on a topic from the last N days (~102 units), or YouTube's popular chart for a region (1 unit). Filter by format and channel size.",
        annotations(title = "Hot videos", read_only_hint = true, open_world_hint = true)
    )]
    async fn hot_videos(
        &self,
        Parameters(a): Parameters<HotArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.finish(self.hot_inner(a).await)
    }

    #[tool(
        name = "channel_performance",
        description = "How a channel's videos perform: views by video age (the typical view curve), median views and VPH, Shorts vs long-form, upload cadence, best weekdays and hours to publish, and top videos. 3–6 units.",
        annotations(
            title = "Channel performance",
            read_only_hint = true,
            open_world_hint = true
        )
    )]
    async fn channel_performance(
        &self,
        Parameters(a): Parameters<PerformanceArgs>,
    ) -> Result<CallToolResult, McpError> {
        let r = async {
            let ch = self.resolve(&a.channel).await?;
            let vids = self
                .yt
                .uploads(&ch, a.videos.unwrap_or(50).clamp(10, 200))
                .await?;
            self.remember_videos(&vids);
            let perf = analysis::performance(&vids, now());
            Ok::<_, YouTubeError>(json!({ "channel": ch, "performance": perf }))
        }
        .await;
        self.finish(r)
    }

    #[tool(
        name = "similar_channels",
        description = "Competitor channels for a channel: matched on the topics in its recent titles and tags, ranked by topic overlap and similar subscriber size. ~104 units.",
        annotations(
            title = "Similar channels",
            read_only_hint = true,
            open_world_hint = true
        )
    )]
    async fn similar_channels(
        &self,
        Parameters(a): Parameters<SimilarArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.finish(self.similar_inner(a).await)
    }

    #[tool(
        name = "comment_insights",
        description = "What an audience asks for in comments: content requests, questions, complaints, praise, with themes and top examples, for a video or across a channel. Pass `idea` to test a video idea against them. 1 unit per 100 comments.",
        annotations(
            title = "Comment insights",
            read_only_hint = true,
            open_world_hint = true
        )
    )]
    async fn comment_insights(
        &self,
        Parameters(a): Parameters<InsightArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.finish(self.insights_inner(a).await)
    }

    #[tool(
        name = "title_score",
        description = "Score 1–10 candidate titles (0–100) on a packaging checklist (length, keyword placement, numbers, hook words, caps, clutter) and rank them, with fixes. Local, no quota.",
        annotations(title = "Score titles", read_only_hint = true, open_world_hint = false)
    )]
    async fn title_score(
        &self,
        Parameters(a): Parameters<TitleArgs>,
    ) -> Result<CallToolResult, McpError> {
        if a.titles.is_empty() || a.titles.len() > 10 {
            return self.finish::<()>(Err(YouTubeError::Invalid("pass 1 to 10 titles".into())));
        }
        let short = a.format == Some(Format::Short);
        let mut scored: Vec<analysis::TitleScore> = a
            .titles
            .iter()
            .map(|t| analysis::title_score(t, a.keyword.as_deref(), short))
            .collect();
        scored.sort_by_key(|c| std::cmp::Reverse(c.score));
        self.finish(Ok(json!({
            "ranked": scored,
            "notes": ["A transparent checklist, not a click-through prediction. Test the top two in YouTube's Test & Compare."],
        })))
    }

    #[tool(
        name = "seo_audit",
        description = "Audit a video's metadata: title, description length and keyword, chapters, hashtags, tags, links, captions, language. Returns a score and the fixes. 1 unit.",
        annotations(title = "SEO audit", read_only_hint = true, open_world_hint = true)
    )]
    async fn seo_audit(
        &self,
        Parameters(a): Parameters<SeoArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.finish(self.seo_inner(a).await)
    }

    #[tool(
        name = "earnings_calculate",
        description = "Estimate monthly YouTube ad earnings (low/mid/high USD) for a number of monthly views by category, audience country and Shorts share. A calculator; no quota.",
        annotations(
            title = "Earnings calculator",
            read_only_hint = true,
            open_world_hint = false
        )
    )]
    async fn earnings_calculate(
        &self,
        Parameters(a): Parameters<EarningsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let country = match non_empty(a.country) {
            Some(c) => match ids::region(&c) {
                Ok(c) => Some(c),
                Err(e) => return self.finish::<()>(Err(YouTubeError::Invalid(e))),
            },
            None => None,
        };
        let e = earnings::estimate(
            a.monthly_views,
            &a.category,
            country.as_deref(),
            a.shorts_share.unwrap_or(0.0),
        );
        self.finish(Ok(
            json!({ "estimate": e, "categories": earnings::categories() }),
        ))
    }

    #[tool(
        name = "video_earnings",
        description = "Estimate what a real video has earned (lifetime and per average month) from its views, category, format and audience country. 1–2 units.",
        annotations(
            title = "Video earnings",
            read_only_hint = true,
            open_world_hint = true
        )
    )]
    async fn video_earnings(
        &self,
        Parameters(a): Parameters<VideoEarningsArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.finish(self.video_earnings_inner(a).await)
    }
}
