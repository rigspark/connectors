//! A small JSON-over-HTTPS client with the defaults every connector wants:
//! timeouts, a bounded retry on transient failures, a response size cap, and secret
//! scrubbing on every error message.

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;

use crate::secret::{Secret, scrub};

/// Largest response body a connector will read (4 MiB).
pub const MAX_BODY: usize = 4 * 1024 * 1024;
/// Retries after the first attempt for 429 and 5xx responses and connection errors.
pub const RETRIES: u32 = 2;

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// The server answered with a non-success status. `body` is scrubbed and trimmed.
    #[error("HTTP {status}: {body}")]
    Status { status: u16, body: String },
    /// Transport failure (DNS, TLS, timeout, connection reset). Message is scrubbed.
    #[error("request failed: {0}")]
    Transport(String),
    #[error("response was larger than {MAX_BODY} bytes")]
    TooLarge,
    #[error("response was not valid JSON: {0}")]
    Decode(String),
}

/// Shared HTTP client. Cheap to clone.
#[derive(Clone)]
pub struct HttpClient {
    inner: reqwest::Client,
    base: String,
    /// Header that carries the credential on every request, e.g. `x-goog-api-key`.
    auth: Option<(HeaderName, Secret)>,
    backoff: Duration,
}

impl HttpClient {
    /// `base` is the API root without a trailing slash, e.g. `https://www.googleapis.com/youtube/v3`.
    pub fn new(base: impl Into<String>, user_agent: &str) -> Result<Self, HttpError> {
        let inner = reqwest::Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .https_only(false) // tests use a local http mock; production bases are https
            .build()
            .map_err(|e| HttpError::Transport(e.to_string()))?;
        Ok(Self {
            inner,
            base: base.into().trim_end_matches('/').to_owned(),
            auth: None,
            backoff: Duration::from_millis(400),
        })
    }

    /// Sends `secret` in `header` on every request. Prefer this to query-string keys:
    /// URLs end up in logs and error messages, headers don't.
    pub fn with_auth_header(mut self, header: &'static str, secret: Secret) -> Self {
        self.auth = Some((HeaderName::from_static(header), secret));
        self
    }

    /// Overrides the retry backoff (tests use zero).
    pub fn with_backoff(mut self, backoff: Duration) -> Self {
        self.backoff = backoff;
        self
    }

    fn secrets(&self) -> Vec<&Secret> {
        self.auth.iter().map(|(_, s)| s).collect()
    }

    fn clean(&self, s: &str) -> String {
        crate::text::trim(&scrub(s, &self.secrets()), 1_000)
    }

    /// `GET {base}{path}?{query}` and decode JSON.
    pub async fn get_json(&self, path: &str, query: &[(&str, String)]) -> Result<Value, HttpError> {
        let url = format!("{}{}", self.base, path);
        let mut headers = HeaderMap::new();
        if let Some((name, secret)) = &self.auth {
            let mut v = HeaderValue::from_str(secret.expose()).map_err(|_| {
                HttpError::Transport("credential is not a valid header value".into())
            })?;
            v.set_sensitive(true);
            headers.insert(name.clone(), v);
        }

        let mut attempt = 0;
        loop {
            let sent = self
                .inner
                .get(&url)
                .headers(headers.clone())
                .query(query)
                .send()
                .await;
            let retryable = match &sent {
                Ok(r) => r.status().as_u16() == 429 || r.status().is_server_error(),
                Err(e) => e.is_connect() || e.is_timeout(),
            };
            if retryable && attempt < RETRIES {
                attempt += 1;
                tracing::warn!(path, attempt, "transient failure, retrying");
                tokio::time::sleep(self.backoff * attempt).await;
                continue;
            }
            let resp =
                sent.map_err(|e| HttpError::Transport(self.clean(&e.without_url().to_string())))?;
            let status = resp.status();
            if resp.content_length().is_some_and(|n| n as usize > MAX_BODY) {
                return Err(HttpError::TooLarge);
            }
            let bytes = resp
                .bytes()
                .await
                .map_err(|e| HttpError::Transport(self.clean(&e.without_url().to_string())))?;
            if bytes.len() > MAX_BODY {
                return Err(HttpError::TooLarge);
            }
            if !status.is_success() {
                return Err(HttpError::Status {
                    status: status.as_u16(),
                    body: self.clean(&String::from_utf8_lossy(&bytes)),
                });
            }
            return serde_json::from_slice(&bytes).map_err(|e| HttpError::Decode(e.to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer) -> HttpClient {
        HttpClient::new(server.uri(), "test")
            .unwrap()
            .with_auth_header("x-api-key", Secret::new("sekrit-key"))
            .with_backoff(Duration::ZERO)
    }

    #[tokio::test]
    async fn sends_auth_header_and_decodes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/ping"))
            .and(header("x-api-key", "sekrit-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
            .mount(&server)
            .await;
        let v = client(&server).get_json("/ping", &[]).await.unwrap();
        assert_eq!(v["ok"], true);
    }

    #[tokio::test]
    async fn retries_then_gives_up_on_5xx() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503).set_body_string("down"))
            .expect(1 + RETRIES as u64)
            .mount(&server)
            .await;
        let e = client(&server).get_json("/x", &[]).await.unwrap_err();
        assert!(matches!(e, HttpError::Status { status: 503, .. }));
    }

    #[tokio::test]
    async fn does_not_retry_4xx_and_scrubs_secret_from_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403).set_body_string("bad key sekrit-key"))
            .expect(1)
            .mount(&server)
            .await;
        let e = client(&server).get_json("/x", &[]).await.unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("[redacted]"), "{msg}");
        assert!(!msg.contains("sekrit-key"), "{msg}");
    }
}
