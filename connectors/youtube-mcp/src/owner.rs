//! Channel-owner calls (OAuth): YouTube Analytics reports and the write actions.

use std::path::{Path, PathBuf};
use std::time::Duration;

use connector_kit::text::trim;
use connector_kit::{HttpClient, Req, Secret};
use serde::Serialize;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::api::{Channel, YouTube, YouTubeError, channel_from};

/// Upload chunk size: a multiple of 256 KiB, as Google requires.
const CHUNK: u64 = 8 * 1024 * 1024;
pub const MAX_THUMBNAIL: u64 = 2 * 1024 * 1024;

async fn send(yt: &YouTube, req: Req<'_>) -> Result<Value, YouTubeError> {
    Ok(yt.http().send(req).await?.json()?)
}

pub async fn my_channels(yt: &YouTube, tok: &Secret) -> Result<Vec<Channel>, YouTubeError> {
    let v = send(
        yt,
        Req::get("/channels")
            .query("part", "snippet,statistics,contentDetails")
            .query("mine", "true")
            .query("maxResults", "50")
            .bearer(tok),
    )
    .await?;
    Ok(v["items"]
        .as_array()
        .map(|a| a.iter().map(channel_from).collect())
        .unwrap_or_default())
}

// ---------- analytics ----------

#[derive(Debug, Clone, Default)]
pub struct AnalyticsQuery {
    pub channel_id: Option<String>,
    pub start: String,
    pub end: String,
    pub metrics: Vec<String>,
    pub dimensions: Vec<String>,
    pub filters: Option<String>,
    pub sort: Option<String>,
    pub max_results: Option<u32>,
    pub currency: Option<String>,
}

/// A named report: (metrics, dimensions, sort, max results).
pub type PresetShape = (
    &'static [&'static str],
    &'static [&'static str],
    Option<&'static str>,
    Option<u32>,
);

pub fn preset(name: &str) -> Option<PresetShape> {
    Some(match name {
        "overview" => (
            &[
                "views",
                "engagedViews",
                "estimatedMinutesWatched",
                "averageViewDuration",
                "subscribersGained",
                "subscribersLost",
                "likes",
                "comments",
                "shares",
            ],
            &[],
            None,
            None,
        ),
        "daily" => (
            &[
                "views",
                "engagedViews",
                "estimatedMinutesWatched",
                "subscribersGained",
                "subscribersLost",
            ],
            &["day"],
            Some("day"),
            None,
        ),
        "top_videos" => (
            &[
                "views",
                "engagedViews",
                "estimatedMinutesWatched",
                "averageViewPercentage",
                "subscribersGained",
                "likes",
            ],
            &["video"],
            Some("-views"),
            Some(25),
        ),
        "audience_retention" => (
            &["audienceWatchRatio", "relativeRetentionPerformance"],
            &["elapsedVideoTimeRatio"],
            None,
            None,
        ),
        "traffic_sources" => (
            &["views", "engagedViews", "estimatedMinutesWatched"],
            &["insightTrafficSourceType"],
            Some("-views"),
            None,
        ),
        "search_terms" => (
            &["views", "estimatedMinutesWatched"],
            &["insightTrafficSourceDetail"],
            Some("-views"),
            Some(25),
        ),
        "audience_demographics" => (&["viewerPercentage"], &["ageGroup", "gender"], None, None),
        "audience_geography" => (
            &["views", "estimatedMinutesWatched", "averageViewDuration"],
            &["country"],
            Some("-views"),
            Some(50),
        ),
        "devices" => (
            &["views", "estimatedMinutesWatched"],
            &["deviceType"],
            Some("-views"),
            None,
        ),
        "subscribed_status" => (
            &["views", "estimatedMinutesWatched", "averageViewDuration"],
            &["subscribedStatus"],
            None,
            None,
        ),
        "shorts_vs_longform" => (
            &[
                "views",
                "engagedViews",
                "estimatedMinutesWatched",
                "subscribersGained",
            ],
            &["creatorContentType"],
            None,
            None,
        ),
        "revenue" => (
            &[
                "estimatedRevenue",
                "estimatedAdRevenue",
                "grossRevenue",
                "cpm",
                "playbackBasedCpm",
                "monetizedPlaybacks",
                "views",
            ],
            &["day"],
            Some("day"),
            None,
        ),
        "revenue_per_video" => (
            &[
                "estimatedRevenue",
                "views",
                "cpm",
                "playbackBasedCpm",
                "monetizedPlaybacks",
            ],
            &["video"],
            Some("-estimatedRevenue"),
            Some(25),
        ),
        _ => return None,
    })
}

pub const PRESETS: &[&str] = &[
    "overview",
    "daily",
    "top_videos",
    "audience_retention",
    "traffic_sources",
    "search_terms",
    "audience_demographics",
    "audience_geography",
    "devices",
    "subscribed_status",
    "shorts_vs_longform",
    "revenue",
    "revenue_per_video",
];

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub start: String,
    pub end: String,
    pub columns: Vec<String>,
    pub rows: Vec<Map<String, Value>>,
    pub notes: Vec<String>,
}

pub async fn analytics(
    yt: &YouTube,
    tok: &Secret,
    q: AnalyticsQuery,
) -> Result<Report, YouTubeError> {
    let ids = q
        .channel_id
        .as_deref()
        .map(|c| format!("channel=={c}"))
        .unwrap_or_else(|| "channel==MINE".into());
    let url = format!("{}/reports", yt.endpoints().analytics);
    let mut req = Req::get(url)
        .query("ids", ids)
        .query("startDate", q.start.clone())
        .query("endDate", q.end.clone())
        .query("metrics", q.metrics.join(","))
        .bearer(tok);
    if !q.dimensions.is_empty() {
        req = req.query("dimensions", q.dimensions.join(","));
    }
    req = req
        .query_opt("filters", q.filters.clone())
        .query_opt("sort", q.sort.clone())
        .query_opt("maxResults", q.max_results.map(|m| m.to_string()))
        .query_opt("currency", q.currency.clone());
    let v = send(yt, req).await?;
    let columns: Vec<String> = v["columnHeaders"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| c["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let mut rows: Vec<Map<String, Value>> = v["rows"]
        .as_array()
        .map(|rs| {
            rs.iter()
                .filter_map(|r| r.as_array())
                .map(|r| columns.iter().cloned().zip(r.iter().cloned()).collect())
                .collect()
        })
        .unwrap_or_default();
    let mut notes = Vec::new();
    // derived RPM where revenue and views are both present
    if columns.iter().any(|c| c == "estimatedRevenue") && columns.iter().any(|c| c == "views") {
        for r in &mut rows {
            if let (Some(rev), Some(views)) = (r["estimatedRevenue"].as_f64(), r["views"].as_f64())
                && views > 0.0
            {
                r.insert(
                    "rpm".into(),
                    json!((rev / views * 1000.0 * 100.0).round() / 100.0),
                );
            }
        }
        notes.push("rpm = estimatedRevenue ÷ views × 1000 (what you keep per 1,000 views).".into());
    }
    if q.metrics.iter().any(|m| m == "views") {
        notes.push("Since 2026-08-27 YouTube counts long-form views from the first frame; compare with engagedViews across that date.".into());
    }
    if rows.is_empty() {
        notes.push("No rows: the range may be too recent (analytics lag 1–3 days) or the filter matched nothing.".into());
    }
    Ok(Report {
        start: q.start,
        end: q.end,
        columns,
        rows,
        notes,
    })
}

// ---------- writes ----------

#[derive(Debug, Clone, Default)]
pub struct VideoChanges {
    pub title: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub category_id: Option<String>,
    pub privacy: Option<String>,
    pub publish_at: Option<String>,
    pub default_language: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Change {
    pub field: &'static str,
    pub before: Value,
    pub after: Value,
}

/// Reads the video, applies `changes`, and writes back only `snippet`/`status` as needed.
pub async fn update_video(
    yt: &YouTube,
    tok: &Secret,
    video_id: &str,
    c: VideoChanges,
) -> Result<Vec<Change>, YouTubeError> {
    let v = send(
        yt,
        Req::get("/videos")
            .query("part", "snippet,status")
            .query("id", video_id)
            .bearer(tok),
    )
    .await?;
    let item = v["items"]
        .get(0)
        .cloned()
        .ok_or_else(|| YouTubeError::NotFound(format!("video {video_id} (or it isn't yours)")))?;
    let mut snippet = item["snippet"].clone();
    let mut status = item["status"].clone();
    let mut changes = Vec::new();
    fn set(obj: &mut Value, key: &str, field: &'static str, new: Value, changes: &mut Vec<Change>) {
        let before = obj[key].clone();
        if before != new {
            changes.push(Change {
                field,
                before: trimmed(&before),
                after: trimmed(&new),
            });
            obj[key] = new;
        }
    }
    if let Some(t) = c.title {
        set(&mut snippet, "title", "title", json!(t), &mut changes);
    }
    if let Some(d) = c.description {
        set(
            &mut snippet,
            "description",
            "description",
            json!(d),
            &mut changes,
        );
    }
    if let Some(tags) = c.tags {
        set(&mut snippet, "tags", "tags", json!(tags), &mut changes);
    }
    if let Some(cat) = c.category_id {
        set(
            &mut snippet,
            "categoryId",
            "category_id",
            json!(cat),
            &mut changes,
        );
    }
    if let Some(l) = c.default_language {
        set(
            &mut snippet,
            "defaultLanguage",
            "default_language",
            json!(l),
            &mut changes,
        );
    }
    let snippet_changed = !changes.is_empty();
    let before_status = changes.len();
    if let Some(p) = c.privacy {
        set(
            &mut status,
            "privacyStatus",
            "privacy",
            json!(p),
            &mut changes,
        );
    }
    if let Some(at) = c.publish_at {
        set(
            &mut status,
            "publishAt",
            "publish_at",
            json!(at),
            &mut changes,
        );
    }
    let status_changed = changes.len() > before_status;
    if changes.is_empty() {
        return Ok(changes);
    }
    // These read-only fields make the update fail if sent back.
    if let Some(o) = snippet.as_object_mut() {
        for k in [
            "thumbnails",
            "channelTitle",
            "publishedAt",
            "channelId",
            "liveBroadcastContent",
            "localized",
            "defaultAudioLanguage",
        ] {
            o.remove(k);
        }
    }
    let mut parts = Vec::new();
    let mut body = json!({ "id": video_id });
    if snippet_changed {
        parts.push("snippet");
        body["snippet"] = snippet;
    }
    if status_changed {
        parts.push("status");
        let mut st = Map::new();
        for k in [
            "privacyStatus",
            "publishAt",
            "embeddable",
            "license",
            "publicStatsViewable",
            "selfDeclaredMadeForKids",
        ] {
            if !status[k].is_null() {
                st.insert(k.into(), status[k].clone());
            }
        }
        body["status"] = Value::Object(st);
    }
    send(
        yt,
        Req::put("/videos")
            .query("part", parts.join(","))
            .json(body)
            .bearer(tok),
    )
    .await?;
    Ok(changes)
}

fn trimmed(v: &Value) -> Value {
    match v {
        Value::String(s) => json!(trim(s, 300)),
        other => other.clone(),
    }
}

pub async fn reply_to_comment(
    yt: &YouTube,
    tok: &Secret,
    parent_id: &str,
    text: &str,
) -> Result<Value, YouTubeError> {
    let v = send(
        yt,
        Req::post("/comments")
            .query("part", "snippet")
            .json(json!({"snippet": {"parentId": parent_id, "textOriginal": text}}))
            .bearer(tok),
    )
    .await?;
    Ok(
        json!({ "comment_id": v["id"], "parent_id": parent_id, "text": v["snippet"]["textOriginal"] }),
    )
}

pub async fn post_comment(
    yt: &YouTube,
    tok: &Secret,
    video_id: &str,
    text: &str,
) -> Result<Value, YouTubeError> {
    let v = send(
        yt,
        Req::post("/commentThreads")
            .query("part", "snippet")
            .json(json!({"snippet": {"videoId": video_id, "topLevelComment": {"snippet": {"textOriginal": text}}}}))
            .bearer(tok),
    )
    .await?;
    Ok(
        json!({ "thread_id": v["id"], "video_id": video_id, "url": format!("https://www.youtube.com/watch?v={video_id}&lc={}", v["id"].as_str().unwrap_or_default()) }),
    )
}

/// Resolves `path` and checks it is a regular file inside `allowed`.
pub fn check_path(path: &str, allowed: Option<&Path>) -> Result<PathBuf, YouTubeError> {
    let dir = allowed.ok_or_else(|| {
        YouTubeError::Invalid("uploads are off: the host must set YOUTUBE_MCP_UPLOAD_DIR to the folder files may be uploaded from".into())
    })?;
    let dir = dir
        .canonicalize()
        .map_err(|e| YouTubeError::Invalid(format!("YOUTUBE_MCP_UPLOAD_DIR is not usable: {e}")))?;
    let p = Path::new(path);
    let p = if p.is_relative() {
        dir.join(p)
    } else {
        p.to_path_buf()
    };
    let real = p
        .canonicalize()
        .map_err(|_| YouTubeError::Invalid(format!("no such file: {}", trim(path, 200))))?;
    if !real.starts_with(&dir) {
        return Err(YouTubeError::Invalid(format!(
            "{} is outside the upload folder {}",
            trim(path, 200),
            dir.display()
        )));
    }
    if !real.is_file() {
        return Err(YouTubeError::Invalid(format!(
            "{} is not a file",
            trim(path, 200)
        )));
    }
    Ok(real)
}

pub async fn set_thumbnail(
    yt: &YouTube,
    tok: &Secret,
    video_id: &str,
    file: &Path,
) -> Result<Value, YouTubeError> {
    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        _ => {
            return Err(YouTubeError::Invalid(
                "thumbnails must be .jpg or .png".into(),
            ));
        }
    };
    let meta = tokio::fs::metadata(file)
        .await
        .map_err(|e| YouTubeError::Invalid(e.to_string()))?;
    if meta.len() > MAX_THUMBNAIL {
        return Err(YouTubeError::Invalid(format!(
            "thumbnail is {} bytes; YouTube's limit is 2 MB",
            meta.len()
        )));
    }
    let data = tokio::fs::read(file)
        .await
        .map_err(|e| YouTubeError::Invalid(e.to_string()))?;
    let url = format!("{}/thumbnails/set", yt.endpoints().upload);
    let v = send(
        yt,
        Req::post(url)
            .query("videoId", video_id)
            .bytes(data, mime)
            .bearer(tok),
    )
    .await?;
    Ok(
        json!({ "video_id": video_id, "thumbnail": v["items"][0]["maxres"]["url"].as_str().or(v["items"][0]["default"]["url"].as_str()) }),
    )
}

#[derive(Debug, Clone, Default)]
pub struct UploadMeta {
    pub title: String,
    pub description: String,
    pub tags: Vec<String>,
    pub category_id: String,
    pub privacy: String,
    pub publish_at: Option<String>,
    pub made_for_kids: bool,
    pub notify_subscribers: bool,
}

/// Resumable upload in 8 MiB chunks.
pub async fn upload_video(
    yt: &YouTube,
    tok: &Secret,
    file: &Path,
    m: UploadMeta,
) -> Result<Value, YouTubeError> {
    let size = tokio::fs::metadata(file)
        .await
        .map_err(|e| YouTubeError::Invalid(e.to_string()))?
        .len();
    if size == 0 {
        return Err(YouTubeError::Invalid("the file is empty".into()));
    }
    let mime = match file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        _ => "application/octet-stream",
    };
    let mut status =
        json!({ "privacyStatus": m.privacy, "selfDeclaredMadeForKids": m.made_for_kids });
    if let Some(at) = &m.publish_at {
        status["publishAt"] = json!(at);
    }
    let body = json!({
        "snippet": { "title": m.title, "description": m.description, "tags": m.tags, "categoryId": m.category_id },
        "status": status,
    });
    let url = format!("{}/videos", yt.endpoints().upload);
    let start = yt
        .http()
        .send(
            Req::post(url)
                .query("uploadType", "resumable")
                .query("part", "snippet,status")
                .query("notifySubscribers", m.notify_subscribers.to_string())
                .header("x-upload-content-type", mime)
                .header("x-upload-content-length", size.to_string())
                .json(body)
                .bearer(tok),
        )
        .await?;
    let session = start
        .header("location")
        .ok_or_else(|| YouTubeError::Http("upload session had no Location header".into()))?
        .to_owned();

    let uploader = HttpClient::with_timeout("", crate::api::UA, Duration::from_secs(600))
        .map_err(|e| YouTubeError::Http(e.to_string()))?;
    let mut f = tokio::fs::File::open(file)
        .await
        .map_err(|e| YouTubeError::Invalid(e.to_string()))?;
    let mut offset = 0u64;
    loop {
        let len = CHUNK.min(size - offset);
        let mut buf = vec![0u8; len as usize];
        f.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|e| YouTubeError::Invalid(e.to_string()))?;
        f.read_exact(&mut buf)
            .await
            .map_err(|e| YouTubeError::Invalid(e.to_string()))?;
        let resp = uploader
            .send(
                Req::put(session.clone())
                    .header(
                        "content-range",
                        format!("bytes {}-{}/{}", offset, offset + len - 1, size),
                    )
                    .bytes(buf, mime)
                    .bearer(tok),
            )
            .await?;
        if resp.status == 308 {
            // Google tells us how much it has: "Range: bytes=0-N"
            offset = resp
                .header("range")
                .and_then(|r| r.rsplit('-').next())
                .and_then(|n| n.parse::<u64>().ok())
                .map(|n| n + 1)
                .unwrap_or(offset + len);
            tracing::info!(offset, size, "upload progress");
            continue;
        }
        let v = resp.json()?;
        let id = v["id"].as_str().unwrap_or_default().to_owned();
        return Ok(json!({
            "video_id": id,
            "url": format!("https://www.youtube.com/watch?v={id}"),
            "studio_url": format!("https://studio.youtube.com/video/{id}/edit"),
            "privacy": v["status"]["privacyStatus"],
            "upload_status": v["status"]["uploadStatus"],
            "bytes": size,
        }));
    }
}
