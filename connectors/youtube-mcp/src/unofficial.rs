//! Undocumented YouTube endpoints: search autocomplete and caption tracks.
//!
//! These are what youtube.com itself uses. They need no key and cost no quota, but they are
//! not a supported API: they can change without notice and their use sits in a grey area of
//! YouTube's terms. Hosts can switch them off with `YOUTUBE_MCP_UNOFFICIAL=off`. The API key
//! is never sent to them.

use std::collections::HashMap;
use std::sync::Arc;

use connector_kit::text::trim;
use connector_kit::{Req, Response};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::api::{YouTube, YouTubeError};

async fn get(yt: &YouTube, req: Req<'_>) -> Result<Response, YouTubeError> {
    // no_auth: the Data API key must never go to youtube.com endpoints
    Ok(yt.http().send(req.no_auth()).await?)
}

// ---------- autocomplete ----------

/// YouTube search suggestions for `q`, in the order YouTube ranks them.
pub async fn suggest(
    yt: &YouTube,
    q: &str,
    language: &str,
    region: &str,
) -> Result<Vec<String>, YouTubeError> {
    let resp = get(
        yt,
        Req::get(yt.endpoints().suggest.clone())
            .query("client", "firefox")
            .query("ds", "yt")
            .query("hl", language.to_owned())
            .query("gl", region.to_owned())
            .query("q", q.to_owned()),
    )
    .await?;
    let text = resp.text();
    let v: Value = serde_json::from_str(&text)
        .map_err(|_| YouTubeError::Http("autocomplete answered in an unexpected format".into()))?;
    Ok(v[1]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().or_else(|| s[0].as_str()).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdeaMode {
    /// The seed's own suggestions.
    Suggestions,
    /// Seed + a…z: long-tail variations that keep the seed.
    LongTail,
    /// how/what/why/… + seed.
    Questions,
}

const QUESTION_WORDS: &[&str] = &[
    "how", "how to", "what", "why", "when", "where", "which", "who", "can", "is", "does", "should",
    "best",
];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Idea {
    pub keyword: String,
    /// 0–100 from how often and how high YouTube suggests it across the probes. A demand
    /// signal, not a search volume.
    pub popularity: u8,
    /// Probes that surfaced it.
    pub seen: usize,
    pub best_rank: usize,
}

pub async fn keyword_ideas(
    yt: &YouTube,
    seed: &str,
    mode: IdeaMode,
    language: &str,
    region: &str,
    limit: usize,
) -> Result<Vec<Idea>, YouTubeError> {
    let probes: Vec<String> = match mode {
        IdeaMode::Suggestions => vec![seed.to_owned(), format!("{seed} ")],
        IdeaMode::LongTail => std::iter::once(seed.to_owned())
            .chain(('a'..='z').map(|c| format!("{seed} {c}")))
            .collect(),
        IdeaMode::Questions => QUESTION_WORDS
            .iter()
            .map(|w| format!("{w} {seed}"))
            .collect(),
    };
    let sem = Arc::new(Semaphore::new(6));
    let mut set = JoinSet::new();
    for p in probes {
        let (yt, sem, lang, reg) = (
            yt.clone(),
            sem.clone(),
            language.to_owned(),
            region.to_owned(),
        );
        set.spawn(async move {
            let _permit = sem.acquire().await;
            suggest(&yt, &p, &lang, &reg).await
        });
    }
    let mut scores: HashMap<String, (f64, usize, usize)> = HashMap::new();
    let mut errors = 0;
    let mut last_err = None;
    while let Some(r) = set.join_next().await {
        match r {
            Ok(Ok(list)) => {
                for (rank, s) in list.into_iter().enumerate() {
                    let e = scores
                        .entry(s.to_lowercase())
                        .or_insert((0.0, 0, usize::MAX));
                    e.0 += 1.0 / (1.0 + rank as f64 * 0.35);
                    e.1 += 1;
                    e.2 = e.2.min(rank + 1);
                }
            }
            Ok(Err(e)) => {
                errors += 1;
                last_err = Some(e);
            }
            Err(_) => errors += 1,
        }
    }
    if scores.is_empty()
        && let Some(e) = last_err
    {
        return Err(e);
    }
    if errors > 0 {
        tracing::warn!(errors, "some autocomplete probes failed");
    }
    let seed_l = seed.to_lowercase();
    let mut out: Vec<(String, (f64, usize, usize))> = scores
        .into_iter()
        .filter(|(k, _)| {
            mode == IdeaMode::Questions
                || k.contains(seed_l.split_whitespace().next().unwrap_or(""))
        })
        .collect();
    out.sort_by(|a, b| b.1.0.total_cmp(&a.1.0).then_with(|| a.0.cmp(&b.0)));
    let top = out.first().map(|x| x.1.0).unwrap_or(1.0).max(1e-9);
    Ok(out
        .into_iter()
        .take(limit)
        .map(|(k, (s, seen, best))| Idea {
            keyword: k,
            popularity: ((s / top) * 100.0).round() as u8,
            seen,
            best_rank: best,
        })
        .collect())
}

// ---------- transcripts ----------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Track {
    pub language: String,
    pub name: String,
    /// `auto` for YouTube's speech recognition, `manual` for uploaded captions.
    pub kind: &'static str,
    #[serde(skip)]
    pub url: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Transcript {
    pub video_id: String,
    pub language: String,
    pub kind: &'static str,
    pub available: Vec<Track>,
    pub characters: usize,
    pub truncated: bool,
    /// One line per caption: `[mm:ss] text` when timestamps are on.
    pub text: String,
}

fn tracks_from(v: &Value) -> Vec<Track> {
    v.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| {
                    Some(Track {
                        url: t["baseUrl"].as_str()?.to_owned(),
                        language: t["languageCode"].as_str().unwrap_or_default().to_owned(),
                        name: t["name"]["simpleText"]
                            .as_str()
                            .map(str::to_owned)
                            .or_else(|| t["name"]["runs"][0]["text"].as_str().map(str::to_owned))
                            .unwrap_or_default(),
                        kind: if t["kind"].as_str() == Some("asr") {
                            "auto"
                        } else {
                            "manual"
                        },
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Pulls the `captionTracks` array out of a watch page.
fn tracks_from_html(html: &str) -> Vec<Track> {
    let Some(i) = html.find("\"captionTracks\":") else {
        return Vec::new();
    };
    let rest = &html[i + "\"captionTracks\":".len()..];
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (j, c) in rest.char_indices() {
        match (in_str, esc, c) {
            (true, true, _) => esc = false,
            (true, false, '\\') => esc = true,
            (true, false, '"') => in_str = false,
            (false, _, '"') => in_str = true,
            (false, _, '[') => depth += 1,
            (false, _, ']') => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_str::<Value>(&rest[..=j])
                        .map(|v| tracks_from(&v))
                        .unwrap_or_default();
                }
            }
            _ => {}
        }
    }
    Vec::new()
}

async fn caption_tracks(yt: &YouTube, video_id: &str) -> Result<Vec<Track>, YouTubeError> {
    let web = yt.endpoints().web.clone();
    // 1) the player API, as the Android app calls it
    let body = json!({
        "context": { "client": { "clientName": "ANDROID", "clientVersion": "20.10.38", "androidSdkVersion": 30, "hl": "en", "gl": "US" } },
        "videoId": video_id,
    });
    let player = get(
        yt,
        Req::post(format!("{web}/youtubei/v1/player"))
            .query("prettyPrint", "false")
            .header(
                "user-agent",
                "com.google.android.youtube/20.10.38 (Linux; U; Android 11) gzip",
            )
            .json(body),
    )
    .await;
    if let Ok(r) = &player
        && let Ok(v) = r.json()
    {
        let status = v["playabilityStatus"]["status"].as_str().unwrap_or("OK");
        if matches!(status, "ERROR" | "LOGIN_REQUIRED") && v["videoDetails"].is_null() {
            let reason = v["playabilityStatus"]["reason"]
                .as_str()
                .unwrap_or("unavailable");
            return Err(YouTubeError::NotFound(format!(
                "video {video_id}: {}",
                trim(reason, 150)
            )));
        }
        let t = tracks_from(&v["captions"]["playerCaptionsTracklistRenderer"]["captionTracks"]);
        if !t.is_empty() {
            return Ok(t);
        }
    }
    // 2) the watch page
    let page = get(
        yt,
        Req::get(format!("{web}/watch"))
            .query("v", video_id.to_owned())
            .query("hl", "en")
            .header("accept-language", "en-US,en;q=0.8"),
    )
    .await?;
    Ok(tracks_from_html(&page.text()))
}

fn mmss(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// Parses a `fmt=json3` caption document.
fn parse_json3(v: &Value, timestamps: bool) -> String {
    let mut out = String::new();
    for e in v["events"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let Some(segs) = e["segs"].as_array() else {
            continue;
        };
        let line: String = segs
            .iter()
            .filter_map(|s| s["utf8"].as_str())
            .collect::<String>()
            .replace('\n', " ");
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if timestamps {
            out.push_str(&format!("[{}] ", mmss(e["tStartMs"].as_u64().unwrap_or(0))));
            out.push_str(line);
            out.push('\n');
        } else {
            out.push_str(line);
            out.push(' ');
        }
    }
    out.trim_end().to_owned()
}

/// Parses the XML `timedtext` format (fallback when json3 isn't served).
fn parse_xml(xml: &str, timestamps: bool) -> String {
    let mut out = String::new();
    for part in xml.split("<text").skip(1) {
        let (attrs, rest) = part.split_once('>').unwrap_or(("", ""));
        let body = rest.split("</text>").next().unwrap_or("");
        let start = attrs
            .split("start=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.0);
        let text = body
            .replace("&amp;", "&")
            .replace("&#39;", "'")
            .replace("&quot;", "\"")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace('\n', " ");
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if timestamps {
            out.push_str(&format!("[{}] {text}\n", mmss((start * 1000.0) as u64)));
        } else {
            out.push_str(text);
            out.push(' ');
        }
    }
    out.trim_end().to_owned()
}

pub async fn transcript(
    yt: &YouTube,
    video_id: &str,
    language: Option<&str>,
    timestamps: bool,
    max_chars: usize,
) -> Result<Transcript, YouTubeError> {
    let tracks = caption_tracks(yt, video_id).await?;
    if tracks.is_empty() {
        return Err(YouTubeError::NotFound(format!(
            "no captions for {video_id}: the video has none, captions are still processing, it's live, or YouTube blocked the unofficial request"
        )));
    }
    let pick = match language {
        Some(l) => {
            let l = l.to_lowercase();
            tracks
                .iter()
                .filter(|t| {
                    t.language.to_lowercase() == l
                        || t.language.to_lowercase().starts_with(&format!("{l}-"))
                })
                .min_by_key(|t| (t.kind == "auto") as u8)
                .ok_or_else(|| {
                    YouTubeError::NotFound(format!(
                        "no {l} captions; available: {}",
                        tracks
                            .iter()
                            .map(|t| format!("{} ({})", t.language, t.kind))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                })?
        }
        None => tracks
            .iter()
            .min_by_key(|t| (t.kind == "auto") as u8)
            .unwrap_or(&tracks[0]),
    };
    let web = yt.endpoints().web.clone();
    if !(pick.url.starts_with("https://www.youtube.com/")
        || pick.url.starts_with(&format!("{web}/")))
    {
        return Err(YouTubeError::Http(
            "caption track pointed outside youtube.com; refusing to fetch it".into(),
        ));
    }
    let sep = if pick.url.contains('?') { '&' } else { '?' };
    let doc = get(yt, Req::get(format!("{}{sep}fmt=json3", pick.url))).await?;
    let raw = doc.text();
    let full = match serde_json::from_str::<Value>(&raw) {
        Ok(v) => parse_json3(&v, timestamps),
        Err(_) => parse_xml(&raw, timestamps),
    };
    if full.is_empty() {
        return Err(YouTubeError::NotFound(format!(
            "YouTube returned an empty caption track for {video_id}; try again later"
        )));
    }
    let characters = full.chars().count();
    let truncated = characters > max_chars;
    Ok(Transcript {
        video_id: video_id.to_owned(),
        language: pick.language.clone(),
        kind: pick.kind,
        available: tracks.clone(),
        characters,
        truncated,
        text: if truncated {
            trim(&full, max_chars)
        } else {
            full
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_caption_formats() {
        let v = json!({"events": [
            {"tStartMs": 0, "segs": [{"utf8": "hello "}, {"utf8": "world"}]},
            {"tStartMs": 61_500, "segs": [{"utf8": "\n"}]},
            {"tStartMs": 3_725_000, "segs": [{"utf8": "later"}]}
        ]});
        assert_eq!(
            parse_json3(&v, true),
            "[00:00] hello world\n[1:02:05] later"
        );
        assert_eq!(parse_json3(&v, false), "hello world later");
        let xml = r#"<transcript><text start="1.5" dur="2">it&#39;s &amp; ok</text><text start="65">next</text></transcript>"#;
        assert_eq!(parse_xml(xml, true), "[00:01] it's & ok\n[01:05] next");
    }

    #[test]
    fn finds_tracks_in_watch_page() {
        let html = r#"var x = {"captions":{"playerCaptionsTracklistRenderer":{"captionTracks":[{"baseUrl":"https://www.youtube.com/api/timedtext?v=x&lang=en","name":{"simpleText":"English [auto]"},"languageCode":"en","kind":"asr"}],"audioTracks":[]}}};"#;
        let t = tracks_from_html(html);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].kind, "auto");
        assert!(t[0].url.contains("lang=en"));
    }
}
