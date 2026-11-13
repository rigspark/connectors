//! Everything the connector reads from its environment.

use std::path::PathBuf;

use connector_kit::{Secret, SecretError};

/// Where each upstream lives. `YOUTUBE_API_BASE` points them all at one mock (tests, proxies).
#[derive(Debug, Clone)]
pub struct Endpoints {
    /// Data API v3.
    pub data: String,
    /// YouTube Analytics API v2.
    pub analytics: String,
    /// Upload endpoints (videos.insert, thumbnails.set).
    pub upload: String,
    /// Google OAuth token endpoint.
    pub token: String,
    /// youtube.com (unofficial: player/captions).
    pub web: String,
    /// Search autocomplete (unofficial).
    pub suggest: String,
}

impl Endpoints {
    pub fn production() -> Self {
        Self {
            data: "https://www.googleapis.com/youtube/v3".into(),
            analytics: "https://youtubeanalytics.googleapis.com/v2".into(),
            upload: "https://www.googleapis.com/upload/youtube/v3".into(),
            token: "https://oauth2.googleapis.com/token".into(),
            web: "https://www.youtube.com".into(),
            suggest: "https://suggestqueries-clients6.youtube.com/complete/search".into(),
        }
    }

    /// Every upstream under one base: data at `{base}`, the rest at `{base}/analytics`,
    /// `{base}/upload`, `{base}/token`, `{base}/web`, `{base}/suggest`.
    pub fn under(base: &str) -> Self {
        let b = base.trim_end_matches('/');
        Self {
            data: b.into(),
            analytics: format!("{b}/analytics"),
            upload: format!("{b}/upload"),
            token: format!("{b}/token"),
            web: format!("{b}/web"),
            suggest: format!("{b}/suggest"),
        }
    }
}

/// OAuth client and refresh token for the creator's own channel.
#[derive(Debug, Clone)]
pub struct OAuthCreds {
    pub client_id: String,
    pub client_secret: Secret,
    pub refresh_token: Secret,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub key: Secret,
    pub endpoints: Endpoints,
    pub oauth: Option<OAuthCreds>,
    /// Autocomplete keyword ideas and transcripts.
    pub unofficial: bool,
    /// History database; `None` when switched off.
    pub db: Option<PathBuf>,
    /// The only folder `upload_video` and `set_thumbnail` may read from.
    pub upload_dir: Option<PathBuf>,
}

pub const KEY_ENV: &str = "YOUTUBE_API_KEY";
pub const BASE_ENV: &str = "YOUTUBE_API_BASE";
pub const CLIENT_ID_ENV: &str = "YOUTUBE_OAUTH_CLIENT_ID";
pub const CLIENT_SECRET_ENV: &str = "YOUTUBE_OAUTH_CLIENT_SECRET";
pub const REFRESH_ENV: &str = "YOUTUBE_OAUTH_REFRESH_TOKEN";
pub const UNOFFICIAL_ENV: &str = "YOUTUBE_MCP_UNOFFICIAL";
pub const DB_ENV: &str = "YOUTUBE_MCP_DB";
pub const UPLOAD_DIR_ENV: &str = "YOUTUBE_MCP_UPLOAD_DIR";

fn var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
}

fn off(v: &str) -> bool {
    matches!(
        v.to_ascii_lowercase().as_str(),
        "0" | "off" | "false" | "no"
    )
}

pub fn default_db() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".rigspark")
            .join("youtube-mcp")
            .join("history.sqlite"),
    )
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let key = Secret::from_env(KEY_ENV).map_err(|e: SecretError| e.to_string())?;
        let endpoints = var(BASE_ENV)
            .map(|b| Endpoints::under(&b))
            .unwrap_or_else(Endpoints::production);
        let oauth = match (var(CLIENT_ID_ENV), var(CLIENT_SECRET_ENV), var(REFRESH_ENV)) {
            (Some(id), Some(secret), Some(refresh)) => Some(OAuthCreds {
                client_id: id,
                client_secret: Secret::new(secret),
                refresh_token: Secret::new(refresh),
            }),
            (None, None, None) => None,
            _ => {
                return Err(format!(
                    "set all three of {CLIENT_ID_ENV}, {CLIENT_SECRET_ENV} and {REFRESH_ENV} for channel-owner tools, or none of them"
                ));
            }
        };
        let unofficial = !var(UNOFFICIAL_ENV).is_some_and(|v| off(&v));
        let db = match var(DB_ENV) {
            Some(v) if off(&v) => None,
            Some(v) => Some(PathBuf::from(v)),
            None => default_db(),
        };
        let upload_dir = var(UPLOAD_DIR_ENV).map(PathBuf::from);
        Ok(Self {
            key,
            endpoints,
            oauth,
            unofficial,
            db,
            upload_dir,
        })
    }

    /// For tests: mock endpoints, no OAuth, no DB, unofficial on.
    pub fn for_tests(base: &str) -> Self {
        Self {
            key: Secret::new("test-key-not-real"),
            endpoints: Endpoints::under(base),
            oauth: None,
            unofficial: true,
            db: None,
            upload_dir: None,
        }
    }
}
