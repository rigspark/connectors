//! Local snapshot history. Every video and channel the connector fetches is recorded, so
//! growth, views-per-hour trends and title/thumbnail changes can be read back later.
//! History starts empty and fills as the connector is used (or as `track_channel` runs on a
//! schedule). Nothing here leaves the machine.
#![allow(clippy::type_complexity)] // row tuples straight from SQL

use std::path::Path;
use std::sync::Mutex;

use connector_kit::time::{format_rfc3339, parse_rfc3339};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::api::{Channel, Video};

/// Snapshots closer together than this, with no change, are skipped.
const MIN_GAP_SECS: i64 = 15 * 60;

pub struct Store {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct VideoSnap {
    pub at: String,
    pub views: Option<u64>,
    pub likes: Option<u64>,
    pub comments: Option<u64>,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_hash: Option<String>,
    /// Views per hour since the previous snapshot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vph_since_previous: Option<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ChannelSnap {
    pub at: String,
    pub subscribers: Option<u64>,
    pub views: Option<u64>,
    pub videos: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscribers_per_day_since_previous: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub views_per_day_since_previous: Option<f64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PackagingChange {
    pub video_id: String,
    pub url: String,
    pub channel_id: String,
    pub changed_between: [String; 2],
    /// `title`, `thumbnail` or `both`.
    pub change: &'static str,
    pub title_before: String,
    pub title_after: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vph_before: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vph_after: Option<f64>,
    /// vph_after ÷ vph_before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lift: Option<f64>,
}

#[derive(Debug, thiserror::Error)]
#[error("history database: {0}")]
pub struct StoreError(String);

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError(e.to_string())
    }
}

fn r1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| StoreError(format!("can't create {}: {e}", dir.display())))?;
        }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn in_memory() -> Result<Self, StoreError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, StoreError> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA busy_timeout=3000;
             CREATE TABLE IF NOT EXISTS video_snap(
                video_id TEXT NOT NULL, ts INTEGER NOT NULL, channel_id TEXT NOT NULL, published INTEGER,
                views INTEGER, likes INTEGER, comments INTEGER, title TEXT NOT NULL, thumb TEXT,
                PRIMARY KEY(video_id, ts));
             CREATE INDEX IF NOT EXISTS video_snap_channel ON video_snap(channel_id, ts);
             CREATE TABLE IF NOT EXISTS channel_snap(
                channel_id TEXT NOT NULL, ts INTEGER NOT NULL, title TEXT NOT NULL,
                subs INTEGER, views INTEGER, videos INTEGER,
                PRIMARY KEY(channel_id, ts));",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Records videos; `thumbs` optionally maps video id → thumbnail hash.
    pub fn record_videos(
        &self,
        videos: &[Video],
        thumbs: &[(String, String)],
        now: i64,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        for v in videos {
            let thumb = thumbs
                .iter()
                .find(|(id, _)| *id == v.id)
                .map(|(_, h)| h.clone());
            let last: Option<(i64, Option<i64>, String, Option<String>)> = tx
                .query_row(
                    "SELECT ts, views, title, thumb FROM video_snap WHERE video_id=?1 ORDER BY ts DESC LIMIT 1",
                    params![v.id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            if let Some((ts, views, title, last_thumb)) = &last {
                let same = *views == v.views.map(|x| x as i64)
                    && *title == v.title
                    && (thumb.is_none() || thumb == *last_thumb);
                if now - ts < MIN_GAP_SECS && same || *ts >= now {
                    continue;
                }
            }
            tx.execute(
                "INSERT OR IGNORE INTO video_snap(video_id, ts, channel_id, published, views, likes, comments, title, thumb)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    v.id,
                    now,
                    v.channel_id,
                    parse_rfc3339(&v.published_at),
                    v.views.map(|x| x as i64),
                    v.likes.map(|x| x as i64),
                    v.comments.map(|x| x as i64),
                    v.title,
                    thumb
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn record_channel(&self, c: &Channel, now: i64) -> Result<(), StoreError> {
        let conn = self.lock();
        let last: Option<(i64, Option<i64>, Option<i64>)> = conn
            .query_row(
                "SELECT ts, subs, views FROM channel_snap WHERE channel_id=?1 ORDER BY ts DESC LIMIT 1",
                params![c.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some((ts, subs, views)) = last {
            let same =
                subs == c.subscribers.map(|x| x as i64) && views == c.views.map(|x| x as i64);
            if now - ts < MIN_GAP_SECS && same || ts >= now {
                return Ok(());
            }
        }
        conn.execute(
            "INSERT OR IGNORE INTO channel_snap(channel_id, ts, title, subs, views, videos) VALUES(?1,?2,?3,?4,?5,?6)",
            params![c.id, now, c.title, c.subscribers.map(|x| x as i64), c.views.map(|x| x as i64), c.videos.map(|x| x as i64)],
        )?;
        Ok(())
    }

    pub fn video_history(&self, video_id: &str) -> Result<Vec<VideoSnap>, StoreError> {
        let conn = self.lock();
        let mut st = conn.prepare(
            "SELECT ts, views, likes, comments, title, thumb FROM video_snap WHERE video_id=?1 ORDER BY ts",
        )?;
        let rows: Vec<(
            i64,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            String,
            Option<String>,
        )> = st
            .query_map(params![video_id], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })?
            .collect::<Result<_, _>>()?;
        let mut out = Vec::with_capacity(rows.len());
        let mut prev: Option<(i64, Option<i64>)> = None;
        for (ts, views, likes, comments, title, thumb) in rows {
            let vph = match (prev, views) {
                (Some((pts, Some(pv))), Some(v)) if ts > pts => {
                    Some(r1((v - pv) as f64 / ((ts - pts) as f64 / 3600.0)))
                }
                _ => None,
            };
            prev = Some((ts, views));
            out.push(VideoSnap {
                at: format_rfc3339(ts),
                views: views.map(|x| x as u64),
                likes: likes.map(|x| x as u64),
                comments: comments.map(|x| x as u64),
                title,
                thumbnail_hash: thumb,
                vph_since_previous: vph,
            });
        }
        Ok(out)
    }

    pub fn channel_history(&self, channel_id: &str) -> Result<Vec<ChannelSnap>, StoreError> {
        let conn = self.lock();
        let mut st = conn.prepare(
            "SELECT ts, subs, views, videos FROM channel_snap WHERE channel_id=?1 ORDER BY ts",
        )?;
        let rows: Vec<(i64, Option<i64>, Option<i64>, Option<i64>)> = st
            .query_map(params![channel_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<Result<_, _>>()?;
        let mut out = Vec::new();
        let mut prev: Option<(i64, Option<i64>, Option<i64>)> = None;
        for (ts, subs, views, videos) in rows {
            let per_day = |a: Option<i64>, b: Option<i64>, pts: i64| match (a, b) {
                (Some(a), Some(b)) if ts > pts => {
                    Some(r1((b - a) as f64 / ((ts - pts) as f64 / 86_400.0)))
                }
                _ => None,
            };
            let (spd, vpd) = match prev {
                Some((pts, ps, pv)) => (per_day(ps, subs, pts), per_day(pv, views, pts)),
                None => (None, None),
            };
            prev = Some((ts, subs, views));
            out.push(ChannelSnap {
                at: format_rfc3339(ts),
                subscribers: subs.map(|x| x as u64),
                views: views.map(|x| x as u64),
                videos: videos.map(|x| x as u64),
                subscribers_per_day_since_previous: spd,
                views_per_day_since_previous: vpd,
            });
        }
        Ok(out)
    }

    /// Videos whose title or thumbnail changed between snapshots, with views-per-hour
    /// before and after the change.
    pub fn packaging_changes(
        &self,
        channel_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<PackagingChange>, StoreError> {
        let ids: Vec<(String, String)> = {
            let conn = self.lock();
            let mut st = conn.prepare(
                "SELECT video_id, channel_id FROM video_snap WHERE (?1 IS NULL OR channel_id=?1)
                 GROUP BY video_id HAVING COUNT(DISTINCT title) > 1 OR COUNT(DISTINCT thumb) > 1
                 ORDER BY MAX(ts) DESC LIMIT 500",
            )?;
            st.query_map(params![channel_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<_, _>>()?
        };
        let mut out = Vec::new();
        for (vid, chan) in ids {
            let snaps = self.raw_video(&vid)?;
            // last change point
            let mut idx = None;
            let mut last_thumb: Option<&str> = None;
            for i in 1..snaps.len() {
                let title_changed = snaps[i].4 != snaps[i - 1].4;
                if snaps[i - 1].5.is_some() {
                    last_thumb = snaps[i - 1].5.as_deref();
                }
                let thumb_changed = snaps[i].5.is_some()
                    && last_thumb.is_some()
                    && snaps[i].5.as_deref() != last_thumb;
                if title_changed || thumb_changed {
                    idx = Some((i, title_changed, thumb_changed));
                }
            }
            let Some((i, t, th)) = idx else { continue };
            let rate =
                |a: &(i64, Option<i64>, String, String, String, Option<String>),
                 b: &(i64, Option<i64>, String, String, String, Option<String>)| {
                    match (a.1, b.1) {
                        (Some(x), Some(y)) if b.0 > a.0 => {
                            Some(r1((y - x) as f64 / ((b.0 - a.0) as f64 / 3600.0)))
                        }
                        _ => None,
                    }
                };
            let before = rate(&snaps[0], &snaps[i - 1])
                .or_else(|| (i >= 1).then(|| rate(&snaps[i - 1], &snaps[i])).flatten());
            let after = rate(&snaps[i], &snaps[snaps.len() - 1]);
            let lift = match (before, after) {
                (Some(b), Some(a)) if b > 0.0 => Some((a / b * 100.0).round() / 100.0),
                _ => None,
            };
            out.push(PackagingChange {
                url: format!("https://www.youtube.com/watch?v={vid}"),
                video_id: vid,
                channel_id: chan,
                changed_between: [format_rfc3339(snaps[i - 1].0), format_rfc3339(snaps[i].0)],
                change: match (t, th) {
                    (true, true) => "both",
                    (true, false) => "title",
                    _ => "thumbnail",
                },
                title_before: snaps[i - 1].4.clone(),
                title_after: snaps[i].4.clone(),
                vph_before: before,
                vph_after: after,
                lift,
            });
        }
        out.sort_by(|a, b| b.lift.unwrap_or(0.0).total_cmp(&a.lift.unwrap_or(0.0)));
        out.truncate(limit);
        Ok(out)
    }

    fn raw_video(
        &self,
        vid: &str,
    ) -> Result<Vec<(i64, Option<i64>, String, String, String, Option<String>)>, StoreError> {
        let conn = self.lock();
        let mut st = conn.prepare("SELECT ts, views, video_id, channel_id, title, thumb FROM video_snap WHERE video_id=?1 ORDER BY ts")?;
        let rows = st
            .query_map(params![vid], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// Tracked channels and how much history each has.
    pub fn tracked(&self) -> Result<Vec<(String, String, usize, String)>, StoreError> {
        let conn = self.lock();
        let mut st = conn.prepare(
            "SELECT channel_id, title, COUNT(*), MIN(ts) FROM channel_snap GROUP BY channel_id ORDER BY MAX(ts) DESC LIMIT 200",
        )?;
        let rows = st
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get::<_, i64>(2)? as usize,
                    format_rfc3339(r.get(3)?),
                ))
            })?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(views: u64, title: &str) -> Video {
        Video {
            id: "aaaaaaaaaaa".into(),
            channel_id: "UCx".into(),
            title: title.into(),
            views: Some(views),
            published_at: "2026-01-01T00:00:00Z".into(),
            ..Default::default()
        }
    }

    #[test]
    fn records_dedups_and_detects_changes() {
        let s = Store::in_memory().unwrap();
        let t0 = 1_790_000_000;
        s.record_videos(&[v(1000, "Old title")], &[], t0).unwrap();
        s.record_videos(&[v(1000, "Old title")], &[], t0 + 60)
            .unwrap(); // dup, skipped
        s.record_videos(&[v(2000, "Old title")], &[], t0 + 3600)
            .unwrap(); // 1000 vph
        s.record_videos(&[v(2500, "New title")], &[], t0 + 7200)
            .unwrap();
        s.record_videos(&[v(8500, "New title")], &[], t0 + 10_800)
            .unwrap(); // 6000 vph after
        let h = s.video_history("aaaaaaaaaaa").unwrap();
        assert_eq!(h.len(), 4);
        assert_eq!(h[1].vph_since_previous, Some(1000.0));
        let ch = s.packaging_changes(None, 10).unwrap();
        assert_eq!(ch.len(), 1);
        assert_eq!(ch[0].change, "title");
        assert_eq!(ch[0].title_after, "New title");
        assert!(ch[0].lift.unwrap() > 1.5, "{:?}", ch[0]);
    }

    #[test]
    fn channel_growth() {
        let s = Store::in_memory().unwrap();
        let mut c = Channel {
            id: "UCx".into(),
            title: "X".into(),
            handle: None,
            description: String::new(),
            published_at: String::new(),
            country: None,
            subscribers: Some(1000),
            views: Some(50_000),
            videos: Some(10),
            uploads_playlist_id: None,
            url: String::new(),
        };
        s.record_channel(&c, 0).unwrap();
        c.subscribers = Some(1100);
        s.record_channel(&c, 86_400).unwrap();
        let h = s.channel_history("UCx").unwrap();
        assert_eq!(h[1].subscribers_per_day_since_previous, Some(100.0));
        assert_eq!(s.tracked().unwrap().len(), 1);
    }
}
