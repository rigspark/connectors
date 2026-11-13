//! Private channel insights, ported from ViewsFix (yt-reviewer)'s private YouTube MCP tools:
//! retention, click-through, traffic sources, audience and the last 48 hours, joined per video,
//! with a diagnosis of which videos lose people at the thumbnail and which lose them in the video.
//!
//! Everything here is pure: it takes YouTube Analytics report rows and returns summaries, so it
//! is tested without the network. `tools::owner::channel_insights` does the fetching.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;
use serde_json::{Map, Value};

pub type Row = Map<String, Value>;

/// Videos need this many thumbnail impressions before their CTR is compared with the channel's.
pub const MIN_IMPRESSIONS: f64 = 1_000.0;
/// …and this many views before their retention is.
pub const MIN_VIEWS: f64 = 100.0;
/// A video is "below" or "above" the channel when it is 15% off its average.
const BAND: f64 = 0.15;

fn num(r: &Row, k: &str) -> Option<f64> {
    r.get(k).and_then(Value::as_f64)
}
fn text(r: &Row, k: &str) -> Option<String> {
    r.get(k).and_then(Value::as_str).map(str::to_owned)
}
fn r1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}
fn r2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// YouTube documents click rates as percentages, but some responses carry a 0–1 fraction.
/// Returns the factor that turns this report's rates into percent: 100 when every rate is ≤ 1.
pub fn ctr_scale(rates: &[f64]) -> f64 {
    if !rates.is_empty() && rates.iter().all(|r| *r <= 1.0) {
        100.0
    } else {
        1.0
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct VideoInsight {
    pub video_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub views: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub impressions: Option<u64>,
    /// Thumbnail click-through rate, percent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctr_pct: Option<f64>,
    /// Average share of the video watched, percent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_view_pct: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_view_secs: Option<u64>,
    /// `packaging` (people skip the thumbnail/title), `content` (they click, then leave),
    /// `packaging_and_content`, `outperforming` or `on_par`. Absent when the sample is too small.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnosis: Option<&'static str>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct ChannelAverages {
    /// Clicks ÷ impressions over every video in the CTR report, percent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctr_pct: Option<f64>,
    /// View-weighted average percentage watched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_view_pct: Option<f64>,
    /// View-weighted average view duration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_view_secs: Option<u64>,
}

/// Joins the `ctr_by_video` and `retention_by_video` reports by video, works out the channel's
/// averages, and diagnoses each video against them. Sorted by views, highest first.
pub fn videos(ctr: &[Row], retention: &[Row]) -> (Vec<VideoInsight>, ChannelAverages) {
    let scale = ctr_scale(
        &ctr.iter()
            .filter_map(|r| num(r, "videoThumbnailImpressionsClickRate"))
            .collect::<Vec<_>>(),
    );
    let mut by_id: BTreeMap<String, VideoInsight> = BTreeMap::new();

    let (mut impr_sum, mut click_sum) = (0.0, 0.0);
    for r in ctr {
        let Some(id) = text(r, "video") else { continue };
        let impr = num(r, "videoThumbnailImpressions");
        let rate = num(r, "videoThumbnailImpressionsClickRate").map(|x| x * scale);
        if let (Some(i), Some(c)) = (impr, rate) {
            impr_sum += i;
            click_sum += i * c / 100.0;
        }
        let v = slot(&mut by_id, &id);
        v.impressions = impr.map(|x| x.round() as u64);
        v.ctr_pct = rate.map(r2);
        if v.views.is_none() {
            v.views = num(r, "views").map(|x| x.round() as u64);
        }
    }

    let (mut w_views, mut w_pct, mut w_secs) = (0.0, 0.0, 0.0);
    for r in retention {
        let Some(id) = text(r, "video") else { continue };
        let views = num(r, "views");
        let pct = num(r, "averageViewPercentage");
        let secs = num(r, "averageViewDuration");
        if let (Some(n), Some(p), Some(s)) = (views, pct, secs) {
            w_views += n;
            w_pct += n * p;
            w_secs += n * s;
        }
        let v = slot(&mut by_id, &id);
        if let Some(n) = views {
            v.views = Some(n.round() as u64);
        }
        v.avg_view_pct = pct.map(r1);
        v.avg_view_secs = secs.map(|x| x.round() as u64);
    }

    let avg = ChannelAverages {
        ctr_pct: (impr_sum > 0.0).then(|| r2(click_sum / impr_sum * 100.0)),
        avg_view_pct: (w_views > 0.0).then(|| r1(w_pct / w_views)),
        avg_view_secs: (w_views > 0.0).then(|| (w_secs / w_views).round() as u64),
    };

    let mut list: Vec<VideoInsight> = by_id.into_values().collect();
    for v in &mut list {
        v.diagnosis = diagnose(v, &avg);
    }
    list.sort_by_key(|v| std::cmp::Reverse(v.views.unwrap_or(0)));
    (list, avg)
}

fn slot<'a>(m: &'a mut BTreeMap<String, VideoInsight>, id: &str) -> &'a mut VideoInsight {
    m.entry(id.to_owned()).or_insert_with(|| VideoInsight {
        video_id: id.to_owned(),
        title: None,
        url: format!("https://www.youtube.com/watch?v={id}"),
        views: None,
        impressions: None,
        ctr_pct: None,
        avg_view_pct: None,
        avg_view_secs: None,
        diagnosis: None,
    })
}

fn diagnose(v: &VideoInsight, avg: &ChannelAverages) -> Option<&'static str> {
    let ctr = v.ctr_pct.zip(avg.ctr_pct)?;
    let ret = v.avg_view_pct.zip(avg.avg_view_pct)?;
    if (v.impressions? as f64) < MIN_IMPRESSIONS || (v.views? as f64) < MIN_VIEWS {
        return None;
    }
    let low = |(x, a): (f64, f64)| x < a * (1.0 - BAND);
    let high = |(x, a): (f64, f64)| x > a * (1.0 + BAND);
    Some(match (low(ctr), low(ret)) {
        (true, true) => "packaging_and_content",
        (true, false) => "packaging",
        (false, true) => "content",
        _ if high(ctr) && high(ret) => "outperforming",
        _ => "on_par",
    })
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Share {
    pub name: String,
    pub views: u64,
    pub share_pct: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub watch_minutes: Option<u64>,
}

/// Each row's share of the report's views, keyed on `dim` (e.g. `insightTrafficSourceType`).
pub fn shares(rows: &[Row], dim: &str) -> Vec<Share> {
    let total: f64 = rows.iter().filter_map(|r| num(r, "views")).sum();
    let mut out: Vec<Share> = rows
        .iter()
        .filter_map(|r| {
            let views = num(r, "views")?;
            Some(Share {
                name: text(r, dim)?,
                views: views.round() as u64,
                share_pct: if total > 0.0 {
                    r1(views / total * 100.0)
                } else {
                    0.0
                },
                watch_minutes: num(r, "estimatedMinutesWatched").map(|m| m.round() as u64),
            })
        })
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.views));
    out
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Pct {
    pub name: String,
    pub pct: f64,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Audience {
    /// Share of views by age group (signed-in viewers), largest first.
    pub age_groups: Vec<Pct>,
    pub gender: Vec<Pct>,
}

/// Sums the `audience_demographics` report (`ageGroup`, `gender`, `viewerPercentage`) into
/// separate age and gender splits.
pub fn audience(rows: &[Row]) -> Audience {
    let mut ages: HashMap<String, f64> = HashMap::new();
    let mut genders: HashMap<String, f64> = HashMap::new();
    for r in rows {
        let Some(p) = num(r, "viewerPercentage") else {
            continue;
        };
        if let Some(a) = text(r, "ageGroup") {
            *ages
                .entry(a.trim_start_matches("age").to_owned())
                .or_default() += p;
        }
        if let Some(g) = text(r, "gender") {
            let g = match g.as_str() {
                "user_specified" => "other".to_owned(),
                _ => g.to_lowercase(),
            };
            *genders.entry(g).or_default() += p;
        }
    }
    let sorted = |m: HashMap<String, f64>| {
        let mut v: Vec<Pct> = m
            .into_iter()
            .map(|(name, pct)| Pct { name, pct: r1(pct) })
            .collect();
        v.sort_by(|a, b| b.pct.total_cmp(&a.pct).then(a.name.cmp(&b.name)));
        v
    };
    Audience {
        age_groups: sorted(ages),
        gender: sorted(genders),
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Day {
    pub date: String,
    pub views: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct VideoViews {
    pub video_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub views: u64,
    pub url: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Recent {
    pub total_views: u64,
    pub days: Vec<Day>,
    pub top_videos: Vec<VideoViews>,
}

/// The last-48-hours section from a `day` report and a `video` report over the same dates.
pub fn recent(days: &[Row], videos: &[Row]) -> Recent {
    let days: Vec<Day> = days
        .iter()
        .filter_map(|r| {
            Some(Day {
                date: text(r, "day")?,
                views: num(r, "views")?.round() as u64,
            })
        })
        .collect();
    let mut top: Vec<VideoViews> = videos
        .iter()
        .filter_map(|r| {
            let id = text(r, "video")?;
            Some(VideoViews {
                url: format!("https://www.youtube.com/watch?v={id}"),
                video_id: id,
                title: None,
                views: num(r, "views")?.round() as u64,
            })
        })
        .collect();
    top.sort_by_key(|v| std::cmp::Reverse(v.views));
    top.truncate(10);
    Recent {
        total_views: days.iter().map(|d| d.views).sum(),
        days,
        top_videos: top,
    }
}

/// One line per diagnosis, for the model to lead with.
pub fn headlines(videos: &[VideoInsight], avg: &ChannelAverages, traffic: &[Share]) -> Vec<String> {
    let mut out = Vec::new();
    let count = |d: &str| videos.iter().filter(|v| v.diagnosis == Some(d)).count();
    if let Some(c) = avg.ctr_pct {
        let n = count("packaging") + count("packaging_and_content");
        if n > 0 {
            out.push(format!(
                "{n} video(s) get clicked well below your {c}% average CTR: test new titles and thumbnails first."
            ));
        }
    }
    if let Some(p) = avg.avg_view_pct {
        let n = count("content") + count("packaging_and_content");
        if n > 0 {
            out.push(format!(
                "{n} video(s) are watched well below your {p}% average: check their first 30 seconds and pacing."
            ));
        }
    }
    let n = count("outperforming");
    if n > 0 {
        out.push(format!(
            "{n} video(s) beat your averages on both CTR and retention: make more like them."
        ));
    }
    if let Some(top) = traffic.first() {
        out.push(format!(
            "Most views come from {} ({}%).",
            traffic_name(&top.name),
            top.share_pct
        ));
    }
    out
}

/// Plain names for YouTube's traffic source codes.
pub fn traffic_name(code: &str) -> &str {
    match code {
        "SUBSCRIBER" => "browse features (home, subscriptions)",
        "YT_SEARCH" => "YouTube search",
        "RELATED_VIDEO" => "suggested videos",
        "SHORTS" => "the Shorts feed",
        "EXT_URL" => "external sites",
        "PLAYLIST" => "playlists",
        "NO_LINK_OTHER" => "direct or unknown",
        "YT_CHANNEL" => "channel pages",
        "NOTIFICATION" => "notifications",
        "END_SCREEN" => "end screens",
        "YT_OTHER_PAGE" => "other YouTube pages",
        "HASHTAGS" => "hashtag pages",
        "SOUND_PAGE" => "sound pages",
        "ANNOTATION" => "cards and annotations",
        "ADVERTISING" => "ads",
        _ => code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(v: Value) -> Row {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn ctr_scale_detects_fractions() {
        assert_eq!(ctr_scale(&[0.04, 0.081]), 100.0);
        assert_eq!(ctr_scale(&[4.0, 8.1]), 1.0);
        assert_eq!(ctr_scale(&[]), 1.0);
    }

    #[test]
    fn joins_videos_and_diagnoses_against_channel_average() {
        let ctr = vec![
            row(
                json!({"video":"good","videoThumbnailImpressions":10000.0,"videoThumbnailImpressionsClickRate":8.0,"views":900.0}),
            ),
            row(
                json!({"video":"lowctr","videoThumbnailImpressions":10000.0,"videoThumbnailImpressionsClickRate":2.0,"views":250.0}),
            ),
            row(
                json!({"video":"lowret","videoThumbnailImpressions":10000.0,"videoThumbnailImpressionsClickRate":5.0,"views":500.0}),
            ),
            row(
                json!({"video":"tiny","videoThumbnailImpressions":50.0,"videoThumbnailImpressionsClickRate":1.0,"views":1.0}),
            ),
        ];
        let ret = vec![
            row(
                json!({"video":"good","views":1000.0,"averageViewDuration":300.0,"averageViewPercentage":60.0}),
            ),
            row(
                json!({"video":"lowctr","views":300.0,"averageViewDuration":280.0,"averageViewPercentage":52.0}),
            ),
            row(
                json!({"video":"lowret","views":500.0,"averageViewDuration":90.0,"averageViewPercentage":20.0}),
            ),
        ];
        let (vids, avg) = videos(&ctr, &ret);
        // clicks 800+200+500+0.5 over 30050 impressions
        assert_eq!(avg.ctr_pct, Some(4.99));
        // (1000·60 + 300·52 + 500·20) / 1800
        assert_eq!(avg.avg_view_pct, Some(47.6));
        assert_eq!(vids[0].video_id, "good");
        let d = |id: &str| vids.iter().find(|v| v.video_id == id).unwrap().diagnosis;
        assert_eq!(d("good"), Some("outperforming"));
        assert_eq!(d("lowctr"), Some("packaging"));
        assert_eq!(d("lowret"), Some("content"));
        assert_eq!(d("tiny"), None, "too few impressions to judge");
    }

    #[test]
    fn fractional_ctr_is_reported_in_percent() {
        let ctr = vec![row(
            json!({"video":"a","videoThumbnailImpressions":2000.0,"videoThumbnailImpressionsClickRate":0.045}),
        )];
        let (vids, avg) = videos(&ctr, &[]);
        assert_eq!(vids[0].ctr_pct, Some(4.5));
        assert_eq!(avg.ctr_pct, Some(4.5));
        assert_eq!(vids[0].diagnosis, None, "no retention data");
    }

    #[test]
    fn traffic_shares_and_audience() {
        let t = shares(
            &[
                row(
                    json!({"insightTrafficSourceType":"YT_SEARCH","views":300.0,"estimatedMinutesWatched":900.0}),
                ),
                row(json!({"insightTrafficSourceType":"RELATED_VIDEO","views":700.0})),
            ],
            "insightTrafficSourceType",
        );
        assert_eq!(t[0].name, "RELATED_VIDEO");
        assert_eq!(t[0].share_pct, 70.0);
        assert_eq!(t[1].watch_minutes, Some(900));

        let a = audience(&[
            row(json!({"ageGroup":"age18-24","gender":"male","viewerPercentage":30.0})),
            row(json!({"ageGroup":"age18-24","gender":"female","viewerPercentage":20.0})),
            row(json!({"ageGroup":"age25-34","gender":"male","viewerPercentage":40.0})),
            row(json!({"ageGroup":"age25-34","gender":"user_specified","viewerPercentage":10.0})),
        ]);
        assert_eq!(
            a.age_groups[0],
            Pct {
                name: "18-24".into(),
                pct: 50.0
            }
        );
        assert_eq!(
            a.gender[0],
            Pct {
                name: "male".into(),
                pct: 70.0
            }
        );
        assert!(a.gender.iter().any(|g| g.name == "other"));
    }

    #[test]
    fn recent_totals_and_headlines() {
        let r = recent(
            &[
                row(json!({"day":"2026-11-11","views":120.0})),
                row(json!({"day":"2026-11-12","views":80.0})),
            ],
            &[row(json!({"video":"x","views":150.0}))],
        );
        assert_eq!(r.total_views, 200);
        assert_eq!(r.top_videos[0].url, "https://www.youtube.com/watch?v=x");

        let traffic = vec![Share {
            name: "YT_SEARCH".into(),
            views: 10,
            share_pct: 62.5,
            watch_minutes: None,
        }];
        let h = headlines(&[], &ChannelAverages::default(), &traffic);
        assert_eq!(
            h,
            vec!["Most views come from YouTube search (62.5%).".to_owned()]
        );
    }
}
