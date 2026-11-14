#![forbid(unsafe_code)]
//! Shared outbound HTTP client: pooling, timeouts, TLS verification,
//! correlation/trace propagation, safe error mapping. No domain adapters.

use reqwest::{Client, RequestBuilder, Response};
use std::time::Duration;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("request failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("upstream returned status {status}")]
    Status { status: u16 },
    #[error("response body too large")]
    TooLarge,
}

/// Standard headers propagated on every outbound request.
pub const HEADER_REQUEST_ID: &str = "x-request-id";
pub const HEADER_TRACEPARENT: &str = "traceparent";
pub const HEADER_CORRELATION: &str = "x-correlation-id";

#[derive(Debug, Clone)]
pub struct HttpClientConfig {
    pub timeout: Duration,
    pub connect_timeout: Duration,
    pub user_agent: String,
    pub max_body_bytes: usize,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            connect_timeout: Duration::from_secs(3),
            user_agent: "iaop-service".into(),
            max_body_bytes: 10 * 1024 * 1024,
        }
    }
}

/// Shared outbound client. Cheap to clone (reqwest pool is shared).
#[derive(Debug, Clone)]
pub struct HttpClient {
    inner: Client,
    cfg: HttpClientConfig,
}

impl HttpClient {
    pub fn new(cfg: HttpClientConfig) -> Result<Self, ClientError> {
        let inner = Client::builder()
            .timeout(cfg.timeout)
            .connect_timeout(cfg.connect_timeout)
            .user_agent(cfg.user_agent.clone())
            .build()?;
        Ok(Self { inner, cfg })
    }

    pub fn get(&self, url: &str) -> RequestBuilder {
        self.inner.get(url)
    }
    pub fn post(&self, url: &str) -> RequestBuilder {
        self.inner.post(url)
    }
    pub fn put(&self, url: &str) -> RequestBuilder {
        self.inner.put(url)
    }
    pub fn delete(&self, url: &str) -> RequestBuilder {
        self.inner.delete(url)
    }

    /// Injects correlation identifiers into a request builder.
    pub fn with_correlation(
        &self,
        req: RequestBuilder,
        request_id: Option<Uuid>,
        correlation_id: Option<Uuid>,
    ) -> RequestBuilder {
        let mut req = req.header("user-agent", self.cfg.user_agent.clone());
        if let Some(id) = request_id {
            req = req.header(HEADER_REQUEST_ID, id.to_string());
        }
        if let Some(id) = correlation_id {
            req = req.header(HEADER_CORRELATION, id.to_string());
        }
        req
    }

    /// Sends a request and enforces the configured body-size cap.
    pub async fn send_limited(&self, req: RequestBuilder) -> Result<Response, ClientError> {
        let resp = req.send().await?;
        if let Some(len) = resp.content_length() {
            if len as usize > self.cfg.max_body_bytes {
                return Err(ClientError::TooLarge);
            }
        }
        Ok(resp)
    }

    /// Fetches a response body as bytes bounded by `max_body_bytes`.
    pub async fn bytes_limited(&self, resp: Response) -> Result<Vec<u8>, ClientError> {
        let mut resp = resp;
        let mut buf = Vec::new();
        while let Some(chunk) = resp.chunk().await? {
            buf.extend_from_slice(&chunk);
            if buf.len() > self.cfg.max_body_bytes {
                return Err(ClientError::TooLarge);
            }
        }
        Ok(buf)
    }

    pub fn config(&self) -> &HttpClientConfig {
        &self.cfg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_builds() {
        let c = HttpClient::new(HttpClientConfig::default()).unwrap();
        assert_eq!(c.config().timeout, Duration::from_secs(10));
    }

    #[tokio::test]
    async fn correlation_headers_set() {
        // build a request against an unreachable-but-parseable URL to inspect headers
        let c = HttpClient::new(HttpClientConfig::default()).unwrap();
        let rid = Uuid::now_v7();
        let cid = Uuid::now_v7();
        let req = c
            .with_correlation(c.get("http://127.0.0.1:1/x"), Some(rid), Some(cid))
            .build()
            .unwrap();
        assert_eq!(req.headers()[HEADER_REQUEST_ID], rid.to_string());
        assert_eq!(req.headers()[HEADER_CORRELATION], cid.to_string());
    }
}
