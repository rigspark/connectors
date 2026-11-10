//! Parsing the ids, handles and URLs a model is likely to pass in.

/// What a `channel` argument refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelRef {
    /// `UC…` channel id.
    Id(String),
    /// `@handle` (stored without the `@`).
    Handle(String),
}

fn id_chars(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Strips scheme, `www.`/`m.` and returns host + path + query for URL-ish input.
fn strip_url(s: &str) -> Option<&str> {
    let s = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(s);
    let s = s
        .strip_prefix("www.")
        .or_else(|| s.strip_prefix("m."))
        .unwrap_or(s);
    (s.starts_with("youtube.com/")
        || s.starts_with("youtu.be/")
        || s.starts_with("music.youtube.com/"))
    .then_some(s)
}

/// Accepts an 11-character video id or any common YouTube video URL
/// (`watch?v=`, `youtu.be/`, `shorts/`, `embed/`, `live/`).
pub fn video_id(input: &str) -> Result<String, String> {
    let s = input.trim();
    let candidate = match strip_url(s) {
        Some(rest) => {
            if let Some(r) = rest.strip_prefix("youtu.be/") {
                r.split(['?', '&', '/', '#']).next().unwrap_or("")
            } else {
                let path = rest.split_once('/').map(|(_, p)| p).unwrap_or("");
                let from_path = ["shorts/", "embed/", "live/", "v/"]
                    .iter()
                    .find_map(|p| path.strip_prefix(p))
                    .and_then(|r| r.split(['?', '/', '#']).next());
                let from_query = || {
                    rest.split_once('?')
                        .and_then(|(_, q)| q.split(['&', '#']).find_map(|kv| kv.strip_prefix("v=")))
                };
                from_path.or_else(from_query).unwrap_or("")
            }
        }
        None => s,
    };
    if candidate.len() == 11 && id_chars(candidate) {
        Ok(candidate.to_owned())
    } else {
        Err(format!(
            "`{}` is not a YouTube video id or video URL",
            short(input)
        ))
    }
}

/// Accepts a `UC…` channel id, an `@handle`, a bare handle, or a channel URL
/// (`/channel/UC…`, `/@handle`).
pub fn channel_ref(input: &str) -> Result<ChannelRef, String> {
    let s = input.trim();
    let s = match strip_url(s) {
        Some(rest) => {
            let path = rest.split_once('/').map(|(_, p)| p).unwrap_or("");
            let first = path.split(['/', '?', '#']).next().unwrap_or("");
            if first == "channel" {
                path.split('/')
                    .nth(1)
                    .unwrap_or("")
                    .split(['?', '#'])
                    .next()
                    .unwrap_or("")
            } else {
                first
            }
        }
        None => s,
    };
    if s.len() == 24 && s.starts_with("UC") && id_chars(s) {
        return Ok(ChannelRef::Id(s.to_owned()));
    }
    let h = s.strip_prefix('@').unwrap_or(s);
    let ok = (3..=30).contains(&h.chars().count())
        && h.chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if ok {
        Ok(ChannelRef::Handle(h.to_owned()))
    } else {
        Err(format!(
            "`{}` is not a channel id (UC…), @handle or channel URL",
            short(input)
        ))
    }
}

/// Accepts a playlist id (`PL…`, `UU…`, `OL…`, …) or a URL with `list=`.
pub fn playlist_id(input: &str) -> Result<String, String> {
    let s = input.trim();
    let candidate = match strip_url(s) {
        Some(rest) => rest
            .split_once('?')
            .and_then(|(_, q)| q.split('&').find_map(|kv| kv.strip_prefix("list=")))
            .unwrap_or(""),
        None => s,
    };
    if (10..=64).contains(&candidate.len()) && id_chars(candidate) {
        Ok(candidate.to_owned())
    } else {
        Err(format!(
            "`{}` is not a YouTube playlist id or playlist URL",
            short(input)
        ))
    }
}

/// Two-letter ISO 3166-1 region code, upper-cased.
pub fn region(input: &str) -> Result<String, String> {
    let r = input.trim().to_ascii_uppercase();
    if r.len() == 2 && r.chars().all(|c| c.is_ascii_uppercase()) {
        Ok(r)
    } else {
        Err(format!(
            "`{}` is not a two-letter region code such as US or IN",
            short(input)
        ))
    }
}

fn short(s: &str) -> String {
    connector_kit::text::trim(s, 80)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn videos() {
        for u in [
            "dQw4w9WgXcQ",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtube.com/watch?feature=share&v=dQw4w9WgXcQ&t=10",
            "https://youtu.be/dQw4w9WgXcQ?t=3",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
            "youtube.com/embed/dQw4w9WgXcQ",
            "https://m.youtube.com/live/dQw4w9WgXcQ?si=x",
        ] {
            assert_eq!(video_id(u).as_deref(), Ok("dQw4w9WgXcQ"), "{u}");
        }
        assert!(video_id("nope").is_err());
        assert!(video_id("https://evil.com/watch?v=dQw4w9WgXcQ").is_err());
        assert!(video_id("dQw4w9WgXc!").is_err());
    }

    #[test]
    fn channels() {
        let id = "UC_x5XG1OV2P6uZZ5FSM9Ttw";
        assert_eq!(channel_ref(id), Ok(ChannelRef::Id(id.into())));
        assert_eq!(
            channel_ref(&format!("https://www.youtube.com/channel/{id}/videos")),
            Ok(ChannelRef::Id(id.into()))
        );
        assert_eq!(
            channel_ref("@GoogleDevelopers"),
            Ok(ChannelRef::Handle("GoogleDevelopers".into()))
        );
        assert_eq!(
            channel_ref("https://youtube.com/@GoogleDevelopers/featured"),
            Ok(ChannelRef::Handle("GoogleDevelopers".into()))
        );
        assert!(channel_ref("@a").is_err());
        assert!(channel_ref("has space").is_err());
    }

    #[test]
    fn playlists_and_regions() {
        assert_eq!(
            playlist_id("https://www.youtube.com/playlist?list=PLabcdefghijk").as_deref(),
            Ok("PLabcdefghijk")
        );
        assert!(playlist_id("short").is_err());
        assert_eq!(region("in").as_deref(), Ok("IN"));
        assert!(region("IND").is_err());
    }
}
