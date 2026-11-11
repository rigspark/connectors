//! A small JSON-over-HTTPS client with the defaults every connector wants:
//! timeouts, a bounded retry on transient failures (idempotent methods only), a response
//! size cap, and secret scrubbing on every error message.

use std::time::Duration;

pub use reqwest::Method;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;

use crate::secret::{Secret, scrub};

/// Largest response body a connector will read (8 MiB).
pub const MAX_BODY: usize = 8 * 1024 * 1024;
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

/// Request body.
#[derive(Debug, Clone, Default)]
pub enum Body {
    #[default]
    None,
    Json(Value),
    Form(Vec<(String, String)>),
    Bytes {
        data: Vec<u8>,
        content_type: String,
    },
}

/// One request. Build with [`Req::get`] / [`Req::post`] / [`Req::put`] and the setters.
#[derive(Debug)]
pub struct Req<'a> {
    pub method: Method,
    /// Path appended to the client's base, or an absolute `http(s)://` URL.
    pub path: String,
    pub query: Vec<(&'a str, String)>,
    pub body: Body,
    /// `Authorization: Bearer …` for this request only.
    pub bearer: Option<&'a Secret>,
    pub headers: Vec<(HeaderName, String)>,
    /// Don't send the client's standing auth header (e.g. for an OAuth token endpoint).
    pub no_auth: bool,
}

impl<'a> Req<'a> {
    pub fn new(method: Method, path: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            query: Vec::new(),
            body: Body::None,
            bearer: None,
            headers: Vec::new(),
            no_auth: false,
        }
    }
    pub fn get(path: impl Into<String>) -> Self {
        Self::new(Method::GET, path)
    }
    pub fn post(path: impl Into<String>) -> Self {
        Self::new(Method::POST, path)
    }
    pub fn put(path: impl Into<String>) -> Self {
        Self::new(Method::PUT, path)
    }
    pub fn query(mut self, k: &'a str, v: impl Into<String>) -> Self {
        self.query.push((k, v.into()));
        self
    }
    pub fn query_opt(mut self, k: &'a str, v: Option<String>) -> Self {
        if let Some(v) = v {
            self.query.push((k, v));
        }
        self
    }
    pub fn queries(mut self, q: Vec<(&'a str, String)>) -> Self {
        self.query.extend(q);
        self
    }
    pub fn json(mut self, v: Value) -> Self {
        self.body = Body::Json(v);
        self
    }
    pub fn form(mut self, f: Vec<(String, String)>) -> Self {
        self.body = Body::Form(f);
        self
    }
    pub fn bytes(mut self, data: Vec<u8>, content_type: impl Into<String>) -> Self {
        self.body = Body::Bytes {
            data,
            content_type: content_type.into(),
        };
        self
    }
    pub fn bearer(mut self, token: &'a Secret) -> Self {
        self.bearer = Some(token);
        self
    }
    pub fn header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers
            .push((HeaderName::from_static(name), value.into()));
        self
    }
    pub fn no_auth(mut self) -> Self {
        self.no_auth = true;
        self
    }
}

/// A completed response with a success status (or 308, Google's "resume incomplete").
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Response {
    pub fn json(&self) -> Result<Value, HttpError> {
        if self.body.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&self.body).map_err(|e| HttpError::Decode(e.to_string()))
    }
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
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
    /// Pass `""` to use absolute URLs only.
    pub fn new(base: impl Into<String>, user_agent: &str) -> Result<Self, HttpError> {
        Self::with_timeout(base, user_agent, Duration::from_secs(30))
    }

    /// Like [`HttpClient::new`] with a custom overall timeout (uploads need longer).
    pub fn with_timeout(
        base: impl Into<String>,
        user_agent: &str,
        timeout: Duration,
    ) -> Result<Self, HttpError> {
        let inner = reqwest::Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(10))
            .timeout(timeout)
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

    pub fn base(&self) -> &str {
        &self.base
    }

    fn clean(&self, s: &str, extra: Option<&Secret>) -> String {
        let mut secrets: Vec<&Secret> = self.auth.iter().map(|(_, s)| s).collect();
        secrets.extend(extra);
        crate::text::trim(&scrub(s, &secrets), 1_000)
    }

    fn url(&self, path: &str) -> String {
        if path.starts_with("http://") || path.starts_with("https://") {
            path.to_owned()
        } else {
            format!("{}{}", self.base, path)
        }
    }

    /// Sends `req`. Non-success statuses become [`HttpError::Status`].
    pub async fn send(&self, req: Req<'_>) -> Result<Response, HttpError> {
        let url = self.url(&req.path);
        let mut headers = HeaderMap::new();
        let sensitive = |v: &str| {
            HeaderValue::from_str(v)
                .map(|mut h| {
                    h.set_sensitive(true);
                    h
                })
                .map_err(|_| HttpError::Transport("credential is not a valid header value".into()))
        };
        if let (Some((name, secret)), false) = (&self.auth, req.no_auth) {
            headers.insert(name.clone(), sensitive(secret.expose())?);
        }
        if let Some(t) = req.bearer {
            headers.insert(
                reqwest::header::AUTHORIZATION,
                sensitive(&format!("Bearer {}", t.expose()))?,
            );
        }
        for (k, v) in &req.headers {
            let v = HeaderValue::from_str(v)
                .map_err(|_| HttpError::Transport(format!("bad header value for {k}")))?;
            headers.insert(k.clone(), v);
        }
        let idempotent = matches!(
            req.method,
            Method::GET | Method::PUT | Method::HEAD | Method::DELETE
        );

        let mut attempt = 0;
        loop {
            let mut rb = self
                .inner
                .request(req.method.clone(), &url)
                .headers(headers.clone())
                .query(&req.query);
            rb = match &req.body {
                Body::None => rb,
                Body::Json(v) => rb.json(v),
                Body::Form(f) => rb.form(f),
                Body::Bytes { data, content_type } => rb
                    .header(reqwest::header::CONTENT_TYPE, content_type)
                    .body(data.clone()),
            };
            let sent = rb.send().await;
            let retryable = idempotent
                && match &sent {
                    Ok(r) => r.status().as_u16() == 429 || r.status().is_server_error(),
                    Err(e) => e.is_connect() || e.is_timeout(),
                };
            if retryable && attempt < RETRIES {
                attempt += 1;
                tracing::warn!(path = %req.path, attempt, "transient failure, retrying");
                tokio::time::sleep(self.backoff * attempt).await;
                continue;
            }
            let resp = sent.map_err(|e| {
                HttpError::Transport(self.clean(&e.without_url().to_string(), req.bearer))
            })?;
            let status = resp.status();
            let resp_headers = resp.headers().clone();
            if resp.content_length().is_some_and(|n| n as usize > MAX_BODY) {
                return Err(HttpError::TooLarge);
            }
            let bytes = resp.bytes().await.map_err(|e| {
                HttpError::Transport(self.clean(&e.without_url().to_string(), req.bearer))
            })?;
            if bytes.len() > MAX_BODY {
                return Err(HttpError::TooLarge);
            }
            // 308 is "resume incomplete" for Google resumable uploads: not an error.
            if !status.is_success() && status.as_u16() != 308 {
                return Err(HttpError::Status {
                    status: status.as_u16(),
                    body: self.clean(&String::from_utf8_lossy(&bytes), req.bearer),
                });
            }
            return Ok(Response {
                status: status.as_u16(),
                headers: resp_headers,
                body: bytes.to_vec(),
            });
        }
    }

    /// `GET {base}{path}?{query}` and decode JSON.
    pub async fn get_json(&self, path: &str, query: &[(&str, String)]) -> Result<Value, HttpError> {
        self.send(Req::get(path).queries(query.to_vec()))
            .await?
            .json()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_string_contains, header, method, path};
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
    async fn never_retries_post() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .expect(1)
            .mount(&server)
            .await;
        let e = client(&server)
            .send(Req::post("/x").json(serde_json::json!({})))
            .await
            .unwrap_err();
        assert!(matches!(e, HttpError::Status { status: 503, .. }));
    }

    #[tokio::test]
    async fn bearer_form_and_no_auth() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{\"access_token\":\"t\"}"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/me"))
            .and(header("authorization", "Bearer tok-123"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .expect(1)
            .mount(&server)
            .await;
        let c = client(&server);
        let r = c
            .send(
                Req::post("/token")
                    .no_auth()
                    .form(vec![("grant_type".into(), "refresh_token".into())]),
            )
            .await
            .unwrap();
        assert_eq!(r.json().unwrap()["access_token"], "t");
        let reqs = server.received_requests().await.unwrap();
        assert!(reqs[0].headers.get("x-api-key").is_none());
        let tok = Secret::new("tok-123");
        c.send(Req::get("/me").bearer(&tok)).await.unwrap();
    }

    #[tokio::test]
    async fn does_not_retry_4xx_and_scrubs_secret_from_body() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(403).set_body_string("bad key sekrit-key and tok-999"),
            )
            .expect(1)
            .mount(&server)
            .await;
        let tok = Secret::new("tok-999");
        let e = client(&server)
            .send(Req::get("/x").bearer(&tok))
            .await
            .unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains("[redacted]"), "{msg}");
        assert!(
            !msg.contains("sekrit-key") && !msg.contains("tok-999"),
            "{msg}"
        );
    }
}
