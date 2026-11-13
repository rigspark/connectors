//! Google OAuth for channel-owner tools.
//!
//! * At runtime the connector holds a refresh token (from env) and swaps it for short-lived
//!   access tokens, cached until a minute before they expire.
//! * `youtube-mcp auth` runs the installed-app flow once (loopback redirect + PKCE) and
//!   prints the refresh token for the host to store in its credential store.

use std::time::{Duration, Instant};

use base64::Engine;
use connector_kit::{HttpClient, Req, Secret};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

use crate::api::YouTubeError;
use crate::config::OAuthCreds;

pub const SCOPES_READ: &[&str] = &[
    "https://www.googleapis.com/auth/youtube.readonly",
    "https://www.googleapis.com/auth/yt-analytics.readonly",
    "https://www.googleapis.com/auth/yt-analytics-monetary.readonly",
];
pub const SCOPES_WRITE: &[&str] = &[
    "https://www.googleapis.com/auth/youtube.force-ssl",
    "https://www.googleapis.com/auth/youtube.upload",
];
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";

pub struct OAuth {
    creds: OAuthCreds,
    token_url: String,
    cache: Mutex<Option<(Secret, Instant)>>,
}

impl OAuth {
    pub fn new(creds: OAuthCreds, token_url: String) -> Self {
        Self {
            creds,
            token_url,
            cache: Mutex::new(None),
        }
    }

    /// Secrets to scrub from anything returned to the model.
    pub fn secrets(&self) -> [&Secret; 2] {
        [&self.creds.client_secret, &self.creds.refresh_token]
    }

    pub async fn access_token(&self, http: &HttpClient) -> Result<Secret, YouTubeError> {
        let mut cache = self.cache.lock().await;
        if let Some((tok, until)) = cache.as_ref()
            && Instant::now() < *until
        {
            return Ok(tok.clone());
        }
        let resp = http
            .send(
                Req::post(self.token_url.clone()).no_auth().form(vec![
                    ("client_id".into(), self.creds.client_id.clone()),
                    ("client_secret".into(), self.creds.client_secret.expose().to_owned()),
                    ("refresh_token".into(), self.creds.refresh_token.expose().to_owned()),
                    ("grant_type".into(), "refresh_token".into()),
                ]),
            )
            .await
            .map_err(|e| {
                let msg = connector_kit::scrub(&e.to_string(), &self.secrets());
                match e {
                    connector_kit::HttpError::Status { status, .. } if status == 400 || status == 401 => {
                        YouTubeError::Forbidden("Google rejected the refresh token (revoked, expired, or for another client). Run `youtube-mcp auth` again".into())
                    }
                    _ => YouTubeError::Http(msg),
                }
            })?;
        let v = resp.json().map_err(|e| YouTubeError::Http(e.to_string()))?;
        let tok = v["access_token"]
            .as_str()
            .ok_or_else(|| YouTubeError::Http("token response had no access_token".into()))?;
        let ttl = v["expires_in"]
            .as_u64()
            .unwrap_or(3600)
            .saturating_sub(60)
            .max(30);
        let tok = Secret::new(tok);
        *cache = Some((tok.clone(), Instant::now() + Duration::from_secs(ttl)));
        Ok(tok)
    }
}

// ---------- `youtube-mcp auth` ----------

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn random(n: usize) -> Result<Vec<u8>, String> {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).map_err(|e| format!("no system randomness: {e}"))?;
    Ok(b)
}

pub fn pct(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn unpct(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                if let Some(x) = s
                    .get(i + 1..i + 3)
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                {
                    out.push(x);
                    i += 3;
                    continue;
                }
                out.push(b'%');
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Runs the sign-in flow and returns the refresh token.
pub async fn run_auth(
    client_id: &str,
    client_secret: &Secret,
    write: bool,
    token_url: &str,
) -> Result<Secret, String> {
    let verifier = b64(&random(32)?);
    let challenge = b64(&Sha256::digest(verifier.as_bytes()));
    let state = b64(&random(16)?);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("can't open a local port: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}");
    let mut scopes: Vec<&str> = SCOPES_READ.to_vec();
    if write {
        scopes.extend(SCOPES_WRITE);
    }
    let url = format!(
        "{AUTH_URL}?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent&code_challenge={challenge}&code_challenge_method=S256&state={state}",
        pct(client_id),
        pct(&redirect),
        pct(&scopes.join(" "))
    );
    eprintln!("\nSign in with the Google account that owns the channel:\n\n  {url}\n");
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(&url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    eprintln!("Waiting for the browser to come back to {redirect} …");

    let code = loop {
        let (mut sock, _) = listener.accept().await.map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 8192];
        let n = tokio::time::timeout(Duration::from_secs(10), sock.read(&mut buf))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]).into_owned();
        let path = req.split_whitespace().nth(1).unwrap_or("/");
        let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
        let get = |k: &str| {
            query
                .split('&')
                .find_map(|kv| kv.strip_prefix(&format!("{k}=")))
                .map(unpct)
        };
        let (body, result) = match (get("code"), get("state"), get("error")) {
            (_, _, Some(err)) => (
                "Sign-in was cancelled. You can close this tab.",
                Some(Err(format!("Google returned: {err}"))),
            ),
            (Some(code), Some(st), _) if st == state => (
                "Signed in. You can close this tab and return to the terminal.",
                Some(Ok(code)),
            ),
            (Some(_), _, _) => ("State mismatch; ignored.", None),
            _ => ("Waiting for Google sign-in…", None),
        };
        let page = format!(
            "<!doctype html><meta charset=utf-8><title>youtube-mcp</title><body style=\"font:16px system-ui;margin:3em\">{body}</body>"
        );
        let _ = sock
            .write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}", page.len()).as_bytes())
            .await;
        if let Some(r) = result {
            break r?;
        }
    };

    let http = HttpClient::new("", crate::api::UA).map_err(|e| e.to_string())?;
    let resp = http
        .send(Req::post(token_url.to_owned()).form(vec![
            ("code".into(), code),
            ("client_id".into(), client_id.into()),
            ("client_secret".into(), client_secret.expose().into()),
            ("redirect_uri".into(), redirect),
            ("grant_type".into(), "authorization_code".into()),
            ("code_verifier".into(), verifier),
        ]))
        .await
        .map_err(|e| connector_kit::scrub(&e.to_string(), &[client_secret]))?;
    let v = resp.json().map_err(|e| e.to_string())?;
    v["refresh_token"]
        .as_str()
        .map(Secret::new)
        .ok_or_else(|| "Google didn't return a refresh token (remove the app's access at myaccount.google.com/permissions and try again)".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_round_trip() {
        let s = "https://www.googleapis.com/auth/youtube.readonly a+b/ü";
        assert_eq!(unpct(&pct(s)), s);
        assert_eq!(unpct("4%2F0Ab"), "4/0Ab");
    }
}
