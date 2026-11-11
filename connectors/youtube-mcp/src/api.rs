//! YouTube Data API v3 client, returning compact model-facing types.
//!
//! The API key travels in the `x-goog-api-key` header, never the URL. Quota costs (units per
//! call, out of 10,000/day by default) are noted on each method.

use connector_kit::text::{iso8601_duration_secs, trim};
use connector_kit::{HttpClient, HttpError, Secret};
use serde::Serialize;
use serde_json::Value;

use crate::config::Endpoints;
use crate::ids::ChannelRef;

pub const UA: &str = concat!("rigspark-youtube-mcp/", env!("CARGO_PKG_VERSION"));

const DESC: usize = 600;
const COMMENT: usize = 1_000;

#[derive(Debug, thiserror::Error)]
pub enum YouTubeError {
    #[error(
        "YouTube API quota for today is used up (it resets at midnight Pacific time). Try again later or use fewer `search` calls (100 units each)."
    )]
    QuotaExceeded,
    #[error(
        "YouTube rejected the API key. Check YOUTUBE_API_KEY and that the YouTube Data API v3 is enabled for its Google Cloud project."
    )]
    BadKey,
    #[error("YouTube is rate limiting requests. Wait a moment and try again.")]
    RateLimited,
    #[error("comments are turned off for this video")]
    CommentsDisabled,
    #[error(
        "the signed-in Google account lacks permission for this ({0}). Re-run `youtube-mcp auth --write` for write tools, and make sure you own the channel or video."
    )]
    Forbidden(String),
    #[error(
        "channel-owner tools aren't set up: run `youtube-mcp auth` and pass YOUTUBE_OAUTH_CLIENT_ID, YOUTUBE_OAUTH_CLIENT_SECRET and YOUTUBE_OAUTH_REFRESH_TOKEN to the connector"
    )]
    NoOAuth,
    #[error("{0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("YouTube refused the request ({status}, {reason}): {message}")]
    Api {
        status: u16,
        reason: String,
        message: String,
    },
    #[error("{0}")]
    Http(String),
}

impl From<HttpError> for YouTubeError {
    fn from(e: HttpError) -> Self {
        match e {
            HttpError::Status { status, body } => classify(status, &body),
            other => YouTubeError::Http(other.to_string()),
        }
    }
}

/// Maps a Google API error body to a message the model can act on.
fn classify(status: u16, body: &str) -> YouTubeError {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let err = &v["error"];
    let message = err["message"].as_str().unwrap_or(body).to_owned();
    if let Some(e) = v["error"].as_str() {
        // OAuth token endpoint: {"error":"invalid_grant","error_description":"..."}
        return match e {
            "invalid_grant" => YouTubeError::Forbidden(
                "the refresh token was revoked or expired; run `youtube-mcp auth` again".into(),
            ),
            _ => YouTubeError::Api {
                status,
                reason: e.to_owned(),
                message: trim(v["error_description"].as_str().unwrap_or(e), 300),
            },
        };
    }
    let reason = err["errors"][0]["reason"]
        .as_str()
        .or_else(|| err["details"][0]["reason"].as_str())
        .or_else(|| err["status"].as_str())
        .unwrap_or("unknown")
        .to_owned();
    match reason.as_str() {
        "quotaExceeded" | "dailyLimitExceeded" => YouTubeError::QuotaExceeded,
        "keyInvalid" | "API_KEY_INVALID" | "accessNotConfigured" | "SERVICE_DISABLED" => {
            YouTubeError::BadKey
        }
        "rateLimitExceeded" | "userRateLimitExceeded" => YouTubeError::RateLimited,
        "commentsDisabled" => YouTubeError::CommentsDisabled,
        "insufficientPermissions"
        | "forbidden"
        | "ACCESS_TOKEN_SCOPE_INSUFFICIENT"
        | "PERMISSION_DENIED" => YouTubeError::Forbidden(trim(&message, 200)),
        "invalid_grant" => YouTubeError::Forbidden(
            "the refresh token was revoked or expired; run `youtube-mcp auth` again".into(),
        ),
        r if r.ends_with("NotFound") => YouTubeError::NotFound(message),
        _ if status == 404 => YouTubeError::NotFound(message),
        _ if message.contains("API key not valid") => YouTubeError::BadKey,
        _ => YouTubeError::Api {
            status,
            reason,
            message: trim(&message, 300),
        },
    }
}

// ---------- model-facing types ----------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_page_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_results: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SearchHit {
    pub kind: String,
    pub id: String,
    pub title: String,
    pub channel_id: String,
    pub channel_title: String,
    pub published_at: String,
    pub description: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub channel_id: String,
    pub channel_title: String,
    pub published_at: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub views: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub likes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comments: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category_id: Option<String>,
    /// `none`, `live` or `upcoming`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_captions: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Channel {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    pub description: String,
    pub published_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// `None` when the channel hides its subscriber count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscribers: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub views: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub videos: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uploads_playlist_id: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Playlist {
    pub id: String,
    pub title: String,
    pub channel_title: String,
    pub description: String,
    pub published_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_count: Option<u64>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PlaylistItem {
    pub position: u64,
    pub video_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_published_at: Option<String>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Comment {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_id: Option<String>,
    pub author: String,
    pub text: String,
    pub likes: u64,
    pub published_at: String,
    pub replies: u64,
}

// ---------- helpers ----------

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_owned()
}
fn opt_s(v: &Value) -> Option<String> {
    v.as_str().filter(|x| !x.is_empty()).map(str::to_owned)
}
/// The API returns counts as strings.
fn count(v: &Value) -> Option<u64> {
    v.as_str()
        .and_then(|x| x.parse().ok())
        .or_else(|| v.as_u64())
}
fn items(v: &Value) -> &[Value] {
    v["items"].as_array().map(Vec::as_slice).unwrap_or_default()
}
fn page<T>(v: &Value, items: Vec<T>) -> Page<T> {
    Page {
        items,
        next_page_token: opt_s(&v["nextPageToken"]),
        total_results: v["pageInfo"]["totalResults"].as_u64(),
    }
}
fn best_thumb(t: &Value) -> Option<String> {
    ["maxres", "standard", "high", "medium", "default"]
        .iter()
        .find_map(|k| opt_s(&t[k]["url"]))
}

/// Plain text from the HTML the comments API returns in `textDisplay`.
fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    let mut tag = String::new();
    for c in s.chars() {
        match (in_tag, c) {
            (false, '<') => {
                in_tag = true;
                tag.clear();
            }
            (true, '>') => {
                in_tag = false;
                if tag.eq_ignore_ascii_case("br") || tag.eq_ignore_ascii_case("br/") {
                    out.push('\n');
                }
            }
            (true, c) => tag.push(c),
            (false, c) => out.push(c),
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

pub fn video_from(v: &Value) -> Video {
    let sn = &v["snippet"];
    let st = &v["statistics"];
    let cd = &v["contentDetails"];
    let id = v["id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| s(&v["id"]["videoId"]));
    Video {
        url: format!("https://www.youtube.com/watch?v={id}"),
        id,
        title: s(&sn["title"]),
        channel_id: s(&sn["channelId"]),
        channel_title: s(&sn["channelTitle"]),
        published_at: s(&sn["publishedAt"]),
        description: trim(sn["description"].as_str().unwrap_or_default(), DESC),
        duration_secs: cd["duration"].as_str().and_then(iso8601_duration_secs),
        views: count(&st["viewCount"]),
        likes: count(&st["likeCount"]),
        comments: count(&st["commentCount"]),
        tags: sn["tags"]
            .as_array()
            .map(|a| {
                a.iter()
                    .take(15)
                    .filter_map(|t| t.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        category_id: opt_s(&sn["categoryId"]),
        live: opt_s(&sn["liveBroadcastContent"]),
        has_captions: cd["caption"].as_str().map(|c| c == "true"),
        thumbnail: best_thumb(&sn["thumbnails"]),
    }
}

pub fn channel_from(v: &Value) -> Channel {
    let sn = &v["snippet"];
    let st = &v["statistics"];
    let id = s(&v["id"]);
    let hidden = st["hiddenSubscriberCount"].as_bool().unwrap_or(false);
    Channel {
        url: format!("https://www.youtube.com/channel/{id}"),
        id,
        title: s(&sn["title"]),
        handle: opt_s(&sn["customUrl"]),
        description: trim(sn["description"].as_str().unwrap_or_default(), DESC),
        published_at: s(&sn["publishedAt"]),
        country: opt_s(&sn["country"]),
        subscribers: if hidden {
            None
        } else {
            count(&st["subscriberCount"])
        },
        views: count(&st["viewCount"]),
        videos: count(&st["videoCount"]),
        uploads_playlist_id: opt_s(&v["contentDetails"]["relatedPlaylists"]["uploads"]),
    }
}

// ---------- client ----------

/// Whose comments to list.
#[derive(Debug, Clone, Copy)]
pub enum CommentTarget<'a> {
    Video(&'a str),
    /// Every thread on any of the channel's videos.
    Channel(&'a str),
}

#[derive(Clone)]
pub struct YouTube {
    http: HttpClient,
    ep: Endpoints,
}

/// Search filters beyond the query text.
#[derive(Debug, Default, Clone)]
pub struct SearchParams {
    pub query: String,
    /// `video`, `channel` or `playlist`; `None` searches all three.
    pub kind: Option<String>,
    pub max_results: u32,
    /// `relevance`, `date`, `viewCount`, `rating`, `title`, `videoCount`.
    pub order: Option<String>,
    pub channel_id: Option<String>,
    pub published_after: Option<String>,
    pub published_before: Option<String>,
    pub region: Option<String>,
    pub language: Option<String>,
    /// `short` (<4 min), `medium` (4–20 min), `long` (>20 min). Video searches only.
    pub duration: Option<String>,
    pub page_token: Option<String>,
}

impl YouTube {
    pub fn new(key: Secret, ep: Endpoints) -> Result<Self, YouTubeError> {
        let http = HttpClient::new(ep.data.clone(), UA)
            .map_err(|e| YouTubeError::Http(e.to_string()))?
            .with_auth_header("x-goog-api-key", key);
        Ok(Self { http, ep })
    }

    pub fn http(&self) -> &HttpClient {
        &self.http
    }

    pub fn endpoints(&self) -> &Endpoints {
        &self.ep
    }

    pub(crate) async fn get(
        &self,
        path: &str,
        q: Vec<(&str, String)>,
    ) -> Result<Value, YouTubeError> {
        Ok(self.http.get_json(path, &q).await?)
    }

    /// `search.list` — **100 units**.
    pub async fn search(&self, p: SearchParams) -> Result<Page<SearchHit>, YouTubeError> {
        let mut q = vec![
            ("part", "snippet".to_owned()),
            ("q", p.query),
            ("maxResults", p.max_results.to_string()),
            ("safeSearch", "moderate".to_owned()),
        ];
        let video_only = p.duration.is_some();
        let kind = p.kind.or(video_only.then(|| "video".to_owned()));
        let opts = [
            ("type", kind),
            ("order", p.order),
            ("channelId", p.channel_id),
            ("publishedAfter", p.published_after),
            ("publishedBefore", p.published_before),
            ("regionCode", p.region),
            ("relevanceLanguage", p.language),
            ("videoDuration", p.duration),
            ("pageToken", p.page_token),
        ];
        q.extend(opts.into_iter().filter_map(|(k, v)| v.map(|v| (k, v))));
        let v = self.get("/search", q).await?;
        let hits = items(&v)
            .iter()
            .filter_map(|it| {
                let idv = &it["id"];
                let (kind, id, url) = match idv["kind"].as_str()? {
                    "youtube#video" => {
                        let id = s(&idv["videoId"]);
                        (
                            "video",
                            id.clone(),
                            format!("https://www.youtube.com/watch?v={id}"),
                        )
                    }
                    "youtube#channel" => {
                        let id = s(&idv["channelId"]);
                        (
                            "channel",
                            id.clone(),
                            format!("https://www.youtube.com/channel/{id}"),
                        )
                    }
                    "youtube#playlist" => {
                        let id = s(&idv["playlistId"]);
                        (
                            "playlist",
                            id.clone(),
                            format!("https://www.youtube.com/playlist?list={id}"),
                        )
                    }
                    _ => return None,
                };
                let sn = &it["snippet"];
                Some(SearchHit {
                    kind: kind.into(),
                    id,
                    url,
                    title: s(&sn["title"]),
                    channel_id: s(&sn["channelId"]),
                    channel_title: s(&sn["channelTitle"]),
                    published_at: s(&sn["publishedAt"]),
                    description: trim(sn["description"].as_str().unwrap_or_default(), DESC),
                })
            })
            .collect();
        Ok(page(&v, hits))
    }

    /// `videos.list` for up to 50 ids — **1 unit**. Missing ids are simply absent.
    pub async fn videos(&self, ids: &[String]) -> Result<Vec<Video>, YouTubeError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let v = self
            .get(
                "/videos",
                vec![
                    ("part", "snippet,contentDetails,statistics".into()),
                    ("id", ids.join(",")),
                    ("maxResults", "50".into()),
                ],
            )
            .await?;
        Ok(items(&v).iter().map(video_from).collect())
    }

    /// `channels.list` — **1 unit**.
    pub async fn channel(&self, r: &ChannelRef) -> Result<Channel, YouTubeError> {
        let (k, val, label) = match r {
            ChannelRef::Id(id) => ("id", id.clone(), id.clone()),
            ChannelRef::Handle(h) => ("forHandle", format!("@{h}"), format!("@{h}")),
        };
        let v = self
            .get(
                "/channels",
                vec![
                    ("part", "snippet,statistics,contentDetails".into()),
                    (k, val),
                ],
            )
            .await?;
        items(&v)
            .first()
            .map(channel_from)
            .ok_or_else(|| YouTubeError::NotFound(format!("no channel {label}")))
    }

    /// `playlistItems.list` — **1 unit**.
    pub async fn playlist_items(
        &self,
        playlist_id: &str,
        max_results: u32,
        page_token: Option<String>,
    ) -> Result<Page<PlaylistItem>, YouTubeError> {
        let mut q = vec![
            ("part", "snippet,contentDetails".into()),
            ("playlistId", playlist_id.to_owned()),
            ("maxResults", max_results.to_string()),
        ];
        if let Some(t) = page_token {
            q.push(("pageToken", t));
        }
        let v = self.get("/playlistItems", q).await?;
        let list = items(&v)
            .iter()
            .map(|it| {
                let sn = &it["snippet"];
                let vid = it["contentDetails"]["videoId"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| s(&sn["resourceId"]["videoId"]));
                PlaylistItem {
                    position: sn["position"].as_u64().unwrap_or_default(),
                    url: format!("https://www.youtube.com/watch?v={vid}"),
                    video_id: vid,
                    title: s(&sn["title"]),
                    video_owner: opt_s(&sn["videoOwnerChannelTitle"]),
                    video_published_at: opt_s(&it["contentDetails"]["videoPublishedAt"]),
                }
            })
            .collect();
        Ok(page(&v, list))
    }

    /// A channel's latest uploads with stats — **3 units** (channel, uploads page, videos).
    pub async fn channel_videos(
        &self,
        r: &ChannelRef,
        max_results: u32,
        page_token: Option<String>,
    ) -> Result<Page<Video>, YouTubeError> {
        let ch = self.channel(r).await?;
        let uploads = ch.uploads_playlist_id.ok_or_else(|| {
            YouTubeError::NotFound(format!("channel {} has no public uploads", ch.title))
        })?;
        let items = self
            .playlist_items(&uploads, max_results, page_token)
            .await?;
        let ids: Vec<String> = items.items.iter().map(|i| i.video_id.clone()).collect();
        let mut vids = self.videos(&ids).await?;
        // keep the uploads order (newest first)
        vids.sort_by_key(|v| ids.iter().position(|i| *i == v.id).unwrap_or(usize::MAX));
        Ok(Page {
            items: vids,
            next_page_token: items.next_page_token,
            total_results: items.total_results,
        })
    }

    /// `playlists.list` for a channel — **1 unit** (plus 1 if a handle must be resolved).
    pub async fn channel_playlists(
        &self,
        r: &ChannelRef,
        max_results: u32,
        page_token: Option<String>,
    ) -> Result<Page<Playlist>, YouTubeError> {
        let id = match r {
            ChannelRef::Id(id) => id.clone(),
            ChannelRef::Handle(_) => self.channel(r).await?.id,
        };
        let mut q = vec![
            ("part", "snippet,contentDetails".into()),
            ("channelId", id),
            ("maxResults", max_results.to_string()),
        ];
        if let Some(t) = page_token {
            q.push(("pageToken", t));
        }
        let v = self.get("/playlists", q).await?;
        let list = items(&v)
            .iter()
            .map(|it| {
                let sn = &it["snippet"];
                let id = s(&it["id"]);
                Playlist {
                    url: format!("https://www.youtube.com/playlist?list={id}"),
                    id,
                    title: s(&sn["title"]),
                    channel_title: s(&sn["channelTitle"]),
                    description: trim(sn["description"].as_str().unwrap_or_default(), DESC),
                    published_at: s(&sn["publishedAt"]),
                    item_count: it["contentDetails"]["itemCount"].as_u64(),
                }
            })
            .collect();
        Ok(page(&v, list))
    }

    /// `commentThreads.list` — **1 unit**. Top-level comments only.
    pub async fn comments(
        &self,
        target: CommentTarget<'_>,
        max_results: u32,
        order: Option<String>,
        search_terms: Option<String>,
        page_token: Option<String>,
    ) -> Result<Page<Comment>, YouTubeError> {
        let mut q = vec![
            ("part", "snippet".into()),
            match target {
                CommentTarget::Video(id) => ("videoId", id.to_owned()),
                CommentTarget::Channel(id) => ("allThreadsRelatedToChannelId", id.to_owned()),
            },
            ("maxResults", max_results.to_string()),
            ("textFormat", "plainText".into()),
        ];
        for (k, v) in [
            ("order", order),
            ("searchTerms", search_terms),
            ("pageToken", page_token),
        ] {
            if let Some(v) = v {
                q.push((k, v));
            }
        }
        let v = self.get("/commentThreads", q).await?;
        let list = items(&v)
            .iter()
            .map(|it| {
                let top = &it["snippet"]["topLevelComment"]["snippet"];
                let raw = top["textOriginal"]
                    .as_str()
                    .or(top["textDisplay"].as_str())
                    .unwrap_or_default();
                Comment {
                    id: s(&it["id"]),
                    video_id: opt_s(&it["snippet"]["videoId"]),
                    author: s(&top["authorDisplayName"]),
                    text: trim(&strip_html(raw), COMMENT),
                    likes: top["likeCount"].as_u64().unwrap_or_default(),
                    published_at: s(&top["publishedAt"]),
                    replies: it["snippet"]["totalReplyCount"]
                        .as_u64()
                        .unwrap_or_default(),
                }
            })
            .collect();
        Ok(page(&v, list))
    }

    /// `videos.list chart=mostPopular` — **1 unit**.
    pub async fn trending(
        &self,
        region: &str,
        category_id: Option<String>,
        max_results: u32,
        page_token: Option<String>,
    ) -> Result<Page<Video>, YouTubeError> {
        let mut q = vec![
            ("part", "snippet,contentDetails,statistics".into()),
            ("chart", "mostPopular".into()),
            ("regionCode", region.to_owned()),
            ("maxResults", max_results.to_string()),
        ];
        for (k, v) in [("videoCategoryId", category_id), ("pageToken", page_token)] {
            if let Some(v) = v {
                q.push((k, v));
            }
        }
        let v = self.get("/videos", q).await?;
        let vids = items(&v).iter().map(video_from).collect();
        Ok(page(&v, vids))
    }

    /// `channels.list` for up to 50 ids — **1 unit**.
    pub async fn channels(&self, ids: &[String]) -> Result<Vec<Channel>, YouTubeError> {
        let mut out = Vec::new();
        for chunk in ids.chunks(50) {
            let v = self
                .get(
                    "/channels",
                    vec![
                        ("part", "snippet,statistics,contentDetails".into()),
                        ("id", chunk.join(",")),
                        ("maxResults", "50".into()),
                    ],
                )
                .await?;
            out.extend(items(&v).iter().map(channel_from));
        }
        Ok(out)
    }

    /// Raw `videos.list` items (full description, every tag, status) — **1 unit** per 50.
    pub async fn videos_raw(&self, ids: &[String], part: &str) -> Result<Vec<Value>, YouTubeError> {
        let mut out = Vec::new();
        for chunk in ids.chunks(50) {
            let v = self
                .get(
                    "/videos",
                    vec![
                        ("part", part.into()),
                        ("id", chunk.join(",")),
                        ("maxResults", "50".into()),
                    ],
                )
                .await?;
            out.extend(items(&v).iter().cloned());
        }
        Ok(out)
    }

    /// A channel's latest `n` (≤ 200) uploads with stats, newest first — **2 + n/50 units**
    /// after the channel itself is known.
    pub async fn uploads(&self, ch: &Channel, n: u32) -> Result<Vec<Video>, YouTubeError> {
        let uploads = ch.uploads_playlist_id.clone().ok_or_else(|| {
            YouTubeError::NotFound(format!("channel {} has no public uploads", ch.title))
        })?;
        let mut ids = Vec::new();
        let mut token = None;
        while (ids.len() as u32) < n {
            let page = self
                .playlist_items(&uploads, (n - ids.len() as u32).min(50), token)
                .await?;
            ids.extend(page.items.into_iter().map(|i| i.video_id));
            token = page.next_page_token;
            if token.is_none() {
                break;
            }
        }
        let mut vids = Vec::new();
        for chunk in ids.chunks(50) {
            vids.extend(self.videos(chunk).await?);
        }
        vids.sort_by_key(|v| ids.iter().position(|i| *i == v.id).unwrap_or(usize::MAX));
        Ok(vids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_google_errors() {
        let quota =
            r#"{"error":{"code":403,"message":"quota","errors":[{"reason":"quotaExceeded"}]}}"#;
        assert!(matches!(classify(403, quota), YouTubeError::QuotaExceeded));
        let key = r#"{"error":{"code":400,"message":"API key not valid. Please pass a valid API key.","status":"INVALID_ARGUMENT","details":[{"reason":"API_KEY_INVALID"}]}}"#;
        assert!(matches!(classify(400, key), YouTubeError::BadKey));
        let nf = r#"{"error":{"code":404,"message":"The video identified by the videoId parameter could not be found.","errors":[{"reason":"videoNotFound"}]}}"#;
        assert!(matches!(classify(404, nf), YouTubeError::NotFound(_)));
        assert!(matches!(
            classify(500, "oops"),
            YouTubeError::Api { status: 500, .. }
        ));
    }

    #[test]
    fn strips_comment_html() {
        assert_eq!(
            strip_html("a<br>b <a href=\"x\">link</a> &amp; c"),
            "a\nb link & c"
        );
    }
}
